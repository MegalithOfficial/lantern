pub mod docs;
pub mod download;
pub mod engine;
pub mod models;
pub mod rerank;
pub mod setup;
pub mod store;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use lantern_core::{
    db::Connection,
    query::{self, MethodRef},
    settings::Settings,
};
use rusqlite::params;

use crate::{
    engine::{Engine, dot},
    models::{ModelSpec, RerankerSpec},
    rerank::Reranker,
    store::Store,
};

/// Where models, settings and the vectors of every version live.
pub fn data_dir(db_path: &Path) -> PathBuf {
    lantern_core::data_dir(db_path)
}

/// The vectors of the index `conn` is open on. Each version has its own, switching between
/// versions loses none of them.
pub fn vectors_dir(conn: &Connection, data_dir: &Path) -> PathBuf {
    lantern_core::versions::vectors_dir(data_dir, &query::meta(conn, "version").unwrap_or_default())
}

/// Names one build of the index. Symbol ids are only comparable within the same build.
pub fn index_id(conn: &Connection) -> Result<String> {
    let version: String = conn.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get(0)).unwrap_or_default();
    let (classes, methods, calls): (i64, i64, i64) =
        conn.query_row("SELECT (SELECT count(*) FROM classes), (SELECT count(*) FROM methods), (SELECT count(*) FROM calls)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    Ok(format!("{version}/{classes}/{methods}/{calls}"))
}

pub fn default_threads() -> usize {
    // Leave room for the rest of the machine, this runs for minutes.
    std::thread::available_parallelism().map_or(4, |n| (n.get() * 3 / 4).max(1))
}

pub fn load_engine(spec: &'static ModelSpec, data_dir: &Path, threads: usize) -> Result<Engine> {
    if !download::is_downloaded(spec.source(), data_dir) {
        bail!("{} is not downloaded yet, run `lantern models download {}`", spec.name, spec.id);
    }
    let files = download::fetch(spec.source(), data_dir, |_| {})?;
    Engine::load(spec, &files, threads)
}

pub fn load_reranker(spec: &'static RerankerSpec, data_dir: &Path, threads: usize) -> Result<Reranker> {
    if !download::is_downloaded(spec.source(), data_dir) {
        bail!("{} is not downloaded yet, run `lantern models download {}`", spec.name, spec.id);
    }
    let files = download::fetch(spec.source(), data_dir, |_| {})?;
    Reranker::load(spec, &files, threads)
}

#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Chunking,
    Reusing,
    Embedding,
}

#[derive(Clone, Copy, Debug)]
pub struct BuildProgress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug)]
pub struct BuildReport {
    pub chunks: usize,
    pub embedded: usize,
    pub reused: usize,
    pub removed: usize,
    pub seconds: f32,
}

const BATCH: usize = 16;

