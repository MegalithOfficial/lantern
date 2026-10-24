//! Lookup: running a lookup and showing what it found.

use super::*;

impl App {
    pub(super) fn on_lookup(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, _root: &gtk::Window) {
        match message {
            Msg::Lookup => {
                let text = widgets.lookup_entry.text().trim().to_owned();
                if text.is_empty() {
                    return;
                }
                let query = match widgets.lookup_kind.get() {
                    0 => Query::Topic { seed: text, depth: 1 },
                    1 => Query::Class { name: text },
                    2 => Query::Callers { target: text },
                    3 => Query::Callees { target: text },
                    4 => Query::Refs { target: text },
                    5 => Query::Key { ident: text },
                    6 => Query::Strings { pattern: text },
                    7 => Query::Search { query: text, limit: 40 },
                    _ => match text.split_once(['.', '#']) {
                        Some((class, method)) => Query::Source { class: class.to_owned(), method: Some(method.to_owned()) },
                        None => Query::Source { class: text, method: None },
                    },
                };
                let db_path = self.db_path.clone();
                sender.spawn_oneshot_command(move || Cmd::LookedUp(jobs::lookup(&db_path, &query).map_err(|e| format!("{e:#}"))));
            }
            Msg::LookupFor(kind, text) => {
                widgets.lookup_entry.set_text(&text);
                // Picking the kind runs the lookup, when it is picked already that has to be done here.
                match widgets.lookup_kinds.row_at_index(kind as i32) {
                    Some(row) if widgets.lookup_kind.get() != kind => widgets.lookup_kinds.select_row(Some(&row)),
                    _ => sender.input(Msg::Lookup),
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_lookup_cmd(&mut self, widgets: &mut Widgets, message: Cmd, _sender: ComponentSender<Self>) {
        match message {
            Cmd::LookedUp(Ok(doc)) => {
                crate::pages::lookup::show(&widgets.lookup_out, &widgets.lookup_links, &doc);
                widgets.lookup_stack.set_visible_child_name("out");
            }
            Cmd::LookedUp(Err(err)) => {
                widgets.lookup_error.set_text(&err);
                widgets.lookup_stack.set_visible_child_name("error");
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }
}
