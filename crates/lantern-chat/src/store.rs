//! Saved conversations. One SQLite file next to the index, one row per chat and one per
//! message. Messages are stored the way they go to the provider, so a chat that is opened
//! again is the same prefix the provider cached, and tool calls keep their results.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::client::Usage;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chats (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    model TEXT NOT NULL,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    cost REAL
) STRICT;
CREATE TABLE IF NOT EXISTS messages (
    chat_id INTEGER NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (chat_id, position)
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug)]
pub struct ChatInfo {
    pub id: i64,
    pub title: String,
    pub model: String,
    pub updated: i64,
    pub messages: usize,
}

pub struct ChatStore {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("chats.db")
}

impl ChatStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let conn = Connection::open(path(data_dir))?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<ChatInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.model, c.updated, (SELECT count(*) FROM messages m WHERE m.chat_id = c.id)
             FROM chats c ORDER BY c.updated DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ChatInfo { id: r.get(0)?, title: r.get(1)?, model: r.get(2)?, updated: r.get(3)?, messages: r.get::<_, i64>(4)? as usize })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The messages of a chat, without the system prompt, in order.
    pub fn load(&self, id: i64) -> Result<Vec<Value>> {
        let mut stmt = self.conn.prepare("SELECT json FROM messages WHERE chat_id = ?1 ORDER BY position")?;
        let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|row| serde_json::from_str(&row.ok()?).ok()).collect())
    }

    pub fn usage(&self, id: i64) -> Result<Usage> {
        Ok(self.conn.query_row("SELECT prompt_tokens, cached_tokens, completion_tokens, cost FROM chats WHERE id = ?1", params![id], |r| {
            Ok(Usage {
                prompt: r.get::<_, i64>(0)? as u64,
                cached: r.get::<_, i64>(1)? as u64,
                cache_written: 0,
                completion: r.get::<_, i64>(2)? as u64,
                cost: r.get(3)?,
            })
        })?)
    }

    /// Writes a chat, creating it when `id` is None, and returns its id. The messages of the
    /// chat are replaced, which is what a regenerated turn needs.
    pub fn save(&mut self, id: Option<i64>, title: &str, model: &str, messages: &[Value], total: Usage) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = match id {
            Some(id) => {
                tx.execute(
                    "UPDATE chats SET title = ?2, model = ?3, updated = ?4, prompt_tokens = ?5, cached_tokens = ?6, completion_tokens = ?7, cost = ?8
                     WHERE id = ?1",
                    params![id, title, model, now(), total.prompt as i64, total.cached as i64, total.completion as i64, total.cost],
                )?;
                tx.execute("DELETE FROM messages WHERE chat_id = ?1", params![id])?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO chats (title, model, created, updated, prompt_tokens, cached_tokens, completion_tokens, cost)
                     VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7)",
                    params![title, model, now(), total.prompt as i64, total.cached as i64, total.completion as i64, total.cost],
                )?;
                tx.last_insert_rowid()
            }
        };
        {
            let mut insert = tx.prepare("INSERT INTO messages (chat_id, position, json) VALUES (?1, ?2, ?3)")?;
            for (position, message) in messages.iter().enumerate() {
                insert.execute(params![id, position as i64, serde_json::to_string(message)?])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE chat_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM chats WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn rename(&self, id: i64, title: &str) -> Result<()> {
        self.conn.execute("UPDATE chats SET title = ?2 WHERE id = ?1", params![id, title])?;
        Ok(())
    }
}

/// What a chat is called: its first question, on one line.
pub fn title_of(messages: &[Value]) -> String {
    let first = messages.iter().find(|m| m["role"] == "user").and_then(|m| m["content"].as_str()).unwrap_or("Chat");
    let one_line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    match one_line.char_indices().nth(70) {
        Some((cut, _)) => format!("{}...", one_line[..cut].trim_end()),
        None => one_line,
    }
}

/// "4m", "3h", "yesterday", "12 Mar".
pub fn ago(timestamp: i64) -> String {
    let seconds = (now() - timestamp).max(0);
    match seconds {
        ..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        86400..172800 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86400),
    }
}
