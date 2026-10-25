use std::{cell::Cell, rc::Rc};

use gtk::prelude::*;
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    pages::setup::Setup,
    theme::icons::{self, colored, with_icon},
};

/// One kind of lookup: its name in the sidebar, what it answers, what to type and an example
/// that finds something in a recent index.
pub struct Lookup {
    pub name: &'static str,
    pub icon: &'static str,
    pub about: &'static str,
    pub hint: &'static str,
    pub example: &'static str,
}

pub const LOOKUPS: &[Lookup] = &[
    Lookup {
        name: "Topic",
        icon: "brain",
        about: "Everything around one subject: the classes that name it, their neighbours in the call graph and the data they touch.",
        hint: "A word or two",
        example: "sniffer",
    },
    Lookup {
        name: "Class",
        icon: "box",
        about: "Super class, interfaces, fields with their constant values and every method.",
        hint: "A class name, short or with its package",
        example: "Sniffer",
    },
    Lookup {
        name: "Callers",
        icon: "arrow-left",
        about: "Every place that calls a method, calls through overrides included.",
        hint: "Class.method",
        example: "LivingEntity.hurtServer",
    },
    Lookup {
        name: "Callees",
        icon: "arrow-right",
        about: "What a method calls, line by line, with the class each call lands on.",
        hint: "Class.method",
        example: "Creeper.explodeCreeper",
    },
    Lookup { name: "Field refs", icon: "link", about: "Where a field is read and where it is written.", hint: "Class.field", example: "Creeper.swell" },
    Lookup {
        name: "Key or tag",
        icon: "tag",
        about: "A registry key or tag: its members, the Java field that holds it and the code that uses that field.",
        hint: "namespace:path",
        example: "minecraft:sniffer_diggable_block",
    },
    Lookup {
        name: "Strings",
        icon: "quote",
        about: "Methods that load a string constant containing the text, good for ids, sound events and translation keys.",
        hint: "Part of a string",
        example: "entity.sniffer",
    },
    Lookup {
        name: "Text search",
        icon: "search",
        about: "Full text search over the decompiled sources, the matching words marked.",
        hint: "Words from the code",
        example: "explosion radius",
    },
    Lookup {
        name: "Source",
        icon: "file-code",
        about: "The decompiled source of a class, or of one method in it.",
        hint: "Class or Class.method",
        example: "Creeper.explodeCreeper",
    },
];

/// Page name in the stack and what the sidebar calls it.
pub const PAGES: &[(&str, &str)] = &[("browse", "Browse"), ("chat", "Chat"), ("lookup", "Lookup"), ("compare", "Compare"), ("setup", "Setup")];

fn page_icon(page: &str) -> &'static str {
    match page {
        "browse" => "folder-tree",
        "chat" => "message-square",
        "lookup" => "terminal",
        "compare" => "git-compare",
        _ => "settings",
    }
}

pub struct Widgets {
    /// "main", or "onboarding" on the first run.
    pub top: gtk::Stack,
    pub onboarding: crate::pages::onboarding::Widgets,
    pub compare: crate::pages::compare::Widgets,
    pub stack: gtk::Stack,
    pub nav: Vec<(&'static str, gtk::ToggleButton)>,
    /// Set while the app moves the sidebar selection itself. Without it the toggle would
    /// report a page change, which selects the item, which reports a page change.
    pub nav_syncing: Rc<Cell<bool>>,
    pub version_badge: gtk::Label,
    pub crumb_path: gtk::Label,
    pub crumb_title: gtk::Label,

    /// The panel of the sidebar, one per page.
    pub side_stack: gtk::Stack,
    pub class_list: gtk::ListBox,
    pub class_scroll: gtk::ScrolledWindow,
    pub member_list: gtk::ListBox,
    pub callers_list: gtk::ListBox,
    pub callers_title: gtk::Label,
    pub callees_list: gtk::ListBox,
    pub panes: crate::browse::panes::Panes,
    pub find_status: gtk::Label,
    pub location: gtk::Label,
    pub open_file: gtk::Button,
    pub back: gtk::Button,
    pub forward: gtk::Button,

    /// The right side of Browse: the members of the class, or the search results over them.
    pub inspector_stack: gtk::Stack,
    pub ask_entry: gtk::SearchEntry,
    pub ask_spinner: gtk::Spinner,
    pub ask_status: gtk::Label,
    pub ask_empty: gtk::Box,
    pub ask_empty_title: gtk::Label,
    pub ask_empty_help: gtk::Label,
    pub ask_list: gtk::ListBox,

    pub chat_scroll: gtk::ScrolledWindow,
    pub chat_empty: gtk::Box,
    /// Whether the chat sticks to its end. Off while the reader has scrolled up.
    pub chat_follow: Rc<Cell<bool>>,
    pub chat_box: gtk::Box,
    pub chat_entry: gtk::Entry,
    pub chat_send: gtk::Button,
    pub chat_stop: gtk::Button,
    pub chat_usage: gtk::Label,
    pub chat_new: gtk::Button,
    /// The answer that is being streamed into, if any.
    pub chat_current: Option<crate::chat::markdown::Document>,
    /// The turn that is running.
    pub chat_live: Option<crate::chat::turn::Turn>,
    /// The question bubble of the last turn, Regenerate removes what is below it.
    pub chat_turn: Option<gtk::Widget>,
    /// Regenerate and Copy, only under the last turn.
    pub chat_actions: Option<gtk::Box>,

    /// The saved conversations, newest first.
    pub chat_list: gtk::ListBox,
    /// Which of `LOOKUPS` is picked. The sidebar sets it, no message needed.
    pub lookup_kind: Rc<Cell<usize>>,
    pub lookup_entry: gtk::Entry,
    pub lookup_out: gtk::TextView,
    pub lookup_links: crate::pages::lookup::Links,
    /// The kinds in the sidebar, for a link that runs another kind of lookup.
    pub lookup_kinds: gtk::ListBox,
    /// "empty", "out" or "error".
    pub lookup_stack: gtk::Stack,
    pub lookup_error: gtk::Label,

    pub setup: Setup,
}

/// Replaces the rows of a list. Every row is a title with an optional dim second line.
pub fn fill(list: &gtk::ListBox, rows: impl IntoIterator<Item = (String, String)>) {
    list.remove_all();
    for (title, detail) in rows {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 1);
        cell.set_margin_top(5);
        cell.set_margin_bottom(5);
        cell.set_margin_start(8);
        cell.set_margin_end(8);
        cell.append(&label(&title, "row-title"));
        if !detail.is_empty() {
            cell.append(&label(&detail, "row-detail"));
        }
        list.append(&cell);
    }
}

