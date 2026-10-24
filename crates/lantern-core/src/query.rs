//! Structured queries over the index. What the CLI, the MCP server and Lookup show of them is
//! built in `render`.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rusqlite::{Connection, params};

#[derive(Clone, Debug)]
pub struct ClassHit {
    pub id: i64,
    pub name: String,
    pub is_external: bool,
}

#[derive(Clone, Debug)]
pub struct MethodRef {
    pub id: i64,
    pub class: String,
    pub name: String,
    pub descriptor: String,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub id: i64,
    pub name: String,
    pub descriptor: String,
    pub constant: Option<String>,
    /// Tag or registry id this field stands for.
    pub key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub id: i64,
    pub name: String,
    pub descriptor: String,
    pub has_source: bool,
    pub overrides: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub id: i64,
    pub name: String,
    pub super_class: Option<String>,
    pub interfaces: Vec<String>,
    pub subclasses: Vec<String>,
    pub inner: Vec<String>,
    pub fields: Vec<FieldInfo>,
    pub methods: Vec<MethodInfo>,
}

#[derive(Clone, Debug)]
pub struct CallSite {
    pub caller: MethodRef,
    pub callee: MethodRef,
    /// Static type at the call site when it differs from where the callee is declared.
    pub receiver: String,
    pub kind: String,
    pub line: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct FieldUse {
    pub method: MethodRef,
    pub is_write: bool,
    pub line: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct DataFile {
    pub registry: String,
    pub ident: String,
    pub path: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct KeyHolder {
    pub class: String,
    pub field: String,
    pub via: Option<String>,
    pub uses: Vec<FieldUse>,
}

#[derive(Clone, Debug, Default)]
pub struct KeyInfo {
    pub ident: String,
    pub files: Vec<DataFile>,
    /// Tags that list this id, as (registry, tag).
    pub member_of: Vec<(String, String)>,
    pub holders: Vec<KeyHolder>,
}

#[derive(Clone, Debug)]
pub struct StringHit {
    pub value: String,
    pub method: MethodRef,
    pub line: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub class: String,
    /// None for a hit on a whole class.
    pub method: Option<MethodRef>,
    pub snippet: String,
}

#[derive(Clone, Debug, Default)]
pub struct Topic {
    pub seed: String,
    pub classes: Vec<String>,
    pub used_by: Vec<(String, usize)>,
    pub uses: Vec<(String, usize)>,
    /// (holder class, field, id)
    pub keys: Vec<(String, String, String)>,
    pub data: Vec<(String, String)>,
    /// (class, field, value)
    pub constants: Vec<(String, String, String)>,
}

/// `Warden`, `monster.warden.Warden` and `net/minecraft/.../Warden` all work, and so does `Inner`
/// for `Outer$Inner`. Exact matches come first, classes from the jar before external ones.
pub fn find_classes(conn: &Connection, query: &str) -> Result<Vec<ClassHit>> {
    let query = query.trim().replace('.', "/");
    let mut stmt = conn.prepare(
        "SELECT id, name, is_external FROM classes
         WHERE name = ?1 OR simple = ?1 OR name LIKE '%/' || ?1 OR simple LIKE '%$' || ?1
         ORDER BY (name = ?1) DESC, (simple = ?1) DESC, is_external, length(name)
         LIMIT 50",
    )?;
    let hits: Vec<ClassHit> =
        stmt.query_map(params![query], |r| Ok(ClassHit { id: r.get(0)?, name: r.get(1)?, is_external: r.get(2)? }))?.collect::<rusqlite::Result<_>>()?;
    Ok(hits)
}

/// Every class of the game that has a file of its own, sorted. Inner classes are left out,
/// they are reached through their outer class.
pub fn top_level_classes(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM classes WHERE is_external = 0 AND instr(name, '$') = 0 ORDER BY name")?;
    let names = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

/// Classes whose simple name contains the text, for search boxes.
pub fn suggest_classes(conn: &Connection, text: &str, limit: usize) -> Result<Vec<ClassHit>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, is_external FROM classes
         WHERE is_external = 0 AND simple LIKE '%' || ?1 || '%'
         ORDER BY (simple = ?1) DESC, (simple LIKE ?1 || '%') DESC, length(simple), name
         LIMIT ?2",
    )?;
    let hits = stmt
        .query_map(params![text.trim(), limit as i64], |r| Ok(ClassHit { id: r.get(0)?, name: r.get(1)?, is_external: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(hits)
}

fn one_class(conn: &Connection, query: &str) -> Result<ClassHit> {
    match find_classes(conn, query)?.into_iter().next() {
        Some(hit) => Ok(hit),
        None => bail!("no class matches `{query}`"),
    }
}

pub fn split_member(target: &str) -> Result<(&str, &str)> {
    let target = target.trim().trim_end_matches("()");
    match target.rsplit_once(['.', '#']) {
        Some((class, member)) if !class.is_empty() && !member.is_empty() => Ok((class, member)),
        _ => bail!("expected Class.member, got `{target}`"),
    }
}

pub fn class_info(conn: &Connection, query: &str) -> Result<ClassInfo> {
    let hit = one_class(conn, query)?;
    let names = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        Ok(stmt.query_map(params![hit.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    };

    let super_class = conn.query_row("SELECT p.name FROM classes c JOIN classes p ON p.id = c.super_id WHERE c.id = ?1", params![hit.id], |r| r.get(0)).ok();
    let interfaces = names("SELECT c.name FROM interfaces i JOIN classes c ON c.id = i.interface_id WHERE i.class_id = ?1 ORDER BY c.name")?;
    let subclasses = names(
        "SELECT c.name FROM classes c WHERE c.super_id = ?1
         UNION SELECT c.name FROM interfaces i JOIN classes c ON c.id = i.class_id WHERE i.interface_id = ?1
         ORDER BY 1 LIMIT 80",
    )?;
    let inner = names("SELECT name FROM classes WHERE outer_id = ?1 ORDER BY name")?;

    let mut stmt = conn.prepare(
        "SELECT f.id, f.name, f.descriptor, f.constant, k.ident FROM fields f
         LEFT JOIN key_fields k ON k.field_id = f.id WHERE f.class_id = ?1 ORDER BY f.id",
    )?;
    let fields = stmt
        .query_map(params![hit.id], |r| Ok(FieldInfo { id: r.get(0)?, name: r.get(1)?, descriptor: r.get(2)?, constant: r.get(3)?, key: r.get(4)? }))?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT m.id, m.name, m.descriptor, s.method_id IS NOT NULL, oc.name FROM methods m
         LEFT JOIN method_sources s ON s.method_id = m.id
         LEFT JOIN methods o ON o.id = m.overrides_id LEFT JOIN classes oc ON oc.id = o.class_id
         WHERE m.class_id = ?1 ORDER BY m.line_start IS NULL, m.line_start, m.id",
    )?;
    let methods = stmt
        .query_map(params![hit.id], |r| Ok(MethodInfo { id: r.get(0)?, name: r.get(1)?, descriptor: r.get(2)?, has_source: r.get(3)?, overrides: r.get(4)? }))?
        .collect::<rusqlite::Result<_>>()?;

    Ok(ClassInfo { id: hit.id, name: hit.name, super_class, interfaces, subclasses, inner, fields, methods })
}

/// Methods called `member` in the class, or the nearest inherited ones if it declares none.
pub fn find_methods(conn: &Connection, class_query: &str, member: &str) -> Result<Vec<MethodRef>> {
    let hit = one_class(conn, class_query)?;
    let mut stmt = conn.prepare(
        "SELECT m.id, c.name, m.name, m.descriptor FROM ancestors a
         JOIN methods m ON m.class_id = a.ancestor_id JOIN classes c ON c.id = m.class_id
         WHERE a.class_id = ?1 AND m.name = ?2
         ORDER BY a.depth, m.id",
    )?;
    let all: Vec<(i64, MethodRef)> = stmt
        .query_map(params![hit.id, member], |r| {
            Ok((r.get::<_, i64>(0)?, MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let Some(nearest) = all.first().map(|(_, m)| m.class.clone()) else {
        bail!("{} has no method `{member}`, inherited ones included", hit.name);
    };
    Ok(all.into_iter().map(|(_, m)| m).filter(|m| m.class == nearest).collect())
}

/// Every declaration that a call to `method` can end up in, or that can end up in `method`:
/// same name and descriptor anywhere above or below its class.
fn family_sql() -> &'static str {
    "SELECT m2.id FROM methods m1 JOIN methods m2 ON m2.name = m1.name AND m2.descriptor = m1.descriptor
     WHERE m1.id = ?1 AND (
         m2.id = m1.id
         OR (m1.flags & 10 = 0 AND m1.name NOT LIKE '<%' AND (
             m2.class_id IN (SELECT ancestor_id FROM ancestors WHERE class_id = m1.class_id)
             OR m2.class_id IN (SELECT class_id FROM ancestors WHERE ancestor_id = m1.class_id))))"
}

fn call_sites(conn: &Connection, sql: &str, id: i64) -> Result<Vec<CallSite>> {
    let mut stmt = conn.prepare(sql)?;
    let sites = stmt
        .query_map(params![id], |r| {
            Ok(CallSite {
                caller: MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? },
                callee: MethodRef { id: r.get(4)?, class: r.get(5)?, name: r.get(6)?, descriptor: r.get(7)? },
                receiver: r.get(8)?,
                kind: r.get(9)?,
                line: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(sites)
}

const CALL_COLUMNS: &str = "cm.id, cc.name, cm.name, cm.descriptor, tm.id, tc.name, tm.name, tm.descriptor, rc.name, calls.kind, calls.line
     FROM calls
     JOIN methods cm ON cm.id = calls.caller_id JOIN classes cc ON cc.id = cm.class_id
     JOIN methods tm ON tm.id = calls.callee_id JOIN classes tc ON tc.id = tm.class_id
     JOIN classes rc ON rc.id = calls.receiver_id";

pub fn callers(conn: &Connection, method_id: i64) -> Result<Vec<CallSite>> {
    let sql = format!("SELECT {CALL_COLUMNS} WHERE calls.callee_id IN ({}) ORDER BY cc.name, cm.name, calls.line", family_sql());
    call_sites(conn, &sql, method_id)
}

pub fn callees(conn: &Connection, method_id: i64) -> Result<Vec<CallSite>> {
    let sql = format!("SELECT {CALL_COLUMNS} WHERE calls.caller_id = ?1 ORDER BY calls.rowid");
    call_sites(conn, &sql, method_id)
}

/// Implementations of `method` further down the hierarchy.
pub fn overriders(conn: &Connection, method_id: i64) -> Result<Vec<MethodRef>> {
    let mut stmt = conn.prepare(
        "SELECT m2.id, c.name, m2.name, m2.descriptor FROM methods m1
         JOIN methods m2 ON m2.name = m1.name AND m2.descriptor = m1.descriptor AND m2.id != m1.id
         JOIN classes c ON c.id = m2.class_id
         WHERE m1.id = ?1 AND m2.class_id IN (SELECT class_id FROM ancestors WHERE ancestor_id = m1.class_id)
         ORDER BY c.name",
    )?;
    let rows = stmt
        .query_map(params![method_id], |r| Ok(MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn find_field(conn: &Connection, class_query: &str, field: &str) -> Result<(i64, String)> {
    let hit = one_class(conn, class_query)?;
    let found = conn.query_row(
        "SELECT f.id, c.name FROM ancestors a JOIN fields f ON f.class_id = a.ancestor_id JOIN classes c ON c.id = f.class_id
         WHERE a.class_id = ?1 AND f.name = ?2 ORDER BY a.depth LIMIT 1",
        params![hit.id, field],
        |r| Ok((r.get(0)?, r.get(1)?)),
    );
    match found {
        Ok(found) => Ok(found),
        Err(_) => bail!("{} has no field `{field}`", hit.name),
    }
}

pub fn field_uses(conn: &Connection, field_id: i64) -> Result<Vec<FieldUse>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, c.name, m.name, m.descriptor, fr.is_write, fr.line FROM field_refs fr
         JOIN methods m ON m.id = fr.method_id JOIN classes c ON c.id = m.class_id
         WHERE fr.field_id = ?1 ORDER BY c.name, m.name, fr.line",
    )?;
    let uses = stmt
        .query_map(params![field_id], |r| {
            Ok(FieldUse { method: MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? }, is_write: r.get(4)?, line: r.get(5)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(uses)
}

pub fn key_info(conn: &Connection, ident: &str) -> Result<KeyInfo> {
    let bare = ident.trim().trim_start_matches('#');
    let path = bare.rsplit(':').next().unwrap_or(bare).to_owned();
    let full = if bare.contains(':') { bare.to_owned() } else { format!("minecraft:{bare}") };
    let mut info = KeyInfo { ident: full.clone(), ..KeyInfo::default() };

    let mut stmt = conn.prepare("SELECT id, registry, ident, path FROM data_files WHERE ident = ?1 ORDER BY registry")?;
    let files: Vec<(i64, DataFile)> = stmt
        .query_map(params![full], |r| Ok((r.get(0)?, DataFile { registry: r.get(1)?, ident: r.get(2)?, path: r.get(3)?, members: Vec::new() })))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, mut file) in files {
        let mut stmt = conn.prepare("SELECT member FROM tag_members WHERE tag_id = ?1 ORDER BY member")?;
        file.members = stmt.query_map(params![id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        info.files.push(file);
    }

    let mut stmt = conn.prepare(
        "SELECT d.registry, d.ident FROM tag_members t JOIN data_files d ON d.id = t.tag_id
         WHERE t.member = ?1 OR t.member = '#' || ?1 ORDER BY d.registry, d.ident",
    )?;
    info.member_of = stmt.query_map(params![full], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT f.id, c.name, f.name, vc.name || '.' || vf.name FROM key_fields k
         JOIN fields f ON f.id = k.field_id JOIN classes c ON c.id = f.class_id
         LEFT JOIN fields vf ON vf.id = k.via_id LEFT JOIN classes vc ON vc.id = vf.class_id
         WHERE k.ident = ?1 OR k.ident = ?2 ORDER BY c.name, f.name",
    )?;
    let holders: Vec<(i64, String, String, Option<String>)> =
        stmt.query_map(params![path, full], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    for (field_id, class, field, via) in holders {
        info.holders.push(KeyHolder { class, field, via, uses: field_uses(conn, field_id)? });
    }

    if info.files.is_empty() && info.member_of.is_empty() && info.holders.is_empty() {
        bail!("nothing known about `{ident}`, try the strings query with `{path}`");
    }
    Ok(info)
}

pub fn strings(conn: &Connection, pattern: &str, limit: usize) -> Result<Vec<StringHit>> {
    let mut stmt = conn.prepare(
        "SELECT s.value, m.id, c.name, m.name, m.descriptor, s.line FROM strings s
         JOIN methods m ON m.id = s.method_id JOIN classes c ON c.id = m.class_id
         WHERE s.value LIKE '%' || ?1 || '%' ORDER BY length(s.value), s.value, c.name LIMIT ?2",
    )?;
    let hits = stmt
        .query_map(params![pattern, limit as i64], |r| {
            Ok(StringHit { value: r.get(0)?, method: MethodRef { id: r.get(1)?, class: r.get(2)?, name: r.get(3)?, descriptor: r.get(4)? }, line: r.get(5)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(hits)
}

/// The words of `text`, each quoted so that FTS5 takes it as it is.
fn fts_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| w.len() > 1).map(|w| format!("\"{w}\"")).collect()
}

/// Free text as an FTS5 query that can not fail to parse: the words quoted and joined with OR.
/// Text that already uses FTS syntax is passed on as it is.
pub fn fts_query(text: &str) -> String {
    let uses_syntax = text.contains('"') || text.contains(" AND ") || text.contains(" OR ") || text.contains(" NEAR") || text.contains('*');
    if uses_syntax {
        return text.to_owned();
    }
    fts_terms(text).join(" OR ")
}

/// Hits that have every word come first, hits that have some of them fill the rest. A plain OR
/// ranks a method that says `golem` ten times above the one that has all the words once.
pub fn search(conn: &Connection, text: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let any = fts_query(text);
    if any.is_empty() {
        bail!("nothing to search for in `{text}`");
    }
    let terms = fts_terms(text);
    let mut hits = Vec::new();
    if any != text && terms.len() > 1 {
        hits = search_with(conn, &terms.join(" AND "), limit)?;
    }
    for hit in search_with(conn, &any, limit)? {
        let known = hits.iter().any(|h: &SearchHit| h.class == hit.class && h.method.as_ref().map(|m| m.id) == hit.method.as_ref().map(|m| m.id));
        if !known {
            hits.push(hit);
        }
    }
    hits.truncate(limit + limit / 3);
    Ok(hits)
}

fn search_with(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT m.id, c.name, m.name, m.descriptor, snippet(method_fts, 1, '>>', '<<', ' ... ', 16)
         FROM method_fts JOIN methods m ON m.id = method_fts.rowid JOIN classes c ON c.id = m.class_id
         WHERE method_fts MATCH ?1 ORDER BY bm25(method_fts, 4.0, 1.0) LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![query, limit as i64], |r| {
        Ok(SearchHit {
            class: r.get(1)?,
            method: Some(MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? }),
            snippet: r.get(4)?,
        })
    })?;
    for row in rows {
        hits.push(row?);
    }

    let mut stmt = conn.prepare(
        "SELECT c.name, snippet(class_fts, 1, '>>', '<<', ' ... ', 16)
         FROM class_fts JOIN classes c ON c.id = class_fts.rowid
         WHERE class_fts MATCH ?1 ORDER BY bm25(class_fts, 8.0, 1.0) LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![query, (limit / 3).max(3) as i64], |r| Ok(SearchHit { class: r.get(0)?, method: None, snippet: r.get(1)? }))?;
    for row in rows {
        hits.push(row?);
    }
    for hit in &mut hits {
        hit.snippet = hit.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    Ok(hits)
}

pub fn class_source(conn: &Connection, class_query: &str) -> Result<String> {
    let hit = one_class(conn, class_query)?;
    // Inner classes are decompiled into the file of their outermost class.
    let outer = hit.name.split('$').next().unwrap_or(&hit.name);
    let found = conn.query_row("SELECT s.text FROM class_sources s JOIN classes c ON c.id = s.class_id WHERE c.name = ?1", params![outer], |r| r.get(0));
    match found {
        Ok(text) => Ok(text),
        Err(_) => bail!("no decompiled source for {outer}, was the index built with --src?"),
    }
}

pub fn method_source(conn: &Connection, method_id: i64) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT text FROM method_sources WHERE method_id = ?1", params![method_id], |r| r.get(0)).ok())
}

/// A place on disk, for opening the raw file.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: String,
    pub line: Option<i64>,
}

impl Location {
    pub fn url(&self) -> String {
        let path = self.path.replace('\\', "/");
        if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}", self.path),
            None => write!(f, "{}", self.path),
        }
    }
}

/// Stored paths use `/`, the result uses whatever the platform does.
fn join(root: &str, relative: &str) -> String {
    let mut path = std::path::PathBuf::from(root);
    path.extend(relative.split('/'));
    path.display().to_string()
}

/// A value of the meta table: version, jar, src_root, data_root.
pub fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).ok()
}

/// The decompiled file of a class. Inner classes live in the file of their outermost class.
pub fn class_location(conn: &Connection, class: &str) -> Option<Location> {
    let outer = class.split('$').next().unwrap_or(class);
    let relative: String =
        conn.query_row("SELECT s.path FROM class_sources s JOIN classes c ON c.id = s.class_id WHERE c.name = ?1", params![outer], |r| r.get(0)).ok()?;
    Some(Location { path: join(&meta(conn, "src_root")?, &relative), line: None })
}

pub fn method_location(conn: &Connection, method_id: i64) -> Option<Location> {
    let (class, line): (String, i64) = conn
        .query_row(
            "SELECT c.name, s.line FROM method_sources s JOIN methods m ON m.id = s.method_id
             JOIN classes c ON c.id = m.class_id WHERE s.method_id = ?1",
            params![method_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let mut location = class_location(conn, &class)?;
    location.line = Some(line);
    Some(location)
}

pub fn data_location(conn: &Connection, relative: &str) -> Option<Location> {
    Some(Location { path: join(&meta(conn, "data_root")?, relative), line: None })
}

pub fn topic(conn: &Connection, seed: &str, depth: usize) -> Result<Topic> {
    let tokens: Vec<String> = seed.split_whitespace().map(str::to_lowercase).collect();
    if tokens.is_empty() {
        bail!("empty topic");
    }

    let mut stmt = conn.prepare("SELECT id, name FROM classes WHERE is_external = 0")?;
    let all: Vec<(i64, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let score = |name: &str| {
        let lower = name.to_lowercase();
        tokens.iter().filter(|t| lower.contains(t.as_str())).count()
    };
    let best = all.iter().map(|(_, name)| score(name)).max().unwrap_or(0);
    if best == 0 {
        bail!("no class name contains any of: {}", tokens.join(", "));
    }
    let mut core: HashMap<i64, String> = all.iter().filter(|(_, n)| score(n) == best).map(|(id, n)| (*id, n.clone())).collect();

    let mut topic = Topic { seed: seed.to_owned(), ..Topic::default() };
    topic.classes = core.values().cloned().collect();
    topic.classes.sort();

    for round in 0..depth.clamp(1, 3) {
        let ids = core.keys().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let ranked = |sql: String| -> Result<Vec<(i64, String, usize)>> {
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize)))?.collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        };
        let used_by = ranked(format!(
            "SELECT cc.id, cc.name, count(*) FROM calls
             JOIN methods tm ON tm.id = calls.callee_id JOIN methods cm ON cm.id = calls.caller_id
             JOIN classes cc ON cc.id = cm.class_id
             WHERE tm.class_id IN ({ids}) AND cm.class_id NOT IN ({ids})
             GROUP BY cc.id ORDER BY count(*) DESC, cc.name LIMIT 30"
        ))?;
        let uses = ranked(format!(
            "SELECT tc.id, tc.name, count(*) FROM calls
             JOIN methods cm ON cm.id = calls.caller_id JOIN methods tm ON tm.id = calls.callee_id
             JOIN classes tc ON tc.id = tm.class_id
             WHERE cm.class_id IN ({ids}) AND tm.class_id NOT IN ({ids}) AND tc.is_external = 0
             GROUP BY tc.id ORDER BY count(*) DESC, tc.name LIMIT 30"
        ))?;

        if round == 0 {
            topic.used_by = used_by.iter().map(|(_, n, c)| (n.clone(), *c)).collect();
            topic.uses = uses.iter().map(|(_, n, c)| (n.clone(), *c)).collect();
        }
        if round + 1 < depth {
            for (id, name, _) in used_by.into_iter().take(8) {
                core.insert(id, name);
            }
        }
    }

    let ids = core.keys().map(ToString::to_string).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT kc.simple, kf.name, k.ident FROM field_refs fr
         JOIN methods m ON m.id = fr.method_id JOIN key_fields k ON k.field_id = fr.field_id
         JOIN fields kf ON kf.id = k.field_id JOIN classes kc ON kc.id = kf.class_id
         WHERE m.class_id IN ({ids}) ORDER BY kc.simple, kf.name"
    ))?;
    topic.keys = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(&format!(
        "SELECT c.simple, f.name, f.constant FROM fields f JOIN classes c ON c.id = f.class_id
         WHERE f.class_id IN ({ids}) AND f.constant IS NOT NULL ORDER BY c.simple, f.id"
    ))?;
    topic.constants = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare("SELECT registry, ident FROM data_files ORDER BY registry, ident")?;
    topic.data = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(Result::ok)
        .filter(|(_, ident)| tokens.iter().all(|t| ident.contains(t.as_str())))
        .take(60)
        .collect();
    Ok(topic)
}

/// Something a method does at a line of the class file: a call, a field access or a string
/// constant.
#[derive(Clone, Debug, PartialEq)]
pub enum Needle {
    Call { callee_id: i64, class: String, name: String },
    Field { field_id: i64, name: String },
    Str(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Site {
    pub method_id: i64,
    /// As the class file numbers it.
    pub line: i64,
    pub needle: Needle,
}

/// The line of the decompiled file where `site` happens. The index has the line numbers of the
/// class file, which the decompiled text does not keep. The k-th time the method does the thing
/// in bytecode order is taken to be the k-th time its source shows it. None when the source
/// does not show it at all.
pub fn source_line(conn: &Connection, site: &Site) -> Option<i64> {
    let start = method_location(conn, site.method_id)?.line?;
    let text = method_source(conn, site.method_id).ok()??;
    let lines: Vec<Option<i64>> = match &site.needle {
        Needle::Call { callee_id, .. } => lines_of(conn, "SELECT line FROM calls WHERE caller_id = ?1 AND callee_id = ?2", site.method_id, callee_id),
        Needle::Field { field_id, .. } => lines_of(conn, "SELECT line FROM field_refs WHERE method_id = ?1 AND field_id = ?2", site.method_id, field_id),
        Needle::Str(value) => lines_of(conn, "SELECT line FROM strings WHERE method_id = ?1 AND value = ?2", site.method_id, value),
    };
    let mut lines: Vec<i64> = lines.into_iter().flatten().collect();
    lines.sort_unstable();
    let rank = lines.iter().position(|l| *l == site.line).unwrap_or(0);
    let found = occurrences(&text, &site.needle);
    if let Some(index) = found.get(rank).or(found.last()) {
        return Some(start + *index as i64);
    }
    // Not in the text: an iterator of a for loop, boxing, a lambda body. As far into the method
    // as the class file line is past the method's first line, which is close when the decompiler
    // kept the order of statements.
    let first: Option<i64> = conn
        .query_row(
            "SELECT min(line) FROM (SELECT line FROM calls WHERE caller_id = ?1 UNION ALL SELECT line FROM field_refs WHERE method_id = ?1)",
            params![site.method_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let length = text.lines().count().max(1) as i64;
    Some(start + 1 + (site.line - first?).clamp(0, length - 2))
}

fn lines_of(conn: &Connection, sql: &str, method_id: i64, other: &dyn rusqlite::ToSql) -> Vec<Option<i64>> {
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    stmt.query_map(params![method_id, other], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// The 0 based lines of `text` where the needle shows, once per time it shows.
fn occurrences(text: &str, needle: &Needle) -> Vec<usize> {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    // A word that stands alone, followed by what has to come after it.
    let words = |line: &str, word: &str, after: &dyn Fn(&str) -> bool| {
        line.match_indices(word).filter(|(at, _)| !line[..*at].ends_with(ident) && after(&line[at + word.len()..])).count()
    };
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let count = match needle {
            Needle::Call { class, name, .. } if name == "<init>" => {
                let simple = class.rsplit(['/', '$']).next().unwrap_or(class);
                let created = words(line, simple, &|rest| rest.trim_start().starts_with(['(', '<']));
                // `super(...)` and `this(...)` in a constructor call one too.
                created + words(line, "super", &|rest| rest.starts_with('(')) + words(line, "this", &|rest| rest.starts_with('('))
            }
            Needle::Call { name, .. } => {
                // `name(` or a method reference `::name`. A variable of the same name is neither.
                let reference = format!("::{name}");
                let references = line.match_indices(reference.as_str()).filter(|(at, _)| !line[at + reference.len()..].starts_with(ident)).count();
                words(line, name, &|rest| rest.trim_start().starts_with('(')) + references
            }
            Needle::Field { name, .. } => words(line, name, &|rest| !rest.starts_with(ident) && !rest.trim_start().starts_with('(')),
            Needle::Str(value) => {
                let quoted = format!("{value:?}");
                line.matches(quoted.as_str()).count()
            }
        };
        out.extend(std::iter::repeat_n(index, count));
    }
    out
}

/// The 1 based line of the file of `class` where `field` is declared. Fields have no line in
/// the class file, so the source is read: the first `Type NAME = ...` or `Type NAME;`, or an
/// enum constant `NAME(`, `NAME,` at the start of its line. An assignment in a method has no
/// type in front of the name and does not count.
pub fn field_line(conn: &Connection, class: &str, field: &str) -> Option<i64> {
    let text = class_source(conn, class).ok()?;
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("return ") {
            continue;
        }
        for (at, _) in line.match_indices(field) {
            let before = &line[..at];
            let after = &line[at + field.len()..];
            if before.ends_with(ident) || after.starts_with(ident) || before.trim_end().ends_with('.') {
                continue;
            }
            let after = after.trim_start();
            let declared = (after.starts_with('=') && !after.starts_with("==")) || after.starts_with(';');
            let constant = before.trim().is_empty() && after.starts_with(['(', ',', ';']);
            if (declared && !before.trim().is_empty()) || constant {
                return Some(index as i64 + 1);
            }
        }
    }
    None
}
