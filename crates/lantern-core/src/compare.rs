//! What changed between two versions: which files of their decompiled sources and data folders
//! differ, by how many lines, and the lines of one file side by side with the words that changed
//! in them. Works on the `src/` folders Vineflower wrote, so both have to be decompiled.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result, bail};
use similar::{ChangeTag, TextDiff};

use crate::versions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Added,
    Removed,
    Modified,
}

impl Status {
    /// The letter git uses.
    pub fn letter(self) -> char {
        match self {
            Status::Added => 'A',
            Status::Removed => 'D',
            Status::Modified => 'M',
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileChange {
    /// Relative to the source root, with `/`.
    pub path: String,
    pub status: Status,
    pub added: usize,
    pub removed: usize,
    /// Not text: no line counts and no diff.
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub kind: LineKind,
    /// 1 based, None on the side the line is not on.
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
    /// Byte ranges of `text` that changed, for a line that has a counterpart on the other side.
    pub emphasis: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<Line>,
}

/// The decompiled sources of a downloaded version.
pub fn source_root(data_dir: &Path, version: &str) -> Result<PathBuf> {
    let src = versions::root(data_dir).join(versions::folder(version)).join("src");
    if !src.is_dir() {
        bail!("{version} has no decompiled sources, download and decompile it first");
    }
    Ok(src)
}

/// Every file whose content differs between the two roots, sorted by path. `on_progress` gets
/// files looked at and the total.
pub fn changes(old: &Path, new: &Path, mut on_progress: impl FnMut(usize, usize)) -> Result<Vec<FileChange>> {
    let old_files = list(old)?;
    let new_files = list(new)?;
    let mut paths: BTreeMap<&str, (Option<u64>, Option<u64>)> = BTreeMap::new();
    for (path, size) in &old_files {
        paths.entry(path).or_default().0 = Some(*size);
    }
    for (path, size) in &new_files {
        paths.entry(path).or_default().1 = Some(*size);
    }

    let work: Vec<(&str, (Option<u64>, Option<u64>))> = paths.into_iter().collect();
    let total = work.len();
    // Spread over the cores, each takes the next file until none are left. The counter is what
    // the progress reads.
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut out: Vec<FileChange> = std::thread::scope(|scope| -> Result<Vec<FileChange>> {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<FileChange>> {
                    let mut found = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some((path, sizes)) = work.get(index) else { break };
                        if let Some(change) = compare_one(old, new, path, *sizes)? {
                            found.push(change);
                        }
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(found)
                })
            })
            .collect();
        while workers.iter().any(|w| !w.is_finished()) {
            on_progress(done.load(Ordering::Relaxed), total);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut all = Vec::new();
        for worker in workers {
            all.extend(worker.join().map_err(|_| anyhow::anyhow!("a comparison thread failed"))??);
        }
        Ok(all)
    })?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    on_progress(total, total);
    Ok(out)
}

fn compare_one(old: &Path, new: &Path, path: &str, sizes: (Option<u64>, Option<u64>)) -> Result<Option<FileChange>> {
    let read = |root: &Path| std::fs::read(root.join(path)).with_context(|| format!("reading {path}"));
    let change = match sizes {
        (Some(_), None) => counted(path, Status::Removed, &read(old)?, &[]),
        (None, Some(_)) => counted(path, Status::Added, &[], &read(new)?),
        (Some(a), Some(b)) => {
            let (before, after) = (read(old)?, read(new)?);
            if a == b && before == after {
                return Ok(None);
            }
            counted(path, Status::Modified, &before, &after)
        }
        (None, None) => return Ok(None),
    };
    // A file whose bytes differ only in line endings has no changed lines, it is no change.
    if change.status == Status::Modified && !change.binary && change.added == 0 && change.removed == 0 {
        return Ok(None);
    }
    Ok(Some(change))
}

fn counted(path: &str, status: Status, before: &[u8], after: &[u8]) -> FileChange {
    let (Ok(before), Ok(after)) = (std::str::from_utf8(before), std::str::from_utf8(after)) else {
        return FileChange { path: path.to_owned(), status, added: 0, removed: 0, binary: true };
    };
    let diff = TextDiff::configure().algorithm(similar::Algorithm::Patience).diff_lines(before, after);
    let (mut added, mut removed) = (0, 0);
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            match change.tag() {
                ChangeTag::Insert => added += 1,
                ChangeTag::Delete => removed += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    FileChange { path: path.to_owned(), status, added, removed, binary: false }
}

/// Relative path and size of every file under `root`.
fn list(root: &Path) -> Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).into_iter() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        // The jar signature lists a hash of every file, it changes all over in every version
        // and says nothing.
        if relative.starts_with("META-INF/") {
            continue;
        }
        out.push((relative, entry.metadata()?.len()));
    }
    Ok(out)
}

/// The changed lines of one file with `context` unchanged lines around them. A file missing
/// on one side counts as empty there.
pub fn diff_file(old: &Path, new: &Path, path: &str, context: usize) -> Result<Vec<Hunk>> {
    let read = |root: &Path| match std::fs::read_to_string(root.join(path)) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).with_context(|| format!("reading {path}")),
    };
    Ok(diff_text(&read(old)?, &read(new)?, context))
}

pub fn diff_text(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = TextDiff::configure().algorithm(similar::Algorithm::Patience).diff_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let (old_range, new_range) = (first.old_range().start..last.old_range().end, first.new_range().start..last.new_range().end);
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_inline_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => LineKind::Added,
                    ChangeTag::Delete => LineKind::Removed,
                };
                let mut text = String::new();
                let mut emphasis = Vec::new();
                for (emphasized, part) in change.iter_strings_lossy() {
                    if emphasized {
                        emphasis.push((text.len(), text.len() + part.len()));
                    }
                    text.push_str(&part);
                }
                // The newline is not part of the line, nor of what changed in it.
                let trimmed = text.trim_end_matches(['\n', '\r']).len();
                text.truncate(trimmed);
                let emphasis = emphasis.into_iter().filter_map(|(a, b)| (a < trimmed).then_some((a, b.min(trimmed)))).filter(|(a, b)| a < b).collect();
                lines.push(Line {
                    kind,
                    old: change.old_index().map(|i| i + 1),
                    new: change.new_index().map(|i| i + 1),
                    text,
                    // Only a changed line has changed words.
                    emphasis: if kind == LineKind::Context { Vec::new() } else { emphasis },
                });
            }
        }
        hunks.push(Hunk { old_start: old_range.start + 1, old_len: old_range.len(), new_start: new_range.start + 1, new_len: new_range.len(), lines });
    }
    hunks
}

/// `git diff` style text, for the CLI.
pub fn unified(path: &str, hunks: &[Hunk]) -> String {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    for hunk in hunks {
        out += &format!("@@ -{},{} +{},{} @@\n", hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len);
        for line in &hunk.lines {
            let sign = match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            };
            out += &format!("{sign}{}\n", line.text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{LineKind, diff_text};

    #[test]
    fn marks_the_words_that_changed() {
        let old = "a\nint radius = 2;\nb\n";
        let new = "a\nint radius = 3;\nb\nc\n";
        let hunks = diff_text(old, new, 1);
        assert_eq!(hunks.len(), 1);
        let kinds: Vec<_> = hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Context, LineKind::Added]);
        let added = &hunks[0].lines[2];
        assert_eq!(added.new, Some(2));
        let marked: Vec<&str> = added.emphasis.iter().map(|(a, b)| &added.text[*a..*b]).collect();
        assert_eq!(marked, ["3;"]);
    }
}
