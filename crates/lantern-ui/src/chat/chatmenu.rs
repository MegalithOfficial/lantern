//! The right click menu of the chat. One gesture on the whole conversation looks at what is
//! under the pointer and offers what makes sense there.

use gtk::{gdk, gio, glib, prelude::*};
use relm4::ComponentSender;

use crate::app::{App, Msg};

/// The text of everything inside a widget, in reading order. Buttons and the header of a code
/// block are chrome, not content.
fn text_of(widget: &gtk::Widget, out: &mut String) {
    if widget.is::<gtk::Button>() || widget.has_css_class("md-code-head") || widget.has_css_class("md-marker") {
        return;
    }
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        out.push_str(&label.text());
        out.push('\n');
    } else if let Some(view) = widget.downcast_ref::<gtk::TextView>() {
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        out.push_str(&buffer.text(&start, &end, false));
        out.push('\n');
    } else {
        let mut child = widget.first_child();
        while let Some(current) = child {
            text_of(&current, out);
            child = current.next_sibling();
        }
    }
}

fn ancestor_with_class(widget: &gtk::Widget, class: &str) -> Option<gtk::Widget> {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if widget.has_css_class(class) {
            return Some(widget);
        }
        current = widget.parent();
    }
    None
}

/// The link a point of a label is on, if any.
fn symbol_at(label: &gtk::Label, x: f64, y: f64) -> Option<String> {
    let layout = label.layout();
    let (offset_x, offset_y) = label.layout_offsets();
    let (inside, index, _) = layout.xy_to_index((x as i32 - offset_x) * gtk::pango::SCALE, (y as i32 - offset_y) * gtk::pango::SCALE);
    if !inside {
        return None;
    }
    // Code spans are drawn in a monospace font, the word under the pointer is the symbol.
    let text = label.text();
    let bytes = text.as_bytes();
    let index = (index as usize).min(bytes.len());
    let is_part = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'.' | b'/');
    let start = (0..index).rev().take_while(|i| is_part(bytes[*i])).last().unwrap_or(index);
    let end = (index..bytes.len()).take_while(|i| is_part(bytes[*i])).last().map_or(index, |i| i + 1);
    let word = text.get(start..end)?.trim_matches('.');
    crate::chat::markdown::looks_like_symbol(word).then(|| word.to_owned())
}

pub fn popup(chat_box: &gtk::Box, sender: &ComponentSender<App>, x: f64, y: f64) {
    let Some(picked) = chat_box.pick(x, y, gtk::PickFlags::DEFAULT) else { return };
    // The message is the child of the conversation the pointer is in.
    let mut message = picked.clone();
    while let Some(parent) = message.parent() {
        if parent == *chat_box.upcast_ref::<gtk::Widget>() {
            break;
        }
        message = parent;
    }
    if message == *chat_box.upcast_ref::<gtk::Widget>() {
        return;
    }

    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();
    let clipboard = chat_box.clipboard();
    let add = |section: &gio::Menu, title: &str, name: &str, run: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run());
        actions.add_action(&action);
        section.append(Some(title), Some(&format!("msg.{name}")));
    };

    let first = gio::Menu::new();
    let label = picked.downcast_ref::<gtk::Label>().cloned().or_else(|| picked.parent().and_downcast::<gtk::Label>());
    if let Some(label) = &label {
        if let Some((from, to)) = label.selection_bounds() {
            let selected: String = label.text().chars().skip(from as usize).take((to - from) as usize).collect();
            let clipboard = clipboard.clone();
            add(&first, "Copy selection", "copy-selection", Box::new(move || clipboard.set_text(&selected)));
        }
        if let Some(point) = chat_box.compute_point(label, &gtk::graphene::Point::new(x as f32, y as f32)) {
            if let Some(symbol) = symbol_at(label, f64::from(point.x()), f64::from(point.y())) {
                let sender = sender.clone();
                add(&first, &format!("Open {symbol} in Browse"), "open-symbol", Box::new(move || sender.input(Msg::OpenSymbol(symbol.clone()))));
            }
        }
    }
    if let Some(code) = ancestor_with_class(&picked, "md-code") {
        let mut text = String::new();
        text_of(&code, &mut text);
        let clipboard = clipboard.clone();
        add(&first, "Copy code", "copy-code", Box::new(move || clipboard.set_text(text.trim_end())));
    } else if let Some(label) = &label {
        let text = label.text().to_string();
        let clipboard = clipboard.clone();
        add(&first, "Copy paragraph", "copy-paragraph", Box::new(move || clipboard.set_text(&text)));
    }
    menu.append_section(None, &first);

    let second = gio::Menu::new();
    let mut whole = String::new();
    text_of(&message, &mut whole);
    if !whole.trim().is_empty() {
        let clipboard = clipboard.clone();
        add(&second, "Copy message", "copy-message", Box::new(move || clipboard.set_text(whole.trim_end())));
    }
    if let Some(label) = label {
        add(&second, "Select all", "select-all", Box::new(move || label.select_region(0, -1)));
    }
    menu.append_section(None, &second);
    if actions.list_actions().is_empty() {
        return;
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.insert_action_group("msg", Some(&actions));
    popover.set_parent(chat_box);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    // Parented by hand, so it has to be taken off by hand, after the action has run.
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

pub fn install(chat_box: &gtk::Box, sender: &ComponentSender<App>) {
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    // Before the labels get it, their own menu is what did not work.
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (target, sender) = (chat_box.clone(), sender.clone());
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        popup(&target, &sender, x, y);
    });
    chat_box.add_controller(click);
}