/// Embeds whatever is missing. Safe to interrupt, every batch is committed on its own and a
/// later run only does what is left. `should_stop` is asked between batches.
pub fn build(
    conn: &Connection,
    data_dir: &Path,
    engine: &mut Engine,
    limit: Option<usize>,
    should_stop: impl Fn() -> bool,
    mut on_progress: impl FnMut(BuildProgress),
) -> Result<BuildReport> {
    let started = Instant::now();
    let mut store = Store::open(&vectors_dir(conn, data_dir), engine.spec, &index_id(conn)?)?;

    let mut chunks = docs::build_chunks(conn, engine, |done, total| {
        on_progress(BuildProgress { phase: Phase::Chunking, done, total });
    })?;
    if let Some(limit) = limit {
        chunks.truncate(limit);
    }

    let known = store.hashes()?;
    let keep = chunks.iter().map(|c| c.hash).collect();
    let removed = if limit.is_none() { store.retain(&keep)? } else { 0 };

    let mut pending: Vec<&docs::Chunk> = chunks.iter().filter(|c| !known.contains(&c.hash)).collect();
    let mut reused = chunks.len() - pending.len();
    let current = store::store_path(&vectors_dir(conn, data_dir), engine.spec);
    let mut donors: Vec<PathBuf> = std::fs::read_dir(data_dir.join("embeddings"))
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| store::store_path(&entry.path(), engine.spec))
        .filter(|path| path != &current && path.is_file())
        .collect();
    donors.sort();
    for donor in donors {
        if pending.is_empty() || should_stop() {
            break;
        }
        let (mut checked, total) = (0, pending.len());
        on_progress(BuildProgress { phase: Phase::Reusing, done: 0, total });
        let copied = store.reuse_from(&donor, engine.spec, &pending, &should_stop, |count| {
            checked += count;
            on_progress(BuildProgress { phase: Phase::Reusing, done: checked, total });
        })?;
        reused += copied.len();
        pending.retain(|chunk| !copied.contains(&chunk.hash));
    }

    if should_stop() {
        return Ok(BuildReport { chunks: chunks.len(), embedded: 0, reused, removed, seconds: started.elapsed().as_secs_f32() });
    }

    // Padding follows tokens rather than bytes, especially for identifiers and string tables.
    let mut by_tokens = Vec::with_capacity(pending.len());
    for chunk in pending {
        if should_stop() {
            return Ok(BuildReport { chunks: chunks.len(), embedded: 0, reused, removed, seconds: started.elapsed().as_secs_f32() });
        }
        by_tokens.push((engine.count_tokens(&chunk.text)?, chunk));
    }
    by_tokens.sort_by_key(|(tokens, _)| *tokens);

    let total = by_tokens.len();
    let mut done = 0;
    for batch in by_tokens.chunks(BATCH) {
        if should_stop() {
            break;
        }
        let texts: Vec<String> = batch.iter().map(|(_, chunk)| chunk.text.clone()).collect();
        let vectors = engine.embed(&texts).with_context(|| format!("embedding a batch starting at `{}`", batch[0].1.title))?;
        let rows: Vec<(&docs::Chunk, Vec<f32>)> = batch.iter().map(|(_, chunk)| *chunk).zip(vectors).collect();
        store.insert(&rows)?;
        done += batch.len();
        on_progress(BuildProgress { phase: Phase::Embedding, done, total });
    }

    Ok(BuildReport { chunks: chunks.len(), embedded: done, reused, removed, seconds: started.elapsed().as_secs_f32() })
}

/// What the reranker reads for a candidate: its name and the source, or the summary of a class.
fn rerank_text(conn: &Connection, answer: &Answer) -> Result<String> {
    let simple = answer.class.rsplit('/').next().unwrap_or(&answer.class);
    Ok(match &answer.method {
        Some(method) => {
            let source = query::method_source(conn, method.id)?.unwrap_or_else(|| answer.snippet.clone());
            format!("{simple}.{}\n{source}", method.name)
        }
        None => {
            let id: i64 = conn.query_row("SELECT id FROM classes WHERE name = ?1", params![answer.class], |r| r.get(0))?;
            docs::class_summary(conn, id, &answer.class)?
        }
    })
}

/// What the settings ask for, falling back to what is there: a model without vectors or a
/// reranker that is not downloaded is no use to anyone.
pub fn pick_models(data_dir: &Path, vectors: &Path, settings: &Settings) -> (&'static ModelSpec, Option<&'static RerankerSpec>) {
    let has_vectors = |m: &&ModelSpec| store::stored_count(vectors, m).is_some_and(|n| n > 0);
    let model = models::find(&settings.embedding_model)
        .filter(has_vectors)
        .or_else(|| models::MODELS.iter().find(has_vectors))
        .or_else(|| models::find(&settings.embedding_model))
        .or_else(|| models::find(models::DEFAULT_MODEL))
        .expect("the default model is in the list");
    let reranker = match settings.reranker.as_str() {
        "off" => None,
        "" => models::find_reranker(models::DEFAULT_RERANKER),
        id => models::find_reranker(id),
    }
    .filter(|r| download::is_downloaded(r.source(), data_dir));
    (model, reranker)
}

pub fn threads(settings: &Settings) -> usize {
    if settings.threads == 0 { default_threads() } else { settings.threads }
}

