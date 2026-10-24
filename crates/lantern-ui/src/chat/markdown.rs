//! Chat answers as widgets: paragraphs, headings, lists, quotes, tables and highlighted code.
//! Inline code that looks like a class or a member becomes a link that opens it in Browse.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use gtk::{glib, prelude::*};
use lantern_core::{query, render::short};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    view,
};

thread_local! {
    /// Where the index is, a connection to it and what symbols turned out to be. Looked up
    /// once per name, an answer is drawn many times while it streams in.
    static INDEX: RefCell<(Option<PathBuf>, Option<lantern_core::db::Connection>, HashMap<String, Option<String>>)> =
        RefCell::new((None, None, HashMap::new()));
}

/// Points symbol lookups at an index. Also drops the open connection, which has to happen
/// before the index file is replaced.
pub fn set_index(path: Option<&Path>) {
    INDEX.with_borrow_mut(|index| *index = (path.map(Path::to_path_buf), None, HashMap::new()));
}

/// What a name in an answer refers to, as a line for a tooltip. None when the index has no
/// such class or member, the name then stays plain text instead of a link that goes nowhere.
fn describe(symbol: &str) -> Option<String> {
    INDEX.with_borrow_mut(|(path, conn, known)| {
        if let Some(found) = known.get(symbol) {
            return found.clone();
        }
        if conn.is_none() {
            *conn = lantern_core::db::open(path.as_deref()?).ok();
        }
        let conn = conn.as_ref()?;
        let name = symbol.trim_end_matches("()");
        let method = name.rsplit_once('.').and_then(|(class, member)| {
            let found = query::find_methods(conn, class, member).ok()?;
            found.iter().find(|m| query::method_location(conn, m.id).is_some_and(|l| l.line.is_some())).or(found.first()).cloned()
        });
        let described = match method {
            Some(method) => {
                let declaration = query::method_source(conn, method.id).ok().flatten().and_then(|text| {
                    text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('@')).map(|l| l.trim_end_matches('{').trim().to_owned())
                });
                let line = query::method_location(conn, method.id).and_then(|l| l.line).map_or(String::new(), |l| format!(", line {l}"));
                Some(format!("{}\n{}{line}", declaration.unwrap_or_else(|| format!("{}.{}", short(&method.class), method.name)), short(&method.class)))
            }
            None => {
                let class = name.rsplit_once('.').map_or(name, |(class, _)| class);
                query::class_info(conn, class).ok().map(|info| {
                    let parent = info.super_class.as_deref().map_or(String::new(), |p| format!(" extends {}", p.rsplit('/').next().unwrap_or(p)));
                    // A field or constant of a class that exists: the link opens the class.
                    let member = name.rsplit_once('.').map_or(String::new(), |(_, member)| format!("{member} in "));
                    format!("{member}{}{parent}\n{} methods, {} fields", short(&info.name), info.methods.len(), info.fields.len())
                })
            }
        };
        known.insert(symbol.to_owned(), described.clone());
        described
    })
}

/// The scheme of links that point into the index instead of the web.
const REF: &str = "lantern:";

/// `Warden`, `Warden.doPush`, `VibrationSystem$User`, `world/entity/Mob.tick`.
pub fn looks_like_symbol(text: &str) -> bool {
    let name = text.trim_end_matches("()");
    let class = name.rsplit('/').next().unwrap_or(name);
    class.starts_with(|c: char| c.is_ascii_uppercase())
        && class.len() > 2
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '/' | '<' | '>'))
        && class.chars().any(|c| c.is_ascii_lowercase())
}

struct Builder<'a> {
    sender: &'a ComponentSender<App>,
    /// Where blocks go: the answer itself, then list items and quotes inside it.
    containers: Vec<gtk::Box>,
    inline: String,
    /// Open lists with the next number of an ordered one.
    lists: Vec<Option<u64>>,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
    table: Option<(gtk::Grid, i32, i32, bool)>,
}

