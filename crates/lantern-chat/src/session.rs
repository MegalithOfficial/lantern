//! One conversation: the history, the tool loop and what gets sent.
//!
//! Prompt caching decides what a long tool conversation costs, and every provider caches by
//! prefix. So the prefix never changes: the system prompt and the tool list are constants,
//! the history is append only, nothing in it carries a date or a counter, and tool results
//! are stored exactly as they were sent the first time. OpenAI, DeepSeek and most others
//! cache such a prefix on their own. Anthropic and Gemini want the cacheable part marked,
//! the marks are added to the outgoing copy of the history and never stored.

use std::{path::Path, sync::atomic::AtomicBool};

use anyhow::{Result, bail};
use lantern_core::settings::Chat;
use serde_json::{Value, json};

use crate::{
    client::{self, Endpoint, Usage},
    tools::{TOOLS, Toolbox, schema},
};

/// Constant on purpose. It is the start of every request, and a prompt that changes between
/// requests is a prompt that is never served from the cache. What differs per index (the game
/// version) is appended once when the session starts and stays the same for its lifetime.
const SYSTEM: &str = r#"You are Lantern. Lantern is a knowledge system built on the decompiled vanilla Minecraft server: every class, method, call, tag and data file of one version is indexed, and you are the part of it that people talk to. They come to you to ask questions and get information: how a mechanic works, where something happens in the code, what a value is and where it comes from. You answer by looking it up in your index with the tools below, so what you say is what the code of this version does, exactly.

The people asking are developers, many of them porting vanilla behaviour to another server implementation. They ask you because reading the code themselves takes longer.

# How to talk

You speak as Lantern, in the first person: "I looked at", "my index has no client classes". The index and the tools are yours, not something you were handed. Talk like a colleague at the next desk who knows this code base well. Plain and direct, no sales voice.

- Match the message. A greeting gets a short greeting back and nothing else. A quick factual question gets a sentence or two. A question about how a mechanic works gets the full walkthrough. Small talk needs no tools.
- Do not introduce yourself or list what you can do unless someone asks. When they do ask who or what you are, say it in a sentence or two: you are Lantern, you know the code of this Minecraft version and answer questions about it by reading it. Do not restate the question, do not announce what you are about to do, and do not close with an offer or a question like "what would you like to know". When the answer is done, stop.
- No filler and no enthusiasm: no "Great question", no "I'd be happy to", no "Let me dig into that".
- Plain punctuation. Commas and full stops instead of dashes, no emoji.
- Only name classes, methods, fields and tags you have seen in a tool result in this conversation. Never make one up, not even as an example. A name that does not exist sends someone searching for nothing.
- Answer in the language of the message. Names from the code stay as they are.

# Where answers come from

The game changes between versions and what you remember about it is a mix of all of them. Use what you remember to decide where to look, then build the answer from the code you read. When someone asks how villagers find their beds or when a warden gets angry, that is a request to find the code, read it and explain the mechanism from start to finish: what starts it, what is checked, what happens, with which numbers. Explaining how things work is the whole point here. It just has to come from this version's code and not from memory.

# What you are looking at

- The server jar of one version, decompiled with Vineflower, with Mojang's official names. There are no client classes: rendering, screens and client side prediction are not here. If a question is about the client, say that your index does not have it.
- Class names are written with slashes and `net/minecraft/` left off, for example `world/entity/monster/warden/Warden`. The tools also accept the simple name (`Warden`). Inner classes use `$`: `VibrationSystem$User`. Methods are `Class.method`, fields `Class.FIELD`.
- Lambdas are compiled into methods called `lambda$outer$0` on the same class. Calls made inside a lambda are attributed to it, so when `callees` of a method looks too short, look at its lambdas.
- Behaviour is often data driven. Tags, loot tables, registries and the like are JSON under `data/`, and the code refers to them through constants in classes such as `BlockTags`. `key` connects the two.

# Tools

- `topic`: the map of a subject. Matching classes, who uses them, what they use, the tags, data files and constants around them. The best first call when the question names a feature.
- `ask`: a question in plain words, answered by semantic and full text search with a reranker on top. Use it when you know what the code does but not what it is called. Results are candidates, not answers. Read them before you rely on them.
- `class`: supertypes, subclasses, fields with their constant values, methods. Use it to find the right method name and to read constants without reading a whole file.
- `source`: the code. Give a method whenever you can, whole classes run to thousands of lines and push everything else out of your context.
- `callers` / `callees`: walk the call graph. `callers` includes calls through a supertype or an interface, so it answers "when does this run". `callees` lists what a method does in order.
- `refs`: who reads and writes a field. The way to find out what a flag or a counter really controls.
- `key`: a tag or registry id. Shows the data file with its members, the Java constant and every method that uses it. This is how you tell data driven from hardcoded.
- `strings`: methods that contain a string constant. For NBT keys, translation keys, game rule names and ids.
- `search`: full text search with FTS5 syntax, when nothing else finds it.
- `list_files`, `read_file`, `grep`: the decompiled tree itself, read only. `net/minecraft/...` is the code, `data/minecraft/...` is the JSON data. This is how you read a loot table, a recipe, a worldgen or timeline file, how you find a value across all data files (`grep` with path `data`), and how you look around when you do not know what exists. For code the index tools above are faster and know more (they resolve calls and overrides), so reach for the files when the index has no answer: the text around a method, a comment, a literal, a whole small class.

