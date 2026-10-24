//! Read only access to the decompiled tree, for a model that wants to look around by itself.
//! Everything is relative to the sources the index was built from, with `data/` standing for
//! the data folder. Nothing outside those two folders can be reached and nothing is written.

use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use regex::RegexBuilder;

const MAX_ENTRIES: usize = 400;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub struct Roots {
    src: Option<PathBuf>,
    data: Option<PathBuf>,
}

impl Roots {
    pub fn new(src: Option<String>, data: Option<String>) -> Self {
        // Canonical once, so that the containment check below compares like with like.
        let open = |path: Option<String>| path.filter(|p| !p.is_empty()).and_then(|p| std::fs::canonicalize(p).ok());
        Self { src: open(src), data: open(data) }
    }

    /// The folder a path lives under and what is left of the path.
    fn root_for<'a>(&self, path: &'a str) -> Result<(&Path, &'a str)> {
        let path = path.trim().trim_start_matches("./").trim_matches('/');
        let in_data = path == "data" || path.starts_with("data/");
        match (in_data, &self.data, &self.src) {
            (true, Some(data), _) => Ok((data, path.strip_prefix("data").unwrap_or(path).trim_start_matches('/'))),
            (_, _, Some(src)) => Ok((src, path)),
            (true, None, None) | (false, _, None) => bail!("the index was built without decompiled sources, there are no files to look at"),
        }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let (root, rest) = self.root_for(path)?;
        // Refused before the disk is touched, the answer must not depend on what exists outside.
        ensure!(!rest.split(['/', '\\']).any(|part| part == ".."), "`{path}` is outside the decompiled tree");
        let full = std::fs::canonicalize(root.join(rest)).with_context(|| format!("`{path}` does not exist"))?;
        // `..` and links are resolved by now, whatever still points outside is refused.
        ensure!(full.starts_with(root), "`{path}` is outside the decompiled tree");
        Ok(full)
    }

    /// How a file is called from the outside: relative, with forward slashes.
    fn display(&self, full: &Path) -> String {
        let relative = match (&self.data, &self.src) {
            (Some(data), _) if full.starts_with(data) && !self.src.as_ref().is_some_and(|src| data.starts_with(src)) => {
                Path::new("data").join(full.strip_prefix(data).unwrap_or(full))
            }
            (_, Some(src)) if full.starts_with(src) => full.strip_prefix(src).unwrap_or(full).to_path_buf(),
            (Some(data), _) => Path::new("data").join(full.strip_prefix(data).unwrap_or(full)),
            _ => full.to_path_buf(),
        };
        relative.to_string_lossy().replace('\\', "/")
    }

    pub fn list(&self, path: &str) -> Result<String> {
        let dir = self.resolve(path)?;
        ensure!(dir.is_dir(), "`{path}` is a file, use read_file");
        let mut entries: Vec<(bool, String, u64)> = std::fs::read_dir(&dir)?
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                Some((meta.is_dir(), entry.file_name().to_string_lossy().into_owned(), meta.len()))
            })
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let mut out = format!("{}/  ({} entries)\n", self.display(&dir), entries.len());
        for (is_dir, name, size) in entries.iter().take(MAX_ENTRIES) {
            if *is_dir {
                let _ = writeln!(out, "  {name}/");
            } else {
                let _ = writeln!(out, "  {name}  {size} bytes");
            }
        }
        if entries.len() > MAX_ENTRIES {
            let _ = writeln!(out, "  ... {} more, list a subfolder or use grep", entries.len() - MAX_ENTRIES);
        }
        Ok(out)
    }

    pub fn read(&self, path: &str, from: usize, lines: usize) -> Result<String> {
        let file = self.resolve(path)?;
        ensure!(file.is_file(), "`{path}` is a folder, use list_files");
        ensure!(file.metadata()?.len() <= MAX_FILE_BYTES, "`{path}` is too large to read, use grep to find the part you need");
        let text = std::fs::read_to_string(&file).with_context(|| format!("`{path}` is not a text file"))?;
        let total = text.lines().count();
        let from = from.max(1);
        let mut out = format!("{}  (lines {from}-{} of {total})\n", self.display(&file), (from + lines - 1).min(total));
        for (number, line) in text.lines().enumerate().skip(from - 1).take(lines) {
            let _ = writeln!(out, "{:>5}  {line}", number + 1);
        }
        if from + lines <= total {
            let _ = writeln!(out, "... continues, read from line {}", from + lines);
        }
        Ok(out)
    }

    pub fn grep(&self, pattern: &str, path: &str, extension: Option<&str>, limit: usize) -> Result<String> {
        // A pattern that is not a valid expression is most likely meant as plain text.
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(!pattern.chars().any(char::is_uppercase))
            .size_limit(1 << 20)
            .build()
            .or_else(|_| RegexBuilder::new(&regex::escape(pattern)).case_insensitive(true).build())?;
        let start = self.resolve(path)?;
        let extension = extension.map(|e| e.trim_start_matches('.').to_ascii_lowercase());

        let mut files = Vec::new();
        collect(&start, &mut files);
        files.sort();

        let (mut out, mut shown, mut total, mut matched_files) = (String::new(), 0usize, 0usize, 0usize);
        for file in &files {
            if let Some(wanted) = &extension {
                if file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() != Some(wanted.as_str()) {
                    continue;
                }
            }
            let Ok(text) = std::fs::read_to_string(file) else { continue };
            let mut any = false;
            for (number, line) in text.lines().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                any = true;
                total += 1;
                if shown < limit {
                    shown += 1;
                    let line = line.trim();
                    let cut: String = line.chars().take(220).collect();
                    let _ = writeln!(out, "{}:{}: {cut}", self.display(file), number + 1);
                }
            }
            matched_files += usize::from(any);
        }
        if total == 0 {
            return Ok(format!("no match for `{pattern}` in {} files under {}", files.len(), self.display(&start)));
        }
        let _ = writeln!(
            out,
            "{total} matches in {matched_files} files{}",
            if total > shown { format!(", {shown} shown. Narrow the pattern or the path") } else { String::new() }
        );
        Ok(out)
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        // Links are not followed, that is the other way out of the tree.
        if kind.is_dir() {
            collect(&entry.path(), out);
        } else if kind.is_file() && entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES) {
            out.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Roots;

    #[test]
    fn stays_inside_the_tree() {
        let dir = std::env::temp_dir().join(format!("lantern-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/org/example")).unwrap();
        std::fs::create_dir_all(dir.join("src/data/example/tags")).unwrap();
        std::fs::write(dir.join("src/org/example/Util.java"), "class Util {\n  int MAX = 3;\n}\n").unwrap();
        std::fs::write(dir.join("src/data/example/tags/lamps.json"), "{\"values\": [\"example:red_lamp\"]}\n").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();

        let roots = Roots::new(Some(dir.join("src").display().to_string()), Some(dir.join("src/data").display().to_string()));
        assert!(roots.list("").unwrap().contains("org/"));
        assert!(roots.read("org/example/Util.java", 2, 1).unwrap().contains("    2    int MAX = 3;"));
        assert!(roots.grep("red_lamp", "data", Some("json"), 10).unwrap().contains("data/example/tags/lamps.json:1:"));
        assert!(roots.grep("max", "", None, 10).unwrap().contains("Util.java:2:"));
        assert!(roots.read("../secret.txt", 1, 10).is_err());
        assert!(roots.read("org/../../secret.txt", 1, 10).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