/// Opens a searcher the way the settings file says. For the CLI and the MCP server.
pub fn open_default(conn: &Connection, data_dir: &Path) -> Result<Searcher> {
    let settings = Settings::load(data_dir);
    let (model, reranker) = pick_models(data_dir, &vectors_dir(conn, data_dir), &settings);
    let mut searcher = Searcher::open(conn, data_dir, model, reranker, threads(&settings))?;
    searcher.rerank_pool = settings.rerank_pool;
    Ok(searcher)
}

/// Plain text for the CLI and the MCP server, with the file and line of every answer.
pub fn render_answers(conn: &Connection, answers: &[Answer]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for answer in answers {
        let (label, place) = match &answer.method {
            Some(m) => (
                format!("{}.{} {}", lantern_core::render::short(&m.class), m.name, lantern_core::render::signature(&m.descriptor)),
                query::method_location(conn, m.id),
            ),
            None => (format!("{} (class)", lantern_core::render::short(&answer.class)), query::class_location(conn, &answer.class)),
        };
        let mut why = Vec::new();
        why.extend(answer.relevance.map(|r| format!("relevance {r:.2}")));
        why.extend(answer.similarity.map(|s| format!("similarity {s:.2}")));
        why.extend(answer.text_match.then(|| "text match".to_owned()));
        let _ = writeln!(out, "{label}  [{}]", why.join(", "));
        if let Some(place) = place {
            let _ = writeln!(out, "    {place}");
        }
        let _ = writeln!(out, "    {}", answer.snippet.chars().take(200).collect::<String>());
    }
    out
}

pub struct Searcher {
    engine: Engine,
    reranker: Option<Reranker>,
    /// How many candidates the reranker gets to read. Every one costs a forward pass.
    pub rerank_pool: usize,
    store: Store,
    ids: Vec<i64>,
    vectors: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Answer {
    pub class: String,
    pub method: Option<MethodRef>,
    /// Cosine similarity of the best chunk, when the semantic side found it.
    pub similarity: Option<f32>,
    /// Whether the full text search found it as well.
    pub text_match: bool,
    /// What the reranker made of it, 0 to 1. None when no reranker is loaded.
    pub relevance: Option<f32>,
    pub snippet: String,
}

impl Searcher {
    pub fn open(conn: &Connection, data_dir: &Path, spec: &'static ModelSpec, reranker: Option<&'static RerankerSpec>, threads: usize) -> Result<Self> {
        let vectors = vectors_dir(conn, data_dir);
        if !Store::exists(&vectors, spec) {
            bail!("no embeddings for {} yet, run `lantern embed --model {}`", spec.name, spec.id);
        }
        let store = Store::open(&vectors, spec, &index_id(conn)?)?;
        let (ids, vectors) = store.load_vectors()?;
        if ids.is_empty() {
            bail!("the embeddings of {} are empty or belong to an older index, run `lantern embed --model {}`", spec.name, spec.id);
        }
        let engine = load_engine(spec, data_dir, threads)?;
        let reranker = reranker.map(|spec| load_reranker(spec, data_dir, threads)).transpose()?;
        Ok(Self { engine, reranker, rerank_pool: 40, store, ids, vectors })
    }

    pub fn reranker(&self) -> Option<&'static RerankerSpec> {
        self.reranker.as_ref().map(|r| r.spec)
    }

    pub fn chunk_count(&self) -> usize {
        self.ids.len()
    }

