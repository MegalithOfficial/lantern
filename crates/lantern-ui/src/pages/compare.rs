//! The Compare page: two decompiled versions, the files that differ between them as a tree in
//! the sidebar, and the diff of the picked file, unified or side by side.

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use gtk::prelude::*;
use lantern_core::compare::{FileChange, Hunk, Line, LineKind, Status};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    theme::icons::{self, colored},
    view::{icon_button, label, scrolled, wrapping},
};

pub struct Widgets {
    pub old: gtk::DropDown,
    pub new: gtk::DropDown,
    pub run: gtk::Button,
    pub filter: gtk::SearchEntry,
    /// All, Code, Data.
    pub kinds: [gtk::ToggleButton; 3],
    pub summary: gtk::Label,
    pub files: gtk::ListBox,

    /// "empty", "busy", "diff".
    pub stack: gtk::Stack,
    pub empty_title: gtk::Label,
    pub empty_help: gtk::Label,
    pub bar: gtk::ProgressBar,
    pub busy: gtk::Label,
    pub status: gtk::Label,
    pub path: gtk::Label,
    pub counts: gtk::Label,
    pub split: gtk::ToggleButton,
    /// "unified" or "split".
    pub views: gtk::Stack,
    pub unified: gtk::TextView,
    pub left: gtk::TextView,
    pub right: gtk::TextView,
}

/// What is compared and what is shown of it.
#[derive(Default)]
pub struct State {
    /// The two versions the list is for, and their source folders.
    pub pair: Option<(String, String, PathBuf, PathBuf)>,
    pub changes: Vec<FileChange>,
    open: HashSet<String>,
    rows: Vec<Target>,
    pub selected: Option<usize>,
    file: Option<FileView>,
}

/// The file that is shown, with its text at hand for opening up what the hunks leave out.
struct FileView {
    java: bool,
    /// The older text. The gaps between hunks are lines both sides have.
    old: Vec<String>,
    hunks: Vec<Hunk>,
    /// Which line of the view is the header of which gap: gap k is above hunk k, gap
    /// `hunks.len()` is after the last one.
    headers: Vec<(usize, usize)>,
}

impl State {
    pub fn new(pair: (String, String, PathBuf, PathBuf), changes: Vec<FileChange>) -> Self {
        Self { pair: Some(pair), changes, ..Default::default() }
    }
}

#[derive(Clone)]
enum Target {
    Dir(String),
    File(usize),
}

/// The versions that can be compared: the ones with decompiled sources, newest first.
pub fn set_versions(widgets: &Widgets, ids: &[String]) {
    let keep = |dropdown: &gtk::DropDown| dropdown.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string());
    let (old, new) = (keep(&widgets.old), keep(&widgets.new));
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    for dropdown in [&widgets.old, &widgets.new] {
        dropdown.set_model(Some(&gtk::StringList::new(&names)));
    }
    // The newest against the one before it, unless something was picked already.
    let position = |name: Option<String>, fallback: usize| {
        name.and_then(|name| ids.iter().position(|id| *id == name)).unwrap_or(fallback.min(ids.len().saturating_sub(1))) as u32
    };
    widgets.new.set_selected(position(new, 0));
    widgets.old.set_selected(position(old, 1));
    widgets.run.set_sensitive(ids.len() >= 2);
    if ids.len() < 2 {
        widgets.empty_title.set_text("Two versions are needed");
        widgets.empty_help.set_text("Download and decompile at least two in Setup, then compare them here.");
    }
}

pub fn picked(widgets: &Widgets) -> Option<(String, String)> {
    let get = |dropdown: &gtk::DropDown| dropdown.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string());
    Some((get(&widgets.old)?, get(&widgets.new)?))
}