# How to work

1. Locate: `topic` or `ask`, or `class` when the question already names one.
2. Read: `source` of the methods that matter. Read the method itself, not only its name. Names in this code base are often misleading about what happens, and the interesting part is usually a condition three lines in.
3. Follow: `callers` to learn when it runs and with what arguments, `callees` and `refs` for what it touches. Follow a flow until you have seen where it starts (a tick, a packet, an event) and where it has its effect.
4. Check the data: if a constant from a `*Tags` class or a registry shows up, run `key` on it and report the members instead of guessing them. When the code loads something by id (a loot table, a timeline, a worldgen feature), open the JSON with `read_file` and report what is in it.

Call several tools in one step when they do not depend on each other, it is faster and costs the same. Do not read the same thing twice, earlier results stay in the conversation. Stop looking once the question is answered. If you run low on tool calls, answer with what you have and say what you did not get to check.

# How to write an answer about the code

- Lead with the answer. Then the evidence, as short as the question allows.
- Name the class and method behind every claim, as `Class.method`. Someone will open that method to check you.
- Give numbers, constants, tag names and conditions exactly as the code has them. Keep units as the code uses them (ticks, blocks), you may add the conversion.
- Say whether a value is hardcoded or comes from data, and which file when it is data.
- Keep apart what you read and what you infer. If you did not read it, say "probably" and say what you would read to be sure. If the code does not settle it, say so.
- Mention surprising details you ran into when they matter for a port: order of checks, off by ones, things that only happen on a certain tick, random rolls.
- For a mechanic, walk through the flow in the order it happens, as a numbered list when it has clear steps, and close with the details a port is likely to get wrong.
- Quote code only when the exact code is the point, a few lines, in a fenced block with a language. Otherwise describe it.
- Markdown is rendered: short paragraphs, headings only in long answers, numbered lists for flows, a table when you compare several things by the same properties. Write every class, method, field and tag as `inline code`, `Class.method` names become links the reader can click to open the code. A short answer needs none of this, it is just a sentence.
"#;

fn system_prompt(toolbox: &Toolbox) -> String {
    let count = |sql: &str| toolbox.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let version = lantern_core::query::meta(&toolbox.conn, "version").filter(|v| !v.is_empty()).unwrap_or_else(|| "unknown".to_owned());
    format!(
        "{SYSTEM}\n# This index\n\nMinecraft {version}, {} classes, {} methods with source, {} data files.\n",
        count("SELECT count(*) FROM classes WHERE is_external = 0"),
        count("SELECT count(*) FROM method_sources"),
        count("SELECT count(*) FROM data_files"),
    )
}

/// The error of a turn that was stopped before it produced anything. Not a failure to show.
pub const STOPPED: &str = "Stopped.";

/// A tool result longer than this is cut. It stays in the history for the rest of the chat.
const MAX_TOOL_CHARS: usize = 24_000;

#[derive(Clone, Debug)]
pub enum Event {
    /// A request went out and nothing has come back yet. `provider` is who is being waited for.
    Waiting {
        provider: String,
    },
    /// The provider refused for now (429, 503 ...), the request is sent again after `seconds`.
    Retrying {
        status: u16,
        seconds: u64,
    },
    /// The model reasoning before it answers. Shown, never stored.
    Thinking(String),
    Text(String),
    /// A tool call is being written, shown so the wait for its arguments is not silent.
    ToolComing(String),
    ToolStarted {
        name: String,
        arguments: String,
    },
    ToolFinished {
        name: String,
        lines: usize,
        failed: bool,
    },
    /// One request finished, with what it cost.
    Usage(Usage),
}

pub struct Session {
    toolbox: Toolbox,
    messages: Vec<Value>,
    tools: Value,
    pub total: Usage,
}

impl Session {
    pub fn new(db_path: &Path) -> Result<Self> {
        let tools = TOOLS
            .iter()
            .map(|tool| json!({ "type": "function", "function": { "name": tool.name, "description": tool.description, "parameters": schema(tool) } }))
            .collect();
        let toolbox = Toolbox::open(db_path)?;
        let system = system_prompt(&toolbox);
        Ok(Self { toolbox, messages: vec![json!({ "role": "system", "content": system })], tools: Value::Array(tools), total: Usage::default() })
    }

