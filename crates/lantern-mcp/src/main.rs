use std::io::{BufRead, Write};

use anyhow::Result;
use lantern_chat::tools::{TOOLS, Toolbox, schema};
use serde_json::{Value, json};

fn tool_list() -> Value {
    let tools: Vec<Value> = TOOLS.iter().map(|tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": schema(tool) })).collect();
    json!({ "tools": tools })
}

const MAX_OUTPUT: usize = 60_000;

fn main() -> Result<()> {
    let explicit = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    serve(Toolbox::open(&lantern_core::db_path(explicit))?)
}

fn serve(mut toolbox: Toolbox) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = request.get("id").cloned() else { continue };

        let result = match request["method"].as_str().unwrap_or_default() {
            "initialize" => Ok(json!({
                "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "lantern", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tool_list()),
            "tools/call" => {
                let params = &request["params"];
                Ok(match toolbox.run(params["name"].as_str().unwrap_or_default(), &params["arguments"], MAX_OUTPUT) {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                    Err(err) => json!({ "content": [{ "type": "text", "text": format!("{err:#}") }], "isError": true }),
                })
            }
            other => Err(format!("method not found: {other}")),
        };

        let response = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(message) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": message } }),
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}
