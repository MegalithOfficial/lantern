//! Chat: sending questions, the turn that streams in, and the saved conversations.

use super::*;

impl App {
    pub(super) fn on_chat(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, _root: &gtk::Window) {
        match message {
            Msg::ChatOpen(index) => {
                let Some(info) = self.chats.get(index).cloned() else { return };
                if self.chatting || self.chat_id == Some(info.id) {
                    return;
                }
                let store = match self.chat_store() {
                    Ok(store) => store,
                    Err(err) => return widgets.chat_usage.set_text(&format!("{err:#}")),
                };
                let (messages, total) = match (store.load(info.id), store.usage(info.id)) {
                    (Ok(messages), Ok(total)) => (messages, total),
                    _ => return widgets.chat_usage.set_text("That chat could not be read."),
                };
                self.chat_id = Some(info.id);
                // The session is idle, nothing else holds it.
                if let Ok(mut guard) = self.chat.try_lock() {
                    match guard.as_mut() {
                        Some(session) => session.restore(messages.clone(), total),
                        None => match lantern_chat::session::Session::new(&self.db_path) {
                            Ok(mut session) => {
                                session.restore(messages.clone(), total);
                                *guard = Some(session);
                            }
                            Err(err) => return widgets.chat_usage.set_text(&format!("{err:#}")),
                        },
                    }
                }
                self.show_chat(widgets, &sender, &messages);
                self.show_usage(widgets, total);
            }
            Msg::ChatMenu(index, x, y) => {
                let Some(info) = self.chats.get(index).cloned() else { return };
                let menu = gio::Menu::new();
                menu.append(Some("Delete chat"), Some("chat.delete"));
                let actions = gio::SimpleActionGroup::new();
                let action = gio::SimpleAction::new("delete", None);
                let sender = sender.clone();
                action.connect_activate(move |_, _| sender.input(Msg::ChatDelete(index)));
                actions.add_action(&action);

                let popover = gtk::PopoverMenu::from_model(Some(&menu));
                popover.insert_action_group("chat", Some(&actions));
                popover.set_parent(&widgets.chat_list);
                popover.set_has_arrow(false);
                popover.set_halign(gtk::Align::Start);
                popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover.connect_closed(|popover| {
                    let popover = popover.clone();
                    glib::idle_add_local_once(move || popover.unparent());
                });
                popover.popup();
                let _ = info;
                return;
            }
            Msg::ChatDelete(index) => {
                let Some(info) = self.chats.get(index).cloned() else { return };
                if let Ok(store) = self.chat_store() {
                    let _ = store.delete(info.id);
                }
                if self.chat_id == Some(info.id) {
                    sender.input(Msg::ChatNew);
                }
                self.refresh_chats(widgets);
                return;
            }
            Msg::ChatSend => {
                let text = widgets.chat_entry.text().trim().to_owned();
                if text.is_empty() || self.chatting {
                    return;
                }
                widgets.chat_entry.set_text("");
                if let Some(actions) = widgets.chat_actions.take() {
                    widgets.chat_box.remove(&actions);
                }
                self.append_question(widgets, &text);
                widgets.chat_turn = widgets.chat_box.last_child();
                self.chat_question = text;
                self.start_chat(widgets, &sender, false);
            }
            Msg::ChatRegenerate => {
                if self.chatting || self.chat_question.is_empty() {
                    return;
                }
                // Everything below the question goes, the question stays where it is.
                widgets.chat_actions = None;
                while let Some(last) = widgets.chat_box.last_child().filter(|last| Some(last) != widgets.chat_turn.as_ref()) {
                    widgets.chat_box.remove(&last);
                }
                self.start_chat(widgets, &sender, !self.chat_failed);
            }
            Msg::ChatCopy => widgets.chat_box.clipboard().set_text(&self.chat_answer),
            Msg::ChatStop => self.chat_stop.store(true, Ordering::Relaxed),
            Msg::ChatNew => {
                if self.chatting {
                    return;
                }
                // Not chatting, so nothing holds the lock.
                if let Ok(mut guard) = self.chat.try_lock() {
                    *guard = None;
                }
                while let Some(child) = widgets.chat_box.first_child() {
                    widgets.chat_box.remove(&child);
                }
                widgets.chat_current = None;
                widgets.chat_live = None;
                widgets.chat_actions = None;
                widgets.chat_turn = None;
                self.chat_id = None;
                self.chat_question.clear();
                widgets.chat_list.select_row(gtk::ListBoxRow::NONE);
                self.chat_question.clear();
                widgets.chat_usage.set_text("");
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_chat_cmd(&mut self, widgets: &mut Widgets, message: Cmd, sender: ComponentSender<Self>) {
        match message {
            Cmd::Chat(event) => {
                use lantern_chat::session::Event;
                match event {
                    Event::Waiting { provider } => {
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.waiting(&provider);
                        }
                    }
                    Event::Retrying { status, seconds } => {
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.retrying(status, seconds);
                        }
                    }
                    Event::Thinking(piece) => {
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.thinking(&piece);
                        }
                    }
                    Event::Text(piece) => {
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.text();
                        }
                        self.chat_text.push_str(&piece);
                        self.chat_answer.push_str(&piece);
                        self.chat_dirty = true;
                    }
                    Event::ToolComing(name) => {
                        self.absorb_narration(widgets, &sender);
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.tool_coming(&name);
                        }
                    }
                    Event::ToolStarted { name, arguments } => {
                        self.absorb_narration(widgets, &sender);
                        let values = tool_values(&arguments);
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.tool_started(&format!("{name}  {}", values.join("  ")));
                        }
                    }
                    Event::ToolFinished { lines, failed, .. } => {
                        if let Some(turn) = &mut widgets.chat_live {
                            turn.tool_finished(&if failed { "failed".to_owned() } else { format!("{lines} lines") }, failed);
                        }
                    }
                    Event::Usage(_) => {}
                }
            }
            Cmd::ChatDone(result) => {
                let failed = result.is_err();
                self.chat_failed = failed;
                self.chatting = false;
                self.render_chat(widgets, &sender);
                let stopped = self.chat_stop.load(Ordering::Relaxed);
                if let Some(turn) = widgets.chat_live.take() {
                    turn.finish(failed, stopped);
                }
                widgets.chat_current = None;
                match result {
                    Ok(total) => {
                        self.show_usage(widgets, total);
                        self.save_chat(widgets, total);
                    }
                    // Stopping is not an error, the header already says it.
                    Err(err) if err == lantern_chat::session::STOPPED => {}
                    Err(err) => {
                        let error = view::wrapping(&err, "chat-error");
                        error.set_selectable(true);
                        widgets.chat_box.append(&error);
                    }
                }
                self.append_actions(widgets, &sender, failed);
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    /// What the model said before a tool call belongs to the work, the answer is the text
    /// after the last call.
    pub(super) fn absorb_narration(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>) {
        self.render_chat(widgets, sender);
        let narration = widgets.chat_current.take();
        self.chat_text.clear();
        self.chat_answer.clear();
        if let (Some(turn), Some(document)) = (&mut widgets.chat_live, &narration) {
            turn.absorb(document.container());
        }
    }

    /// Draws the block that is being streamed.
    pub(super) fn render_chat(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>) {
        self.chat_dirty = false;
        if self.chat_text.trim().is_empty() {
            return;
        }
        let document = widgets.chat_current.get_or_insert_with(|| {
            let block = gtk::Box::new(gtk::Orientation::Vertical, 10);
            widgets.chat_box.append(&block);
            crate::chat::markdown::Document::new(&block)
        });
        document.update(&self.chat_text, sender);
    }

    pub(super) fn chat_label(&self) -> String {
        let endpoint = self.settings.chat.endpoint();
        format!("{}  ·  {}", endpoint.provider.name, endpoint.model)
    }

    pub(super) fn chat_store(&self) -> anyhow::Result<lantern_chat::store::ChatStore> {
        lantern_chat::store::ChatStore::open(&self.data_dir)
    }

    pub(super) fn refresh_chats(&mut self, widgets: &Widgets) {
        self.chats = self.chat_store().and_then(|store| store.list()).unwrap_or_default();
        fill(
            &widgets.chat_list,
            self.chats.iter().map(|chat| {
                let model = chat.model.rsplit('/').next().unwrap_or(&chat.model);
                (chat.title.clone(), format!("{}  ·  {model}", lantern_chat::store::ago(chat.updated)))
            }),
        );
        if let Some(row) = self.chats.iter().position(|chat| Some(chat.id) == self.chat_id) {
            widgets.chat_list.select_row(widgets.chat_list.row_at_index(row as i32).as_ref());
        }
    }

    /// Writes the conversation as it now stands. Called after every answer, so a chat that is
    /// left open is on disk anyway.
    pub(super) fn save_chat(&mut self, widgets: &Widgets, total: lantern_chat::client::Usage) {
        let Ok(guard) = self.chat.try_lock() else { return };
        let Some(session) = guard.as_ref() else { return };
        let messages = session.history().to_vec();
        drop(guard);
        if messages.is_empty() {
            return;
        }
        let title = lantern_chat::store::title_of(&messages);
        let model = self.settings.chat.endpoint().model;
        match self.chat_store().and_then(|mut store| store.save(self.chat_id, &title, &model, &messages, total)) {
            Ok(id) => {
                self.chat_id = Some(id);
                self.refresh_chats(widgets);
            }
            Err(err) => eprintln!("could not save the chat: {err:#}"),
        }
    }

    pub(super) fn show_usage(&self, widgets: &Widgets, total: lantern_chat::client::Usage) {
        let share = if total.prompt > 0 { total.cached * 100 / total.prompt } else { 0 };
        let cost = total.cost.map_or(String::new(), |c| format!("  ·  ${c:.4}"));
        widgets.chat_usage.set_text(&format!(
            "{}  ·  {} tokens in, {share}% from cache  ·  {} out{cost}",
            self.chat_label(),
            grouped(total.prompt as i64),
            grouped(total.completion as i64)
        ));
    }

    pub(super) fn append_actions(&self, widgets: &mut Widgets, sender: &ComponentSender<Self>, failed: bool) {
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.append(&view::icon_button(sender, "refresh-cw", if failed { "Try again" } else { "Regenerate" }, "small", Msg::ChatRegenerate));
        if !self.chat_answer.trim().is_empty() {
            actions.append(&view::icon_button(sender, "copy", "Copy", "small", Msg::ChatCopy));
        }
        widgets.chat_box.append(&actions);
        widgets.chat_actions = Some(actions);
    }

    pub(super) fn append_question(&mut self, widgets: &Widgets, text: &str) {
        let bubble = view::wrapping(text, "bubble");
        bubble.set_selectable(true);
        bubble.set_halign(gtk::Align::End);
        widgets.chat_box.append(&bubble);
    }

    /// Draws a saved conversation: questions, what the work was, and the answers.
    pub(super) fn show_chat(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>, messages: &[serde_json::Value]) {
        while let Some(child) = widgets.chat_box.first_child() {
            widgets.chat_box.remove(&child);
        }
        widgets.chat_current = None;
        widgets.chat_live = None;
        widgets.chat_actions = None;
        widgets.chat_turn = None;
        self.chat_text.clear();
        self.chat_answer.clear();
        self.chat_failed = false;

        // How much every tool call came back with, by the id the call carries.
        let results: std::collections::HashMap<&str, usize> = messages
            .iter()
            .filter(|m| m["role"] == "tool")
            .filter_map(|m| Some((m["tool_call_id"].as_str()?, m["content"].as_str()?.lines().count())))
            .collect();

        let mut turn: Option<crate::chat::turn::Turn> = None;
        for message in messages {
            let content = message["content"].as_str().unwrap_or_default();
            match message["role"].as_str().unwrap_or_default() {
                "user" => {
                    if let Some(turn) = turn.take() {
                        turn.finish(false, false);
                    }
                    self.append_question(widgets, content);
                    self.chat_question = content.to_owned();
                }
                "assistant" => match message["tool_calls"].as_array() {
                    Some(calls) if !calls.is_empty() => {
                        let turn = turn.get_or_insert_with(|| crate::chat::turn::Turn::replayed(&widgets.chat_box, sender));
                        if !content.trim().is_empty() {
                            let block = gtk::Box::new(gtk::Orientation::Vertical, 10);
                            widgets.chat_box.append(&block);
                            crate::chat::markdown::Document::new(&block).update(content, sender);
                            turn.absorb(&block);
                        }
                        for call in calls {
                            let name = call["function"]["name"].as_str().unwrap_or_default();
                            let arguments = call["function"]["arguments"].as_str().unwrap_or_default();
                            turn.tool_started(&format!("{name}  {}", tool_values(arguments).join("  ")));
                            match call["id"].as_str().and_then(|id| results.get(id)) {
                                Some(lines) => turn.tool_finished(&format!("{lines} lines"), false),
                                None => turn.tool_finished("no result", true),
                            }
                        }
                    }
                    _ => {
                        if let Some(turn) = turn.take() {
                            turn.finish(false, false);
                        }
                        if !content.trim().is_empty() {
                            let block = gtk::Box::new(gtk::Orientation::Vertical, 10);
                            widgets.chat_box.append(&block);
                            crate::chat::markdown::Document::new(&block).update(content, sender);
                            self.chat_answer = content.to_owned();
                        }
                    }
                },
                _ => {}
            }
        }
        if let Some(turn) = turn.take() {
            turn.finish(false, false);
        }
        if !self.chat_question.is_empty() {
            self.append_actions(widgets, sender, false);
        }
        jump_to_end(widgets);
    }

    /// Sends `chat_question`. With `again` the last turn is taken back first.
    pub(super) fn start_chat(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>, again: bool) {
        self.chatting = true;
        self.chat_text.clear();
        self.chat_answer.clear();
        self.chat_stop.store(false, Ordering::Relaxed);
        widgets.chat_current = None;
        widgets.chat_live = Some(crate::chat::turn::Turn::new(&widgets.chat_box, sender));
        jump_to_end(widgets);

        let (shared, stop, db_path) = (self.chat.clone(), self.chat_stop.clone(), self.db_path.clone());
        let (settings, text) = (self.settings.chat.clone(), self.chat_question.clone());
        sender.spawn_command(move |out| {
            let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            let result = (|| {
                if guard.is_none() {
                    *guard = Some(lantern_chat::session::Session::new(&db_path)?);
                }
                let session = guard.as_mut().expect("created right above");
                if again {
                    session.rewind();
                }
                session.send(&settings, &text, &stop, |event| {
                    let _ = out.send(Cmd::Chat(event));
                })?;
                Ok::<_, anyhow::Error>(session.total)
            })();
            let _ = out.send(Cmd::ChatDone(result.map_err(|e| format!("{e:#}"))));
        });
    }
}

/// For a new question: back to the end and following again, wherever the view was.
pub(super) fn jump_to_end(widgets: &Widgets) {
    widgets.chat_follow.set(true);
    let adjustment = widgets.chat_scroll.vadjustment();
    adjustment.set_value(adjustment.upper() - adjustment.page_size());
}

/// The argument values of a tool call, for the one line a call gets.
pub(super) fn tool_values(arguments: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| value.as_object().map(|fields| fields.values().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).collect()))
        .unwrap_or_default()
}
