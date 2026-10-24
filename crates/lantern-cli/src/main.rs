mod setup;
mod setupmcp;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use lantern_core::{
    db, index,
    render::{self, Query},
};

#[derive(Parser)]
#[command(name = "lantern", version, about = "Index and query a decompiled Minecraft server")]
struct Cli {
    /// Index database, defaults to $LANTERN_DB or ~/.local/share/lantern/lantern.db
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register Lantern's MCP server with Claude Code or Codex
    #[command(name = "setupmcp")]
    SetupMcp {
        #[arg(value_enum)]
        client: setupmcp::Client,
        /// MCP server binary, defaults to lantern-mcp next to this executable
        #[arg(long)]
        mcp_bin: Option<PathBuf>,
    },
    /// Set Lantern up: download versions and models, decompile, index and embed. Asks what to
    /// set up when nothing is given
    Setup {
        /// A version from 26.1 on, more than once for several. `release` and `snapshot` mean
        /// the newest of each
        #[arg(long = "version", value_name = "VERSION")]
        versions: Vec<String>,
        /// Embedding model id, see `lantern models`
        #[arg(long)]
        model: Option<String>,
        /// Reranker id
        #[arg(long)]
        reranker: Option<String>,
        /// Do not embed the versions, `lantern embed` does it later
        #[arg(long)]
        no_embed: bool,
        /// Chat provider id, like openrouter or ollama
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        chat_model: Option<String>,
        /// Ask nothing, take the defaults for what is not given
        #[arg(long, short)]
        yes: bool,
    },
    /// Build the index from the server jar, the data folder and the decompiled sources
    Index {
        #[arg(long)]
        jar: PathBuf,
        /// The `data` folder of the datapack, the one that contains `minecraft/`
        #[arg(long)]
        data: Option<PathBuf>,
        /// Root of the decompiled sources, the folder that contains `net/`
        #[arg(long)]
        src: Option<PathBuf>,
        #[arg(long, default_value = "unknown")]
        version: String,
    },
    /// Classes matching a name, with super class, interfaces, fields and methods
    Class { name: String },
    /// Who calls `Class.method`, overrides included
    Callers { target: String },
    /// What `Class.method` calls
    Callees { target: String },
    /// Reads and writes of `Class.FIELD`
    Refs { target: String },
    /// A tag or registry key: members, the Java field holding it and where that field is used
    Key { ident: String },
    /// Methods that load a string constant containing the pattern
    Strings { pattern: String },
    /// Full text search over the decompiled sources
    Search {
        query: String,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Decompiled source of a class, or of one method in it
    Source { class: String, method: Option<String> },
    /// List the embedding models, or download one
    Models {
        #[command(subcommand)]
        action: Option<ModelAction>,
    },
    /// Embed the index with a model so that `ask` works. Can be interrupted and resumed
    Embed {
        #[arg(long)]
        model: Option<String>,
        /// Only embed the first N chunks, for trying a model out
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        threads: Option<usize>,
    },
    /// What changed between two versions: the changed files, or the diff of one of them
    Compare {
        /// A downloaded and decompiled version, or a folder of decompiled sources
        old: String,
        new: String,
        /// A file relative to the source root, or a class name like `Creeper`
        file: Option<String>,
        /// Unchanged lines shown around a change
        #[arg(long, default_value_t = 3)]
        context: usize,
    },
    /// Ask in plain English, answered from embeddings and full text search together
    Ask {
        question: String,
        #[arg(long)]
        model: Option<String>,
        /// Reranker to order the results with, the default one is used when it is downloaded
        #[arg(long)]
        rerank: Option<String>,
        #[arg(long, conflicts_with = "rerank")]
        no_rerank: bool,
        #[arg(long, default_value_t = 12)]
        limit: usize,
    },
    /// Ask a chat model that uses Lantern as its tools. Endpoint, key and model come from the settings
    Chat {
        question: String,
        #[arg(long)]
        model: Option<String>,
        /// Base URL of an OpenAI compatible API, instead of the one in the settings
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Everything connected to a topic: matching classes, their neighbours in the call graph and the data they touch
    Topic {
        seed: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
    },
}

#[derive(Subcommand)]
enum ModelAction {
    /// Download a model into the lantern data folder
    Download { id: String },
}

fn model_spec(id: Option<&str>) -> Result<&'static lantern_embed::models::ModelSpec> {
    let id = id.unwrap_or(lantern_embed::models::DEFAULT_MODEL);
    lantern_embed::models::find(id).ok_or_else(|| anyhow::anyhow!("unknown model `{id}`, see `lantern models`"))
}

fn progress_line(label: &str, done: usize, total: usize) {
    use std::io::Write;
    let percent = if total == 0 { 100.0 } else { done as f32 * 100.0 / total as f32 };
    eprint!("\r{label} {done}/{total} ({percent:.1}%)   ");
    let _ = std::io::stderr().flush();
}

fn run_compare(old: &str, new: &str, file: Option<&str>, context: usize) -> Result<()> {
    use lantern_core::compare;
    let data_dir = lantern_core::default_data_dir();
    let root = |side: &str| {
        let path = PathBuf::from(side);
        if path.is_dir() { Ok(path) } else { compare::source_root(&data_dir, side) }
    };
    let (old_root, new_root) = (root(old)?, root(new)?);

    let Some(file) = file else {
        let changes = compare::changes(&old_root, &new_root, |done, total| progress_line("comparing", done, total))?;
        eprintln!();
        let (mut added, mut removed) = (0, 0);
        for change in &changes {
            let lines = if change.binary { "binary".to_owned() } else { format!("+{} -{}", change.added, change.removed) };
            println!("{}  {}  {lines}", change.status.letter(), change.path);
            added += change.added;
            removed += change.removed;
        }
        println!("{} files changed, {added} insertions(+), {removed} deletions(-)", changes.len());
        return Ok(());
    };

    // A bare class name stands for its file, wherever it is in either version.
    let path = if file.contains('/') {
        file.to_owned()
    } else {
        let name = if file.contains('.') { file.to_owned() } else { format!("{file}.java") };
        [&old_root, &new_root]
            .iter()
            .find_map(|root| {
                walkdir::WalkDir::new(root)
                    .into_iter()
                    .flatten()
                    .find(|e| e.file_name().to_string_lossy() == name)
                    .and_then(|e| e.path().strip_prefix(root).ok().map(|p| p.to_string_lossy().replace('\\', "/")))
            })
            .ok_or_else(|| anyhow::anyhow!("no file called {name} in either version"))?
    };
    let hunks = compare::diff_file(&old_root, &new_root, &path, context)?;
    if hunks.is_empty() {
        println!("{path} is the same in both");
    } else {
        print!("{}", compare::unified(&path, &hunks));
    }
    Ok(())
}

fn run_models(path: &std::path::Path, action: Option<ModelAction>) -> Result<()> {
    let data_dir = lantern_embed::data_dir(path);
    let vectors = db::open(path).ok().map(|conn| lantern_embed::vectors_dir(&conn, &data_dir));
    match action {
        None => {
            for spec in lantern_embed::models::MODELS {
                let default = if spec.id == lantern_embed::models::DEFAULT_MODEL { " (default)" } else { "" };
                let state = match (
                    lantern_embed::download::is_downloaded(spec.source(), &data_dir),
                    vectors.as_ref().is_some_and(|vectors| lantern_embed::store::Store::exists(vectors, spec)),
                ) {
                    (true, true) => "downloaded, embedded",
                    (true, false) => "downloaded",
                    _ => "not downloaded",
                };
                println!(
                    "{}{default}\n  {}, {}M parameters, {} MB, {} tokens context, {}",
                    spec.id, spec.name, spec.params_millions, spec.download_mb, spec.context_tokens, spec.license
                );
                println!("  {}\n  [{state}]\n", spec.note);
            }
            println!("rerankers\n");
            for spec in lantern_embed::models::RERANKERS {
                let default = if spec.id == lantern_embed::models::DEFAULT_RERANKER { " (default)" } else { "" };
                let state = if lantern_embed::download::is_downloaded(spec.source(), &data_dir) { "downloaded" } else { "not downloaded" };
                println!(
                    "{}{default}\n  {}, {}M parameters, {} MB, {} tokens context, {}",
                    spec.id, spec.name, spec.params_millions, spec.download_mb, spec.context_tokens, spec.license
                );
                println!("  {}\n  [{state}]\n", spec.note);
            }
        }
        Some(ModelAction::Download { id }) => {
            let (name, source) = match (lantern_embed::models::find(&id), lantern_embed::models::find_reranker(&id)) {
                (Some(spec), _) => (spec.name, spec.source()),
                (None, Some(spec)) => (spec.name, spec.source()),
                (None, None) => anyhow::bail!("no model called `{id}`, `lantern models` lists them"),
            };
            lantern_embed::download::fetch(source, &data_dir, |p| {
                progress_line(&format!("{} ({}/{})", p.file, p.file_index + 1, p.file_count), p.downloaded / 1_048_576, p.total / 1_048_576);
            })?;
            eprintln!();
            println!("{name} is ready in {}", lantern_embed::download::models_dir(&data_dir).display());
        }
    }
    Ok(())
}

impl Command {
    fn into_query(self) -> Query {
        match self {
            Self::Class { name } => Query::Class { name },
            Self::Callers { target } => Query::Callers { target },
            Self::Callees { target } => Query::Callees { target },
            Self::Refs { target } => Query::Refs { target },
            Self::Key { ident } => Query::Key { ident },
            Self::Strings { pattern } => Query::Strings { pattern },
            Self::Search { query, limit } => Query::Search { query, limit },
            Self::Source { class, method } => Query::Source { class, method },
            Self::Topic { seed, depth } => Query::Topic { seed, depth },
            Self::Setup { .. }
            | Self::SetupMcp { .. }
            | Self::Index { .. }
            | Self::Compare { .. }
            | Self::Models { .. }
            | Self::Embed { .. }
            | Self::Ask { .. }
            | Self::Chat { .. } => {
                unreachable!()
            }
        }
    }
}

fn main() -> Result<()> {
    // `lantern topic x | head` should end quietly, Rust ignores SIGPIPE and println! panics.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let path = lantern_core::db_path(cli.db.clone());

    if let Command::Setup { versions, model, reranker, no_embed, provider, api_key, chat_model, yes } = cli.command {
        return setup::run(&lantern_core::data_dir(&path), setup::Options { versions, model, reranker, no_embed, provider, api_key, chat_model, yes });
    }

    if let Command::SetupMcp { client, mcp_bin } = &cli.command {
        return setupmcp::run(*client, &path, mcp_bin.as_deref());
    }

    if let Command::Index { jar, data, src, version } = &cli.command {
        let started = std::time::Instant::now();
        let inputs = index::Inputs { version: version.clone(), jar: jar.clone(), src: src.clone(), data: data.clone() };
        // Without --db or $LANTERN_DB the index goes into the folder of its version and that
        // version becomes the one every command reads.
        let chosen = cli.db.is_none() && std::env::var_os("LANTERN_DB").is_none();
        let data_dir = lantern_core::default_data_dir();
        let path = if chosen { lantern_core::versions::index_path(&data_dir, version) } else { path };
        let mut step = None;
        let stats = index::build_file(&path, &inputs, &mut |p| {
            if step != Some(p.step) {
                if step.is_some() {
                    eprintln!();
                }
                step = Some(p.step);
            }
            if p.total > 0 {
                progress_line(p.step.label(), p.done + 1, p.total);
            } else {
                eprint!("\r{} ...", p.step.label());
            }
        })?;
        eprintln!();
        if chosen {
            if !jar.starts_with(lantern_core::versions::root(&data_dir)) {
                lantern_core::versions::mark_custom(&data_dir, version)?;
            }
            let mut settings = lantern_core::settings::Settings::load(&data_dir);
            settings.version = lantern_core::versions::folder(version);
            settings.save(&data_dir)?;
        }
        for (name, err) in &stats.skipped {
            eprintln!("skipped {name}: {err}");
        }
        println!(
            "{} classes (+{} external), {} methods, {} calls ({} unresolved), {} overrides",
            stats.classes, stats.external_classes, stats.methods, stats.calls, stats.unresolved_calls, stats.overrides
        );
        println!("{} key fields, {} method sources, {} data files", stats.key_fields, stats.method_sources, stats.data_files);
        println!("wrote {} in {:.1}s", path.display(), started.elapsed().as_secs_f32());
        return Ok(());
    }

    if let Command::Compare { old, new, file, context } = &cli.command {
        return run_compare(old, new, file.as_deref(), *context);
    }

    if let Command::Models { action } = cli.command {
        return run_models(&path, action);
    }

    let conn = db::open(&path)?;
    let data_dir = lantern_embed::data_dir(&path);
    match cli.command {
        Command::Embed { model, limit, threads } => {
            let spec = model_spec(model.as_deref())?;
            let threads = threads.unwrap_or_else(lantern_embed::default_threads);
            let mut engine = lantern_embed::load_engine(spec, &data_dir, threads)?;
            eprintln!("{} on {threads} threads", spec.name);
            let report = lantern_embed::build(
                &conn,
                &data_dir,
                &mut engine,
                limit,
                || false,
                |p| {
                    let label = match p.phase {
                        lantern_embed::Phase::Chunking => "cutting chunks",
                        lantern_embed::Phase::Reusing => "checking cached embeddings",
                        lantern_embed::Phase::Embedding => "embedding",
                    };
                    progress_line(label, p.done, p.total);
                },
            )?;
            eprintln!();
            println!(
                "{} chunks: {} embedded, {} already there, {} stale removed, {:.0}s",
                report.chunks, report.embedded, report.reused, report.removed, report.seconds
            );
        }
        Command::Ask { question, model, rerank, no_rerank, limit } => {
            let spec = model_spec(model.as_deref())?;
            let reranker = match rerank {
                Some(id) => Some(lantern_embed::models::find_reranker(&id).ok_or_else(|| anyhow::anyhow!("no reranker called `{id}`"))?),
                None if no_rerank => None,
                None => lantern_embed::models::find_reranker(lantern_embed::models::DEFAULT_RERANKER)
                    .filter(|spec| lantern_embed::download::is_downloaded(spec.source(), &data_dir)),
            };
            let mut searcher = lantern_embed::Searcher::open(&conn, &data_dir, spec, reranker, lantern_embed::default_threads())?;
            let started = std::time::Instant::now();
            print!("{}", lantern_embed::render_answers(&conn, &searcher.ask(&conn, &question, limit)?));
            let with = searcher.reranker().map_or(String::new(), |r| format!(", reranked by {}", r.name));
            eprintln!("{:.1}s{with}", started.elapsed().as_secs_f32());
        }
        Command::Chat { question, model, endpoint } => {
            use std::io::Write;
            let mut settings = lantern_core::settings::Settings::load(&data_dir).chat;
            if let Some(endpoint) = endpoint {
                settings.provider = "custom".to_owned();
                settings.accounts.entry("custom".to_owned()).or_default().base_url = endpoint;
            }
            if let Some(model) = model {
                settings.accounts.entry(settings.provider.clone()).or_default().model = model;
            }
            let mut session = lantern_chat::session::Session::new(&path)?;
            let stop = std::sync::atomic::AtomicBool::new(false);
            session.send(&settings, &question, &stop, |event| match event {
                lantern_chat::session::Event::Thinking(text) => eprint!("{text}"),
                lantern_chat::session::Event::ToolComing(_) | lantern_chat::session::Event::Waiting { .. } => {}
                lantern_chat::session::Event::Retrying { status, seconds } => eprintln!("[{status}, trying again in {seconds}s]"),
                lantern_chat::session::Event::Text(text) => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                lantern_chat::session::Event::ToolStarted { name, arguments } => eprintln!("\n[{name} {arguments}]"),
                lantern_chat::session::Event::ToolFinished { lines, failed, .. } => {
                    eprintln!("[{} lines{}]", lines, if failed { ", failed" } else { "" });
                }
                lantern_chat::session::Event::Usage(u) => {
                    eprintln!("[{} prompt tokens, {} from cache, {} written to cache, {} out]", u.prompt, u.cached, u.cache_written, u.completion);
                }
            })?;
            println!();
        }
        other => println!("{}", render::run(&conn, &other.into_query())?),
    }
    Ok(())
}