impl Builder<'_> {
    fn top(&self) -> &gtk::Box {
        self.containers.last().expect("the root is never popped")
    }

    fn text_label(&self, markup: &str, class: &str) -> gtk::Label {
        let label = view::wrapping("", class);
        label.set_markup(markup);
        label.set_selectable(true);
        label.set_hexpand(true);
        let sender = self.sender.clone();
        label.connect_activate_link(move |_, uri| match uri.strip_prefix(REF) {
            Some(symbol) => {
                sender.input(Msg::OpenSymbol(symbol.to_owned()));
                glib::Propagation::Stop
            }
            // A web link, GTK hands it to the browser.
            None => glib::Propagation::Proceed,
        });
        label
    }

    /// Ends the running text and puts it where it belongs.
    fn flush(&mut self) {
        let markup = std::mem::take(&mut self.inline);
        if markup.trim().is_empty() {
            return;
        }
        if let Some((grid, row, column, head)) = &self.table {
            let cell = self.text_label(&if *head { format!("<b>{markup}</b>") } else { markup }, "md-cell");
            grid.attach(&cell, *column, *row, 1, 1);
            return;
        }
        let class = match self.heading.take() {
            Some(HeadingLevel::H1) => "md-h1",
            Some(HeadingLevel::H2) => "md-h2",
            Some(_) => "md-h3",
            None => "md-text",
        };
        let label = self.text_label(&markup, class);
        self.top().append(&label);
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => self.heading = Some(level),
            Tag::CodeBlock(kind) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or_default().to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::List(first) => {
                // A list inside an item starts after the text of that item.
                self.flush();
                self.lists.push(first);
            }
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}.", *number - 1)
                    }
                    _ => "•".to_owned(),
                };
                let marker = view::label(&marker, "md-marker");
                marker.set_valign(gtk::Align::Start);
                marker.set_xalign(1.0);
                let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
                content.set_hexpand(true);
                let item = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                item.append(&marker);
                item.append(&content);
                self.top().append(&item);
                self.containers.push(content);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                let quote = gtk::Box::new(gtk::Orientation::Vertical, 6);
                quote.add_css_class("md-quote");
                self.top().append(&quote);
                self.containers.push(quote);
            }
            Tag::Table(_) => {
                self.flush();
                let grid = gtk::Grid::new();
                grid.add_css_class("md-table");
                grid.set_column_spacing(18);
                grid.set_row_spacing(6);
                self.top().append(&grid);
                self.table = Some((grid, -1, 0, false));
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some((_, row, column, head)) = &mut self.table {
                    *row += 1;
                    *column = 0;
                    *head = matches!(tag, Tag::TableHead);
                }
            }
            Tag::Emphasis => self.inline.push_str("<i>"),
            Tag::Strong => self.inline.push_str("<b>"),
            Tag::Strikethrough => self.inline.push_str("<s>"),
            Tag::Link { dest_url, .. } => self.inline.push_str(&format!("<a href=\"{}\">", glib::markup_escape_text(&dest_url))),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush(),
            TagEnd::CodeBlock => {
                if let Some((language, code)) = self.code.take() {
                    self.top().append(&view::code_block(&language, code.trim_end()));
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item | TagEnd::BlockQuote(_) => {
                self.flush();
                if self.containers.len() > 1 {
                    self.containers.pop();
                }
            }
            TagEnd::TableCell => {
                self.flush();
                if let Some((_, _, column, _)) = &mut self.table {
                    *column += 1;
                }
            }
            TagEnd::Table => self.table = None,
            TagEnd::Emphasis => self.inline.push_str("</i>"),
            TagEnd::Strong => self.inline.push_str("</b>"),
            TagEnd::Strikethrough => self.inline.push_str("</s>"),
            TagEnd::Link => self.inline.push_str("</a>"),
            _ => {}
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some((_, code)) => code.push_str(&text),
                None => self.inline.push_str(&glib::markup_escape_text(&text)),
            },
            Event::Code(code) => {
                let escaped = glib::markup_escape_text(&code);
                let span = format!("<span font_family=\"Lilex\" background=\"#1d2430\" foreground=\"#f0f6fc\"> {escaped} </span>");
                match looks_like_symbol(&code).then(|| describe(&code)).flatten() {
                    Some(about) => {
                        let title = glib::markup_escape_text(&format!("{about}\nClick to open in Browse"));
                        self.inline.push_str(&format!("<a href=\"{REF}{escaped}\" title=\"{title}\">{span}</a>"));
                    }
                    None => self.inline.push_str(&span),
                }
            }
            Event::SoftBreak => self.inline.push(' '),
            Event::HardBreak => self.inline.push('\n'),
            Event::Rule => {
                self.flush();
                self.top().append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            Event::Html(html) | Event::InlineHtml(html) => self.inline.push_str(&glib::markup_escape_text(&html)),
            _ => {}
        }
    }
}