pub fn build(sender: &ComponentSender<App>) -> (Widgets, gtk::Box, gtk::Box) {
    // Sidebar
    let old = gtk::DropDown::from_strings(&[]);
    let new = gtk::DropDown::from_strings(&[]);
    for dropdown in [&old, &new] {
        dropdown.set_hexpand(true);
        dropdown.set_tooltip_text(Some("A version with decompiled sources"));
    }
    // One under the other, side by side they would not fit the sidebar.
    let pair = gtk::Grid::new();
    pair.set_row_spacing(6);
    pair.set_column_spacing(8);
    for (row, (name, dropdown)) in [("From", &old), ("To", &new)].into_iter().enumerate() {
        let caption = label(name, "faint");
        caption.set_width_chars(4);
        pair.attach(&caption, 0, row as i32, 1, 1);
        pair.attach(dropdown, 1, row as i32, 1, 1);
    }
    let run = icon_button(sender, "git-compare", "Compare", "primary", Msg::CompareRun);

    let filter = gtk::SearchEntry::new();
    filter.set_placeholder_text(Some("Filter files"));
    {
        let sender = sender.clone();
        filter.connect_search_changed(move |_| sender.input(Msg::CompareFilter));
    }
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("segmented");
    track.set_homogeneous(true);
    let kinds = ["All", "Code", "Data"].map(gtk::ToggleButton::with_label);
    for (index, kind) in kinds.iter().enumerate() {
        if index > 0 {
            kind.set_group(Some(&kinds[0]));
        }
        let sender = sender.clone();
        kind.connect_toggled(move |kind| {
            if kind.is_active() {
                sender.input(Msg::CompareFilter);
            }
        });
        track.append(kind);
    }
    kinds[0].set_active(true);
    let summary = wrapping("", "compare-summary");
    summary.set_visible(false);

    let files = gtk::ListBox::new();
    files.add_css_class("compare-files");
    files.set_selection_mode(gtk::SelectionMode::Single);
    {
        let sender = sender.clone();
        files.connect_row_activated(move |_, row| sender.input(Msg::CompareActivated(row.index().max(0) as usize)));
    }
    let files_scroll = scrolled(&files);
    files_scroll.set_hexpand(false);

    let controls = gtk::Box::new(gtk::Orientation::Vertical, 8);
    controls.set_margin_start(10);
    controls.set_margin_end(10);
    controls.set_margin_bottom(8);
    controls.append(&pair);
    controls.append(&run);
    controls.append(&filter);
    controls.append(&track);
    controls.append(&summary);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.append(&label("COMPARE", "section"));
    panel.append(&controls);
    panel.append(&files_scroll);

    // Page
    let empty_icon = colored("git-compare", icons::FAINT);
    empty_icon.set_pixel_size(30);
    empty_icon.set_margin_bottom(6);
    let empty_title = label("Compare two versions", "empty-title");
    empty_title.set_halign(gtk::Align::Center);
    let empty_help = wrapping("Pick an older and a newer version on the left. Every file of their sources and data that changed is listed.", "empty-help");
    empty_help.set_halign(gtk::Align::Center);
    empty_help.set_justify(gtk::Justification::Center);
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 6);
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    empty.append(&empty_icon);
    empty.append(&empty_title);
    empty.append(&empty_help);

    let bar = gtk::ProgressBar::new();
    bar.set_width_request(320);
    let busy_text = label("", "empty-help");
    busy_text.set_halign(gtk::Align::Center);
    let busy = gtk::Box::new(gtk::Orientation::Vertical, 10);
    busy.set_valign(gtk::Align::Center);
    busy.set_halign(gtk::Align::Center);
    busy.append(&busy_text);
    busy.append(&bar);

    let status = label("", "diff-status");
    status.set_valign(gtk::Align::Center);
    let path = label("", "diff-path");
    path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path.set_hexpand(true);
    let counts = label("", "diff-counts");
    counts.set_use_markup(true);
    let mode = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode.add_css_class("segmented");
    mode.set_valign(gtk::Align::Center);
    let unified_button = gtk::ToggleButton::with_label("Unified");
    let split = gtk::ToggleButton::with_label("Split");
    split.set_group(Some(&unified_button));
    unified_button.set_active(true);
    {
        let sender = sender.clone();
        split.connect_toggled(move |_| sender.input(Msg::CompareMode));
    }
    mode.append(&unified_button);
    mode.append(&split);
    let previous = icon_button(sender, "chevron-up", "", "flat", Msg::CompareStep(false));
    previous.set_tooltip_text(Some("Previous changed file"));
    let next = icon_button(sender, "chevron-down", "", "flat", Msg::CompareStep(true));
    next.set_tooltip_text(Some("Next changed file"));
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    head.add_css_class("diff-head");
    head.append(&status);
    head.append(&path);
    head.append(&counts);
    head.append(&previous);
    head.append(&next);
    head.append(&mode);

    let unified = diff_view(sender);
    let left = diff_view(sender);
    let right = diff_view(sender);
    let left_scroll = scrolled(&full_width(&left));
    let right_scroll = scrolled(&full_width(&right));
    // One scroll position for both halves, their rows line up.
    right_scroll.set_vadjustment(Some(&left_scroll.vadjustment()));
    // Sideways they follow each other instead of sharing one adjustment: their longest lines
    // differ, and each side would keep setting its own width on a shared one.
    let following = std::rc::Rc::new(std::cell::Cell::new(false));
    for (from, to) in [(&left_scroll, &right_scroll), (&right_scroll, &left_scroll)] {
        let (target, following) = (to.hadjustment(), following.clone());
        from.hadjustment().connect_value_changed(move |source| {
            if following.replace(true) {
                return;
            }
            target.set_value(source.value().min(target.upper() - target.page_size()));
            following.set(false);
        });
    }
    let halves = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    halves.set_homogeneous(true);
    // The line between them is a border: a separator in a homogeneous box gets a third of it.
    right_scroll.add_css_class("split-right");
    halves.append(&left_scroll);
    halves.append(&right_scroll);
    let views = gtk::Stack::new();
    views.add_named(&scrolled(&full_width(&unified)), Some("unified"));
    views.add_named(&halves, Some("split"));
    views.set_vexpand(true);

    let diff = gtk::Box::new(gtk::Orientation::Vertical, 0);
    diff.append(&head);
    diff.append(&views);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(100);
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&busy, Some("busy"));
    stack.add_named(&diff, Some("diff"));
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.append(&stack);
    stack.set_vexpand(true);

    let widgets = Widgets {
        old,
        new,
        run,
        filter,
        kinds,
        summary,
        files,
        stack,
        empty_title,
        empty_help,
        bar,
        busy: busy_text,
        status,
        path,
        counts,
        split,
        views,
        unified,
        left,
        right,
    };
    (widgets, panel, page)
}

