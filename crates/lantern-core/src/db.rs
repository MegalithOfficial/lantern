use std::path::Path;

use anyhow::{Result, ensure};
pub use rusqlite::Connection;

/// Bumped whenever the tables change. An index written by another version is rejected,
/// rebuilding takes seconds.
pub const SCHEMA_VERSION: i32 = 3;

pub const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;

-- Every class that is defined in the jar or referenced from it. External ones (JDK and
-- libraries) only have a name, they exist so that references can point at something.
CREATE TABLE classes (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    simple TEXT NOT NULL,
    package TEXT NOT NULL,
    outer_id INTEGER REFERENCES classes (id),
    super_id INTEGER REFERENCES classes (id),
    flags INTEGER NOT NULL,
    is_interface INTEGER NOT NULL,
    is_external INTEGER NOT NULL,
    source_file TEXT
) STRICT;
CREATE INDEX classes_simple ON classes (simple);
CREATE INDEX classes_super ON classes (super_id);
CREATE INDEX classes_outer ON classes (outer_id);

CREATE TABLE interfaces (
    class_id INTEGER NOT NULL REFERENCES classes (id),
    interface_id INTEGER NOT NULL REFERENCES classes (id),
    PRIMARY KEY (class_id, interface_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX interfaces_reverse ON interfaces (interface_id);

-- Transitive closure of extends and implements. depth 0 is the class itself.
CREATE TABLE ancestors (
    class_id INTEGER NOT NULL REFERENCES classes (id),
    ancestor_id INTEGER NOT NULL REFERENCES classes (id),
    depth INTEGER NOT NULL,
    PRIMARY KEY (class_id, ancestor_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX ancestors_reverse ON ancestors (ancestor_id, class_id);

CREATE TABLE methods (
    id INTEGER PRIMARY KEY,
    class_id INTEGER NOT NULL REFERENCES classes (id),
    name TEXT NOT NULL,
    descriptor TEXT NOT NULL,
    flags INTEGER NOT NULL,
    is_external INTEGER NOT NULL,
    -- line numbers of the original source, only useful for ordering
    line_start INTEGER,
    line_end INTEGER,
    -- nearest declaration this one overrides or implements
    overrides_id INTEGER REFERENCES methods (id),
    UNIQUE (class_id, name, descriptor)
) STRICT;
CREATE INDEX methods_name ON methods (name);
CREATE INDEX methods_overrides ON methods (overrides_id);

CREATE TABLE fields (
    id INTEGER PRIMARY KEY,
    class_id INTEGER NOT NULL REFERENCES classes (id),
    name TEXT NOT NULL,
    descriptor TEXT NOT NULL,
    flags INTEGER NOT NULL,
    is_external INTEGER NOT NULL,
    constant TEXT,
    UNIQUE (class_id, name)
) STRICT;
CREATE INDEX fields_name ON fields (name);

-- callee_id is the declaration the JVM resolves the reference to, receiver_id the static
-- type written at the call site. kind: virtual, static, special, interface, lambda.
CREATE TABLE calls (
    caller_id INTEGER NOT NULL REFERENCES methods (id),
    callee_id INTEGER NOT NULL REFERENCES methods (id),
    receiver_id INTEGER NOT NULL REFERENCES classes (id),
    kind TEXT NOT NULL,
    line INTEGER
) STRICT;
CREATE INDEX calls_caller ON calls (caller_id);
CREATE INDEX calls_callee ON calls (callee_id);

CREATE TABLE field_refs (
    method_id INTEGER NOT NULL REFERENCES methods (id),
    field_id INTEGER NOT NULL REFERENCES fields (id),
    is_write INTEGER NOT NULL,
    line INTEGER
) STRICT;
CREATE INDEX field_refs_method ON field_refs (method_id);
CREATE INDEX field_refs_field ON field_refs (field_id);

CREATE TABLE strings (
    method_id INTEGER NOT NULL REFERENCES methods (id),
    value TEXT NOT NULL,
    line INTEGER
) STRICT;
CREATE INDEX strings_value ON strings (value);
CREATE INDEX strings_method ON strings (method_id);

-- meta holds src_root and data_root, paths below are relative to those.
-- Decompiled text of a method, cut out of the class source once at index time.
CREATE TABLE method_sources (
    method_id INTEGER PRIMARY KEY REFERENCES methods (id),
    -- line in the decompiled file the method starts on
    line INTEGER NOT NULL,
    text TEXT NOT NULL
) STRICT;

CREATE TABLE class_sources (
    class_id INTEGER PRIMARY KEY REFERENCES classes (id),
    path TEXT NOT NULL,
    text TEXT NOT NULL
) STRICT;

-- Everything under data/<namespace>/. registry is the folder (tags/block, loot_table, timeline).
CREATE TABLE data_files (
    id INTEGER PRIMARY KEY,
    registry TEXT NOT NULL,
    ident TEXT NOT NULL,
    path TEXT NOT NULL,
    UNIQUE (registry, ident)
) STRICT;
CREATE INDEX data_files_ident ON data_files (ident);

-- member is an id, or a tag reference starting with #
CREATE TABLE tag_members (
    tag_id INTEGER NOT NULL REFERENCES data_files (id),
    member TEXT NOT NULL,
    PRIMARY KEY (tag_id, member)
) STRICT, WITHOUT ROWID;
CREATE INDEX tag_members_member ON tag_members (member);

-- Static fields that stand for a tag or a registry entry, e.g. BlockTags.DAMPENS_VIBRATIONS.
-- via_id is set when the value was copied from another such field.
CREATE TABLE key_fields (
    field_id INTEGER PRIMARY KEY REFERENCES fields (id),
    ident TEXT NOT NULL,
    via_id INTEGER REFERENCES fields (id)
) STRICT;
CREATE INDEX key_fields_ident ON key_fields (ident);

CREATE VIRTUAL TABLE method_fts USING fts5 (title, body, tokenize = 'unicode61');
CREATE VIRTUAL TABLE class_fts USING fts5 (title, body, tokenize = 'unicode61');

CREATE VIEW v_methods AS
    SELECT m.id, c.name AS class, m.name, m.descriptor, m.is_external, m.line_start
    FROM methods m JOIN classes c ON c.id = m.class_id;

CREATE VIEW v_calls AS
    SELECT calls.caller_id, calls.callee_id, calls.kind, calls.line,
           cc.name AS caller_class, cm.name AS caller_name,
           tc.name AS callee_class, tm.name AS callee_name, tm.descriptor AS callee_descriptor,
           rc.name AS receiver_class
    FROM calls
    JOIN methods cm ON cm.id = calls.caller_id JOIN classes cc ON cc.id = cm.class_id
    JOIN methods tm ON tm.id = calls.callee_id JOIN classes tc ON tc.id = tm.class_id
    JOIN classes rc ON rc.id = calls.receiver_id;
";

pub fn create(path: &Path) -> Result<Connection> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if file.exists() {
            std::fs::remove_file(file)?;
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA foreign_keys = OFF;")?;
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(conn)
}

pub fn open(path: &Path) -> Result<Connection> {
    ensure!(path.exists(), "{} does not exist, run `lantern index` first", path.display());
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        version == SCHEMA_VERSION,
        "{} was written by another version of lantern (schema {version}, this build needs {SCHEMA_VERSION}), run `lantern index` again",
        path.display()
    );
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -65536;")?;
    Ok(conn)
}