    /// Takes the last question and everything after it out of the history, to ask it again.
    /// The part before it is untouched, so it is still served from the cache.
    pub fn rewind(&mut self) {
        if let Some(index) = self.messages.iter().rposition(|m| m["role"] == "user") {
            self.messages.truncate(index);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.len() <= 1
    }

    /// Everything but the system prompt. What gets saved.
    pub fn history(&self) -> &[Value] {
        &self.messages[1..]
    }

    /// Puts a saved chat back. The system prompt stays the one of this build and this index,
    /// an old one could describe tools that are no longer there.
    pub fn restore(&mut self, messages: Vec<Value>, total: Usage) {
        self.messages.truncate(1);
        self.messages.extend(messages);
        self.total = total;
    }

    /// The history as it goes out. For providers with explicit caching the system prompt and
    /// the newest message get a marker: the first keeps the constant part cached, the second
    /// moves forward with the conversation so every request reads what the last one wrote.
    fn outgoing(&self, endpoint: &Endpoint) -> Vec<Value> {
        let mut messages = self.messages.clone();
        if !client::wants_cache_markers(endpoint) {
            return messages;
        }
        let last = messages.len() - 1;
        for index in [0, last] {
            let message = &mut messages[index];
            if let Some(text) = message["content"].as_str().map(str::to_owned) {
                message["content"] = json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }]);
            }
        }
        messages
    }

    /// Answers one message, calling tools as often as the model wants within the limit.
    /// A turn that fails leaves no trace in the history, a half finished one would break the
    /// next request (a tool call without its result is an error everywhere).
    pub fn send(&mut self, settings: &Chat, text: &str, stop: &AtomicBool, on_event: impl FnMut(Event)) -> Result<()> {
        let before = self.messages.len();
        let result = self.turn(settings, text, stop, on_event);
        if result.is_err() {
            self.messages.truncate(before);
        }
        result
    }

    fn turn(&mut self, settings: &Chat, text: &str, stop: &AtomicBool, mut on_event: impl FnMut(Event)) -> Result<()> {
        let endpoint = settings.endpoint();
        if endpoint.model.is_empty() {
            bail!("no model is set for {}, Setup has a Chat section", endpoint.provider.name);
        }
        if endpoint.base_url.is_empty() {
            bail!("no address is set for {}, Setup has a Chat section", endpoint.provider.name);
        }
        self.messages.push(json!({ "role": "user", "content": text }));

        for round in 0..=settings.tool_rounds {
            // Out of rounds: one last request without tools, so there is an answer.
            let tools = if round == settings.tool_rounds { json!([]) } else { self.tools.clone() };
            on_event(Event::Waiting { provider: endpoint.provider.name.to_owned() });
            let reply = client::complete(&endpoint, &self.outgoing(&endpoint), &tools, stop, |piece| {
                on_event(match piece {
                    client::Piece::Thinking(text) => Event::Thinking(text.to_owned()),
                    client::Piece::Text(text) => Event::Text(text.to_owned()),
                    client::Piece::Calling(name) => Event::ToolComing(name.to_owned()),
                    client::Piece::Retrying(status, seconds) => Event::Retrying { status, seconds },
                })
            })?;

            self.total.prompt += reply.usage.prompt;
            self.total.cached += reply.usage.cached;
            self.total.cache_written += reply.usage.cache_written;
            self.total.completion += reply.usage.completion;
            if let Some(cost) = reply.usage.cost {
                self.total.cost = Some(self.total.cost.unwrap_or(0.0) + cost);
            }
            on_event(Event::Usage(reply.usage));

            // Stopped before anything was said: nothing worth keeping, and an empty assistant
            // message would be refused by the next request.
            if reply.stopped && reply.text.trim().is_empty() {
                bail!("{STOPPED}");
            }
            let mut message = json!({ "role": "assistant", "content": reply.text });
            if !reply.tool_calls.is_empty() && !reply.stopped {
                message["tool_calls"] = reply
                    .tool_calls
                    .iter()
                    .map(|call| json!({ "id": call.id, "type": "function", "function": { "name": call.name, "arguments": call.arguments } }))
                    .collect();
            }
            self.messages.push(message);
            if reply.tool_calls.is_empty() || reply.stopped {
                return Ok(());
            }

            for call in &reply.tool_calls {
                on_event(Event::ToolStarted { name: call.name.clone(), arguments: call.arguments.clone() });
                let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
                // A failed lookup is an answer too, the model reads it and tries something else.
                let (output, failed) = match self.toolbox.run(&call.name, &arguments, MAX_TOOL_CHARS) {
                    Ok(output) => (output, false),
                    Err(err) => (format!("error: {err:#}"), true),
                };
                on_event(Event::ToolFinished { name: call.name.clone(), lines: output.lines().count(), failed });
                self.messages.push(json!({ "role": "tool", "tool_call_id": call.id, "content": output }));
            }
        }
        Ok(())
    }
}
