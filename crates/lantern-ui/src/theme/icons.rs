//! Icons from Lucide (ISC, see icons/LICENSE), compiled into the binary and drawn by GTK's
//! own SVG renderer. No icon theme and no librsvg needed, which is what makes them look the
//! same on every system.

use gtk::{glib, prelude::*};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        fn source(name: &str) -> Option<&'static str> {
            match name {
                $($name => Some(include_str!(concat!("icons/", $name, ".svg"))),)*
                _ => None,
            }
        }
    };
}

icons!(
    "folder-tree",
    "search",
    "message-square",
    "terminal",
    "settings",
    "chevron-right",
    "chevron-down",
    "folder",
    "braces",
    "copy",
    "refresh-cw",
    "arrow-up",
    "square",
    "plus",
    "external-link",
    "brain",
    "wrench",
    "check",
    "x",
    "box",
    "square-function",
    "file-code",
    "download",
    "play",
    "arrow-left",
    "arrow-right",
    "link",
    "panel-left",
    "chevron-up",
    "minus",
    "win-minimize",
    "win-maximize",
    "win-restore",
    "win-close",
    "tag",
    "quote",
    "git-compare",
    "arrow-left-right",
);

/// The colours icons come in. GTK's SVG renderer does not resolve `currentColor`, so the
/// colour is written into the source. The UI is dark only, a few fixed ones from the palette
/// in style.css are enough.
pub const MUTED: &str = "#9198a1";
pub const FAINT: &str = "#656c76";
pub const BRIGHT: &str = "#ffffff";
pub const DARK: &str = "#0d1117";
pub const PURPLE: &str = "#be8fff";
pub const BLUE: &str = "#58a6ff";

pub fn icon(name: &str) -> gtk::Image {
    colored(name, MUTED)
}

pub fn colored(name: &str, color: &str) -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(16);
    if let Some(svg) = source(name) {
        let svg = svg.replace("currentColor", color);
        image.set_paintable(Some(&gtk::Svg::from_bytes(&glib::Bytes::from_owned(svg.into_bytes()))));
    }
    image
}

/// An icon in front of a text, the usual content of a button or a row.
pub fn with_icon(name: &str, text: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.append(&icon(name));
    line.append(text);
    line
}