/// Rows of callers and calls. `openable` is false for a method of the JDK or a library: it
/// has no source in the index, so the row is not a link to anywhere.
pub fn fill_calls(list: &gtk::ListBox, rows: impl IntoIterator<Item = (String, String, bool)>) {
    list.remove_all();
    for (title, detail, openable) in rows {
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 1);
        cell.set_margin_top(5);
        cell.set_margin_bottom(5);
        cell.set_margin_start(8);
        cell.set_margin_end(8);
        if !openable {
            cell.add_css_class("external");
        }
        cell.append(&label(&title, "row-title"));
        if !detail.is_empty() {
            cell.append(&label(&detail, "row-detail"));
        }
        list.append(&cell);
    }
}

/// Rows with an icon in front: (icon, colour, title, detail).
pub fn fill_icons(list: &gtk::ListBox, rows: impl IntoIterator<Item = (&'static str, &'static str, String, String)>) {
    list.remove_all();
    for (name, color, title, detail) in rows {
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.append(&label(&title, "row-title"));
        if !detail.is_empty() {
            text.append(&label(&detail, "row-detail"));
        }
        let image = colored(name, color);
        image.set_pixel_size(14);
        image.set_valign(gtk::Align::Start);
        image.set_margin_top(3);
        image.set_opacity(0.85);
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cell.set_margin_top(5);
        cell.set_margin_bottom(5);
        cell.set_margin_start(8);
        cell.set_margin_end(8);
        cell.append(&image);
        cell.append(&text);
        list.append(&cell);
    }
}

/// One row of the Ask page.
pub struct AnswerRow {
    pub icon: &'static str,
    pub name: String,
    pub signature: String,
    pub place: String,
    pub snippet: String,
    /// What the reranker made of it, empty when there is no reranker.
    pub score: String,
}

pub fn fill_answers(list: &gtk::ListBox, rows: impl IntoIterator<Item = AnswerRow>) {
    list.remove_all();
    for row in rows {
        let name = label(&row.name, "answer-name");
        name.set_hexpand(true);
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        heading.append(&name);
        if !row.score.is_empty() {
            let score = label(&row.score, "score");
            score.set_valign(gtk::Align::Center);
            heading.append(&score);
        }

        let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text.set_hexpand(true);
        text.append(&heading);
        if !row.place.is_empty() {
            // The signature is long and the panel is narrow, it lives in the tooltip.
            let place = label(&row.place, "answer-place");
            place.set_tooltip_text(Some(&format!("{} {}", row.name, row.signature)));
            text.append(&place);
        }
        if !row.snippet.is_empty() {
            text.append(&label(&row.snippet, "answer-snippet"));
        }

        let image = colored(row.icon, icons::PURPLE);
        image.set_pixel_size(15);
        image.set_valign(gtk::Align::Start);
        image.set_margin_top(2);
        image.set_opacity(0.85);
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        cell.add_css_class("answer");
        cell.append(&image);
        cell.append(&text);
        list.append(&cell);
    }
}

