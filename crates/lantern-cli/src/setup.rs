//! `lantern setup`: the first run screen of the app for the terminal. Asks what to set up when
//! nothing is given on the command line, then downloads, decompiles, indexes and embeds.

use std::{
    io::{BufRead, IsTerminal, Write},
    sync::atomic::AtomicBool,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use lantern_core::{
    settings::{PROVIDERS, Settings},
    versions::{self, Remote},
};
use lantern_embed::{
    models::{self, DEFAULT_MODEL, DEFAULT_RERANKER, MODELS, RERANKERS},
    setup::{self, Event, Unit},
};

pub struct Options {
    pub versions: Vec<String>,
    pub model: Option<String>,
    pub reranker: Option<String>,
    pub no_embed: bool,
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub chat_model: Option<String>,
    pub yes: bool,
}

fn ask(question: &str) -> Result<String> {
    eprint!("{question} ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// A number from a list, or the default on Enter.
fn pick(question: &str, count: usize, default: usize) -> Result<usize> {
    loop {
        let answer = ask(&format!("{question} [{}]", default + 1))?;
        if answer.is_empty() {
            return Ok(default);
        }
        match answer.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => return Ok(n - 1),
            _ => eprintln!("  a number from 1 to {count}"),
        }
    }
}

fn yes_no(question: &str, default: bool) -> Result<bool> {
    let answer = ask(&format!("{question} [{}]", if default { "Y/n" } else { "y/N" }))?.to_lowercase();
    Ok(if answer.is_empty() { default } else { answer.starts_with('y') })
}

/// `release` and `snapshot` stand for the newest of each, anything else is an id.
fn resolve(list: &[Remote], wanted: &str) -> Result<Remote> {
    let found = match wanted {
        "release" | "snapshot" => list.iter().find(|v| v.kind == wanted),
        id => list.iter().find(|v| v.id == id),
    };
    found.cloned().with_context(|| format!("Mojang lists no version {wanted:?} from 26.1 on"))
}

/// Sets up into `data_dir`, the folder of settings, models and versions.
pub fn run(data_dir: &std::path::Path, options: Options) -> Result<()> {
    let data_dir = data_dir.to_path_buf();
    let interactive = !options.yes && std::io::stdin().is_terminal();
    let list = versions::fetch_list(&data_dir).or_else(|err| {
        let cached = versions::cached_list(&data_dir);
        if cached.is_empty() { Err(err) } else { Ok(cached) }
    })?;
    if list.is_empty() {
        bail!("Mojang lists no versions from 26.1 on");
    }

    // Versions
    let picked: Vec<Remote> = if !options.versions.is_empty() {
        options.versions.iter().map(|v| resolve(&list, v)).collect::<Result<_>>()?
    } else if interactive {
        eprintln!("Versions, from Mojang:");
        let shown: Vec<&Remote> = list.iter().take(15).collect();
        for (index, version) in shown.iter().enumerate() {
            let kind = if version.kind == "release" { "release " } else { "snapshot" };
            eprintln!("  {:>2}  {:<22} {kind}  {}", index + 1, version.id, version.date());
        }
        let default = list.iter().position(|v| v.kind == "release").unwrap_or(0);
        loop {
            let answer = ask(&format!("Which ones? Numbers or ids, comma separated [{}]", default + 1))?;
            let parts: Vec<&str> = answer.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
            if parts.is_empty() {
                break vec![list[default].clone()];
            }
            let chosen: Result<Vec<Remote>> = parts
                .iter()
                .map(|part| match part.parse::<usize>() {
                    Ok(n) if (1..=shown.len()).contains(&n) => Ok(shown[n - 1].clone()),
                    _ => resolve(&list, part),
                })
                .collect();
            match chosen {
                Ok(chosen) => break chosen,
                Err(err) => eprintln!("  {err:#}"),
            }
        }
    } else {
        vec![resolve(&list, "release")?]
    };
    // Newest first, as Mojang lists them.
    let mut picked = picked;
    picked.sort_by(|a, b| b.release_time.cmp(&a.release_time));
    picked.dedup_by(|a, b| a.id == b.id);

    // Models
    let embedding = match (&options.model, interactive) {
        (Some(id), _) => models::find(id).with_context(|| format!("no embedding model called {id:?}"))?,
        (None, true) => {
            eprintln!("\nEmbedding model, turns code and questions into vectors:");
            for (index, spec) in MODELS.iter().enumerate() {
                eprintln!("  {:>2}  {}  ({} MB){}", index + 1, spec.name, spec.download_mb, if spec.id == DEFAULT_MODEL { "  recommended" } else { "" });
            }
            &MODELS[pick("Which one?", MODELS.len(), MODELS.iter().position(|m| m.id == DEFAULT_MODEL).unwrap_or(0))?]
        }
        (None, false) => models::find(DEFAULT_MODEL).expect("the default model is in the list"),
    };
    let reranker = match (&options.reranker, interactive) {
        (Some(id), _) => models::find_reranker(id).with_context(|| format!("no reranker called {id:?}"))?,
        (None, true) => {
            eprintln!("\nReranker, puts the results that answer a question first:");
            for (index, spec) in RERANKERS.iter().enumerate() {
                eprintln!("  {:>2}  {}  ({} MB){}", index + 1, spec.name, spec.download_mb, if spec.id == DEFAULT_RERANKER { "  recommended" } else { "" });
            }
            &RERANKERS[pick("Which one?", RERANKERS.len(), RERANKERS.iter().position(|r| r.id == DEFAULT_RERANKER).unwrap_or(0))?]
        }
        (None, false) => models::find_reranker(DEFAULT_RERANKER).expect("the default reranker is in the list"),
    };
    let embed = !options.no_embed && (!interactive || yes_no("\nEmbed every version? Ask needs it, it runs a while on the CPU per version", true)?);

    // Chat, optional
    let mut settings = Settings::load(&data_dir);
    let chat = match (&options.provider, interactive) {
        (Some(id), _) => Some((
            PROVIDERS.iter().find(|p| p.id == id).with_context(|| format!("no provider called {id:?}"))?,
            options.api_key.clone().unwrap_or_default(),
            options.chat_model.clone().unwrap_or_default(),
        )),
        (None, true) => {
            eprintln!("\nChat, optional. A model that answers questions by looking things up in Lantern.");
            for (index, provider) in PROVIDERS.iter().enumerate() {
                eprintln!("  {:>2}  {}", index + 1, provider.name);
            }
            let answer = ask("Provider number, Enter to skip:")?;
            match answer.parse::<usize>() {
                Ok(n) if (1..=PROVIDERS.len()).contains(&n) => {
                    let provider = &PROVIDERS[n - 1];
                    // Read as typed, it shows. LANTERN_API_KEY works too and is never stored.
                    let key = ask("API key (shown as you type, Enter for none):")?;
                    let model = ask(&format!("Model [{}]:", provider.example_model))?;
                    Some((provider, key, if model.is_empty() { provider.example_model.to_owned() } else { model }))
                }
                _ => None,
            }
        }
        (None, false) => None,
    };

    let tasks = setup::plan(&data_dir, &picked, embedding, reranker, embed);
    eprintln!();
    eprintln!("Versions   {}", picked.iter().map(|v| v.id.as_str()).collect::<Vec<_>>().join(", "));
    eprintln!("Embedding  {}", embedding.name);
    eprintln!("Reranker   {}", reranker.name);
    eprintln!("Chat       {}", chat.as_ref().map_or("not set up".to_owned(), |(p, _, m)| format!("{} {m}", p.name)));
    eprintln!("Into       {}", data_dir.display());
    if versions::find_java().is_none() {
        eprintln!("No Java found: the versions are indexed without sources. Install Java 21 or newer to decompile them.");
    }
    if interactive && !yes_no("Go?", true)? {
        return Ok(());
    }

    // The choices are kept even if a step fails, a later run picks up from there.
    settings.embedding_model = embedding.id.to_owned();
    settings.reranker = reranker.id.to_owned();
    if let Some((provider, key, model)) = &chat {
        settings.chat.provider = provider.id.to_owned();
        let account = settings.chat.accounts.entry(provider.id.to_owned()).or_default();
        account.api_key.clone_from(key);
        account.model.clone_from(model);
    }
    settings.save(&data_dir)?;

    let stop = AtomicBool::new(false);
    let threads = lantern_embed::threads(&settings);
    let mut failed = None;
    let mut phase: Option<(String, Instant, u64)> = None;
    let count = tasks.len();
    setup::run(&tasks, 0, &data_dir, threads, &stop, |event| match event {
        Event::Started(task) => {
            phase = None;
            eprintln!("[{}/{count}] {}", task + 1, tasks[task].title());
        }
        Event::Progress { phase: name, done, total, unit, .. } => {
            if phase.as_ref().is_none_or(|(running, _, _)| *running != name) {
                if phase.is_some() {
                    eprintln!();
                }
                phase = Some((name.clone(), Instant::now(), done));
            }
            let (_, started, base) = phase.as_ref().expect("set right above");
            let seconds = started.elapsed().as_secs_f64();
            let rate = (seconds > 1.5 && done > *base).then(|| (done - base) as f64 / seconds);
            let left = |rate: f64| left(total.saturating_sub(done) as f64 / rate);
            let line = match unit {
                Unit::Bytes => {
                    let mb = |b: u64| b as f64 / 1_048_576.0;
                    let speed = rate.map_or(String::new(), |r| format!(", {:.1} MB/s{}", mb(r as u64), left(r)));
                    format!("{name}: {:.1} of {:.1} MB{speed}", mb(done), mb(total))
                }
                Unit::Count(what) => {
                    let speed = rate.map_or(String::new(), |r| format!(", {r:.0} per second{}", left(r)));
                    format!("{name}: {done} of {total} {what}{speed}")
                }
                Unit::None => format!("{name} ..."),
            };
            eprint!("\r      {line:<100}");
            let _ = std::io::stderr().flush();
        }
        Event::Done(task, result) => {
            if phase.is_some() {
                eprintln!();
            }
            match result {
                Ok(()) => eprintln!("      done"),
                Err(err) => {
                    eprintln!("      failed: {err}");
                    failed = Some(task);
                }
            }
        }
        Event::Finished => {}
    });

    // The newest version that got an index becomes the one every command reads.
    let newest = picked.iter().find(|v| versions::index_path(&data_dir, &v.id).exists());
    let mut settings = Settings::load(&data_dir);
    if let Some(version) = newest {
        settings.version = versions::folder(&version.id);
    }
    settings.onboarded = true;
    settings.save(&data_dir)?;
    match (failed, newest) {
        (Some(task), _) => bail!("{} failed, run `lantern setup` again to retry", tasks[task].title()),
        (None, Some(version)) => eprintln!("\nDone. Lantern reads {} now, try `lantern topic sniffer`.", version.id),
        (None, None) => eprintln!("\nDone."),
    }
    Ok(())
}

fn left(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 1.0 {
        return String::new();
    }
    match seconds as u64 {
        s if s < 60 => format!(", {s} s left"),
        s if s < 3600 => format!(", about {} min left", s.div_ceil(60)),
        s => format!(", about {:.1} h left", s as f64 / 3600.0),
    }
}
