//! Minecraft versions from Mojang's launcher manifest, downloaded into the data folder and
//! decompiled with Vineflower. Each one lives in `versions/<id>/`: `server.jar` is the jar the
//! bundler carries, `src/` is what Vineflower made of it, with the `data/` folder inside, and
//! `index.db` the index built from them. An index built from files elsewhere gets a folder of
//! its own too, with only the index in it.

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
/// The first server jar without obfuscation. Older ones need their mappings applied before the
/// names mean anything, which Lantern does not do, so they are not offered.
const FIRST_UNOBFUSCATED: &str = "26.1-snapshot-1";
const VINEFLOWER: &str = "1.11.1";

/// A version Mojang offers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Remote {
    pub id: String,
    /// `release` or `snapshot`.
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

impl Remote {
    pub fn date(&self) -> &str {
        self.release_time.get(..10).unwrap_or(&self.release_time)
    }
}

/// A version in the data folder.
#[derive(Clone, Debug)]
pub struct Installed {
    pub id: String,
    /// `release`, `snapshot`, or `custom` for an index built from files elsewhere.
    pub kind: String,
    pub release_time: String,
    /// None for a custom index.
    pub jar: Option<PathBuf>,
    pub index: Option<PathBuf>,
    /// Set once Vineflower went through all of it.
    pub src: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Download,
    Unpack,
    Vineflower,
    Decompile,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub step: Step,
    pub done: u64,
    pub total: u64,
}

/// What is written next to the jar, the manifest is not always at hand to sort by.
#[derive(Default, Serialize, Deserialize)]
struct Info {
    kind: String,
    release_time: String,
}

pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("versions")
}

/// The folder name of a version, `/` and the like made harmless. Also what a custom index is
/// filed under, by the name it was given.
pub fn folder(version: &str) -> String {
    let name: String = version.trim().chars().map(|c| if c.is_alphanumeric() || "._-+ ".contains(c) { c } else { '_' }).collect();
    let name = name.trim_start_matches('.').trim().to_owned();
    if name.is_empty() { "custom".to_owned() } else { name }
}

/// Where the index of a version is.
pub fn index_path(data_dir: &Path, version: &str) -> PathBuf {
    root(data_dir).join(folder(version)).join("index.db")
}

fn dir(data_dir: &Path, id: &str) -> Result<PathBuf> {
    // Ids come from the network, they must not lead out of the folder.
    if id.is_empty() || id.starts_with('.') || id.contains(['/', '\\', ':']) {
        bail!("{id:?} is not a version id");
    }
    Ok(root(data_dir).join(id))
}

fn manifest_cache(data_dir: &Path) -> PathBuf {
    root(data_dir).join("manifest.json")
}

fn parse(manifest: &str) -> Result<Vec<Remote>> {
    let mut manifest: Value = serde_json::from_str(manifest)?;
    let versions: Vec<Remote> = serde_json::from_value(manifest["versions"].take()).context("reading the version manifest")?;
    let cutoff = versions.iter().find(|v| v.id == FIRST_UNOBFUSCATED).map(|v| v.release_time.clone());
    Ok(versions.into_iter().filter(|v| cutoff.as_ref().is_none_or(|c| v.release_time >= *c)).collect())
}

/// The list from the last time it was fetched, for when Mojang can not be reached.
pub fn cached_list(data_dir: &Path) -> Vec<Remote> {
    std::fs::read_to_string(manifest_cache(data_dir)).ok().and_then(|text| parse(&text).ok()).unwrap_or_default()
}

/// The versions Mojang offers, newest first.
pub fn fetch_list(data_dir: &Path) -> Result<Vec<Remote>> {
    let text = ureq::get(MANIFEST_URL).call().context("asking Mojang for the versions")?.into_string()?;
    let versions = parse(&text)?;
    std::fs::create_dir_all(root(data_dir))?;
    std::fs::write(manifest_cache(data_dir), &text)?;
    Ok(versions)
}

/// The downloaded versions, newest first.
pub fn installed(data_dir: &Path) -> Vec<Installed> {
    let Ok(entries) = std::fs::read_dir(root(data_dir)) else { return Vec::new() };
    let mut versions: Vec<Installed> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let jar = Some(path.join("server.jar")).filter(|p| p.is_file());
            let index = Some(path.join("index.db")).filter(|p| p.is_file());
            if jar.is_none() && index.is_none() {
                return None;
            }
            let info: Info = std::fs::read_to_string(path.join("version.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let src = Some(path.join("src")).filter(|p| p.is_dir());
            let data = src.as_ref().map(|s| s.join("data")).filter(|p| p.is_dir());
            Some(Installed {
                id: entry.file_name().to_string_lossy().into_owned(),
                kind: info.kind,
                release_time: info.release_time,
                jar,
                index,
                src,
                data,
                bytes: size(&path),
            })
        })
        .collect();
    versions.sort_by(|a, b| b.release_time.cmp(&a.release_time).then_with(|| b.id.cmp(&a.id)));
    versions
}

