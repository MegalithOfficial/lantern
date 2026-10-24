//! The first run setup.

use super::*;

impl App {
    pub(super) fn on_onboarding(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, _root: &gtk::Window) {
        match message {
            Msg::Onboarding(action) => self.onboarding_action(widgets, action, &sender),
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_onboarding_cmd(&mut self, widgets: &mut Widgets, message: Cmd, sender: ComponentSender<Self>) {
        match message {
            Cmd::Setup(event) => {
                if crate::pages::onboarding::event(&mut self.onboarding, &widgets.onboarding, event, &sender) {
                    self.finish_onboarding(widgets, &sender);
                } else if self.onboarding.failed.is_some() {
                    self.job = Job::Idle;
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    /// The first run setup instead of the app, from its first step.
    pub(super) fn show_onboarding(&mut self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        crate::pages::onboarding::restart(&mut self.onboarding, &widgets.onboarding);
        crate::pages::onboarding::fill_versions(&widgets.onboarding, &self.remote, sender, None);
        widgets.top.set_visible_child_name("onboarding");
        sender.input(Msg::RemoteRefresh);
    }

    pub(super) fn onboarding_action(&mut self, widgets: &mut Widgets, action: crate::pages::onboarding::Action, sender: &ComponentSender<Self>) {
        use crate::pages::onboarding::{self, Action, Task};
        let board = &widgets.onboarding;
        match action {
            Action::Filter | Action::Picked => onboarding::fill_versions(board, &self.remote, sender, None),
            Action::Back => {
                onboarding::step(&mut self.onboarding, board, false, &self.remote, &self.data_dir);
            }
            // While embedding, Next is "Skip embedding".
            Action::Next if matches!(self.onboarding.running.and_then(|i| self.onboarding.tasks.get(i)), Some(Task::Embed(..))) => {
                self.stop.store(true, Ordering::Relaxed);
                self.onboarding.skipping = true;
                board.next.set_sensitive(false);
                board.next.set_label("Stopping ...");
            }
            Action::Next => {
                if self.onboarding.running.is_some() || !onboarding::step(&mut self.onboarding, board, true, &self.remote, &self.data_dir) {
                    return;
                }
                if self.job != Job::Idle {
                    return;
                }
                // The chat is saved right away, it needs nothing downloaded.
                if let Some((provider, address, key, model)) = onboarding::chat(board) {
                    let provider = &settings::PROVIDERS[provider];
                    self.settings.chat.provider = provider.id.to_owned();
                    let account = self.settings.chat.accounts.entry(provider.id.to_owned()).or_default();
                    account.api_key = key;
                    account.model = model;
                    account.base_url = if address == provider.base_url { String::new() } else { address };
                    self.load_settings(widgets);
                }
                let board = &widgets.onboarding;
                self.settings.embedding_model = onboarding::embedding(board).id.to_owned();
                self.settings.reranker = onboarding::reranker(board).id.to_owned();
                self.save_settings(widgets);
                let tasks = onboarding::plan(&widgets.onboarding, &self.remote, &self.data_dir);
                onboarding::start(&mut self.onboarding, &widgets.onboarding, tasks);
                self.run_setup(0, sender);
            }
            Action::Retry => {
                let Some(from) = self.onboarding.failed.take() else { return };
                onboarding::reset_footer(board);
                self.run_setup(from, sender);
            }
            Action::Leave => self.finish_onboarding(widgets, sender),
        }
    }

    pub(super) fn run_setup(&mut self, from: usize, sender: &ComponentSender<Self>) {
        self.job = Job::Onboarding;
        self.stop.store(false, Ordering::Relaxed);
        let (tasks, data_dir, stop) = (self.onboarding.tasks.clone(), self.data_dir.clone(), self.stop.clone());
        let threads = lantern_embed::threads(&self.settings);
        sender.spawn_command(move |out| {
            lantern_embed::setup::run(&tasks, from, &data_dir, threads, &stop, |event| {
                let _ = out.send(Cmd::Setup(event));
            });
        });
    }

    /// Into the app, with the newest version that got an index open.
    pub(super) fn finish_onboarding(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>) {
        self.job = Job::Idle;
        self.settings.onboarded = true;
        if let Some(model) = lantern_embed::models::find(&self.settings.embedding_model) {
            self.model = model;
        }
        self.reranker = lantern_embed::models::find_reranker(&self.settings.reranker);
        self.save_settings(widgets);
        self.installed = versions::installed(&self.data_dir);
        let picked = crate::pages::onboarding::picked(&widgets.onboarding, &self.remote);
        let newest = picked
            .iter()
            .map(|v| versions::index_path(&self.data_dir, &v.id))
            .find(|path| path.exists())
            .or_else(|| self.installed.iter().find_map(|v| v.index.clone()));
        if let Some(index) = newest {
            self.use_index(widgets, index, sender);
        }
        self.refresh_model_state();
        self.show_overview(widgets);
        self.show_versions(widgets, sender);
        widgets.top.set_visible_child_name("main");
        self.show_page(widgets, if self.conn.is_some() { "browse" } else { "setup" });
    }
}