const TAGS: &[(&str, &[(&str, &str)])] = &[
    ("df-gutter", &[("foreground", "#656c76")]),
    ("df-add", &[("paragraph-background", "#12261e")]),
    ("df-remove", &[("paragraph-background", "#2a1619")]),
    ("df-add-word", &[("background", "#1f5530")]),
    ("df-remove-word", &[("background", "#6a2a2c")]),
    ("df-add-sign", &[("foreground", "#3fb950")]),
    ("df-remove-sign", &[("foreground", "#f85149")]),
    ("df-hunk", &[("paragraph-background", "#121d2f"), ("foreground", "#9198a1")]),
    ("df-filler", &[("paragraph-background", "#0a0d12")]),
];

/// A text view scrolls itself and is only as wide as what is visible, so the colour of a line
/// stops at the edge of the window once it is scrolled sideways. In a viewport it is given the
/// width of its longest line and the colours run all the way.
fn full_width(view: &gtk::TextView) -> gtk::Viewport {
    gtk::Viewport::builder()
        .child(view)
        .hscroll_policy(gtk::ScrollablePolicy::Natural)
        .vscroll_policy(gtk::ScrollablePolicy::Natural)
        .scroll_to_focus(false)
        .build()
}

fn scroller(view: &gtk::TextView) -> Option<gtk::ScrolledWindow> {
    view.ancestor(gtk::ScrolledWindow::static_type()).and_downcast()
}

fn diff_view(sender: &ComponentSender<App>) -> gtk::TextView {
    let view = gtk::TextView::new();
    // A click on a hunk header opens the lines it leaves out.
    let click = gtk::GestureClick::new();
    {
        let sender = sender.clone();
        click.connect_released(move |gesture, presses, x, y| {
            let Some(view) = gesture.widget().and_downcast::<gtk::TextView>() else { return };
            if presses != 1 || view.buffer().has_selection() {
                return;
            }
            let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = view.iter_at_location(bx, by) {
                sender.input(Msg::CompareClick(iter.line() as usize));
            }
        });
    }
    view.add_controller(click);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_top_margin(6);
    view.set_bottom_margin(12);
    view.add_css_class("diff-view");
    let buffer = view.buffer();
    for (name, properties) in TAGS {
        let tag = gtk::TextTag::new(Some(name));
        for (key, value) in *properties {
            tag.set_property(key, *value);
        }
        buffer.tag_table().add(&tag);
    }
    view
}