fn size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// Removes the version with its index and the vectors made from that.
pub fn remove(data_dir: &Path, id: &str) -> Result<()> {
    let dir = dir(data_dir, id)?;
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    let vectors = vectors_dir(data_dir, id);
    if vectors.exists() {
        std::fs::remove_dir_all(&vectors)?;
    }
    Ok(())
}

/// Where the vectors of a version's index are kept, one store per model.
pub fn vectors_dir(data_dir: &Path, version: &str) -> PathBuf {
    data_dir.join("embeddings").join(folder(version))
}

/// Notes what a custom index is, so the list can tell it from a download.
pub fn mark_custom(data_dir: &Path, version: &str) -> Result<()> {
    let info = root(data_dir).join(folder(version)).join("version.json");
    if !info.exists() {
        std::fs::write(info, serde_json::to_string_pretty(&Info { kind: "custom".to_owned(), release_time: String::new() })?)?;
    }
    Ok(())
}

/// Moves `lantern.db` from before there were versions into the folder of its version, with
/// its vectors. Returns the version it went to, None when there was nothing to move or that
/// folder has an index already.
pub fn adopt_legacy(data_dir: &Path) -> Result<Option<String>> {
    let legacy = data_dir.join("lantern.db");
    if !legacy.exists() {
        return Ok(None);
    }
    let (version, jar) = {
        let conn = rusqlite::Connection::open_with_flags(&legacy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let get = |key: &str| conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, String>(0)).unwrap_or_default();
        (get("version"), get("jar"))
    };
    // A jar from a downloaded version names the folder, anything else goes by its version.
    let root = root(data_dir);
    let id = Path::new(&jar)
        .strip_prefix(&root)
        .ok()
        .and_then(|rest| rest.iter().next())
        .map(|first| first.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder(&version));
    let target = root.join(&id);
    if target.join("index.db").exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(&target)?;
    std::fs::rename(&legacy, target.join("index.db"))?;
    if !target.join("server.jar").exists() {
        mark_custom(data_dir, &id)?;
    }

    // The vectors were made from that index, they go with it.
    let vectors = vectors_dir(data_dir, &id);
    if let Ok(entries) = std::fs::read_dir(data_dir.join("embeddings")) {
        std::fs::create_dir_all(&vectors)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                std::fs::rename(&path, vectors.join(entry.file_name()))?;
            }
        }
    }
    Ok(Some(id))
}

/// Streams `url` into `target`, checking the SHA-1 when one is known. Written next to the
/// target first, a stopped or broken download never looks finished.
fn download(url: &str, sha1: Option<&str>, target: &Path, stop: &AtomicBool, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
    let response = ureq::get(url).call().with_context(|| format!("downloading {url}"))?;
    let total = response.header("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let partial = target.with_extension("part");
    let mut file = std::fs::File::create(&partial).with_context(|| format!("writing {}", partial.display()))?;
    let mut reader = response.into_reader();
    let mut hasher = sha1_smol::Sha1::new();
    let mut buffer = vec![0; 1 << 16];
    let mut done = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&partial);
            bail!("Stopped");
        }
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
        done += read as u64;
        on_progress(done, total);
    }
    file.flush()?;
    drop(file);
    if let Some(expected) = sha1
        && hasher.digest().to_string() != expected
    {
        let _ = std::fs::remove_file(&partial);
        bail!("{url} came through damaged, the checksum does not match");
    }
    std::fs::rename(&partial, target)?;
    Ok(())
}

/// Downloads the server of `version` and unpacks the jar the bundler carries.
pub fn fetch(data_dir: &Path, version: &Remote, stop: &AtomicBool, mut on_progress: impl FnMut(Progress)) -> Result<()> {
    let dir = dir(data_dir, &version.id)?;
    let details: Value = ureq::get(&version.url).call().context("asking Mojang for the version")?.into_json()?;
    let downloads = &details["downloads"];
    if downloads.get("server_mappings").is_some() {
        bail!("{} is obfuscated, Lantern only reads versions from 26.1 on", version.id);
    }
    let server = &downloads["server"];
    let Some(url) = server["url"].as_str() else { bail!("{} has no server download", version.id) };

    std::fs::create_dir_all(&dir)?;
    let bundler = dir.join("server-bundler.jar");
    download(url, server["sha1"].as_str(), &bundler, stop, |done, total| {
        on_progress(Progress { step: Step::Download, done, total: total.max(server["size"].as_u64().unwrap_or(0)) })
    })?;

    on_progress(Progress { step: Step::Unpack, done: 0, total: 0 });
    let jar = dir.join("server.jar");
    unbundle(&bundler, &jar)?;
    std::fs::remove_file(&bundler)?;
    let info = Info { kind: version.kind.clone(), release_time: version.release_time.clone() };
    std::fs::write(dir.join("version.json"), serde_json::to_string_pretty(&info)?)?;
    Ok(())
}

