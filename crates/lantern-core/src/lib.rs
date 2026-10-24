pub mod compare;
pub mod db;
pub mod index;
pub mod query;
pub mod render;
pub mod settings;
pub mod source;
pub mod updates;
pub mod versions;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

use std::path::{Path, PathBuf};

/// The folder of the platform for Lantern's own data (`~/.local/share/lantern` on Linux,
/// `Application Support` on macOS, `%APPDATA%` on Windows): settings, chats, models, versions.
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("lantern")
}

/// The data folder an index belongs to. One built for a version sits in
/// `<data>/versions/<id>/index.db`, any other is taken to have its data next to it.
pub fn data_dir(db_path: &Path) -> PathBuf {
    let parent = db_path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    match parent.parent() {
        Some(versions) if versions.file_name().is_some_and(|name| name == "versions") => {
            versions.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        }
        _ => parent,
    }
}

/// `--db`, then `$LANTERN_DB`, then the index of the version picked in the settings, then
/// `lantern.db` in the data folder, where it was before there were versions.
pub fn db_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit.or_else(|| std::env::var_os("LANTERN_DB").map(PathBuf::from)) {
        return path;
    }
    let data = default_data_dir();
    let version = settings::Settings::load(&data).version;
    Some(versions::index_path(&data, &version)).filter(|p| !version.is_empty() && p.exists()).unwrap_or_else(|| data.join("lantern.db"))
}
