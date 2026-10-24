use anyhow::Result;
use lantern_core::{db::Connection, index::humanize};
use rusqlite::params;

use crate::engine::Engine;

#[derive(Clone, Debug)]
pub struct Chunk {
    pub class_id: i64,
    /// None for the summary of a class.
    pub method_id: Option<i64>,
    pub part: usize,
    pub parts: usize,
    pub title: String,
    /// Exactly what goes into the model, prefix included.
    pub text: String,
    pub hash: u64,
}

/// FNV-1a. Has to stay the same between runs and Rust versions, the stored hashes decide
/// what gets embedded again.
pub fn stable_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn tail(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

pub fn class_summary(conn: &Connection, class_id: i64, name: &str) -> Result<String> {
    let list = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        Ok(stmt.query_map(params![class_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    };
    let parents = list(
        "SELECT c.name FROM ancestors a JOIN classes c ON c.id = a.ancestor_id
         WHERE a.class_id = ?1 AND a.depth > 0 AND c.name != 'java/lang/Object' ORDER BY a.depth LIMIT 10",
    )?;
    let fields = list("SELECT name FROM fields WHERE class_id = ?1 AND is_external = 0 AND name NOT LIKE '%$%' ORDER BY id LIMIT 60")?;
    let methods = list(
        "SELECT DISTINCT name FROM methods WHERE class_id = ?1 AND is_external = 0
         AND name NOT LIKE '%$%' AND name NOT LIKE '<%' ORDER BY id LIMIT 80",
    )?;
    let uses = list(
        "SELECT tc.name FROM calls JOIN methods cm ON cm.id = calls.caller_id
         JOIN methods tm ON tm.id = calls.callee_id JOIN classes tc ON tc.id = tm.class_id
         WHERE cm.class_id = ?1 AND tc.is_external = 0 AND tc.id != ?1
         GROUP BY tc.id ORDER BY count(*) DESC LIMIT 12",
    )?;
    let keys = list(
        "SELECT DISTINCT k.ident FROM field_refs fr JOIN methods m ON m.id = fr.method_id
         JOIN key_fields k ON k.field_id = fr.field_id WHERE m.class_id = ?1 LIMIT 24",
    )?;

    let simple = tail(name);
    let package = name.rsplit_once('/').map_or("", |(p, _)| p).replace('/', ".");
    let mut out = format!("class {simple} ({}) in {package}\n", humanize(simple));
    for (label, items) in [("extends and implements", &parents), ("uses", &uses)] {
        if !items.is_empty() {
            out.push_str(&format!("{label}: {}\n", items.iter().map(|n| tail(n)).collect::<Vec<_>>().join(", ")));
        }
    }
    for (label, items) in [("fields", &fields), ("methods", &methods), ("tags and registry keys", &keys)] {
        if !items.is_empty() {
            out.push_str(&format!("{label}: {}\n", items.join(", ")));
        }
    }
    Ok(out)
}

const MIN_METHOD_LINES: usize = 4;
const OVERLAP_LINES: usize = 6;

/// Splits `body` so that header plus piece stays inside the chunk budget of the model.
/// Pieces break on line boundaries and repeat a few lines of the previous piece.
fn split_to_budget(engine: &Engine, header: &str, body: &str) -> Result<Vec<String>> {
    let spec = engine.spec;
    let limit = spec.chunk_tokens.min(spec.context_tokens);

    // Most methods fit in one piece. Only the long ones pay for counting line by line.
    if engine.count_tokens(&engine.document_text("x", &format!("{header}{body}")))? <= limit {
        return Ok(vec![body.to_owned()]);
    }

    let header_cost = engine.count_tokens(&engine.document_text("x", header))?;
    let budget = limit.saturating_sub(header_cost + 8).max(64);
    let lines: Vec<&str> = body.lines().collect();
    let mut costs = Vec::with_capacity(lines.len());
    for line in &lines {
        costs.push(engine.count_tokens(line)?.saturating_sub(1).max(1));
    }

    let mut pieces = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut used = 0;
        while end < lines.len() && (used + costs[end] <= budget || end == start) {
            used += costs[end];
            end += 1;
        }
        let mut piece = lines[start..end].join("\n");
        // A single line can be longer than the whole budget (long string tables). Cut it by
        // characters until it fits, there is nothing smarter to do with it.
        while engine.count_tokens(&engine.document_text("x", &format!("{header}{piece}")))? > spec.context_tokens {
            let mut cut = piece.len() / 2;
            while !piece.is_char_boundary(cut) {
                cut -= 1;
            }
            piece.truncate(cut);
        }
        pieces.push(piece);
        if end >= lines.len() {
            break;
        }
        start = end - OVERLAP_LINES.min((end - start) / 3);
    }
    Ok(pieces)
}

pub fn build_chunks(conn: &Connection, engine: &Engine, mut on_progress: impl FnMut(usize, usize)) -> Result<Vec<Chunk>> {
    let mut chunks = Vec::new();

    let mut stmt = conn.prepare("SELECT id, name FROM classes WHERE is_external = 0 AND simple NOT GLOB '*$[0-9]*' ORDER BY id")?;
    let classes: Vec<(i64, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT m.id, m.class_id, c.name, m.name, s.text FROM method_sources s
         JOIN methods m ON m.id = s.method_id JOIN classes c ON c.id = m.class_id ORDER BY m.id",
    )?;
    let methods: Vec<(i64, i64, String, String, String)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;

    let total = classes.len() + methods.len();
    for (index, (class_id, name)) in classes.iter().enumerate() {
        if index % 256 == 0 {
            on_progress(index, total);
        }
        let summary = class_summary(conn, *class_id, name)?;
        push_chunks(engine, &mut chunks, *class_id, None, tail(name), "", &summary)?;
    }

    for (index, (method_id, class_id, class, method, text)) in methods.iter().enumerate() {
        if index % 256 == 0 {
            on_progress(classes.len() + index, total);
        }
        if text.lines().count() < MIN_METHOD_LINES {
            continue;
        }
        let simple = tail(class);
        let name = if method == "<init>" { "constructor" } else { method.as_str() };
        let title = format!("{simple}.{name}");
        let header = format!("// {}\n// {} {}\n", class.replace('/', "."), humanize(simple), humanize(name));
        push_chunks(engine, &mut chunks, *class_id, Some(*method_id), &title, &header, text)?;
    }
    on_progress(total, total);
    Ok(chunks)
}

fn push_chunks(engine: &Engine, chunks: &mut Vec<Chunk>, class_id: i64, method_id: Option<i64>, title: &str, header: &str, body: &str) -> Result<()> {
    let pieces = split_to_budget(engine, header, body)?;
    let parts = pieces.len();
    for (part, piece) in pieces.into_iter().enumerate() {
        let text = engine.document_text(title, &format!("{header}{piece}"));
        chunks.push(Chunk { class_id, method_id, part, parts, title: title.to_owned(), hash: stable_hash(&text), text });
    }
    Ok(())
}
