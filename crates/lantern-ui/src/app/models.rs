//! The Models, Search and Chat sections of Setup: downloading and picking models, embedding, and the settings.

use super::*;

impl App {
    pub(super) fn on_models(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, _root: &gtk::Window) {
        match message {
            Msg::ModelAction(i) => {
                let Some(spec) = MODELS.get(i) else { return };
                if !self.model_states.get(i).is_some_and(|state| state.0) {
                    self.start_download(widgets, &sender, spec.name, spec.source(), Pending::Model(i));
                } else {
                    self.use_model(Pending::Model(i));
                }
            }
            Msg::RerankerAction(i) => match i.checked_sub(1).and_then(|i| RERANKERS.get(i)) {
                Some(spec) if !self.reranker_states.get(i - 1).copied().unwrap_or(false) => {
                    self.start_download(widgets, &sender, spec.name, spec.source(), Pending::Reranker(i));
                }
                _ => self.use_model(Pending::Reranker(i)),
            },
            Msg::SettingsEdited => {
                let setup = &widgets.setup;
                let mut edited = self.settings.clone();
                edited.threads = setup.threads.value() as usize;
                edited.results = setup.results.value() as usize;
                edited.rerank_pool = setup.rerank_pool.value() as usize;
                let preset = settings::provider(&edited.chat.provider).base_url;
                let url = setup.chat_url.text().trim().trim_end_matches('/').to_owned();
                let account = edited.chat.accounts.entry(edited.chat.provider.clone()).or_default();
                account.base_url = if url == preset { String::new() } else { url };
                account.api_key = setup.chat_key.text().trim().to_owned();
                account.model = setup.chat_model.text().trim().to_owned();
                edited.chat.tool_rounds = setup.chat_rounds.value() as usize;
                if edited == self.settings {
                    return;
                }
                self.settings = edited;
                self.save_settings(widgets);
                if !self.chatting {
                    widgets.chat_usage.set_text("");
                }
            }
            Msg::ProviderSelected(i) => {
                let Some(provider) = settings::PROVIDERS.get(i) else { return };
                if provider.id != self.settings.chat.provider {
                    self.settings.chat.provider = provider.id.to_owned();
                    self.save_settings(widgets);
                }
                // The edits these cause read the same values back into the same account.
                self.show_account(widgets);
                if !self.chatting {
                    widgets.chat_usage.set_text("");
                }
            }
            Msg::LoadModels => {
                let endpoint = self.settings.chat.endpoint();
                widgets.setup.chat_models_status.set_visible(true);
                widgets.setup.chat_models_status.set_text(&format!("Asking {} ...", endpoint.provider.name));
                sender.spawn_oneshot_command(move || Cmd::ModelsLoaded(lantern_chat::client::list_models(&endpoint).map_err(|e| format!("{e:#}"))));
                return;
            }
            Msg::ModelPicked(id) => {
                widgets.setup.chat_model.set_text(&id);
                return;
            }
            Msg::Embed => {
                if self.job != Job::Idle {
                    return;
                }
                self.job = Job::Embed;
                self.embed_started = None;
                self.stop.store(false, Ordering::Relaxed);
                widgets.setup.job_bar.set_fraction(0.0);
                widgets.setup.job_status.set_text("Loading the model ...");
                let (db_path, spec, stop) = (self.db_path.clone(), self.model, self.stop.clone());
                let threads = lantern_embed::threads(&self.settings);
                sender.spawn_command(move |out| {
                    let result = jobs::embed(db_path, spec, threads, stop, |p| {
                        let _ = out.send(Cmd::Embedding(p));
                    });
                    let _ = out.send(Cmd::Embedded(result.map_err(|e| format!("{e:#}"))));
                });
            }
            Msg::Stop => {
                self.stop.store(true, Ordering::Relaxed);
                match self.job {
                    Job::Version => widgets.setup.version_status.set_text("Stopping ..."),
                    _ => widgets.setup.job_status.set_text("Stopping after this batch, what is done stays."),
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_models_cmd(&mut self, widgets: &mut Widgets, message: Cmd, _sender: ComponentSender<Self>) {
        match message {
            Cmd::Downloading(p) => {
                if p.total > 0 {
                    widgets.setup.job_bar.set_fraction(p.downloaded as f64 / p.total as f64);
                }
                let mb = |bytes: usize| bytes as f64 / 1_048_576.0;
                widgets.setup.job_status.set_text(&format!(
                    "{} ({} of {}), {:.0} of {:.0} MB",
                    p.file,
                    p.file_index + 1,
                    p.file_count,
                    mb(p.downloaded),
                    mb(p.total)
                ));
            }
            Cmd::Downloaded(result) => {
                self.job = Job::Idle;
                self.refresh_model_state();
                widgets.setup.job_bar.set_fraction(0.0);
                match (result, self.pending.take()) {
                    (Ok(()), Some(pending)) => {
                        self.use_model(pending);
                        widgets.setup.job_status.set_text("");
                    }
                    (Ok(()), None) => widgets.setup.job_status.set_text(""),
                    (Err(err), _) => widgets.setup.job_status.set_text(&err),
                }
            }
            Cmd::Embedding(p) => {
                if p.total > 0 {
                    widgets.setup.job_bar.set_fraction(p.done as f64 / p.total as f64);
                }
                match p.phase {
                    Phase::Chunking => {
                        widgets.setup.job_status.set_text(&format!("Cutting sources into chunks, {} of {} classes and methods", p.done, p.total))
                    }
                    Phase::Reusing => {
                        self.embed_started = None;
                        widgets.setup.job_status.set_text(&format!("Checking cached embeddings, {} of {} chunks", p.done, p.total));
                    }
                    Phase::Embedding => {
                        let (started, base) = *self.embed_started.get_or_insert((Instant::now(), p.done));
                        let seconds = started.elapsed().as_secs_f64();
                        let mut text = format!("Embedded {} of {} chunks", p.done, p.total);
                        if seconds > 5.0 && p.done > base {
                            let rate = (p.done - base) as f64 / seconds;
                            let left = (p.total - p.done) as f64 / rate / 60.0;
                            text += &format!(", {rate:.1} per second, about {left:.0} min left");
                        }
                        widgets.setup.job_status.set_text(&text);
                    }
                }
            }
            Cmd::Embedded(result) => {
                self.job = Job::Idle;
                self.refresh_model_state();
                jobs::forget(&self.searcher);
                widgets.setup.job_bar.set_fraction(0.0);
                widgets.setup.job_status.set_text(&match result {
                    Ok(r) => format!(
                        "{} chunks, {} embedded now, {} already there, {} outdated removed, {:.0}s",
                        r.chunks, r.embedded, r.reused, r.removed, r.seconds
                    ),
                    Err(err) => err,
                });
            }
            Cmd::ModelsLoaded(result) => match result {
                Ok(ids) => {
                    let setup = &widgets.setup;
                    let current = setup.chat_model.text().to_string();
                    let list: Vec<&str> = ids.iter().map(String::as_str).collect();
                    setup.chat_models.set_model(Some(&gtk::StringList::new(&list)));
                    // Without this the first entry would be picked and overwrite the model.
                    setup.chat_models.set_selected(ids.iter().position(|id| *id == current).map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
                    setup.chat_models.set_visible(true);
                    setup.chat_models_status.set_text(&format!("{} models. Not all of them can call tools.", ids.len()));
                }
                Err(err) => {
                    widgets.setup.chat_models.set_visible(false);
                    widgets.setup.chat_models_status.set_text(&err);
                }
            },
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn refresh_model_state(&mut self) {
        self.model_states = MODELS.iter().map(|m| (download::is_downloaded(m.source(), &self.data_dir), store::stored_count(&self.vectors(), m))).collect();
        self.reranker_states = RERANKERS.iter().map(|r| download::is_downloaded(r.source(), &self.data_dir)).collect();
    }

    /// The reranker Ask really runs with: the chosen one, if it is on disk.
    pub(super) fn active_reranker(&self) -> Option<&'static RerankerSpec> {
        let index = RERANKERS.iter().position(|r| Some(r.id) == self.reranker.map(|chosen| chosen.id))?;
        self.reranker_states.get(index).copied().unwrap_or(false).then_some(&RERANKERS[index])
    }

    pub(super) fn use_model(&mut self, pending: Pending) {
        match pending {
            Pending::Model(i) => {
                self.model = &MODELS[i];
                self.settings.embedding_model = self.model.id.to_owned();
            }
            Pending::Reranker(i) => {
                self.reranker = i.checked_sub(1).map(|i| &RERANKERS[i]);
                self.settings.reranker = self.reranker.map_or("off", |r| r.id).to_owned();
            }
        }
        self.refresh_model_state();
        if let Err(err) = self.settings.save(&self.data_dir) {
            eprintln!("could not save the settings: {err:#}");
        }
    }

    pub(super) fn save_settings(&self, widgets: &Widgets) {
        if let Err(err) = self.settings.save(&self.data_dir) {
            widgets.setup.job_status.set_text(&format!("Could not save the settings: {err:#}"));
        }
    }

    pub(super) fn start_download(&mut self, widgets: &Widgets, sender: &ComponentSender<Self>, name: &str, source: Source, then: Pending) {
        if self.job != Job::Idle {
            return;
        }
        self.job = Job::Download;
        self.pending = Some(then);
        widgets.setup.job_bar.set_fraction(0.0);
        widgets.setup.job_status.set_text(&format!("Downloading {name} from huggingface.co/{}", source.repo));
        let data_dir = self.data_dir.clone();
        sender.spawn_command(move |out| {
            let result = jobs::fetch_model(source, &data_dir, |p| {
                let _ = out.send(Cmd::Downloading(p));
            });
            let _ = out.send(Cmd::Downloaded(result.map_err(|e| format!("{e:#}"))));
        });
    }

    /// Puts the settings into their widgets. The edits this causes find nothing changed.
    pub(super) fn load_settings(&self, widgets: &Widgets) {
        let setup = &widgets.setup;
        setup.threads.set_value(lantern_embed::threads(&self.settings) as f64);
        setup.results.set_value(self.settings.results as f64);
        setup.rerank_pool.set_value(self.settings.rerank_pool as f64);
        setup.chat_rounds.set_value(self.settings.chat.tool_rounds as f64);
        let index = settings::PROVIDERS.iter().position(|p| p.id == self.settings.chat.provider).unwrap_or(0);
        setup.chat_provider.set_selected(index as u32);
        self.show_account(widgets);
    }

    /// Address, key and model of the chosen provider.
    pub(super) fn show_account(&self, widgets: &Widgets) {
        let setup = &widgets.setup;
        let provider = settings::provider(&self.settings.chat.provider);
        let account = self.settings.chat.account();
        setup.chat_note.set_text(provider.note);
        setup.chat_note.set_visible(!provider.note.is_empty());
        setup.chat_url.set_text(if account.base_url.is_empty() { provider.base_url } else { &account.base_url });
        setup.chat_key.set_text(&account.api_key);
        setup.chat_model.set_text(&account.model);
        setup.chat_model.set_placeholder_text(Some(provider.example_model));
        setup.chat_models.set_visible(false);
        setup.chat_models_status.set_visible(false);
    }
}