/// The server download is a launcher with the real jar inside, listed in `versions.list`.
/// A jar without that list is the server itself.
fn unbundle(bundler: &Path, jar: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(bundler)?)?;
    let inner = match archive.by_name("META-INF/versions.list") {
        Ok(mut list) => {
            let mut text = String::new();
            list.read_to_string(&mut text)?;
            // `<sha256>\t<id>\t<path>`, one line per jar.
            let path = text.lines().find_map(|line| line.split('\t').nth(2)).context("versions.list is empty")?;
            Some(format!("META-INF/versions/{path}"))
        }
        Err(_) => None,
    };
    let partial = jar.with_extension("part");
    match inner {
        Some(name) => {
            let mut entry = archive.by_name(&name).with_context(|| format!("{name} is missing from the server jar"))?;
            std::io::copy(&mut entry, &mut std::fs::File::create(&partial)?)?;
        }
        None => {
            std::fs::copy(bundler, &partial)?;
        }
    }
    std::fs::rename(&partial, jar)?;
    Ok(())
}

/// `JAVA_HOME` first, then the `PATH`.
pub fn find_java() -> Option<PathBuf> {
    let name = if cfg!(windows) { "java.exe" } else { "java" };
    let home = std::env::var_os("JAVA_HOME").map(|home| PathBuf::from(home).join("bin"));
    let path = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect::<Vec<_>>()).unwrap_or_default();
    home.into_iter().chain(path).map(|dir| dir.join(name)).find(|java| java.is_file())
}

/// The Vineflower jar, downloaded from Maven Central the first time.
fn vineflower(data_dir: &Path, stop: &AtomicBool, mut on_progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let tools = data_dir.join("tools");
    let jar = tools.join(format!("vineflower-{VINEFLOWER}.jar"));
    if jar.is_file() {
        return Ok(jar);
    }
    std::fs::create_dir_all(&tools)?;
    let url = format!("https://repo1.maven.org/maven2/org/vineflower/vineflower/{VINEFLOWER}/vineflower-{VINEFLOWER}.jar");
    let sha1 = ureq::get(&format!("{url}.sha1")).call().context("asking Maven Central for Vineflower")?.into_string()?;
    download(&url, sha1.split_whitespace().next(), &jar, stop, |done, total| on_progress(Progress { step: Step::Vineflower, done, total }))?;
    Ok(jar)
}

/// Runs Vineflower over the jar of `id` into its `src/`. Takes a few minutes and needs Java.
pub fn decompile(data_dir: &Path, id: &str, threads: usize, stop: &AtomicBool, mut on_progress: impl FnMut(Progress)) -> Result<()> {
    let dir = dir(data_dir, id)?;
    let jar = dir.join("server.jar");
    let java = find_java().context("Decompiling needs Java 21 or newer, on the PATH or in JAVA_HOME")?;
    let vineflower = vineflower(data_dir, stop, &mut on_progress)?;

    // Vineflower logs every top level class twice, once when it reads it and once when it
    // writes it, which is what the progress counts.
    let classes = zip::ZipArchive::new(std::fs::File::open(&jar)?)?.file_names().filter(|name| name.ends_with(".class") && !name.contains('$')).count() as u64;
    let total = classes * 2;
    on_progress(Progress { step: Step::Decompile, done: 0, total });

    let out = dir.join("src.part");
    if out.exists() {
        std::fs::remove_dir_all(&out)?;
    }
    let mut child = Command::new(&java)
        .arg("-jar")
        .arg(&vineflower)
        .args(["--folder", "--log-level=info", &format!("--thread-count={}", threads.max(1))])
        .arg(&jar)
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", java.display()))?;

    // Drained on the side so a full pipe never blocks it, the tail explains a failure.
    let stderr = child.stderr.take().expect("piped above");
    let errors = std::thread::spawn(move || {
        let mut tail = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.starts_with("INFO:") {
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
        }
        tail
    });

    let mut done = 0;
    for line in BufReader::new(child.stdout.take().expect("piped above")).lines().map_while(Result::ok) {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if line.starts_with("INFO:  Preprocessing class") || line.starts_with("INFO:  Decompiling class") {
            done += 1;
            on_progress(Progress { step: Step::Decompile, done: done.min(total), total });
        }
    }
    if stop.load(Ordering::Relaxed) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&out);
        bail!("Stopped");
    }
    let status = child.wait()?;
    let tail = errors.join().unwrap_or_default();
    if !status.success() {
        let _ = std::fs::remove_dir_all(&out);
        bail!("Vineflower failed ({status}){}", tail.iter().map(|l| format!("\n{l}")).collect::<String>());
    }
    let src = dir.join("src");
    if src.exists() {
        std::fs::remove_dir_all(&src)?;
    }
    std::fs::rename(&out, &src)?;
    Ok(())
}