/// The package tree of the sidebar: one line per row, indented, packages with a marker and
/// how many classes they hold.
pub fn fill_tree(list: &gtk::ListBox, rows: &[crate::browse::tree::Row]) {
    list.remove_all();
    for row in rows {
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        cell.set_margin_top(3);
        cell.set_margin_bottom(3);
        cell.set_margin_start(6 + 14 * row.depth as i32);
        cell.set_margin_end(8);
        match row.count {
            Some(count) => {
                let marker = colored(if row.open { "chevron-down" } else { "chevron-right" }, icons::FAINT);
                marker.set_pixel_size(13);
                cell.append(&marker);
                let folder = colored("folder", icons::FAINT);
                folder.set_pixel_size(14);
                cell.append(&folder);
                let name = label(&row.label, "tree-package");
                name.set_hexpand(true);
                cell.append(&name);
                cell.append(&label(&count.to_string(), "tree-count"));
            }
            None => {
                // Lines up with the text of a package, past its marker.
                cell.set_margin_start(6 + 14 * row.depth as i32 + 19);
                if let crate::browse::tree::Target::Class(class) = &row.target {
                    let drag = gtk::DragSource::new();
                    drag.set_actions(gtk::gdk::DragAction::COPY);
                    let class = class.clone();
                    drag.connect_prepare(move |_, _, _| Some(gtk::gdk::ContentProvider::for_value(&class.to_value())));
                    cell.add_controller(drag);
                }
                let braces = colored("braces", icons::PURPLE);
                braces.set_pixel_size(14);
                braces.set_opacity(0.8);
                cell.append(&braces);
                cell.append(&label(&row.label, "tree-class"));
            }
        }
        list.append(&cell);
    }
}

/// macOS uses its native titlebar; other platforms draw the controls in the content bar.
pub fn window_controls(window: &gtk::Window) -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    if cfg!(target_os = "macos") {
        return controls;
    }
    controls.set_valign(gtk::Align::Center);
    controls.set_margin_start(10);
    for (icon, class) in [("win-minimize", "minimize"), ("win-maximize", "maximize"), ("win-close", "close")] {
        let button = gtk::Button::new();
        // Drawn bright and dimmed by the stylesheet, which can not recolour the SVG.
        button.set_child(Some(&colored(icon, icons::BRIGHT)));
        button.add_css_class("window-button");
        button.add_css_class(class);
        if class == "maximize" {
            let button = button.clone();
            window.connect_maximized_notify(move |window| {
                button.set_child(Some(&colored(if window.is_maximized() { "win-restore" } else { "win-maximize" }, icons::BRIGHT)));
            });
        }
        let window = window.clone();
        button.connect_clicked(move |_| match class {
            "minimize" => window.minimize(),
            "maximize" if window.is_maximized() => window.unmaximize(),
            "maximize" => window.maximize(),
            _ => window.close(),
        });
        controls.append(&button);
    }
    controls
}

/// One panel of the sidebar: a small heading and whatever belongs under it.
fn panel(title: &str, content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.append(&label(title, "section"));
    panel.append(content);
    panel
}

/// A list of choices in the sidebar, the selected one stays marked.
fn choices(items: impl IntoIterator<Item = (String, &'static str)>, on_pick: impl Fn(usize) + 'static) -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("choices");
    list.set_selection_mode(gtk::SelectionMode::Single);
    for (title, icon) in items {
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cell.set_margin_top(5);
        cell.set_margin_bottom(5);
        cell.set_margin_start(10);
        cell.set_margin_end(10);
        if !icon.is_empty() {
            let image = colored(icon, icons::MUTED);
            image.set_pixel_size(14);
            cell.append(&image);
        }
        cell.append(&label(&title, ""));
        list.append(&cell);
    }
    // Selecting, not activating: one click is enough and the marker follows.
    list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            on_pick(row.index().max(0) as usize);
        }
    });
    list.select_row(list.row_at_index(0).as_ref());
    list
}

pub fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label
}

pub fn wrapping(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    // A wrapping label asks for the width of its text on one line, and a page that centres its
    // content then grows with the longest sentence on it. This caps what the label asks for,
    // it still fills whatever it is given.
    label.set_max_width_chars(60);
    label
}

pub fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder().child(child).hexpand(true).vexpand(true).build()
}

pub fn separator(orientation: gtk::Orientation) -> gtk::Separator {
    gtk::Separator::new(orientation)
}

fn list(sender: &ComponentSender<App>, message: fn(usize) -> Msg) -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let sender = sender.clone();
    list.connect_row_activated(move |_, row| sender.input(message(row.index().max(0) as usize)));
    list
}

pub fn button(sender: &ComponentSender<App>, text: &str, class: &str, message: Msg) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    if !class.is_empty() {
        button.add_css_class(class);
    }
    let sender = sender.clone();
    button.connect_clicked(move |_| sender.input(message.clone()));
    button
}

/// What a page shows before anything has been done on it.
fn empty_state(name: &str, title: &str, help: &str) -> (gtk::Box, gtk::Label, gtk::Label) {
    let image = colored(name, icons::FAINT);
    image.set_pixel_size(30);
    image.set_margin_bottom(6);
    let title = label(title, "empty-title");
    title.set_halign(gtk::Align::Center);
    let help = wrapping(help, "empty-help");
    help.set_halign(gtk::Align::Center);
    help.set_justify(gtk::Justification::Center);
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 6);
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    empty.append(&image);
    empty.append(&title);
    empty.append(&help);
    (empty, title, help)
}

