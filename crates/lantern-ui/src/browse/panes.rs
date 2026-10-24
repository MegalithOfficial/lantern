//! The middle of Browse: the files that are open, next to each other. Panes are dragged by
//! their header to reorder them, their scrolling can be tied together, and one search runs
//! over all of them at once.
//!
//! Walking the code changes the file in the pane that is active, the way a preview tab works,
//! so following a call does not leave a trail of windows. A second pane is asked for with the
//! split button, and from then on each pane is walked on its own.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    theme::icons::{self, colored},
    view::{label, source_view},
};

/// A dragged pane says so, anything else dragged in is the name of a class.
const PANE_PREFIX: &str = "lantern-pane:";

/// Below this a pane is not worth reading, the row scrolls sideways instead.
const MIN_WIDTH: i32 = 340;

struct Pane {
    id: u64,
    class: RefCell<String>,
    root: gtk::Box,
    view: gtk::TextView,
    scroll: gtk::ScrolledWindow,
    title: gtk::Label,
    /// What the class extends and implements, next to its name.
    subtitle: gtk::Label,
    /// How many times the search text is in this file.
    count: gtk::Label,
}

#[derive(Clone)]
pub struct Panes {
    pub root: gtk::ScrolledWindow,
    files: gtk::Box,
    panes: Rc<RefCell<Vec<Pane>>>,
    active: Rc<Cell<u64>>,
    next_id: Rc<Cell<u64>>,
    sync: Rc<Cell<bool>>,
    /// Set while one pane is moving the others, so they do not move it back.
    syncing: Rc<Cell<bool>>,
    needle: Rc<RefCell<String>>,
}