/// Lists the changes as a tree, or as a flat list of the files that match the filter.
pub fn show_files(state: &mut State, widgets: &Widgets) {
    let needle = widgets.filter.text().trim().to_lowercase();
    let kind = widgets.kinds.iter().position(|k| k.is_active()).unwrap_or(0);
    let wanted = |change: &FileChange| {
        let code = change.path.ends_with(".java");
        let kind_ok = match kind {
            1 => code,
            2 => !code,
            _ => true,
        };
        kind_ok && (needle.is_empty() || change.path.to_lowercase().contains(&needle))
    };
    let shown: Vec<usize> = (0..state.changes.len()).filter(|i| wanted(&state.changes[*i])).collect();

    let (mut added, mut removed, mut counts) = (0, 0, [0usize; 3]);
    for index in &shown {
        let change = &state.changes[*index];
        added += change.added;
        removed += change.removed;
        counts[change.status as usize] += 1;
    }
    widgets.summary.set_visible(state.pair.is_some());
    widgets.summary.set_markup(&format!(
        "{} files  ·  <span foreground=\"#3fb950\">+{}</span>  <span foreground=\"#f85149\">−{}</span>\n<span foreground=\"#d29922\">{} changed</span>  ·  <span foreground=\"#3fb950\">{} added</span>  ·  <span foreground=\"#f85149\">{} removed</span>",
        grouped(shown.len()),
        grouped(added),
        grouped(removed),
        grouped(counts[Status::Modified as usize]),
        grouped(counts[Status::Added as usize]),
        grouped(counts[Status::Removed as usize]),
    ));

    let files = &widgets.files;
    files.remove_all();
    state.rows.clear();
    if needle.is_empty() {
        let mut root = Dir::default();
        for index in shown {
            root.add(&state.changes[index].path, index);
        }
        walk(state, widgets, &root, "", 0);
    } else {
        for index in shown {
            let change = &state.changes[index];
            let (dir, name) = change.path.rsplit_once('/').unwrap_or(("", &change.path));
            files.append(&file_row(change, name, Some(short_dir(dir)), 0));
            state.rows.push(Target::File(index));
        }
    }
    if let Some(row) = state.selected.and_then(|s| state.rows.iter().position(|t| matches!(t, Target::File(i) if *i == s))) {
        files.select_row(files.row_at_index(row as i32).as_ref());
    }
}

/// `net/minecraft/world/entity` as `world/entity`, the way the class tree shows packages.
fn short_dir(dir: &str) -> &str {
    dir.strip_prefix("net/minecraft/").or_else(|| dir.strip_prefix("data/minecraft/")).unwrap_or(dir)
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: Vec<(String, usize)>,
    total: usize,
}

impl Dir {
    fn add(&mut self, path: &str, index: usize) {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().unwrap_or(path);
        let mut dir = self;
        dir.total += 1;
        for part in parts {
            dir = dir.dirs.entry(part.to_owned()).or_default();
            dir.total += 1;
        }
        dir.files.push((name.to_owned(), index));
    }
}

fn walk(state: &mut State, widgets: &Widgets, dir: &Dir, path: &str, depth: usize) {
    for (name, child) in &dir.dirs {
        // A folder that holds only one other folder is shown together with it.
        let mut label_text = name.clone();
        let mut full = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
        let mut node = child;
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (next, inner) = node.dirs.iter().next().expect("checked the length");
            label_text = format!("{label_text}/{next}");
            full = format!("{full}/{next}");
            node = inner;
        }
        let open = state.open.contains(&full);
        widgets.files.append(&dir_row(&label_text, node.total, open, depth));
        state.rows.push(Target::Dir(full.clone()));
        if open {
            walk(state, widgets, node, &full, depth + 1);
        }
    }
    for (name, index) in &dir.files {
        widgets.files.append(&file_row(&state.changes[*index], name, None, depth));
        state.rows.push(Target::File(*index));
    }
}