pub fn icon_button(sender: &ComponentSender<App>, name: &str, text: &str, class: &str, message: Msg) -> gtk::Button {
    let button = button(sender, "", class, message);
    let color = match class {
        "primary" => icons::BRIGHT,
        "stop" => icons::DARK,
        _ => icons::MUTED,
    };
    if text.is_empty() {
        button.set_child(Some(&colored(name, color)));
    } else {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&colored(name, color));
        line.append(&label(text, ""));
        button.set_child(Some(&line));
    }
    button
}

/// Keeps a page readable on a wide window.
fn column(child: &gtk::Box, width: i32) -> gtk::Box {
    child.set_width_request(width);
    child.set_halign(gtk::Align::Center);
    child.set_margin_top(18);
    child.set_margin_bottom(18);
    child.set_margin_start(18);
    child.set_margin_end(18);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(child);
    outer
}

/// Lantern is dark only. The base theme has to be told, otherwise the parts the stylesheet
/// does not touch come out light on a light desktop.
fn force_dark() {
    if let Some(settings) = gtk::Settings::default() {
        #[allow(deprecated)]
        settings.set_gtk_application_prefer_dark_theme(true);
    }
}

/// A buffer that highlights `language` in the Lantern colours.
#[cfg(feature = "highlight")]
pub fn code_buffer(language: &str) -> sourceview5::Buffer {
    use sourceview5::prelude::*;

    // The scheme ships inside the binary, GtkSourceView only reads them from a folder.
    thread_local!(static INSTALLED: Cell<bool> = const { Cell::new(false) });
    let schemes = sourceview5::StyleSchemeManager::default();
    if !INSTALLED.replace(true) {
        let dir = gtk::glib::user_cache_dir().join("lantern").join("styles");
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join("lantern-dark.xml"), include_str!("theme/schemes/lantern-dark.xml")).is_ok() {
            schemes.append_search_path(&dir.display().to_string());
        }
    }

    let id = match language.to_ascii_lowercase().as_str() {
        "" => "java",
        "rs" => "rust",
        "py" | "python" => "python3",
        "js" | "javascript" => "js",
        "sh" | "bash" | "shell" => "sh",
        _ => language,
    };
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_language(sourceview5::LanguageManager::default().language(&id.to_ascii_lowercase()).as_ref());
    buffer.set_style_scheme(["lantern-dark", "Adwaita-dark", "classic-dark"].iter().find_map(|id| schemes.scheme(id)).as_ref());
    buffer
}

#[cfg(feature = "highlight")]
pub fn source_view() -> gtk::TextView {
    use sourceview5::prelude::*;

    let view = sourceview5::View::with_buffer(&code_buffer("java"));
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.upcast()
}

#[cfg(not(feature = "highlight"))]
pub fn source_view() -> gtk::TextView {
    gtk::TextView::new()
}

/// A piece of code in a chat answer: the language, a copy button and the highlighted text.
pub fn code_block(language: &str, code: &str) -> gtk::Box {
    #[cfg(feature = "highlight")]
    let text: gtk::TextView = {
        use sourceview5::prelude::*;
        let buffer = code_buffer(language);
        // Nothing is edited here, a bracket lit up next to an invisible cursor is just noise.
        buffer.set_highlight_matching_brackets(false);
        sourceview5::View::with_buffer(&buffer).upcast()
    };
    #[cfg(not(feature = "highlight"))]
    let text = gtk::TextView::new();
    text.buffer().set_text(code);
    if matches!(language, "" | "java") {
        crate::theme::highlight::apply(&text.buffer());
    }
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_monospace(true);
    text.set_left_margin(12);
    text.set_right_margin(12);
    text.set_top_margin(8);
    text.set_bottom_margin(10);
    text.add_css_class("code");

    // Long lines scroll sideways, the height follows the text.
    let scroll = gtk::ScrolledWindow::builder().child(&text).hexpand(true).build();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);

    let name = label(if language.is_empty() { "java" } else { language }, "md-code-language");
    name.set_hexpand(true);
    let copy = gtk::Button::new();
    copy.set_child(Some(&with_icon("copy", &label("Copy", ""))));
    copy.add_css_class("small");
    let copied = code.to_owned();
    copy.connect_clicked(move |button| button.clipboard().set_text(&copied));
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("md-code-head");
    head.append(&name);
    head.append(&copy);

    let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
    block.add_css_class("md-code");
    block.set_overflow(gtk::Overflow::Hidden);
    block.append(&head);
    block.append(&scroll);
    block
}