impl Panes {
    pub fn new(sender: &ComponentSender<App>) -> Self {
        let files = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        files.set_homogeneous(true);
        files.set_hexpand(true);
        files.set_vexpand(true);
        let root = gtk::ScrolledWindow::builder().child(&files).hexpand(true).vexpand(true).build();
        root.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        // A class dropped beside the panes, rather than on one of them, opens next to them.
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::COPY | gdk::DragAction::MOVE);
        {
            let sender = sender.clone();
            drop.connect_drop(move |_, value, _, _| match value.get::<String>() {
                Ok(class) if !class.starts_with(PANE_PREFIX) => {
                    sender.input(Msg::OpenInNewPane(class));
                    true
                }
                _ => false,
            });
        }
        files.add_controller(drop);
        Self {
            root,
            files,
            panes: Rc::new(RefCell::new(Vec::new())),
            active: Rc::new(Cell::new(0)),
            next_id: Rc::new(Cell::new(1)),
            sync: Rc::new(Cell::new(false)),
            syncing: Rc::new(Cell::new(false)),
            needle: Rc::new(RefCell::new(String::new())),
        }
    }

    pub fn active_class(&self) -> Option<String> {
        self.class_of(self.active.get())
    }

    pub fn class_of(&self, id: u64) -> Option<String> {
        self.panes.borrow().iter().find(|pane| pane.id == id).map(|pane| pane.class.borrow().clone())
    }

    pub fn active_view(&self) -> Option<gtk::TextView> {
        let id = self.active.get();
        self.panes.borrow().iter().find(|pane| pane.id == id).map(|pane| pane.view.clone())
    }

    /// Shows a file: in the pane that already has it, otherwise in the active pane, and in a
    /// new one when nothing is open.
    pub fn show(&self, class: &str, text: &str, subtitle: &str, sender: &ComponentSender<App>) {
        if let Some(id) = self.panes.borrow().iter().find(|pane| *pane.class.borrow() == class).map(|pane| pane.id) {
            self.focus(id);
            return;
        }
        let active = self.active.get();
        let known = self.panes.borrow().iter().any(|pane| pane.id == active);
        if known {
            self.fill(active, class, text, subtitle);
            self.focus(active);
        } else {
            self.add(class, text, subtitle, sender);
        }
    }

    /// A second pane on the same file, to walk it in two places at once.
    pub fn split(&self, sender: &ComponentSender<App>) {
        let id = self.active.get();
        let (class, text, subtitle) = {
            let panes = self.panes.borrow();
            let Some(pane) = panes.iter().find(|pane| pane.id == id) else { return };
            let buffer = pane.view.buffer();
            let (start, end) = buffer.bounds();
            (pane.class.borrow().clone(), buffer.text(&start, &end, true).to_string(), pane.subtitle.text().to_string())
        };
        self.add(&class, &text, &subtitle, sender);
    }

    /// An empty pane, for a file that is dropped next to the others.
    pub fn new_pane(&self, sender: &ComponentSender<App>) {
        self.add("", "", "", sender);
    }

    /// The same, right beside the pane it was dropped on.
    pub fn new_pane_after(&self, target: u64, sender: &ComponentSender<App>) {
        self.add("", "", "", sender);
        let new = self.active.get();
        let after = self.panes.borrow().iter().position(|pane| pane.id == target).map(|index| index + 1);
        if let Some(index) = after {
            let mut panes = self.panes.borrow_mut();
            let Some(from) = panes.iter().position(|pane| pane.id == new) else { return };
            let pane = panes.remove(from);
            let at = index.min(panes.len());
            panes.insert(at, pane);
            let mut previous: Option<gtk::Box> = None;
            for pane in panes.iter() {
                self.files.reorder_child_after(&pane.root, previous.as_ref());
                previous = Some(pane.root.clone());
            }
        }
    }

    fn add(&self, class: &str, text: &str, subtitle: &str, sender: &ComponentSender<App>) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let pane = self.build(id, sender);
        self.files.append(&pane.root);
        self.panes.borrow_mut().push(pane);
        self.fill(id, class, text, subtitle);
        self.focus(id);
    }

    /// Puts a file into a pane that already exists.
    fn fill(&self, id: u64, class: &str, text: &str, subtitle: &str) {
        let panes = self.panes.borrow();
        let Some(pane) = panes.iter().find(|pane| pane.id == id) else { return };
        *pane.class.borrow_mut() = class.to_owned();
        pane.title.set_text(class.rsplit('/').next().unwrap_or(class));
        pane.title.set_tooltip_text(Some(class));
        pane.subtitle.set_text(subtitle);
        pane.view.buffer().set_text(text);
        crate::theme::highlight::apply(&pane.view.buffer());
        let found = mark_matches(&pane.view.buffer(), &self.needle.borrow());
        pane.count.set_text(&found.to_string());
        pane.count.set_visible(found > 0);
    }

    pub fn focus(&self, id: u64) {
        self.active.set(id);
        for pane in self.panes.borrow().iter() {
            let active = pane.id == id;
            pane.root.set_css_classes(if active { &["pane", "active"] } else { &["pane"] });
        }
    }

    pub fn close(&self, id: u64) {
        let mut panes = self.panes.borrow_mut();
        let Some(index) = panes.iter().position(|pane| pane.id == id) else { return };
        // The last one stays, an empty middle would only look broken.
        if panes.len() == 1 {
            return;
        }
        self.files.remove(&panes[index].root);
        panes.remove(index);
        let next = panes.get(index.min(panes.len() - 1)).map(|pane| pane.id);
        drop(panes);
        if let Some(next) = next {
            self.focus(next);
        }
    }

    /// Drops the pane `moved` at the place of `target`.
    pub fn reorder(&self, moved: u64, target: u64) {
        let mut panes = self.panes.borrow_mut();
        let (Some(from), Some(to)) = (panes.iter().position(|p| p.id == moved), panes.iter().position(|p| p.id == target)) else {
            return;
        };
        if from == to {
            return;
        }
        let pane = panes.remove(from);
        panes.insert(to, pane);
        // The row is put back in the order of the list, which is simpler than working out
        // which way the pane travelled.
        let mut previous: Option<gtk::Box> = None;
        for pane in panes.iter() {
            self.files.reorder_child_after(&pane.root, previous.as_ref());
            previous = Some(pane.root.clone());
        }
    }

    pub fn set_sync(&self, on: bool) {
        self.sync.set(on);
    }

    /// Marks every occurrence of `needle` in every open file. Returns how many there are and
    /// in how many files.
    pub fn find(&self, needle: &str) -> (usize, usize) {
        *self.needle.borrow_mut() = needle.to_owned();
        let (mut total, mut files) = (0, 0);
        for pane in self.panes.borrow().iter() {
            let found = mark_matches(&pane.view.buffer(), needle);
            pane.count.set_text(&found.to_string());
            pane.count.set_visible(found > 0);
            total += found;
            files += usize::from(found > 0);
        }
        (total, files)
    }

    /// Scrolls the active pane to the next match after the cursor, wrapping at the end.
    pub fn find_next(&self) {
        let needle = self.needle.borrow().to_lowercase();
        let Some(view) = self.active_view().filter(|_| !needle.is_empty()) else { return };
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, true).to_lowercase();
        let chars: Vec<char> = text.chars().collect();
        let pattern: Vec<char> = needle.chars().collect();
        let last = chars.len().saturating_sub(pattern.len());
        let from = (buffer.iter_at_mark(&buffer.get_insert()).offset() as usize + 1).min(last);
        let Some(offset) = (from..=last).chain(0..from).find(|offset| chars[*offset..].starts_with(&pattern)) else { return };
        buffer.place_cursor(&buffer.iter_at_offset(offset as i32));
        view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.3);
    }

    fn build(&self, id: u64, sender: &ComponentSender<App>) -> Pane {
        let view = source_view();
        view.set_editable(false);
        view.set_monospace(true);
        view.set_left_margin(10);
        view.set_top_margin(8);
        view.add_css_class("code");

        let scroll = gtk::ScrolledWindow::builder().child(&view).hexpand(true).vexpand(true).build();
        {
            // Tied scrolling moves the others by the same share of their own length, so two
            // files of different size still line up roughly.
            let (panes, sync, syncing) = (self.panes.clone(), self.sync.clone(), self.syncing.clone());
            scroll.vadjustment().connect_value_changed(move |adjustment| {
                if !sync.get() || syncing.get() {
                    return;
                }
                let span = adjustment.upper() - adjustment.page_size();
                let share = if span > 0.0 { adjustment.value() / span } else { 0.0 };
                syncing.set(true);
                for pane in panes.borrow().iter().filter(|pane| pane.id != id) {
                    let other = pane.scroll.vadjustment();
                    other.set_value(share * (other.upper() - other.page_size()).max(0.0));
                }
                syncing.set(false);
            });
        }

        let title = label("", "pane-title");
        let subtitle = label("", "pane-subtitle");
        subtitle.set_hexpand(true);
        let count = label("", "pane-count");
        count.set_visible(false);
        let split = gtk::Button::new();
        split.set_child(Some(&colored("plus", icons::FAINT)));
        split.add_css_class("pane-button");
        split.set_tooltip_text(Some("Open this file in another pane"));
        let close = gtk::Button::new();
        close.set_child(Some(&colored("x", icons::FAINT)));
        close.add_css_class("pane-button");
        close.set_tooltip_text(Some("Close"));
        for (button, message) in [(&split, Msg::PaneSplit), (&close, Msg::PaneClosed(id))] {
            let sender = sender.clone();
            button.connect_clicked(move |_| sender.input(message.clone()));
        }
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("pane-head");
        header.append(&colored("file-code", icons::MUTED));
        header.append(&title);
        header.append(&subtitle);
        header.append(&count);
        header.append(&split);
        header.append(&close);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("pane");
        root.set_size_request(MIN_WIDTH, -1);
        root.append(&header);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&scroll);

        // Dragging the header moves the pane, dropping on a pane puts it in that place.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&format!("{PANE_PREFIX}{id}").to_value())));
        header.add_controller(drag);

        // A text view brings a drop target of its own, for editing. It is the innermost widget
        // under the pointer and would take every drop, so it is taken off first.
        let controllers = view.observe_controllers();
        for index in (0..controllers.n_items()).rev() {
            if let Some(controller) = controllers.item(index).and_downcast::<gtk::EventController>() {
                if controller.is::<gtk::DropTarget>() || controller.is::<gtk::DropTargetAsync>() {
                    view.remove_controller(&controller);
                }
            }
        }
        for widget in [root.upcast_ref::<gtk::Widget>(), view.upcast_ref::<gtk::Widget>()] {
            let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk::DragAction::MOVE | gdk::DragAction::COPY);
            let sender = sender.clone();
            drop.connect_enter(|_, _, _| gdk::DragAction::COPY);
            drop.connect_drop(move |_, value, _, _| {
                let Ok(dropped) = value.get::<String>() else { return false };
                match dropped.strip_prefix(PANE_PREFIX).and_then(|id| id.parse().ok()) {
                    Some(moved) => sender.input(Msg::PaneMoved(moved, id)),
                    None => sender.input(Msg::OpenInPane(id, dropped)),
                }
                true
            });
            widget.add_controller(drop);
        }

        // Clicking anywhere in a file makes it the one the rest of the window is about.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let sender = sender.clone();
            click.connect_pressed(move |_, _, _, _| sender.input(Msg::PaneFocused(id)));
        }
        root.add_controller(click);

        Pane { id, class: RefCell::new(String::new()), root, view, scroll, title, subtitle, count }
    }
}

/// Tags every occurrence of `needle` and returns how many there were.
fn mark_matches(buffer: &gtk::TextBuffer, needle: &str) -> usize {
    if buffer.tag_table().lookup("lt-find").is_none() {
        buffer.create_tag(Some("lt-find"), &[("background", &"#5a4414"), ("foreground", &"#ffffff")]);
    }
    let (start, end) = buffer.bounds();
    buffer.remove_tag_by_name("lt-find", &start, &end);
    if needle.trim().is_empty() {
        return 0;
    }
    let text = buffer.text(&start, &end, true).to_lowercase();
    let pattern: Vec<char> = needle.to_lowercase().chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let mut found = 0;
    let mut offset = 0;
    while offset + pattern.len() <= chars.len() {
        if chars[offset..].starts_with(&pattern) {
            let from = buffer.iter_at_offset(offset as i32);
            let to = buffer.iter_at_offset((offset + pattern.len()) as i32);
            buffer.apply_tag_by_name("lt-find", &from, &to);
            found += 1;
            offset += pattern.len();
        } else {
            offset += 1;
        }
    }
    found
}
