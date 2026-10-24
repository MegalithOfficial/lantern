use std::{path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use clap::ValueEnum;

#[derive(Clone, Copy, ValueEnum)]
pub enum Client {
    #[value(name = "claudecode")]
    ClaudeCode,
    Codex,
}

pub fn run(client: Client, database: &Path, mcp_bin: Option<&Path>) -> Result<()> {
    let binary = match mcp_bin {
        Some(path) => path.to_path_buf(),
        None => std::env::current_exe().context("locating the Lantern executable")?.with_file_name(format!("lantern-mcp{}", std::env::consts::EXE_SUFFIX)),
    };
    let binary = binary.canonicalize().with_context(|| {
        format!("locating MCP server at {}; build both binaries with `cargo build --release -p lantern-cli -p lantern-mcp` or pass --mcp-bin", binary.display())
    })?;
    ensure!(binary.is_file(), "MCP server is not a file: {}", binary.display());
    let database = std::path::absolute(database).context("resolving the index database path")?;

    let (program, label) = match client {
        Client::ClaudeCode => ("claude", "Claude Code"),
        Client::Codex => ("codex", "Codex"),
    };
    let mut command = Command::new(program);
    command.args(["mcp", "add"]);
    if matches!(client, Client::ClaudeCode) {
        command.args(["--transport", "stdio", "--scope", "user"]);
    }
    let status = command
        .args(["lantern", "--"])
        .arg(&binary)
        .arg(&database)
        .status()
        .with_context(|| format!("running {program}; make sure the {label} CLI is installed and available on PATH"))?;
    ensure!(status.success(), "{label} could not register Lantern ({status})");

    println!("Lantern MCP registered with {label}.");
    println!("Server: {}", binary.display());
    println!("Database: {}", database.display());
    println!("Start a new {label} session and run /mcp to check the connection.");
    Ok(())
}
