//! The output of the Lookup page: a `Doc` drawn in one text view, every class, member, key,
//! string and file in it a link. Source code gets the Java colours of the source panes.

use std::{cell::RefCell, rc::Rc};

use gtk::prelude::*;
use lantern_core::{
    query::Site,
    render::{Block, Doc, Span},
};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    theme::highlight,
};

/// Where a link leads.
#[derive(Clone, Debug)]
enum Link {
    Symbol(String),
    /// Something a method does. The line in the file is worked out when it is clicked, that
    /// means reading the method's source.
    Site(String, Site),
    Key(String),
    Str(String),
    File {
        path: String,
        class: Option<String>,
        line: Option<i64>,
    },
}

impl Link {
    fn message(&self) -> Msg {
        match self {
            Link::Symbol(target) => Msg::OpenSymbol(target.clone()),
            Link::Site(target, site) => {
                let class = target.rsplit_once('.').map_or(target.as_str(), |(class, _)| class);
                Msg::OpenAt { class: class.to_owned(), method: Some(site.method_id), line: None, site: Some(site.clone()) }
            }
            Link::Key(ident) => Msg::LookupFor(KEY, ident.clone()),
            Link::Str(value) => Msg::LookupFor(STRINGS, value.clone()),
            // Decompiled source opens in Browse, anything else in the default editor.
            Link::File { class: Some(class), line, .. } => Msg::OpenAt { class: class.clone(), method: None, line: *line, site: None },
            Link::File { path, .. } => Msg::OpenPath(path.clone()),
        }
    }
}

/// Positions in `view::LOOKUPS`.
const KEY: usize = 5;
const STRINGS: usize = 6;

/// The links of what is shown, as character ranges of the buffer.
#[derive(Clone, Default)]
pub struct Links(Rc<RefCell<Vec<(i32, i32, Link)>>>);

impl Links {
    fn at(&self, offset: i32) -> Option<(i32, i32, Link)> {
        self.0.borrow().iter().find(|(from, to, _)| (*from..*to).contains(&offset)).cloned()
    }
}

const TAGS: &[(&str, &[(&str, &str)])] = &[
    ("lk-title", &[("family", "IBM Plex Sans"), ("weight", "600"), ("scale", "1.35"), ("foreground", "#f0f6fc"), ("pixels-below-lines", "6")]),
    ("lk-section", &[("family", "IBM Plex Sans"), ("weight", "600"), ("foreground", "#f0f6fc"), ("pixels-below-lines", "4")]),
    ("lk-count", &[("family", "IBM Plex Sans"), ("foreground", "#656c76")]),
    ("lk-muted", &[("foreground", "#9198a1")]),
    ("lk-reads", &[("foreground", "#9198a1")]),
    ("lk-writes", &[("foreground", "#d29922")]),
    ("lk-symbol", &[("foreground", "#79c0ff")]),
    ("lk-at", &[("foreground", "#9198a1")]),
    ("lk-key", &[("foreground", "#3fb950")]),
    ("lk-string", &[("foreground", "#a5d6ff")]),
    ("lk-file", &[("foreground", "#9198a1")]),
    ("lk-mark", &[("foreground", "#ffffff"), ("background", "#5a4414"), ("weight", "600")]),
    ("lk-hover", &[("underline", "single")]),
];

/// The view, highlighting Java when the feature is on.
pub fn view(sender: &ComponentSender<App>, links: &Links) -> gtk::TextView {
    #[cfg(feature = "highlight")]
    let view: gtk::TextView = {
        use sourceview5::prelude::*;
        let buffer = crate::view::code_buffer("java");
        buffer.set_highlight_matching_brackets(false);
        sourceview5::View::with_buffer(&buffer).upcast()
    };
    #[cfg(not(feature = "highlight"))]
    let view = gtk::TextView::new();

    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_left_margin(14);
    view.set_right_margin(14);
    view.set_top_margin(12);
    view.set_bottom_margin(12);
    view.set_pixels_below_lines(2);
    view.add_css_class("lookup-view");

    let buffer = view.buffer();
    for (name, properties) in TAGS {
        let tag = gtk::TextTag::new(Some(name));
        for (key, value) in *properties {
            match *key {
                "weight" | "pixels-below-lines" => tag.set_property(key, value.parse::<i32>().unwrap_or_default()),
                "scale" => tag.set_property(key, value.parse::<f64>().unwrap_or(1.0)),
                "underline" => tag.set_property(key, gtk::pango::Underline::Single),
                _ => tag.set_property(key, *value),
            }
        }
        buffer.tag_table().add(&tag);
    }

    // A click that is not the end of a selection follows the link under it.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    {
        let (links, sender) = (links.clone(), sender.clone());
        click.connect_released(move |gesture, presses, x, y| {
            let Some(view) = gesture.widget().and_downcast::<gtk::TextView>() else { return };
            if presses != 1 || view.buffer().has_selection() {
                return;
            }
            if let Some((_, _, link)) = offset_at(&view, x, y).and_then(|offset| links.at(offset)) {
                sender.input(link.message());
            }
        });
    }
    view.add_controller(click);

    // The link under the pointer is underlined and gets a hand cursor.
    let motion = gtk::EventControllerMotion::new();
    let hovered: Rc<RefCell<Option<(i32, i32)>>> = Rc::default();
    {
        let (links, hovered) = (links.clone(), hovered.clone());
        motion.connect_motion(move |controller, x, y| {
            let Some(view) = controller.widget().and_downcast::<gtk::TextView>() else { return };
            let found = offset_at(&view, x, y).and_then(|offset| links.at(offset)).map(|(from, to, _)| (from, to));
            if *hovered.borrow() != found {
                hover(&view, found);
                *hovered.borrow_mut() = found;
            }
        });
    }
    motion.connect_leave(move |controller| {
        if let Some(view) = controller.widget().and_downcast::<gtk::TextView>() {
            hover(&view, None);
        }
        *hovered.borrow_mut() = None;
    });
    view.add_controller(motion);
    view
}