fn dir_row(name: &str, count: usize, open: bool, depth: usize) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    cell.set_margin_top(3);
    cell.set_margin_bottom(3);
    cell.set_margin_start(6 + 14 * depth as i32);
    cell.set_margin_end(8);
    let marker = colored(if open { "chevron-down" } else { "chevron-right" }, icons::FAINT);
    marker.set_pixel_size(13);
    cell.append(&marker);
    let folder = colored("folder", icons::FAINT);
    folder.set_pixel_size(14);
    cell.append(&folder);
    let text = label(name, "tree-package");
    text.set_hexpand(true);
    cell.append(&text);
    cell.append(&label(&count.to_string(), "tree-count"));
    cell
}

fn file_row(change: &FileChange, name: &str, dir: Option<&str>, depth: usize) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    cell.set_margin_top(3);
    cell.set_margin_bottom(3);
    cell.set_margin_start(6 + 14 * depth as i32 + if dir.is_some() { 0 } else { 19 });
    cell.set_margin_end(8);
    let letter = label(&change.status.letter().to_string(), "status-letter");
    letter.add_css_class(status_class(change.status));
    letter.set_valign(gtk::Align::Center);
    cell.append(&letter);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_hexpand(true);
    let title = label(name, "tree-class");
    if change.status == Status::Removed {
        title.add_css_class("removed-file");
    }
    text.append(&title);
    if let Some(dir) = dir {
        text.append(&label(dir, "tree-count"));
    }
    cell.append(&text);
    let counts = label("", "file-counts");
    counts.set_use_markup(true);
    counts.set_markup(&if change.binary { "<span foreground=\"#656c76\">bin</span>".to_owned() } else { count_markup(change) });
    counts.set_valign(gtk::Align::Center);
    cell.append(&counts);
    cell
}

fn count_markup(change: &FileChange) -> String {
    let mut parts = Vec::new();
    if change.added > 0 {
        parts.push(format!("<span foreground=\"#3fb950\">+{}</span>", grouped(change.added)));
    }
    if change.removed > 0 {
        parts.push(format!("<span foreground=\"#f85149\">−{}</span>", grouped(change.removed)));
    }
    parts.join(" ")
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Added => "added",
        Status::Removed => "removed",
        Status::Modified => "modified",
    }
}

/// What a click on row `index` of the list does: opens or closes a folder, or returns the
/// change to show.
pub fn activate(state: &mut State, widgets: &Widgets, index: usize) -> Option<usize> {
    match state.rows.get(index)?.clone() {
        Target::Dir(path) => {
            if !state.open.remove(&path) {
                state.open.insert(path);
            }
            let adjustment = widgets.files.parent().and_then(|p| p.parent()).and_downcast::<gtk::ScrolledWindow>().map(|s| s.vadjustment());
            let before = adjustment.as_ref().map(|a| a.value());
            show_files(state, widgets);
            if let (Some(adjustment), Some(before)) = (adjustment, before) {
                gtk::glib::idle_add_local_once(move || adjustment.set_value(before));
            }
            None
        }
        Target::File(change) => Some(change),
    }
}

/// The change after or before the selected one, in list order, opening folders on the way.
pub fn step(state: &mut State, forward: bool) -> Option<usize> {
    let order: Vec<usize> = {
        let mut sorted: Vec<usize> = (0..state.changes.len()).collect();
        sorted.sort_by(|a, b| state.changes[*a].path.cmp(&state.changes[*b].path));
        sorted
    };
    let at = state.selected.and_then(|s| order.iter().position(|i| *i == s));
    let next = match (at, forward) {
        (None, _) => order.first().copied(),
        (Some(at), true) => order.get(at + 1).copied(),
        (Some(at), false) => at.checked_sub(1).and_then(|at| order.get(at).copied()),
    }?;
    reveal(state, &state.changes[next].path.clone());
    Some(next)
}

/// Opens every folder on the way to `path`. Folded chains open with any of their folders.
fn reveal(state: &mut State, path: &str) {
    let mut end = 0;
    for part in path.split('/') {
        if end + part.len() >= path.len() {
            break;
        }
        end += part.len();
        state.open.insert(path[..end].to_owned());
        end += 1;
    }
}

