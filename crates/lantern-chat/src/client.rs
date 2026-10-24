//! A streaming client for the OpenAI chat completions API, which OpenRouter and most local
//! servers speak as well.

use std::{
    io::{BufRead, BufReader},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

const RETRIES: u32 = 2;

/// The message of an error body. OpenRouter wraps what the provider said in metadata.raw,
/// which is the part that explains anything.
fn error_text(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else { return body.trim().to_owned() };
    let error = &value["error"];
    let message = error["message"].as_str().or_else(|| error.as_str()).unwrap_or(body).to_owned();
    match error["metadata"]["raw"].as_str() {
        Some(raw) => {
            let raw = serde_json::from_str::<Value>(raw).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_owned)).unwrap_or_else(|| raw.to_owned());
            let provider = error["metadata"]["provider_name"].as_str().map_or(String::new(), |p| format!(" ({p})"));
            format!("{message}{provider}: {raw}")
        }
        None => message,
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them, a JSON object in a string.
    pub arguments: String,
}

/// Token counts of one request, as far as the endpoint reports them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub prompt: u64,
    /// The part of the prompt that was served from the cache of the provider.
    pub cached: u64,
    /// The part of the prompt that was written to the cache by this request.
    pub cache_written: u64,
    pub completion: u64,
    /// In dollars, only OpenRouter reports it.
    pub cost: Option<f64>,
}

/// What arrives while a reply streams in.
pub enum Piece<'a> {
    Thinking(&'a str),
    Text(&'a str),
    /// The model started to write a call to this tool. The arguments are still coming.
    Calling(&'a str),
    /// The provider answered with this status, the request goes out again in so many seconds.
    Retrying(u16, u64),
}

enum Incoming {
    Line(String),
    Retrying(u16, u64),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Reply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub stopped: bool,
}

pub use lantern_core::settings::Endpoint;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(Duration::from_secs(300)).build()
}

fn is_openrouter(endpoint: &Endpoint) -> bool {
    endpoint.provider.id == "openrouter" || endpoint.base_url.contains("openrouter.ai")
}

/// Whether the provider wants the cacheable part of a prompt marked. Everyone else either
/// caches by prefix without being asked or does not cache at all.
pub fn wants_cache_markers(endpoint: &Endpoint) -> bool {
    is_openrouter(endpoint) && ["anthropic/", "google/"].iter().any(|family| endpoint.model.starts_with(family))
}

/// The model ids the endpoint offers, sorted.
pub fn list_models(endpoint: &Endpoint) -> Result<Vec<String>> {
    let url = format!("{}/models", endpoint.base_url.trim_end_matches('/'));
    let mut request = agent().get(&url);
    if !endpoint.api_key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", endpoint.api_key));
    }
    let body: Value = match request.call() {
        Ok(response) => response.into_json()?,
        Err(ureq::Error::Status(code, response)) => bail!("{url} answered {code}: {}", error_text(&response.into_string().unwrap_or_default())),
        Err(err) => bail!("could not reach {url}: {err}"),
    };
    let list = body["data"].as_array().or_else(|| body["models"].as_array()).or_else(|| body.as_array()).cloned().unwrap_or_default();
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()))
        // Gemini lists them as models/<id>.
        .map(|id| id.trim_start_matches("models/").to_owned())
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        bail!("{url} answered, but without a list of models");
    }
    Ok(ids)
}