fn offset_at(view: &gtk::TextView, x: f64, y: f64) -> Option<i32> {
    let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
    let iter = view.iter_at_location(bx, by)?;
    // Past the end of a line the iterator stops at its last character, which is no hit.
    let mut next = iter;
    let inside = next.forward_char() && view.iter_location(&iter).x() + view.iter_location(&iter).width() > bx;
    inside.then_some(iter.offset())
}

fn hover(view: &gtk::TextView, range: Option<(i32, i32)>) {
    let buffer = view.buffer();
    let (start, end) = buffer.bounds();
    buffer.remove_tag_by_name("lk-hover", &start, &end);
    if let Some((from, to)) = range {
        buffer.apply_tag_by_name("lk-hover", &buffer.iter_at_offset(from), &buffer.iter_at_offset(to));
    }
    view.set_cursor_from_name(Some(if range.is_some() { "pointer" } else { "text" }));
}

/// Puts `doc` into the view and remembers its links.
pub fn show(view: &gtk::TextView, links: &Links, doc: &Doc) {
    let mut text = String::new();
    let mut chars = 0i32;
    let mut tags: Vec<(i32, i32, &'static str)> = Vec::new();
    let mut found = Vec::new();
    let mut push = |value: &str, tag: Option<&'static str>, text: &mut String| {
        let from = chars;
        chars += value.chars().count() as i32;
        text.push_str(value);
        if let Some(tag) = tag {
            tags.push((from, chars, tag));
        }
        (from, chars)
    };

    let has_code = doc.blocks.iter().any(|b| matches!(b, Block::Code(_)));
    for block in &doc.blocks {
        match block {
            Block::Title(title) => {
                push(title, Some("lk-title"), &mut text);
                push("\n", None, &mut text);
            }
            Block::Section(title) => {
                // `classes (14)`: the count is quieter than the name.
                let (name, count) = match title.rsplit_once(" (") {
                    Some((name, count)) if count.ends_with(')') => (name, Some(&title[name.len()..])),
                    _ => (title.as_str(), None),
                };
                push(&capitalized(name), Some("lk-section"), &mut text);
                if let Some(count) = count {
                    push(count, Some("lk-count"), &mut text);
                }
                push("\n", None, &mut text);
            }
            Block::Line(spans) => {
                for span in spans {
                    let (tag, link) = match span {
                        Span::Text(_) => (None, None),
                        Span::Muted(_) => (Some("lk-muted"), None),
                        Span::Verb(verb) => (Some(if verb.trim() == "writes" { "lk-writes" } else { "lk-reads" }), None),
                        Span::Mark(_) => (Some("lk-mark"), None),
                        Span::Symbol { target, site: None, .. } => (Some("lk-symbol"), Some(Link::Symbol(target.clone()))),
                        Span::Symbol { target, site: Some(site), .. } => (Some("lk-symbol"), Some(Link::Site(target.clone(), site.clone()))),
                        Span::At { target, site, .. } => (Some("lk-at"), Some(Link::Site(target.clone(), site.clone()))),
                        Span::Key { ident, .. } => (Some("lk-key"), Some(Link::Key(ident.clone()))),
                        Span::Str { value, .. } => (Some("lk-string"), Some(Link::Str(value.clone()))),
                        Span::File { path, class, line, .. } => (Some("lk-file"), Some(Link::File { path: path.clone(), class: class.clone(), line: *line })),
                    };
                    // Source headers are Java comments, the syntax colours paint them already.
                    let tag = if has_code && matches!(span, Span::Muted(_)) { None } else { tag };
                    let (from, to) = push(span.text(), tag, &mut text);
                    if let Some(link) = link {
                        found.push((from, to, link));
                    }
                }
                push("\n", None, &mut text);
            }
            Block::Blank => {
                push("\n", None, &mut text);
            }
            Block::Code(code) => {
                push(code, None, &mut text);
            }
        }
    }

    let buffer = view.buffer();
    #[cfg(feature = "highlight")]
    if let Some(source) = buffer.downcast_ref::<sourceview5::Buffer>() {
        use sourceview5::prelude::*;
        source.set_highlight_syntax(has_code);
    }
    buffer.set_text(text.trim_end());
    if has_code {
        highlight::apply(&buffer);
    }
    // Above the syntax colours, which GtkSourceView adds to the table later.
    let table = buffer.tag_table();
    for (name, _) in TAGS {
        if let Some(tag) = table.lookup(name) {
            tag.set_priority(table.size() - 1);
        }
    }
    for (from, to, tag) in tags {
        buffer.apply_tag_by_name(tag, &buffer.iter_at_offset(from), &buffer.iter_at_offset(to));
    }
    *links.0.borrow_mut() = found;
    buffer.place_cursor(&buffer.start_iter());
    if let Some(adjustment) = view.vadjustment() {
        adjustment.set_value(0.0);
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
