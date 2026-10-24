//! IBM Plex Sans and Lilex, the UI and code fonts of Zed, compiled in so the app looks the same
//! whether or not they are installed. Both are under the OFL, see fonts/OFL-*.txt.

use gtk::{glib, prelude::*};

macro_rules! font {
    ($name:literal) => {
        ($name, include_bytes!(concat!("fonts/", $name)).as_slice())
    };
}

const FONTS: &[(&str, &[u8])] = &[
    font!("IBMPlexSans-Regular.ttf"),
    font!("IBMPlexSans-Medium.ttf"),
    font!("IBMPlexSans-SemiBold.ttf"),
    font!("Lilex-Regular.ttf"),
    font!("Lilex-Medium.ttf"),
];

/// Pango only loads fonts from files, the embedded copies are written to the cache folder once
/// and registered from there. A font that fails falls back to the system one, nothing breaks.
pub fn register() {
    let Some(font_map) = gtk::Label::new(None).pango_context().font_map() else { return };
    let dir = glib::user_cache_dir().join("lantern").join("fonts");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {err}", dir.display());
        return;
    }
    for (name, bytes) in FONTS {
        let path = dir.join(name);
        let current = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
        if !current && let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("could not write {}: {err}", path.display());
            continue;
        }
        if let Err(err) = font_map.add_font_file(&path) {
            eprintln!("could not load {}: {err}", path.display());
        }
    }
}