/// Sends one request and streams the answer. `on_text` gets the text as it arrives, `stop` is
/// looked at between chunks.
pub fn complete(endpoint: &Endpoint, messages: &[Value], tools: &Value, stop: &AtomicBool, mut on_piece: impl FnMut(Piece)) -> Result<Reply> {
    let mut body = json!({ "model": endpoint.model, "messages": messages, "stream": true });
    // An empty tool list is an error for some servers, leaving it out means the same.
    if tools.as_array().is_some_and(|t| !t.is_empty()) {
        body["tools"] = tools.clone();
    }
    if endpoint.provider.stream_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if is_openrouter(endpoint) {
        body["usage"] = json!({ "include": true });
    }

    let url = format!("{}/chat/completions", endpoint.base_url.trim_end_matches('/'));
    let mut request = agent().post(&url).set("Content-Type", "application/json").set("Accept", "text/event-stream");
    if !endpoint.api_key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", endpoint.api_key));
    }
    if is_openrouter(endpoint) {
        request = request.set("X-Title", "Lantern");
    }

    // The request and the reading happen on a thread of their own. A read blocks until the
    // server says something, which can be minutes, and Stop has to work during that time too.
    // When the receiver goes away the thread ends at its next line or when the read times out.
    let (lines, incoming) = std::sync::mpsc::channel::<Incoming>();
    let (provider, shown_url) = (endpoint.provider.name, url.clone());
    std::thread::spawn(move || {
        // Rate limits and overloaded providers usually pass within seconds. Nothing has been
        // streamed at this point, so trying again is invisible apart from the note.
        let mut attempt = 0;
        let response = loop {
            match request.clone().send_json(&body) {
                Ok(response) => break response,
                Err(ureq::Error::Status(code, response)) => {
                    let wait = response.header("retry-after").and_then(|v| v.parse::<u64>().ok());
                    let message = error_text(&response.into_string().unwrap_or_default());
                    let transient = matches!(code, 408 | 429 | 500 | 502 | 503 | 529);
                    if !transient || attempt == RETRIES {
                        let _ = lines.send(Incoming::Failed(format!("{provider} answered {code}: {message}")));
                        return;
                    }
                    let seconds = wait.unwrap_or(2 << attempt).min(20);
                    if lines.send(Incoming::Retrying(code, seconds)).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(seconds));
                    attempt += 1;
                }
                Err(err) => {
                    let _ = lines.send(Incoming::Failed(format!("could not reach {shown_url}: {err}")));
                    return;
                }
            }
        };
        for line in BufReader::new(response.into_reader()).lines() {
            let sent = match line {
                Ok(line) => lines.send(Incoming::Line(line)),
                Err(err) => lines.send(Incoming::Failed(format!("the connection to {provider} broke: {err}"))),
            };
            if sent.is_err() {
                return;
            }
        }
    });

    let mut reply = Reply::default();
    loop {
        if stop.load(Ordering::Relaxed) {
            reply.stopped = true;
            break;
        }
        let line = match incoming.recv_timeout(Duration::from_millis(80)) {
            Ok(Incoming::Line(line)) => line,
            Ok(Incoming::Retrying(code, seconds)) => {
                on_piece(Piece::Retrying(code, seconds));
                continue;
            }
            Ok(Incoming::Failed(message)) => bail!("{message}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            // The thread is done, the stream ended without a [DONE].
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Lines that are not data are comments, OpenRouter sends them as keep alives.
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue };
        if let Some(message) = chunk["error"]["message"].as_str() {
            return Err(anyhow!("{} reported: {message}", endpoint.provider.name));
        }

        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            let details = &usage["prompt_tokens_details"];
            reply.usage = Usage {
                prompt: usage["prompt_tokens"].as_u64().unwrap_or(0),
                cached: details["cached_tokens"]
                    .as_u64()
                    .or_else(|| usage["prompt_cache_hit_tokens"].as_u64())
                    .or_else(|| usage["cache_read_input_tokens"].as_u64())
                    .unwrap_or(0),
                cache_written: details["cache_write_tokens"].as_u64().or_else(|| usage["cache_creation_input_tokens"].as_u64()).unwrap_or(0),
                completion: usage["completion_tokens"].as_u64().unwrap_or(0),
                cost: usage["cost"].as_f64(),
            };
        }

        let delta = &chunk["choices"][0]["delta"];
        // Reasoning models think out loud first. DeepSeek, GLM and NIM call the field
        // reasoning_content, OpenRouter calls it reasoning. It is shown and then dropped, the
        // APIs do not want it back in the history.
        if let Some(thought) = delta["reasoning_content"].as_str().or_else(|| delta["reasoning"].as_str()).filter(|t| !t.is_empty()) {
            on_piece(Piece::Thinking(thought));
        }
        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            reply.text.push_str(text);
            on_piece(Piece::Text(text));
        }
        // A call arrives in pieces, the index says which call a piece belongs to.
        for piece in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = piece["index"].as_u64().unwrap_or(0) as usize;
            if reply.tool_calls.len() <= index {
                reply.tool_calls.resize(index + 1, ToolCall::default());
            }
            let call = &mut reply.tool_calls[index];
            if let Some(id) = piece["id"].as_str().filter(|id| !id.is_empty()) {
                call.id = id.to_owned();
            }
            if let Some(name) = piece["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                call.name.push_str(name);
                on_piece(Piece::Calling(&call.name));
            }
            if let Some(arguments) = piece["function"]["arguments"].as_str() {
                call.arguments.push_str(arguments);
            }
        }
    }
    reply.tool_calls.retain(|call| !call.name.is_empty());
    // Some local servers leave the id out, the tool result still has to point at something.
    for (index, call) in reply.tool_calls.iter_mut().enumerate() {
        if call.id.is_empty() {
            call.id = format!("call_{index}");
        }
    }
    Ok(reply)
}