/// Shows the diff of one change.
pub fn show_diff(state: &mut State, widgets: &Widgets, index: usize) {
    let Some((_, _, old, new)) = &state.pair else { return };
    let Some(change) = state.changes.get(index) else { return };
    state.selected = Some(index);
    widgets.stack.set_visible_child_name("diff");
    widgets.status.set_text(match change.status {
        Status::Added => "Added",
        Status::Removed => "Removed",
        Status::Modified => "Modified",
    });
    for class in ["added", "removed", "modified"] {
        widgets.status.remove_css_class(class);
    }
    widgets.status.add_css_class(status_class(change.status));
    widgets.path.set_text(&change.path);
    widgets.counts.set_markup(&count_markup(change));

    let binary = change.binary;
    state.file = (!binary).then(|| {
        let read = |root: &PathBuf| std::fs::read_to_string(root.join(&change.path)).unwrap_or_default();
        let (before, after) = (read(old), read(new));
        FileView {
            java: change.path.ends_with(".java"),
            hunks: lantern_core::compare::diff_text(&before, &after, 3),
            old: before.lines().map(str::to_owned).collect(),
            headers: Vec::new(),
        }
    });
    if binary {
        set_lines(&widgets.unified, &[(String::new(), "A file that is not text, its bytes differ.".to_owned(), Vec::new(), None)], false);
        widgets.views.set_visible_child_name("unified");
    } else {
        render(state, widgets);
    }
    for view in [&widgets.unified, &widgets.left] {
        if let Some(parent) = scroller(view) {
            parent.vadjustment().set_value(0.0);
        }
    }

    if let Some(row) = state.rows.iter().position(|t| matches!(t, Target::File(i) if *i == index)).and_then(|r| widgets.files.row_at_index(r as i32)) {
        widgets.files.select_row(Some(&row));
        // Focus scrolls the list to it, after the rows of opened folders are laid out.
        gtk::glib::idle_add_local_once(move || {
            row.grab_focus();
        });
    }
}

/// Draws the file in the mode that is picked.
fn render(state: &mut State, widgets: &Widgets) {
    let Some(file) = &mut state.file else { return };
    let split = widgets.split.is_active();
    widgets.views.set_visible_child_name(if split { "split" } else { "unified" });
    if split {
        let (left, right, headers) = split_lines(file);
        set_lines(&widgets.left, &left, file.java);
        set_lines(&widgets.right, &right, file.java);
        file.headers = headers;
    } else {
        let (lines, headers) = unified_lines(file);
        set_lines(&widgets.unified, &lines, file.java);
        file.headers = headers;
    }
}

/// Redraws in the other mode.
pub fn redraw(state: &mut State, widgets: &Widgets) {
    render(state, widgets);
}

/// A click on line `line` of the diff: on a hunk header it opens the gap above it, on the last
/// row the rest of the file. The view stays where it was.
pub fn click(state: &mut State, widgets: &Widgets, line: usize) {
    let Some(file) = &mut state.file else { return };
    let Some(&(_, gap)) = file.headers.iter().find(|(at, _)| *at == line) else { return };
    if hidden(file, gap) == 0 {
        return;
    }
    open_gap(file, gap);
    let scroll = [&widgets.unified, &widgets.left].map(|v| scroller(v).map(|s| s.vadjustment()));
    let before: Vec<Option<f64>> = scroll.iter().map(|a| a.as_ref().map(|a| a.value())).collect();
    render(state, widgets);
    for (adjustment, value) in scroll.into_iter().zip(before) {
        if let (Some(adjustment), Some(value)) = (adjustment, value) {
            gtk::glib::idle_add_local_once(move || adjustment.set_value(value));
        }
    }
}

/// Unchanged lines the hunks leave out above hunk `gap`, or after the last one.
fn hidden(file: &FileView, gap: usize) -> usize {
    let next = |h: &Hunk| h.old_start + h.old_len;
    match file.hunks.get(gap) {
        Some(hunk) if gap == 0 => hunk.old_start.saturating_sub(1),
        Some(hunk) => hunk.old_start.saturating_sub(next(&file.hunks[gap - 1])),
        None => file.hunks.last().map_or(0, |last| (file.old.len() + 1).saturating_sub(next(last))),
    }
}

