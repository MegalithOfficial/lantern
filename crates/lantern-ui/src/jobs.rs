//! Everything that runs off the UI thread. Each job opens its own connection.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use lantern_core::{
    db,
    render::{self, Query},
    versions,
};
use lantern_embed::{
    Answer, BuildProgress, BuildReport, Phase, Searcher, download,
    models::{ModelSpec, RerankerSpec, Source},
};

/// The loaded searcher and what it was loaded for. A different model or a rebuilt index
/// makes it useless.
pub type SharedSearcher = Arc<Mutex<Option<(String, Searcher)>>>;

#[derive(Clone, Debug)]
pub struct Download {
    pub file: String,
    pub file_index: usize,
    pub file_count: usize,
    pub downloaded: usize,
    pub total: usize,
}

pub fn ask(
    shared: &SharedSearcher,
    db_path: &Path,
    spec: &'static ModelSpec,
    reranker: Option<&'static RerankerSpec>,
    threads: usize,
    rerank_pool: usize,
    question: &str,
    limit: usize,
) -> Result<Vec<Answer>> {
    let conn = db::open(db_path)?;
    let data_dir = lantern_embed::data_dir(db_path);
    let key = format!("{}|{}|{}", spec.id, reranker.map_or("", |r| r.id), lantern_embed::index_id(&conn)?);

    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_none_or(|(loaded, _)| *loaded != key) {
        *guard = None;
        *guard = Some((key, Searcher::open(&conn, &data_dir, spec, reranker, threads)?));
    }
    let (_, searcher) = guard.as_mut().expect("loaded right above");
    searcher.rerank_pool = rerank_pool;
    searcher.ask(&conn, question, limit)
}

pub fn forget(shared: &SharedSearcher) {
    // If a question is being answered right now the key check in `ask` still catches a new
    // index. Vectors added by an embed run are picked up the next time it is loaded.
    if let Ok(mut guard) = shared.try_lock() {
        *guard = None;
    }
}

pub fn lookup(db_path: &Path, query: &Query) -> Result<render::Doc> {
    render::document(&db::open(db_path)?, query)
}

pub fn fetch_model(source: Source, data_dir: &Path, mut on_progress: impl FnMut(Download)) -> Result<()> {
    let mut last = Instant::now() - Duration::from_secs(1);
    download::fetch(source, data_dir, |p| {
        if last.elapsed() >= Duration::from_millis(100) || p.downloaded == p.total {
            last = Instant::now();
            on_progress(Download { file: p.file.to_owned(), file_index: p.file_index, file_count: p.file_count, downloaded: p.downloaded, total: p.total });
        }
    })?;
    Ok(())
}

pub fn embed(
    db_path: PathBuf,
    spec: &'static ModelSpec,
    threads: usize,
    stop: Arc<AtomicBool>,
    mut on_progress: impl FnMut(BuildProgress),
) -> Result<BuildReport> {
    let conn = db::open(&db_path)?;
    let data_dir = lantern_embed::data_dir(&db_path);
    let mut engine = lantern_embed::load_engine(spec, &data_dir, threads)?;
    lantern_embed::build(
        &conn,
        &data_dir,
        &mut engine,
        None,
        || stop.load(Ordering::Relaxed),
        |p| {
            if matches!(p.phase, Phase::Embedding) || p.done % 128 == 0 || p.done == p.total {
                on_progress(p);
            }
        },
    )
}

pub fn version(
    data_dir: &Path,
    id: &str,
    fetch: Option<&versions::Remote>,
    decompile: bool,
    threads: usize,
    stop: &AtomicBool,
    mut on_progress: impl FnMut(versions::Progress),
) -> Result<()> {
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut report = |p: versions::Progress| {
        if last.elapsed() >= Duration::from_millis(100) || p.done == p.total {
            last = Instant::now();
            on_progress(p);
        }
    };
    if let Some(remote) = fetch {
        versions::fetch(data_dir, remote, stop, &mut report)?;
    }
    if decompile {
        versions::decompile(data_dir, id, threads, stop, &mut report)?;
    }
    Ok(())
}
