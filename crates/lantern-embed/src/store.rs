use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::{docs::Chunk, models::ModelSpec};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    hash INTEGER NOT NULL UNIQUE,
    class_id INTEGER NOT NULL,
    method_id INTEGER,
    part INTEGER NOT NULL,
    parts INTEGER NOT NULL,
    title TEXT NOT NULL,
    text TEXT NOT NULL,
    vector BLOB NOT NULL
) STRICT;
";

const INSERT: &str = "INSERT OR REPLACE INTO chunks (hash, class_id, method_id, part, parts, title, text, vector)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

fn model_signature(spec: &ModelSpec) -> String {
    format!("{}|{}|{}|{}", spec.id, spec.stored_dims, spec.query_prefix, spec.document_prefix)
}

/// `vectors` is the folder of one index, `crate::vectors_dir` finds it.
pub fn store_path(vectors: &Path, spec: &ModelSpec) -> PathBuf {
    vectors.join(format!("{}.db", spec.id))
}

/// How many vectors a model has stored, without touching the store. None when there is no
/// store or it can not be read.
pub fn stored_count(vectors: &Path, spec: &ModelSpec) -> Option<usize> {
    let path = store_path(vectors, spec);
    if !path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0)).ok().map(|n| n as usize)
}

pub struct Store {
    conn: Connection,
    dims: usize,
}

#[derive(Clone, Debug)]
pub struct StoredChunk {
    pub class_id: i64,
    pub method_id: Option<i64>,
    pub part: usize,
    pub parts: usize,
    pub title: String,
    pub text: String,
}

impl Store {
    /// Opens the store of a model, creating it if needed. `index_id` names the index the
    /// vectors belong to, symbol ids from another build would point at the wrong methods.
    pub fn open(vectors: &Path, spec: &ModelSpec, index_id: &str) -> Result<Self> {
        let path = store_path(vectors, spec);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        conn.execute_batch(SCHEMA)?;

        let signature = format!("{}|{index_id}", model_signature(spec));
        let existing: Option<String> = conn.query_row("SELECT value FROM meta WHERE key = 'signature'", [], |r| r.get(0)).ok();
        if existing.as_deref() != Some(signature.as_str()) {
            // Different model settings or a rebuilt index. Nothing in here can be trusted.
            conn.execute("DELETE FROM chunks", [])?;
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('signature', ?1)", params![signature])?;
        }
        Ok(Self { conn, dims: spec.stored_dims })
    }

    pub fn exists(vectors: &Path, spec: &ModelSpec) -> bool {
        store_path(vectors, spec).exists()
    }