fn render_block(markdown: &str, sender: &ComponentSender<App>) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let mut builder = Builder { sender, containers: vec![block.clone()], inline: String::new(), lists: Vec::new(), heading: None, code: None, table: None };
    for event in Parser::new_ext(markdown, OPTIONS) {
        builder.event(event);
    }
    builder.flush();
    // An unfinished code fence simply runs to the end of what is there.
    if let Some((language, code)) = builder.code.take() {
        builder.top().append(&view::code_block(&language, code.trim_end()));
    }
    block
}

const OPTIONS: Options = Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH);

/// Markdown that grows. While an answer streams in it is drawn several times a second, and
/// only the part that changed is drawn again: the text is cut into its top level blocks, a
/// block whose source is the same as last time keeps its widgets. Everything above the
/// paragraph being written stays untouched, which is what keeps scrolling and selecting calm.
pub struct Document {
    container: gtk::Box,
    blocks: Vec<(String, gtk::Box)>,
}

impl Document {
    pub fn new(container: &gtk::Box) -> Self {
        Self { container: container.clone(), blocks: Vec::new() }
    }

    pub fn container(&self) -> &gtk::Box {
        &self.container
    }

    pub fn update(&mut self, markdown: &str, sender: &ComponentSender<App>) {
        // The source of every top level block.
        let mut sources: Vec<&str> = Vec::new();
        let (mut depth, mut start) = (0usize, 0usize);
        for (event, range) in Parser::new_ext(markdown, OPTIONS).into_offset_iter() {
            match event {
                Event::Start(_) => {
                    if depth == 0 {
                        start = range.start;
                    }
                    depth += 1;
                }
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        sources.push(&markdown[start..range.end]);
                    }
                }
                Event::Rule if depth == 0 => sources.push(&markdown[range]),
                _ => {}
            }
        }

        let same = self.blocks.iter().zip(&sources).take_while(|((old, _), new)| old == *new).count();
        let stale: Vec<(String, gtk::Box)> = self.blocks.drain(same..).collect();
        for source in &sources[same..] {
            let block = render_block(source, sender);
            self.container.append(&block);
            self.blocks.push(((*source).to_owned(), block));
        }
        // New first, old after: the container is never shorter than it was, the page does not jump.
        for (_, block) in stale {
            self.container.remove(&block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_symbol;

    #[test]
    fn tells_symbols_from_other_code() {
        for yes in ["Lamp", "Lamp.light", "Lamp.light()", "Shade$Panel", "house/room/Lamp.tick", "LampTags.WARM_LIGHT"] {
            assert!(looks_like_symbol(yes), "{yes}");
        }
        for no in ["example:lamps", "20", "MAX_GLOW", "timestamp % 100 == 0", "getWick", "UP"] {
            assert!(!looks_like_symbol(no), "{no}");
        }
    }
}