/// Puts the unchanged lines of a gap into the hunks, joining the two around it.
fn open_gap(file: &mut FileView, gap: usize) {
    let count = hidden(file, gap);
    let (old_from, new_from) = match gap.checked_sub(1).and_then(|k| file.hunks.get(k)) {
        Some(previous) => (previous.old_start + previous.old_len, previous.new_start + previous.new_len),
        None => (1, 1),
    };
    let lines: Vec<Line> = (0..count)
        .map(|i| Line {
            kind: LineKind::Context,
            old: Some(old_from + i),
            new: Some(new_from + i),
            text: file.old.get(old_from + i - 1).cloned().unwrap_or_default(),
            emphasis: Vec::new(),
        })
        .collect();
    if gap == file.hunks.len() {
        let Some(last) = file.hunks.last_mut() else { return };
        last.lines.extend(lines);
        last.old_len += count;
        last.new_len += count;
    } else if gap == 0 {
        let first = &mut file.hunks[0];
        first.lines.splice(0..0, lines);
        first.old_start = 1;
        first.new_start = 1;
        first.old_len += count;
        first.new_len += count;
    } else {
        let hunk = file.hunks.remove(gap);
        let previous = &mut file.hunks[gap - 1];
        previous.lines.extend(lines);
        previous.lines.extend(hunk.lines);
        previous.old_len = hunk.old_start + hunk.old_len - previous.old_start;
        previous.new_len = hunk.new_start + hunk.new_len - previous.new_start;
    }
}

/// `@@ -6,12 +6,12 @@` and how much there is to open above it.
fn header(file: &FileView, gap: usize) -> String {
    let more = match hidden(file, gap) {
        0 => String::new(),
        1 => "     ⋯  1 hidden line, click to show".to_owned(),
        n => format!("     ⋯  {n} hidden lines, click to show"),
    };
    match file.hunks.get(gap) {
        Some(h) => format!("@@ -{},{} +{},{} @@{more}", h.old_start, h.old_len, h.new_start, h.new_len),
        None => format!("⋯  {} more lines to the end of the file, click to show", hidden(file, gap)),
    }
}

/// One line of a diff view: the gutter, the text, the changed byte ranges in the text and
/// what kind of line it is. None is a hunk header, a filler has an empty gutter and text.
type ViewLine = (String, String, Vec<(usize, usize)>, Option<Row>);

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Context,
    Added,
    Removed,
    Filler,
}

fn number(n: Option<usize>, width: usize) -> String {
    n.map_or_else(|| " ".repeat(width), |n| format!("{n:>width$}"))
}

fn width(hunks: &[Hunk]) -> usize {
    let last = hunks.iter().map(|h| (h.old_start + h.old_len).max(h.new_start + h.new_len)).max().unwrap_or(1);
    last.to_string().len().max(3)
}

fn unified_lines(file: &FileView) -> (Vec<ViewLine>, Vec<(usize, usize)>) {
    let w = width(&file.hunks);
    let mut out = Vec::new();
    let mut headers = Vec::new();
    for (gap, hunk) in file.hunks.iter().enumerate() {
        headers.push((out.len(), gap));
        out.push((String::new(), header(file, gap), Vec::new(), None));
        for line in &hunk.lines {
            let (sign, row) = match line.kind {
                LineKind::Context => (' ', Row::Context),
                LineKind::Added => ('+', Row::Added),
                LineKind::Removed => ('-', Row::Removed),
            };
            out.push((format!(" {} {} {sign} ", number(line.old, w), number(line.new, w)), line.text.clone(), line.emphasis.clone(), Some(row)));
        }
    }
    if hidden(file, file.hunks.len()) > 0 {
        headers.push((out.len(), file.hunks.len()));
        out.push((String::new(), header(file, file.hunks.len()), Vec::new(), None));
    }
    (out, headers)
}

