//! Setting Lantern up in one go: the models, then every version downloaded, decompiled and
//! indexed, then the newest embedded. The first run screen and `lantern setup` both run this.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use lantern_core::{
    index::{self, Inputs},
    versions::{self, Remote},
};

use crate::{
    Phase, download,
    models::{ModelSpec, RerankerSpec},
};

/// One thing the setup does.
#[derive(Clone, Debug)]
pub enum Task {
    Model(&'static ModelSpec),
    Reranker(&'static RerankerSpec),
    Fetch(Remote),
    Decompile(String),
    Index(String),
    Embed(String, &'static ModelSpec),
}

impl Task {
    pub fn title(&self) -> String {
        match self {
            Task::Model(spec) => format!("Download {}", spec.name),
            Task::Reranker(spec) => format!("Download {}", spec.name),
            Task::Fetch(remote) => format!("Download {}", remote.id),
            Task::Decompile(id) => format!("Decompile {id}"),
            Task::Index(id) => format!("Index {id}"),
            Task::Embed(id, _) => format!("Embed {id}"),
        }
    }
}

/// What the numbers of a progress report count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Bytes,
    Count(&'static str),
    /// Busy, nothing to count.
    None,
}

#[derive(Debug)]
pub enum Event {
    Started(usize),
    Progress {
        task: usize,
        phase: String,
        done: u64,
        total: u64,
        unit: Unit,
    },
    Done(usize, Result<(), String>),
    /// Every task went through.
    Finished,
}

/// The tasks for `versions` (newest first) and the models, in the order they run. What is
/// on disk already is left out or skipped when it runs. Every version is embedded once all are
/// indexed, the newest first: it is the one that opens.
pub fn plan(data_dir: &Path, versions: &[Remote], embedding: &'static ModelSpec, reranker: &'static RerankerSpec, embed: bool) -> Vec<Task> {
    let mut tasks = Vec::new();
    if !download::is_downloaded(embedding.source(), data_dir) {
        tasks.push(Task::Model(embedding));
    }
    if !download::is_downloaded(reranker.source(), data_dir) {
        tasks.push(Task::Reranker(reranker));
    }
    let java = versions::find_java().is_some();
    // Oldest first, so the newest is the last one built and the one that opens.
    for version in versions.iter().rev() {
        tasks.push(Task::Fetch(version.clone()));
        if java {
            tasks.push(Task::Decompile(version.id.clone()));
        }
        tasks.push(Task::Index(version.id.clone()));
    }
    if embed && java {
        tasks.extend(versions.iter().map(|version| Task::Embed(version.id.clone(), embedding)));
    }
    tasks
}

/// Runs `tasks` one after the other, from `from` on. Stops at the first that fails. `stop`
/// ends embedding early and skips the embedding left, what is embedded by then stays.
pub fn run(tasks: &[Task], from: usize, data_dir: &Path, threads: usize, stop: &AtomicBool, mut send: impl FnMut(Event)) {
    for (index, task) in tasks.iter().enumerate().skip(from) {
        send(Event::Started(index));
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut report = |phase: &str, done: u64, total: u64, unit: Unit| {
            if last.elapsed() >= Duration::from_millis(120) || done == total {
                last = Instant::now();
                send(Event::Progress { task: index, phase: phase.to_owned(), done, total, unit });
            }
        };
        let result = run_one(task, data_dir, threads, stop, &mut report);
        let failed = result.is_err();
        send(Event::Done(index, result.map_err(|e| format!("{e:#}"))));
        if failed {
            return;
        }
    }
    send(Event::Finished);
}

fn run_one(task: &Task, data_dir: &Path, threads: usize, stop: &AtomicBool, report: &mut dyn FnMut(&str, u64, u64, Unit)) -> Result<()> {
    let version_dir = |id: &str| versions::root(data_dir).join(versions::folder(id));
    let mut model = |source| download::fetch(source, data_dir, |p| report(p.file, p.downloaded as u64, p.total as u64, Unit::Bytes)).map(|_| ());
    match task {
        Task::Model(spec) => model(spec.source()),
        Task::Reranker(spec) => model(spec.source()),
        Task::Fetch(remote) => {
            if version_dir(&remote.id).join("server.jar").is_file() {
                return Ok(());
            }
            versions::fetch(data_dir, remote, stop, |p| match p.step {
                versions::Step::Download => report("Downloading the server", p.done, p.total, Unit::Bytes),
                _ => report("Unpacking", 0, 0, Unit::None),
            })
        }
        Task::Decompile(id) => {
            if version_dir(id).join("src").is_dir() {
                return Ok(());
            }
            versions::decompile(data_dir, id, threads, stop, |p| match p.step {
                versions::Step::Vineflower => report("Downloading Vineflower", p.done, p.total, Unit::Bytes),
                // Every class is counted twice, read and written.
                _ => report("Decompiling with Vineflower", p.done / 2, p.total / 2, Unit::Count("classes")),
            })
        }
        Task::Index(id) => {
            let dir = version_dir(id);
            let src = Some(dir.join("src")).filter(|p| p.is_dir());
            let data = src.as_ref().map(|s| s.join("data")).filter(|p| p.is_dir());
            let inputs = Inputs { version: id.clone(), jar: dir.join("server.jar"), src, data };
            index::build_file(&versions::index_path(data_dir, id), &inputs, &mut |p| {
                if p.total > 0 {
                    report(p.step.label(), p.done as u64, p.total as u64, Unit::Count(p.step.unit()));
                } else {
                    report(p.step.label(), 0, 0, Unit::None);
                }
            })
            .map(|_| ())
        }
        Task::Embed(id, spec) => {
            // Embedding was skipped, the versions after it are too.
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let conn = lantern_core::db::open(&versions::index_path(data_dir, id))?;
            let mut engine = crate::load_engine(spec, data_dir, threads)?;
            crate::build(
                &conn,
                data_dir,
                &mut engine,
                None,
                || stop.load(Ordering::Relaxed),
                |p| match p.phase {
                    Phase::Chunking => report("Cutting sources into chunks", p.done as u64, p.total as u64, Unit::Count("classes and methods")),
                    Phase::Reusing => report("Checking cached embeddings", p.done as u64, p.total as u64, Unit::Count("chunks")),
                    Phase::Embedding => report("Embedding", p.done as u64, p.total as u64, Unit::Count("chunks")),
                },
            )
            .map(|_| ())
        }
    }
}