    pub fn hashes(&self) -> Result<HashSet<u64>> {
        let mut stmt = self.conn.prepare("SELECT hash FROM chunks")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(Result::ok).map(|h| h as u64).collect())
    }

    pub fn len(&self) -> Result<usize> {
        Ok(self.conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    pub fn insert(&mut self, batch: &[(&Chunk, Vec<f32>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(INSERT)?;
            for (chunk, vector) in batch {
                ensure!(vector.len() == self.dims, "vector has {} dims, the store holds {}", vector.len(), self.dims);
                let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
                insert.execute(params![
                    chunk.hash as i64,
                    chunk.class_id,
                    chunk.method_id,
                    chunk.part as i64,
                    chunk.parts as i64,
                    chunk.title,
                    chunk.text,
                    blob
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Copies exact text matches from another version, with the current index's symbol ids.
    /// The donor is read-only; unreadable or incompatible caches leave inference as fallback.
    pub fn reuse_from(
        &mut self,
        path: &Path,
        spec: &ModelSpec,
        chunks: &[&Chunk],
        should_stop: &impl Fn() -> bool,
        mut on_progress: impl FnMut(usize),
    ) -> Result<HashSet<u64>> {
        let Ok(source) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            return Ok(HashSet::new());
        };
        let signature: Option<String> = source.query_row("SELECT value FROM meta WHERE key = 'signature'", [], |row| row.get(0)).ok();
        let prefix = format!("{}|", model_signature(spec));
        if !signature.is_some_and(|signature| signature.starts_with(&prefix)) {
            return Ok(HashSet::new());
        }
        let Ok(mut lookup) = source.prepare("SELECT vector FROM chunks WHERE hash = ?1 AND text = ?2") else {
            return Ok(HashSet::new());
        };
        let mut reused = HashSet::new();
        for batch in chunks.chunks(128) {
            if should_stop() {
                break;
            }
            let mut matches = Vec::new();
            for chunk in batch {
                let vector: rusqlite::Result<Option<Vec<u8>>> = lookup.query_row(params![chunk.hash as i64, chunk.text], |row| row.get(0)).optional();
                if let Ok(Some(blob)) = vector
                    && blob.len() == self.dims * 4
                {
                    matches.push((*chunk, blob));
                }
            }
            if !matches.is_empty() {
                let tx = self.conn.transaction()?;
                {
                    let mut insert = tx.prepare_cached(INSERT)?;
                    for (chunk, blob) in &matches {
                        insert.execute(params![
                            chunk.hash as i64,
                            chunk.class_id,
                            chunk.method_id,
                            chunk.part as i64,
                            chunk.parts as i64,
                            chunk.title,
                            chunk.text,
                            blob
                        ])?;
                    }
                }
                tx.commit()?;
                reused.extend(matches.iter().map(|(chunk, _)| chunk.hash));
            }
            on_progress(batch.len());
        }
        Ok(reused)
    }

    /// Drops chunks that are not part of the current document set.
    pub fn retain(&mut self, keep: &HashSet<u64>) -> Result<usize> {
        let stale: Vec<u64> = self.hashes()?.into_iter().filter(|h| !keep.contains(h)).collect();
        let tx = self.conn.transaction()?;
        for hash in &stale {
            tx.execute("DELETE FROM chunks WHERE hash = ?1", params![*hash as i64])?;
        }
        tx.commit()?;
        Ok(stale.len())
    }

    /// All vectors in one flat buffer, with the row ids next to it.
    pub fn load_vectors(&self) -> Result<(Vec<i64>, Vec<f32>)> {
        let mut stmt = self.conn.prepare("SELECT id, vector FROM chunks ORDER BY id")?;
        let mut ids = Vec::new();
        let mut flat = Vec::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        for row in rows {
            let (id, blob) = row?;
            ensure!(blob.len() == self.dims * 4, "chunk {id} holds {} bytes, expected {}", blob.len(), self.dims * 4);
            ids.push(id);
            flat.extend(blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        }
        Ok((ids, flat))
    }

    pub fn chunk(&self, id: i64) -> Result<StoredChunk> {
        Ok(self.conn.query_row("SELECT class_id, method_id, part, parts, title, text FROM chunks WHERE id = ?1", params![id], |r| {
            Ok(StoredChunk {
                class_id: r.get(0)?,
                method_id: r.get(1)?,
                part: r.get::<_, i64>(2)? as usize,
                parts: r.get::<_, i64>(3)? as usize,
                title: r.get(4)?,
                text: r.get(5)?,
            })
        })?)
    }

    pub fn dims(&self) -> usize {
        self.dims
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::{docs::stable_hash, models::MODELS};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("lantern-vector-reuse-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn chunk(text: &str, class_id: i64, method_id: i64) -> Chunk {
        Chunk { class_id, method_id: Some(method_id), part: 0, parts: 1, title: "Example.tick".to_owned(), text: text.to_owned(), hash: stable_hash(text) }
    }

    #[test]
    fn reuses_vectors_with_current_symbol_ids_without_changing_the_donor() {
        let fixture = Fixture::new();
        let spec = &MODELS[0];
        let source_dir = fixture.0.join("older");
        let source_path = store_path(&source_dir, spec);
        let old = chunk("identical source", 7, 11);
        let current = chunk("identical source", 70, 110);
        let vector = vec![0.25; spec.stored_dims];
        let mut source = Store::open(&source_dir, spec, "old-index").unwrap();
        source.insert(&[(&old, vector.clone())]).unwrap();
        let mut destination = Store::open(&fixture.0.join("newer"), spec, "new-index").unwrap();
        let copied = destination.reuse_from(&source_path, spec, &[&current], &|| false, |_| {}).unwrap();
        assert_eq!(copied, HashSet::from([current.hash]));
        let (ids, vectors) = destination.load_vectors().unwrap();
        assert_eq!(vectors, vector);
        let stored = destination.chunk(ids[0]).unwrap();
        assert_eq!((stored.class_id, stored.method_id), (70, Some(110)));
        let (ids, _) = source.load_vectors().unwrap();
        let original = source.chunk(ids[0]).unwrap();
        assert_eq!((original.class_id, original.method_id), (7, Some(11)));
    }

    #[test]
    fn skips_incompatible_models_hash_collisions_and_invalid_vectors() {
        let fixture = Fixture::new();
        let spec = &MODELS[0];
        let source_dir = fixture.0.join("older");
        let source_path = store_path(&source_dir, spec);
        let original = chunk("old text", 7, 11);
        let mut source = Store::open(&source_dir, spec, "old-index").unwrap();
        source.insert(&[(&original, vec![0.25; spec.stored_dims])]).unwrap();
        let mut destination = Store::open(&fixture.0.join("newer"), spec, "new-index").unwrap();
        let incompatible = ModelSpec { document_prefix: "different prefix", ..MODELS[0] };
        assert!(destination.reuse_from(&source_path, &incompatible, &[&original], &|| false, |_| {}).unwrap().is_empty());
        let mut collision = chunk("different text", 70, 110);
        collision.hash = original.hash;
        assert!(destination.reuse_from(&source_path, spec, &[&collision], &|| false, |_| {}).unwrap().is_empty());
        source.conn.execute("UPDATE chunks SET vector = zeroblob(8)", []).unwrap();
        assert!(destination.reuse_from(&source_path, spec, &[&original], &|| false, |_| {}).unwrap().is_empty());
        assert!(destination.is_empty().unwrap());
    }

    #[test]
    fn cancellation_keeps_committed_vectors_and_can_resume() {
        let fixture = Fixture::new();
        let spec = &MODELS[0];
        let source_dir = fixture.0.join("older");
        let source_path = store_path(&source_dir, spec);
        let chunks: Vec<Chunk> = (0..130).map(|id| chunk(&format!("source {id}"), id, id + 1000)).collect();
        let rows: Vec<_> = chunks.iter().map(|chunk| (chunk, vec![0.25; spec.stored_dims])).collect();
        let mut source = Store::open(&source_dir, spec, "old-index").unwrap();
        source.insert(&rows).unwrap();
        let mut destination = Store::open(&fixture.0.join("newer"), spec, "new-index").unwrap();
        let stop = Cell::new(false);
        let pending: Vec<_> = chunks.iter().collect();
        let copied = destination.reuse_from(&source_path, spec, &pending, &|| stop.get(), |_| stop.set(true)).unwrap();
        assert_eq!(copied.len(), 128);
        assert_eq!(destination.len().unwrap(), 128);
        let remaining: Vec<_> = chunks.iter().filter(|chunk| !copied.contains(&chunk.hash)).collect();
        let copied = destination.reuse_from(&source_path, spec, &remaining, &|| false, |_| {}).unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(destination.len().unwrap(), chunks.len());
    }
}