/// Both sides of the hunks with their rows lined up: a run of removed lines faces the run of
/// added lines that replaced it, the shorter side padded.
fn split_lines(file: &FileView) -> (Vec<ViewLine>, Vec<ViewLine>, Vec<(usize, usize)>) {
    let w = width(&file.hunks);
    let mut headers = Vec::new();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let side = |n: Option<usize>, sign: char, text: &str, emphasis: &[(usize, usize)], row: Row| {
        (format!(" {} {sign} ", number(n, w)), text.to_owned(), emphasis.to_vec(), Some(row))
    };
    let filler = || (String::new(), String::new(), Vec::new(), Some(Row::Filler));
    for (gap, hunk) in file.hunks.iter().enumerate() {
        let header = header(file, gap);
        headers.push((left.len(), gap));
        left.push((String::new(), header.clone(), Vec::new(), None));
        right.push((String::new(), header, Vec::new(), None));
        let (mut removed, mut added): (Vec<ViewLine>, Vec<ViewLine>) = (Vec::new(), Vec::new());
        let flush = |removed: &mut Vec<ViewLine>, added: &mut Vec<ViewLine>, left: &mut Vec<ViewLine>, right: &mut Vec<ViewLine>| {
            let rows = removed.len().max(added.len());
            left.extend(removed.drain(..).chain(std::iter::repeat_with(filler)).take(rows));
            right.extend(added.drain(..).chain(std::iter::repeat_with(filler)).take(rows));
        };
        for line in &hunk.lines {
            match line.kind {
                LineKind::Removed => removed.push(side(line.old, '-', &line.text, &line.emphasis, Row::Removed)),
                LineKind::Added => added.push(side(line.new, '+', &line.text, &line.emphasis, Row::Added)),
                LineKind::Context => {
                    flush(&mut removed, &mut added, &mut left, &mut right);
                    left.push(side(line.old, ' ', &line.text, &[], Row::Context));
                    right.push(side(line.new, ' ', &line.text, &[], Row::Context));
                }
            }
        }
        flush(&mut removed, &mut added, &mut left, &mut right);
    }
    let tail = file.hunks.len();
    if hidden(file, tail) > 0 {
        headers.push((left.len(), tail));
        left.push((String::new(), header(file, tail), Vec::new(), None));
        right.push((String::new(), String::new(), Vec::new(), None));
    }
    (left, right, headers)
}

fn set_lines(view: &gtk::TextView, lines: &[ViewLine], java: bool) {
    let mut text = String::new();
    let mut tags: Vec<(i32, i32, &'static str)> = Vec::new();
    let mut at = 0i32;
    let chars = |s: &str| s.chars().count() as i32;
    for (gutter, body, emphasis, row) in lines {
        let start = at;
        text.push_str(gutter);
        let gutter_end = at + chars(gutter);
        // The sign is the last thing in the gutter before its space.
        match row {
            Some(Row::Added) => tags.push((gutter_end - 2, gutter_end - 1, "df-add-sign")),
            Some(Row::Removed) => tags.push((gutter_end - 2, gutter_end - 1, "df-remove-sign")),
            _ => {}
        }
        tags.push((start, gutter_end, "df-gutter"));
        text.push_str(body);
        for (from, to) in emphasis {
            let from = gutter_end + chars(&body[..*from]);
            let to = gutter_end + chars(&body[..*to]);
            tags.push((from, to, if *row == Some(Row::Added) { "df-add-word" } else { "df-remove-word" }));
        }
        text.push('\n');
        at = gutter_end + chars(body) + 1;
        let line_tag = match row {
            None => Some("df-hunk"),
            Some(Row::Added) => Some("df-add"),
            Some(Row::Removed) => Some("df-remove"),
            Some(Row::Filler) => Some("df-filler"),
            Some(Row::Context) => None,
        };
        if let Some(tag) = line_tag {
            tags.push((start, at, tag));
        }
    }
    // As wide as the longest line, see `full_width`. The font is monospaced, the line with the
    // most characters is the widest.
    if let Some(longest) = text.lines().max_by_key(|line| line.chars().count()) {
        let (width, _) = view.create_pango_layout(Some(longest)).pixel_size();
        view.set_size_request(width + view.left_margin() + view.right_margin() + 24, -1);
    }
    let buffer = view.buffer();
    buffer.set_text(&text);
    if java {
        crate::theme::highlight::apply(&buffer);
    }
    // Over the code colours, which `apply` may have just added to the table.
    let table = buffer.tag_table();
    for (name, _) in TAGS {
        if let Some(tag) = table.lookup(name) {
            tag.set_priority(table.size() - 1);
        }
    }
    for (from, to, tag) in tags {
        buffer.apply_tag_by_name(tag, &buffer.iter_at_offset(from), &buffer.iter_at_offset(to));
    }
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
