//! Browse: the class tree, the open source panes, the members and calls of a class, Ask, and the history of where the view has been.

use super::*;

impl App {
    pub(super) fn on_browse(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, root: &gtk::Window) {
        match message {
            Msg::ClassSearch(text) => self.search_classes(widgets, &text),
            Msg::ClassActivated(i) => match self.classes.get(i).cloned() {
                Some(Target::Class(name)) => {
                    self.remember(widgets);
                    self.show_page(widgets, "browse");
                    self.open_class(widgets, &name, None, &sender);
                }
                Some(Target::Package(path)) => {
                    self.tree.toggle(&path);
                    self.show_tree(widgets, None);
                }
                None => {}
            },
            Msg::MemberActivated(i) => match self.members.get(i) {
                Some(Member::Method(id)) if self.method != Some(*id) => {
                    let id = *id;
                    self.remember(widgets);
                    self.select_method(widgets, id);
                }
                Some(Member::Method(id)) => self.select_method(widgets, *id),
                Some(Member::Field(id)) => self.select_field(widgets, *id),
                None => {}
            },
            Msg::CallerActivated(i) => self.follow(widgets, self.callers.get(i).cloned(), &sender),
            Msg::CalleeActivated(i) => self.follow(widgets, self.callees.get(i).cloned(), &sender),
            Msg::AnswerActivated(i) => {
                if let Some(answer) = self.answers.get(i).cloned() {
                    self.remember(widgets);
                    match answer.method {
                        Some(method) => self.follow(widgets, Some(method), &sender),
                        None => self.open_class(widgets, &answer.class, None, &sender),
                    }
                }
            }
            Msg::OpenFile => {
                if let Some(location) = &self.location {
                    let file = gio::File::for_path(&location.path);
                    gtk::FileLauncher::new(Some(&file)).launch(Some(root), gio::Cancellable::NONE, |_| {});
                }
            }
            Msg::Ask => {
                let question = widgets.ask_entry.text().trim().to_owned();
                if question.is_empty() || self.asking {
                    return;
                }
                self.asking = true;
                widgets.inspector_stack.set_visible_child_name("results");
                fill_answers(&widgets.ask_list, []);
                self.answers.clear();
                widgets.ask_empty_title.set_text("Searching ...");
                widgets.ask_empty_help.set_text(&widgets.ask_entry.tooltip_text().unwrap_or_default());
                widgets.ask_spinner.start();
                widgets.ask_status.set_text("");
                let (shared, db_path, spec) = (self.searcher.clone(), self.db_path.clone(), self.model);
                let reranker = self.active_reranker();
                let (results, pool) = (self.settings.results, self.settings.rerank_pool);
                // A query is a single short input, more threads only add overhead. The reranker
                // reads forty methods and wants all of them.
                let threads = if reranker.is_some() { lantern_embed::threads(&self.settings) } else { lantern_embed::threads(&self.settings).min(4) };
                sender.spawn_oneshot_command(move || {
                    Cmd::Answers(jobs::ask(&shared, &db_path, spec, reranker, threads, pool, &question, results).map_err(|e| format!("{e:#}")))
                });
            }
            Msg::PaneClosed(id) => {
                widgets.panes.close(id);
                if let Some(active) = widgets.panes.active_class() {
                    self.open_class(widgets, &active, None, &sender);
                }
                return;
            }
            Msg::PaneMoved(moved, target) => {
                widgets.panes.reorder(moved, target);
                return;
            }
            Msg::PaneFocused(id) => {
                let Some(class) = widgets.panes.class_of(id) else { return };
                if widgets.panes.active_class().as_deref() == Some(class.as_str()) {
                    return;
                }
                widgets.panes.focus(id);
                self.open_class(widgets, &class, None, &sender);
                return;
            }
            Msg::OpenInPane(id, class) => {
                // Dragging is the way to ask for another pane, so it never takes over one that
                // is already there.
                self.remember(widgets);
                widgets.panes.new_pane_after(id, &sender);
                self.open_class(widgets, &class, None, &sender);
                return;
            }
            Msg::OpenInNewPane(class) => {
                widgets.panes.new_pane(&sender);
                self.remember(widgets);
                self.open_class(widgets, &class, None, &sender);
                return;
            }
            Msg::PaneSplit => {
                widgets.panes.split(&sender);
                return;
            }
            Msg::FindInFiles(needle) => {
                let (matches, files) = widgets.panes.find(&needle);
                widgets.find_status.set_text(&match (needle.is_empty(), matches) {
                    (true, _) => String::new(),
                    (false, 0) => "no match".to_owned(),
                    (false, 1) => "1 match".to_owned(),
                    (false, n) if files > 1 => format!("{n} matches in {files} files"),
                    (false, n) => format!("{n} matches"),
                });
                return;
            }
            Msg::FindNext => {
                widgets.panes.find_next();
                return;
            }
            Msg::ToggleSync(on) => {
                widgets.panes.set_sync(on);
                return;
            }
            Msg::SearchCleared => {
                widgets.inspector_stack.set_visible_child_name("members");
                widgets.ask_status.set_text("");
                widgets.ask_empty_title.set_text("Search the code by meaning");
                widgets.ask_empty_help.set_text("Type what something does, not what it is called.");
                return;
            }
            Msg::FocusSearch => {
                self.show_page(widgets, "browse");
                widgets.ask_entry.grab_focus();
                return;
            }
            Msg::OpenAt { class, method, line, site } => {
                let Some(conn) = &self.conn else { return };
                let line = site.as_ref().and_then(|site| query::source_line(conn, site)).or(line);
                self.remember(widgets);
                self.show_page(widgets, "browse");
                self.open_class(widgets, &class, method, &sender);
                if let (Some(line), Some(view)) = (line, widgets.panes.active_view()) {
                    crate::theme::highlight::line(&view.buffer(), line as i32 - 1);
                    scroll_to(&view, line);
                }
            }
            Msg::OpenPath(path) => {
                let file = gio::File::for_path(path);
                gtk::FileLauncher::new(Some(&file)).launch(Some(root), gio::Cancellable::NONE, |_| {});
            }
            Msg::OpenSymbol(symbol) => {
                let Some(conn) = &self.conn else { return };
                let name = symbol.trim_end_matches("()");
                // `Class.member` first, a dotted name that is no member is tried as a class.
                // Of several overloads the one with source wins, compiler made bridges have none
                // and would open the class without going anywhere.
                let method = name.rsplit_once('.').and_then(|(class, member)| {
                    let found = query::find_methods(conn, class, member).ok()?;
                    found.iter().find(|m| query::method_location(conn, m.id).is_some_and(|l| l.line.is_some())).or(found.first()).cloned()
                });
                let class = name.rsplit_once('.').map_or(name, |(class, _)| class);
                // A field opens the class that declares it, at the declaration.
                let field = method.is_none().then(|| name.rsplit_once('.')).flatten().and_then(|(class, member)| {
                    let (_, owner) = query::find_field(conn, class, member).ok()?;
                    let line = query::field_line(conn, &owner, member);
                    Some((owner, line))
                });
                match method {
                    Some(method) => {
                        self.remember(widgets);
                        self.show_page(widgets, "browse");
                        self.follow(widgets, Some(method), &sender);
                    }
                    None if field.is_some() => {
                        let (class, line) = field.expect("checked right above");
                        sender.input(Msg::OpenAt { class, method: None, line, site: None });
                    }
                    None if query::class_info(conn, class).is_ok() => {
                        let class = class.to_owned();
                        self.remember(widgets);
                        self.show_page(widgets, "browse");
                        self.open_class(widgets, &class, None, &sender);
                    }
                    None => {}
                }
            }
            Msg::Back | Msg::Forward => {
                let back = matches!(message, Msg::Back);
                let (from, to) = if back { (&mut self.history, &mut self.future) } else { (&mut self.future, &mut self.history) };
                let Some((page, class, method)) = from.pop() else { return };
                to.push((self.page, self.class.clone(), self.method));
                self.show_page(widgets, page);
                // Only when it differs, going back to the chat leaves Browse as it is.
                if let Some(class) = class.filter(|c| self.class.as_ref() != Some(c) || method != self.method) {
                    self.open_class(widgets, &class, method, &sender);
                }
                self.sync_history(widgets);
                return;
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_browse_cmd(&mut self, widgets: &mut Widgets, message: Cmd, _sender: ComponentSender<Self>) {
        match message {
            Cmd::Answers(result) => {
                self.asking = false;
                widgets.ask_spinner.stop();
                match result {
                    Ok(answers) => {
                        widgets.ask_status.set_text(&format!("{} RESULTS", answers.len()));
                        widgets.ask_empty_title.set_text("Nothing found");
                        widgets.ask_empty_help.set_text("Try what the code does rather than what it is called.");
                        let conn = self.conn.as_ref();
                        fill_answers(
                            &widgets.ask_list,
                            answers.iter().map(|answer| match &answer.method {
                                Some(method) => {
                                    let line = conn
                                        .and_then(|conn| query::method_location(conn, method.id))
                                        .and_then(|l| l.line)
                                        .map_or(String::new(), |l| format!("  ·  line {l}"));
                                    AnswerRow {
                                        icon: "square-function",
                                        name: format!("{}.{}", simple(&method.class), method.name),
                                        signature: signature(&method.descriptor),
                                        place: format!("{}{line}", short(method.class.rsplit_once('/').map_or("", |(package, _)| package))),
                                        snippet: one_line(&answer.snippet),
                                        score: answer
                                            .relevance
                                            .map_or_else(|| answer.similarity.map_or(String::new(), |s| format!("{s:.2}")), |r| format!("{r:.2}")),
                                    }
                                }
                                None => AnswerRow {
                                    icon: "braces",
                                    name: simple(&answer.class).to_owned(),
                                    signature: String::new(),
                                    place: short(&answer.class).to_owned(),
                                    snippet: one_line(&answer.snippet),
                                    score: answer
                                        .relevance
                                        .map_or_else(|| answer.similarity.map_or(String::new(), |s| format!("{s:.2}")), |r| format!("{r:.2}")),
                                },
                            }),
                        );
                        self.answers = answers;
                    }
                    Err(err) => widgets.ask_status.set_text(&err),
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    /// Remembers where the view is before a jump takes it somewhere else.
    pub(super) fn remember(&mut self, widgets: &Widgets) {
        let here = (self.page, self.class.clone(), self.method);
        if self.history.last() != Some(&here) {
            self.history.push(here);
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        self.future.clear();
        self.sync_history(widgets);
    }

    pub(super) fn sync_history(&self, widgets: &Widgets) {
        widgets.back.set_sensitive(!self.history.is_empty());
        widgets.forward.set_sensitive(!self.future.is_empty());
    }

    pub(super) fn show_page(&mut self, widgets: &Widgets, name: &'static str) {
        self.page = name;
        widgets.stack.set_visible_child_name(name);
        widgets.side_stack.set_visible_child_name(name);
        if let Some((_, item)) = widgets.nav.iter().find(|(page, _)| *page == name) {
            widgets.nav_syncing.set(true);
            item.set_active(true);
            widgets.nav_syncing.set(false);
        }
        self.show_crumb(widgets);
    }

    /// The top bar names the open class on Browse and the page everywhere else.
    pub(super) fn show_crumb(&self, widgets: &Widgets) {
        let (path, title) = match (&self.class, self.page) {
            (Some(class), "browse") => {
                let (package, name) = class.rsplit_once('/').unwrap_or(("", class));
                (format!("{}  /", short(package).replace('/', " / ")), name.to_owned())
            }
            (_, page) => (String::new(), view::PAGES.iter().find(|(name, _)| *name == page).map_or("", |(_, title)| title).to_owned()),
        };
        widgets.crumb_path.set_text(&path);
        widgets.crumb_path.set_visible(!path.is_empty());
        widgets.crumb_title.set_text(&title);
        widgets.open_file.set_visible(self.page == "browse" && self.location.is_some());
        widgets.chat_new.set_visible(self.page == "chat");
    }

    /// Reads the class list again, after start and after a new index.
    pub(super) fn load_tree(&mut self, widgets: &Widgets) {
        let classes = self.conn.as_ref().and_then(|conn| query::top_level_classes(conn).ok()).unwrap_or_default();
        self.tree = ClassTree::new(&classes);
        self.searching = false;
        self.show_tree(widgets, None);
    }

    /// Draws the tree. With `select` the row of that class is selected and scrolled to,
    /// otherwise the list stays where it was.
    pub(super) fn show_tree(&mut self, widgets: &Widgets, select: Option<&str>) {
        let rows = self.tree.rows();
        let adjustment = widgets.class_scroll.vadjustment();
        let before = adjustment.value();
        fill_tree(&widgets.class_list, &rows);
        self.classes = rows.into_iter().map(|row| row.target).collect();

        let selected = select.and_then(|name| self.classes.iter().position(|t| matches!(t, Target::Class(class) if class == name)));
        let list = widgets.class_list.clone();
        glib::idle_add_local_once(move || match selected.and_then(|index| list.row_at_index(index as i32)) {
            Some(row) => {
                list.select_row(Some(&row));
                row.grab_focus();
            }
            None => adjustment.set_value(before),
        });
    }

    pub(super) fn search_classes(&mut self, widgets: &Widgets, text: &str) {
        if text.trim().is_empty() {
            self.searching = false;
            self.show_tree(widgets, self.class.clone().as_deref());
            return;
        }
        self.searching = true;
        let Some(conn) = &self.conn else { return };
        let hits = query::suggest_classes(conn, text, 300).unwrap_or_default();
        fill(
            &widgets.class_list,
            hits.iter().map(|hit| {
                let (package, simple) = hit.name.rsplit_once('/').unwrap_or(("", &hit.name));
                (simple.to_owned(), short(package).to_owned())
            }),
        );
        self.classes = hits.into_iter().map(|hit| Target::Class(hit.name)).collect();
    }

    pub(super) fn open_class(&mut self, widgets: &Widgets, name: &str, method: Option<i64>, sender: &ComponentSender<Self>) {
        let Some(conn) = &self.conn else { return };
        let info = match query::class_info(conn, name) {
            Ok(info) => info,
            Err(err) => {
                eprintln!("{err:#}");
                return;
            }
        };

        let mut summary = Vec::new();
        summary.extend(info.super_class.as_ref().map(|parent| format!("extends {}", simple(parent))));
        if !info.interfaces.is_empty() {
            summary.push(format!("implements {}", info.interfaces.iter().map(|i| simple(i)).collect::<Vec<_>>().join(", ")));
        }
        if !info.subclasses.is_empty() {
            summary.push(format!("{} subclasses", info.subclasses.len()));
        }
        self.class = Some(info.name.clone());
        self.method = None;

        let text = query::class_source(conn, &info.name).unwrap_or_else(|err| format!("// {err:#}"));
        widgets.panes.show(&info.name, &text, &summary.join("  ·  "), sender);
        self.location = query::class_location(conn, &info.name);
        widgets.location.set_text(self.location.as_ref().map_or("", |l| l.path.as_str()));
        self.show_crumb(widgets);

        // Compiler generated accessors and lambda bodies are reachable through their callers.
        // Bridge methods too: they are the ones without source in a class that has some.
        let sourced = info.methods.iter().any(|m| m.has_source);
        let hidden =
            |m: &query::MethodInfo| m.name == "<clinit>" || m.name.starts_with("access$") || m.name.starts_with("lambda$") || (sourced && !m.has_source);
        let methods = info.methods.iter().filter(|m| !hidden(m)).map(|m| {
            let detail = m.overrides.as_ref().map_or(String::new(), |o| format!("overrides {}", simple(o)));
            let signature = signature(&m.descriptor);
            let title = match m.name.as_str() {
                "<init>" => format!("new {}", signature.trim_end_matches(" void")),
                name => format!("{name} {signature}"),
            };
            (Member::Method(m.id), title, detail)
        });
        let fields = info.fields.iter().map(|f| {
            let detail = match (&f.key, &f.constant) {
                (Some(key), _) => format!("key {key}"),
                (None, Some(constant)) => format!("= {constant}"),
                (None, None) => String::new(),
            };
            (Member::Field(f.id), f.name.clone(), detail)
        });
        let (members, rows): (Vec<_>, Vec<_>) = methods
            .chain(fields)
            .map(|(member, title, detail)| {
                let (icon, color) = match member {
                    Member::Method(_) => ("square-function", icons::PURPLE),
                    Member::Field(_) => ("box", icons::BLUE),
                };
                (member, (icon, color, title, detail))
            })
            .unzip();
        fill_icons(&widgets.member_list, rows);
        self.members = members;
        self.callers.clear();
        self.callees.clear();
        widgets.callers_title.set_text("CALLERS");
        widgets.callers_list.remove_all();
        widgets.callees_list.remove_all();

        // Inner classes live in the file of their outer class, that is the one the tree has.
        let outer = name.split('$').next().unwrap_or(name).to_owned();
        if !self.searching && !self.classes.iter().any(|t| matches!(t, Target::Class(class) if *class == outer)) {
            self.tree.reveal(&outer);
            self.show_tree(widgets, Some(&outer));
        }

        if let Some(id) = method {
            if let Some(row) = self.members.iter().position(|m| matches!(m, Member::Method(other) if *other == id)) {
                widgets.member_list.select_row(widgets.member_list.row_at_index(row as i32).as_ref());
            }
            self.select_method(widgets, id);
        }
    }

    pub(super) fn select_method(&mut self, widgets: &Widgets, id: i64) {
        self.method = Some(id);
        let Some(conn) = &self.conn else { return };
        if let Some(line) = query::method_location(conn, id).and_then(|l| l.line) {
            let lines = query::method_source(conn, id).ok().flatten().map_or(1, |text| text.lines().count());
            if let Some(view) = widgets.panes.active_view() {
                crate::theme::highlight::focus(&view.buffer(), line as i32 - 1, lines as i32);
                scroll_to(&view, line);
            }
        }

        let mut callers = query::callers(conn, id).unwrap_or_default();
        let mut callees = query::callees(conn, id).unwrap_or_default();
        let (caller_count, callee_count) = (callers.len(), callees.len());
        callers.truncate(MAX_ROWS);
        callees.truncate(MAX_ROWS);
        let row = |method: &MethodRef, kind: &str, line: Option<i64>| {
            let (package, class) = method.class.rsplit_once('/').unwrap_or(("", &method.class));
            let at = line.map_or(String::new(), |l| format!("  ·  line {l}"));
            let openable = query::method_location(conn, method.id).is_some();
            (format!("{class}.{}", method.name), format!("{}{at}  ·  {kind}", short(package)), openable)
        };
        widgets.callers_title.set_text(&format!("CALLERS  {caller_count}"));
        let cut = |row: (String, String)| (row.0, row.1, false);
        fill_calls(&widgets.callers_list, callers.iter().map(|c| row(&c.caller, &c.kind, c.line)).chain(more(caller_count).map(cut)));
        fill_calls(&widgets.callees_list, callees.iter().map(|c| row(&c.callee, &c.kind, c.line)).chain(more(callee_count).map(cut)));
        self.callers = callers.into_iter().map(|c| c.caller).collect();
        self.callees = callees.into_iter().map(|c| c.callee).collect();
    }

    /// A field has no calls, the callers list shows who reads and writes it instead.
    pub(super) fn select_field(&mut self, widgets: &Widgets, id: i64) {
        let Some(conn) = &self.conn else { return };
        let mut uses = query::field_uses(conn, id).unwrap_or_default();
        let use_count = uses.len();
        uses.truncate(MAX_ROWS);
        widgets.callers_title.set_text(&format!("READS AND WRITES  {use_count}"));
        fill(
            &widgets.callers_list,
            uses.iter()
                .map(|u| {
                    let at = u.line.map_or(String::new(), |l| format!("line {l}, "));
                    (format!("{}.{}", short(&u.method.class), u.method.name), format!("{at}{}", if u.is_write { "write" } else { "read" }))
                })
                .chain(more(use_count)),
        );
        widgets.callees_list.remove_all();
        self.callers = uses.into_iter().map(|u| u.method).collect();
        self.callees.clear();
    }

    pub(super) fn follow(&mut self, widgets: &Widgets, method: Option<MethodRef>, sender: &ComponentSender<Self>) {
        // A method without source is in the JDK or a library, there is nothing to show.
        let method = method.filter(|method| self.conn.as_ref().is_some_and(|conn| query::method_location(conn, method.id).is_some()));
        if let Some(method) = method {
            self.remember(widgets);
            self.open_class(widgets, &method.class, Some(method.id), sender);
        }
    }
}

/// A list row is a widget. Something like Entity.getX has tens of thousands of callers and
/// building a row for each locks the window up for a long time.
pub(super) const MAX_ROWS: usize = 300;

/// The row that says a list was cut short. It sits past the end of the data, clicking it
/// does nothing.
pub(super) fn more(total: usize) -> Option<(String, String)> {
    (total > MAX_ROWS).then(|| (format!("and {} more", total - MAX_ROWS), "Lookup has the full list".to_owned()))
}

/// A snippet on one line, short enough to read at a glance.
pub(super) fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(150) {
        Some((cut, _)) => format!("{} ...", joined[..cut].trim_end()),
        None => joined,
    }
}

pub(super) fn simple(class: &str) -> &str {
    class.rsplit('/').next().unwrap_or(class)
}

pub(super) fn scroll_to(view: &gtk::TextView, line: i64) {
    let buffer = view.buffer();
    let Some(iter) = buffer.iter_at_line((line - 1).max(0) as i32) else { return };
    buffer.place_cursor(&iter);
    // Through a mark and more than once: the text was possibly set a moment ago, line heights
    // are worked out in the background and an early scroll lands in the wrong place.
    for delay in [0, 120, 400] {
        let view = view.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            view.scroll_to_mark(&view.buffer().get_insert(), 0.0, true, 0.0, 0.12);
        });
    }
}