    /// Best chunk per symbol, highest similarity first.
    fn semantic(&mut self, question: &str, limit: usize) -> Result<Vec<(f32, store::StoredChunk)>> {
        let query = self.engine.embed_query(question)?;
        let dims = self.store.dims();
        let mut scored: Vec<(f32, i64)> = self.vectors.chunks_exact(dims).zip(&self.ids).map(|(vector, id)| (dot(&query, vector), *id)).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (score, id) in scored.into_iter().take(limit * 8) {
            let chunk = self.store.chunk(id)?;
            if seen.insert((chunk.class_id, chunk.method_id)) {
                out.push((score, chunk));
                if out.len() == limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Semantic and full text results merged with reciprocal rank fusion. Something both
    /// sides agree on ends up on top, something only one side knows still shows up.
    pub fn ask(&mut self, conn: &Connection, question: &str, limit: usize) -> Result<Vec<Answer>> {
        const K: f32 = 60.0;
        let pool = (limit * 3).max(30);
        let mut fused: HashMap<(Option<i64>, String), (f32, Answer)> = HashMap::new();

        for (rank, (similarity, chunk)) in self.semantic(question, pool)?.into_iter().enumerate() {
            let class: String = conn.query_row("SELECT name FROM classes WHERE id = ?1", params![chunk.class_id], |r| r.get(0))?;
            let method = match chunk.method_id {
                Some(id) => Some(conn.query_row(
                    "SELECT m.id, c.name, m.name, m.descriptor FROM methods m JOIN classes c ON c.id = m.class_id WHERE m.id = ?1",
                    params![id],
                    |r| Ok(MethodRef { id: r.get(0)?, class: r.get(1)?, name: r.get(2)?, descriptor: r.get(3)? }),
                )?),
                None => None,
            };
            let body = chunk.text.split_once("| text: ").map_or(chunk.text.as_str(), |(_, body)| body);
            let snippet =
                body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('@')).take(3).collect::<Vec<_>>().join(" ");
            let answer = Answer { class: class.clone(), method, similarity: Some(similarity), text_match: false, relevance: None, snippet };
            fused.insert((chunk.method_id, class), (1.0 / (K + rank as f32), answer));
        }

        for (rank, hit) in query::search(conn, question, pool).unwrap_or_default().into_iter().enumerate() {
            let key = (hit.method.as_ref().map(|m| m.id), hit.class.clone());
            let bonus = 1.0 / (K + rank as f32);
            match fused.get_mut(&key) {
                Some((score, answer)) => {
                    *score += bonus;
                    answer.text_match = true;
                }
                None => {
                    let answer = Answer { class: hit.class, method: hit.method, similarity: None, text_match: true, relevance: None, snippet: hit.snippet };
                    fused.insert(key, (bonus, answer));
                }
            }
        }

        let mut ranked: Vec<(f32, Answer)> = fused.into_values().collect();

        // A question about a flow is answered by methods that call each other. Every call between
        // two candidates counts for both, a lone getter that only shares a word gets nothing.
        let ids: Vec<i64> = ranked.iter().filter_map(|(_, a)| a.method.as_ref().map(|m| m.id)).collect();
        let mut links: HashMap<i64, f32> = HashMap::new();
        if ids.len() > 1 {
            let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let mut stmt = conn.prepare(&format!(
                "SELECT DISTINCT caller_id, callee_id FROM calls WHERE caller_id IN ({list}) AND callee_id IN ({list}) AND caller_id != callee_id"
            ))?;
            for edge in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
                let (caller, callee) = edge?;
                *links.entry(caller).or_default() += 1.0;
                *links.entry(callee).or_default() += 1.0;
            }
        }
        let linked = |answer: &Answer| answer.method.as_ref().and_then(|m| links.get(&m.id)).map_or(0.0, |n| n.min(3.0));
        for (score, answer) in &mut ranked {
            *score += linked(answer) * 0.35 / K;
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));

        // The reranker reads the candidates the two searches agreed to put forward and has the
        // last word on their order. The call graph still counts a little, it knows something
        // the text of a single method does not.
        if let Some(reranker) = &mut self.reranker {
            ranked.truncate(self.rerank_pool.max(limit));
            let documents = ranked.iter().map(|(_, answer)| rerank_text(conn, answer)).collect::<Result<Vec<_>>>()?;
            let scores = reranker.score(question, &documents)?;
            for ((score, answer), relevance) in ranked.iter_mut().zip(scores) {
                answer.relevance = Some(relevance);
                *score = relevance + linked(answer) * 0.02;
            }
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        }

        // One class should not fill the page. What is over the cap goes behind everything else.
        const PER_CLASS: usize = 3;
        let mut seen: HashMap<String, usize> = HashMap::new();
        let (mut first, mut rest) = (Vec::new(), Vec::new());
        for (_, answer) in ranked {
            let count = seen.entry(answer.class.clone()).or_default();
            *count += 1;
            if *count <= PER_CLASS { first.push(answer) } else { rest.push(answer) }
        }
        first.extend(rest);
        first.truncate(limit);
        Ok(first)
    }
}