pub fn build(window: &gtk::Window, sender: &ComponentSender<App>) -> Widgets {
    force_dark();
    gtk::IconTheme::for_display(&gtk::prelude::WidgetExt::display(window)).add_resource_path("/dev/megalithofficial/Lantern/icons");
    window.set_icon_name(Some("dev.megalithofficial.Lantern"));

    // Keep the native macOS titlebar so its traffic lights have space above the content.
    if !cfg!(target_os = "macos") {
        let stub = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        stub.add_css_class("titlebar-stub");
        window.set_titlebar(Some(&stub));
    }

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);

    // Sidebar
    let brand_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    brand_row.set_margin_start(16);
    brand_row.set_margin_end(12);
    brand_row.set_height_request(46);
    brand_row.append(&crate::theme::brand::image(25));
    brand_row.append(&label("Lantern", "brand"));
    let version_badge = label("", "badge");
    version_badge.set_valign(gtk::Align::Center);
    brand_row.append(&version_badge);
    let brand = gtk::WindowHandle::new();
    brand.set_child(Some(&brand_row));

    // Everything the sidebar can show, one panel per page. The buttons that switch them sit
    // at the bottom, so no page needs a second sidebar of its own.
    let side_stack = gtk::Stack::new();
    side_stack.set_vexpand(true);
    side_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    side_stack.set_transition_duration(100);

    let class_search = gtk::SearchEntry::new();
    class_search.set_placeholder_text(Some("Search classes"));
    class_search.set_margin_start(10);
    class_search.set_margin_end(10);
    class_search.set_margin_bottom(6);
    {
        let sender = sender.clone();
        class_search.connect_search_changed(move |entry| sender.input(Msg::ClassSearch(entry.text().into())));
    }
    let class_list = list(sender, Msg::ClassActivated);
    let class_scroll = scrolled(&class_list);
    class_scroll.set_hexpand(false);
    let browse_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    browse_panel.append(&label("CLASSES", "section"));
    browse_panel.append(&class_search);
    browse_panel.append(&class_scroll);
    side_stack.add_named(&browse_panel, Some("browse"));

    let chat_list = list(sender, Msg::ChatOpen);
    {
        // Right click on a chat offers to delete it.
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let (target, sender) = (chat_list.clone(), sender.clone());
        click.connect_pressed(move |_, _, x, y| {
            if let Some(row) = target.row_at_y(y as i32) {
                target.select_row(Some(&row));
                sender.input(Msg::ChatMenu(row.index().max(0) as usize, x, y));
            }
        });
        chat_list.add_controller(click);
    }
    side_stack.add_named(&panel("CHATS", &scrolled(&chat_list)), Some("chat"));

    // The head of the Lookup page follows the kind picked in the sidebar.
    let lookup_kind = Rc::new(Cell::new(0));
    let lookup_icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    lookup_icon.set_valign(gtk::Align::Center);
    let lookup_title = label("", "lookup-title");
    let lookup_about = wrapping("", "muted");
    let lookup_entry = gtk::Entry::new();
    let lookup_example = gtk::Button::new();
    lookup_example.add_css_class("chip");
    let kinds = {
        let picked = lookup_kind.clone();
        let sender = sender.clone();
        let (icon, title, about, entry, example) =
            (lookup_icon.clone(), lookup_title.clone(), lookup_about.clone(), lookup_entry.clone(), lookup_example.clone());
        choices(LOOKUPS.iter().map(|kind| (kind.name.to_owned(), kind.icon)), move |index| {
            let kind = &LOOKUPS[index];
            picked.set(index);
            if let Some(old) = icon.first_child() {
                icon.remove(&old);
            }
            icon.append(&colored(kind.icon, icons::MUTED));
            title.set_text(kind.name);
            about.set_text(kind.about);
            entry.set_placeholder_text(Some(kind.hint));
            example.set_label(kind.example);
            sender.input(Msg::Lookup);
        })
    };
    side_stack.add_named(&panel("LOOK UP", &scrolled(&kinds)), Some("lookup"));
    let (compare, compare_panel, compare_page) = crate::pages::compare::build(sender);
    side_stack.add_named(&compare_panel, Some("compare"));

    let (setup, setup_page) = crate::pages::setup::build(sender);
    let sections = {
        let pages = setup.pages.clone();
        choices(crate::pages::setup::SECTIONS.iter().map(|(title, icon)| ((*title).to_owned(), *icon)), move |index| {
            pages.set_visible_child_name(crate::pages::setup::SECTIONS[index].0);
        })
    };
    // SNAPSHOT-TMP
    if let Ok(wanted) = std::env::var("LANTERN_SECTION") {
        if let Some(index) = crate::pages::setup::SECTIONS.iter().position(|(title, _)| *title == wanted) {
            sections.select_row(sections.row_at_index(index as i32).as_ref());
        }
    }
    side_stack.add_named(&panel("SETTINGS", &scrolled(&sections)), Some("setup"));

    // Icon only, the name is in the tooltip. One row, at the bottom, always in the same place.
    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_box.add_css_class("navbar");
    nav_box.set_homogeneous(true);
    let nav_syncing = Rc::new(Cell::new(false));
    let mut nav: Vec<(&'static str, gtk::ToggleButton)> = Vec::new();
    for (name, title) in PAGES {
        let item = gtk::ToggleButton::new();
        item.set_child(Some(&colored(page_icon(name), icons::MUTED)));
        item.add_css_class("nav");
        item.set_tooltip_text(Some(title));
        if let Some((_, first)) = nav.first() {
            item.set_group(Some(first));
        }
        let sender = sender.clone();
        let syncing = nav_syncing.clone();
        item.connect_toggled(move |item| {
            if item.is_active() && !syncing.get() {
                sender.input(Msg::Page(name));
            }
        });
        nav_box.append(&item);
        nav.push((name, item));
    }

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("sidebar");
    // Without this the sidebar takes the expanding of the lists inside it and grows with the
    // window, which on a wide screen leaves the code squeezed in the middle.
    sidebar.set_hexpand(false);
    side_stack.set_hexpand(false);
    sidebar.set_width_request(260);
    sidebar.append(&brand);
    sidebar.append(&side_stack);
    sidebar.append(&separator(gtk::Orientation::Horizontal));
    sidebar.append(&nav_box);

    let find_entry = gtk::SearchEntry::new();
    find_entry.set_placeholder_text(Some("Find in open files"));
    find_entry.set_width_request(230);
    {
        let sender = sender.clone();
        find_entry.connect_search_changed(move |entry| sender.input(Msg::FindInFiles(entry.text().into())));
    }
    {
        let sender = sender.clone();
        find_entry.connect_activate(move |_| sender.input(Msg::FindNext));
    }
    let find_status = label("", "muted");
    find_status.add_css_class("caption");
    let sync_button = gtk::ToggleButton::new();
    sync_button.set_child(Some(&colored("link", icons::MUTED)));
    sync_button.add_css_class("flat");
    sync_button.set_tooltip_text(Some("Scroll the open files together"));
    {
        let sender = sender.clone();
        sync_button.connect_toggled(move |button| sender.input(Msg::ToggleSync(button.is_active())));
    }
    let find_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    find_bar.set_valign(gtk::Align::Center);
    find_bar.append(&find_entry);
    find_bar.append(&find_status);
    find_bar.append(&sync_button);

    // Top bar
    let crumb_path = label("", "crumb");
    let crumb_title = label("", "crumb-title");
    let chat_new = icon_button(sender, "plus", "New chat", "", Msg::ChatNew);
    chat_new.set_valign(gtk::Align::Center);
    let open_file = icon_button(sender, "external-link", "Open file", "", Msg::OpenFile);
    open_file.set_tooltip_text(Some("Open the decompiled file in the default editor"));
    open_file.set_valign(gtk::Align::Center);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let controls = window_controls(window);
    let back = icon_button(sender, "arrow-left", "", "flat", Msg::Back);
    back.set_tooltip_text(Some("Back (Alt+Left)"));
    back.set_valign(gtk::Align::Center);
    back.set_sensitive(false);
    let forward = icon_button(sender, "arrow-right", "", "flat", Msg::Forward);
    forward.set_tooltip_text(Some("Forward (Alt+Right)"));
    forward.set_valign(gtk::Align::Center);
    forward.set_sensitive(false);
    let history = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    history.set_margin_end(6);
    history.append(&back);
    history.append(&forward);
    let topbar_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    topbar_row.add_css_class("topbar");
    topbar_row.append(&history);
    topbar_row.append(&crumb_path);
    topbar_row.append(&crumb_title);
    topbar_row.append(&spacer);
    topbar_row.append(&find_bar);
    topbar_row.append(&open_file);
    topbar_row.append(&chat_new);
    topbar_row.append(&controls);
    let topbar = gtk::WindowHandle::new();
    topbar.set_child(Some(&topbar_row));

    // Browse
    let panes = crate::browse::panes::Panes::new(sender);
    let location = label("", "muted");
    location.set_selectable(true);
    location.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    location.add_css_class("caption");
    location.add_css_class("mono");
    location.add_css_class("strip");
    let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
    center.append(&panes.root);
    center.append(&separator(gtk::Orientation::Horizontal));
    center.append(&location);

    let member_list = list(sender, Msg::MemberActivated);
    let callers_list = list(sender, Msg::CallerActivated);
    let callees_list = list(sender, Msg::CalleeActivated);
    let callers_title = label("CALLERS", "section");
    let members = scrolled(&member_list);
    members.set_height_request(320);
    let inspector = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inspector.append(&label("MEMBERS", "section"));
    inspector.append(&members);
    inspector.append(&separator(gtk::Orientation::Horizontal));
    inspector.append(&callers_title);
    inspector.append(&scrolled(&callers_list));
    inspector.append(&separator(gtk::Orientation::Horizontal));
    inspector.append(&label("CALLS", "section"));
    inspector.append(&scrolled(&callees_list));

    // Searching covers the members of the class, it is about the same question: what is here.
    let ask_entry = gtk::SearchEntry::new();
    ask_entry.set_placeholder_text(Some("Search the code by meaning"));
    ask_entry.set_hexpand(true);
    {
        let sender = sender.clone();
        ask_entry.connect_activate(move |_| sender.input(Msg::Ask));
    }
    {
        let sender = sender.clone();
        ask_entry.connect_search_changed(move |entry| {
            if entry.text().is_empty() {
                sender.input(Msg::SearchCleared);
            }
        });
    }
    // The spinner sits inside the box, to the left of its clear button, so the box itself can
    // run the whole width of the panel.
    let ask_spinner = gtk::Spinner::new();
    ask_spinner.set_size_request(16, 16);
    ask_spinner.set_halign(gtk::Align::End);
    ask_spinner.set_valign(gtk::Align::Center);
    ask_spinner.set_margin_end(30);
    let search_bar = gtk::Overlay::new();
    search_bar.add_css_class("search-bar");
    search_bar.set_child(Some(&ask_entry));
    search_bar.add_overlay(&ask_spinner);

    let ask_status = label("", "section");
    let ask_list = list(sender, Msg::AnswerActivated);
    ask_list.add_css_class("results");
    let (ask_empty, ask_empty_title, ask_empty_help) = empty_state("search", "Search the code by meaning", "Type what something does, not what it is called.");
    ask_empty.set_vexpand(true);
    let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
    results.append(&ask_status);
    results.append(&ask_empty);
    results.append(&scrolled(&ask_list));

    let inspector_stack = gtk::Stack::new();
    inspector_stack.set_vexpand(true);
    inspector_stack.add_named(&inspector, Some("members"));
    inspector_stack.add_named(&results, Some("results"));

    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.set_width_request(400);
    right.set_hexpand(false);
    right.append(&search_bar);
    right.append(&separator(gtk::Orientation::Horizontal));
    right.append(&inspector_stack);

    let browse = gtk::Paned::new(gtk::Orientation::Horizontal);
    browse.set_start_child(Some(&center));
    browse.set_end_child(Some(&right));
    browse.set_resize_end_child(false);
    browse.set_shrink_end_child(false);
    browse.set_position(780);
    stack.add_named(&browse, Some("browse"));

    // Chat
    let chat_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
    chat_box.set_margin_top(8);
    chat_box.set_margin_bottom(8);
    crate::chat::chatmenu::install(&chat_box, sender);
    let chat_scroll = scrolled(&chat_box);
    // Following is a state, not a guess made at every update: moving the view decides it,
    // growing content only acts on it. Measuring "is it at the end" when text arrives goes
    // wrong as soon as one update is missed, the gap then looks like the reader scrolled up.
    let chat_follow = Rc::new(Cell::new(true));
    {
        let adjustment = chat_scroll.vadjustment();
        let follow = chat_follow.clone();
        adjustment.connect_value_changed(move |a| follow.set(a.value() + a.page_size() >= a.upper() - 48.0));
        let follow = chat_follow.clone();
        adjustment.connect_changed(move |a| {
            if follow.get() {
                a.set_value(a.upper() - a.page_size());
            }
        });
    }
    let chat_entry = gtk::Entry::new();
    chat_entry.add_css_class("bare");
    chat_entry.set_hexpand(true);
    chat_entry.set_placeholder_text(Some("Ask about the vanilla server, the model reads the code for you"));
    {
        let sender = sender.clone();
        chat_entry.connect_activate(move |_| sender.input(Msg::ChatSend));
    }
    let chat_usage = label("", "muted");
    chat_usage.add_css_class("caption");
    chat_usage.set_hexpand(true);
    let chat_stop = icon_button(sender, "square", "", "stop", Msg::ChatStop);
    chat_stop.add_css_class("round");
    chat_stop.set_tooltip_text(Some("Stop"));
    let chat_send = icon_button(sender, "arrow-up", "", "primary", Msg::ChatSend);
    chat_send.add_css_class("round");
    let chat_foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    chat_foot.append(&chat_usage);
    chat_foot.append(&chat_stop);
    chat_foot.append(&chat_send);
    let chat_composer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    chat_composer.add_css_class("composer");
    chat_composer.append(&chat_entry);
    chat_composer.append(&chat_foot);
    let (chat_empty, _, _) = empty_state(
        "message-square",
        "Ask Lantern",
        "It reads the decompiled code with its own tools and answers with the class and method names behind every claim.",
    );
    let chat_area = gtk::Overlay::new();
    chat_area.set_child(Some(&chat_scroll));
    chat_area.add_overlay(&chat_empty);
    let chat = gtk::Box::new(gtk::Orientation::Vertical, 10);
    chat.append(&chat_area);
    chat.append(&chat_composer);
    let chat = column(&chat, 860);
    stack.add_named(&chat, Some("chat"));

    // Lookup
    let lookup_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lookup_heading.append(&lookup_icon);
    lookup_heading.append(&lookup_title);
    lookup_entry.add_css_class("bare");
    lookup_entry.add_css_class("mono");
    lookup_entry.set_hexpand(true);
    {
        let sender = sender.clone();
        lookup_entry.connect_activate(move |_| sender.input(Msg::Lookup));
    }
    {
        let (picked, entry, sender) = (lookup_kind.clone(), lookup_entry.clone(), sender.clone());
        lookup_example.connect_clicked(move |_| {
            entry.set_text(LOOKUPS[picked.get()].example);
            sender.input(Msg::Lookup);
        });
    }
    let lookup_button = icon_button(sender, "play", "Run", "primary", Msg::Lookup);
    lookup_button.set_valign(gtk::Align::Center);
    let lookup_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lookup_bar.add_css_class("composer");
    lookup_bar.append(&lookup_entry);
    lookup_bar.append(&lookup_button);
    let example_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    example_line.append(&label("Try", "faint"));
    example_line.append(&lookup_example);

    let lookup_links = crate::pages::lookup::Links::default();
    let lookup_out = crate::pages::lookup::view(sender, &lookup_links);
    let (lookup_empty, _, _) = empty_state("terminal", "Nothing looked up yet", "Type a name above and press Enter, or try the example.");
    let lookup_error = wrapping("", "chat-error");
    lookup_error.set_valign(gtk::Align::Start);
    lookup_error.set_margin_top(12);
    lookup_error.set_margin_start(14);
    lookup_error.set_margin_end(14);
    let lookup_stack = gtk::Stack::new();
    lookup_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    lookup_stack.set_transition_duration(100);
    lookup_stack.add_named(&lookup_empty, Some("empty"));
    lookup_stack.add_named(&scrolled(&lookup_out), Some("out"));
    lookup_stack.add_named(&lookup_error, Some("error"));
    lookup_stack.set_vexpand(true);
    lookup_stack.add_css_class("output");

    let lookup = gtk::Box::new(gtk::Orientation::Vertical, 10);
    lookup.append(&lookup_heading);
    lookup.append(&lookup_about);
    lookup.append(&lookup_bar);
    lookup.append(&example_line);
    lookup.append(&lookup_stack);
    let lookup = column(&lookup, 1000);
    stack.add_named(&lookup, Some("lookup"));

    stack.add_named(&compare_page, Some("compare"));
    stack.add_named(&setup_page, Some("setup"));

    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    main.append(&topbar);
    main.append(&separator(gtk::Orientation::Horizontal));
    main.append(&stack);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.append(&sidebar);
    root.append(&separator(gtk::Orientation::Vertical));
    root.append(&main);
    let onboarding = crate::pages::onboarding::build(window, sender);
    let top = gtk::Stack::new();
    top.set_transition_type(gtk::StackTransitionType::Crossfade);
    top.set_transition_duration(250);
    top.add_named(&root, Some("main"));
    top.add_named(&onboarding.root, Some("onboarding"));
    window.set_child(Some(&top));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let sender = sender.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let primary = if cfg!(target_os = "macos") { gtk::gdk::ModifierType::META_MASK } else { gtk::gdk::ModifierType::CONTROL_MASK };
            if modifiers.contains(primary) && key == gtk::gdk::Key::f {
                sender.input(Msg::FocusSearch);
                return gtk::glib::Propagation::Stop;
            }
            if modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) && matches!(key, gtk::gdk::Key::Left | gtk::gdk::Key::Right) {
                sender.input(if key == gtk::gdk::Key::Left { Msg::Back } else { Msg::Forward });
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    // The thumb buttons of a mouse.
    for (number, message) in [(8, Msg::Back), (9, Msg::Forward)] {
        let side_button = gtk::GestureClick::new();
        side_button.set_button(number);
        side_button.set_propagation_phase(gtk::PropagationPhase::Capture);
        let sender = sender.clone();
        side_button.connect_pressed(move |_, _, _, _| sender.input(message.clone()));
        window.add_controller(side_button);
    }

    Widgets {
        stack,
        nav,
        top,
        onboarding,
        compare,
        nav_syncing,
        version_badge,
        crumb_path,
        crumb_title,
        side_stack,
        class_list,
        class_scroll,
        member_list,
        callers_list,
        callers_title,
        callees_list,
        panes,
        find_status,
        location,
        open_file,
        back,
        forward,
        inspector_stack,
        ask_entry,
        ask_spinner,
        ask_status,
        ask_empty,
        ask_empty_title,
        ask_empty_help,
        ask_list,
        chat_scroll,
        chat_empty,
        chat_follow,
        chat_box,
        chat_entry,
        chat_send,
        chat_stop,
        chat_usage,
        chat_new,
        chat_list,
        chat_current: None,
        chat_live: None,
        chat_turn: None,
        chat_actions: None,
        lookup_kind,
        lookup_entry,
        lookup_out,
        lookup_links,
        lookup_kinds: kinds.clone(),
        lookup_stack,
        lookup_error,
        setup,
    }
}
