//! The Storage section of Setup and updates: what is on disk, resetting it, and newer releases of Lantern.

use super::*;

impl App {
    pub(super) fn on_storage(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, root: &gtk::Window) {
        match message {
            Msg::CheckForUpdates => {
                if self.checking_update {
                    return;
                }
                self.checking_update = true;
                widgets.setup.update_check.set_sensitive(false);
                widgets.setup.update_status.set_text("Checking for updates...");
                widgets.setup.update_link.set_visible(false);
                sender.spawn_oneshot_command(|| Cmd::UpdateChecked(lantern_core::updates::check().map_err(|err| format!("{err:#}"))));
            }
            Msg::Reset(kind) => {
                let (message, detail, action) = match kind {
                    Reset::Vectors => ("Delete all vectors?", "Every version loses its embeddings. Ask works again once the index is embedded.", "Delete"),
                    Reset::Models => {
                        ("Delete the downloaded models?", "The embedding models and rerankers are removed, they are downloaded again when used.", "Delete")
                    }
                    Reset::Everything => (
                        "Reset Lantern?",
                        "Deletes every version with its index and vectors, the models, the chats and the settings with the API keys in them. Lantern starts over as on the first run.",
                        "Reset everything",
                    ),
                };
                let dialog = gtk::AlertDialog::builder()
                    .message(message)
                    .detail(detail)
                    .buttons(["Cancel", action])
                    .cancel_button(0)
                    .default_button(0)
                    .modal(true)
                    .build();
                let sender = sender.clone();
                dialog.choose(Some(root), gio::Cancellable::NONE, move |answer| {
                    if answer == Ok(1) {
                        sender.input(Msg::ResetConfirmed(kind));
                    }
                });
            }
            Msg::ResetConfirmed(kind) => {
                if self.job != Job::Idle || self.chatting || self.asking {
                    widgets.setup.job_status.set_text("Wait for what is running to finish, then reset.");
                    return;
                }
                self.start_reset(widgets, kind, &sender);
            }
            Msg::OpenDataFolder => {
                let file = gio::File::for_path(&self.data_dir);
                gtk::FileLauncher::new(Some(&file)).launch(Some(root), gio::Cancellable::NONE, |_| {});
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_storage_cmd(&mut self, widgets: &mut Widgets, message: Cmd, sender: ComponentSender<Self>) {
        match message {
            Cmd::UpdateChecked(result) => {
                self.checking_update = false;
                widgets.setup.update_check.set_sensitive(true);
                let status = match result {
                    Ok(lantern_core::updates::Status::Available { version, url }) => {
                        widgets.setup.update_link.set_uri(&url);
                        widgets.setup.update_link.set_visible(true);
                        format!("Lantern {version} is available. You have {}.", lantern_core::VERSION)
                    }
                    Ok(lantern_core::updates::Status::UpToDate) => "You're up to date.".to_owned(),
                    Ok(lantern_core::updates::Status::NoRelease) => "No public stable release is available yet.".to_owned(),
                    Err(err) => format!("Could not check for updates. {err}"),
                };
                widgets.setup.update_status.set_text(&status);
            }
            Cmd::ResetDone(kind, result) => {
                self.job = Job::Idle;
                for (_, button, text) in &widgets.setup.resets {
                    button.set_label(text);
                    button.set_sensitive(true);
                }
                if let Err(err) = result {
                    widgets.setup.job_status.set_text(&format!("Could not delete everything: {err}"));
                }
                self.after_reset(widgets, kind, &sender);
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    /// The numbers on the Index and Storage sections.
    pub(super) fn show_overview(&self, widgets: &Widgets) {
        let setup = &widgets.setup;
        let size = std::fs::metadata(&self.db_path).map_or(0, |m| m.len());
        setup.index_pill.set_visible(self.conn.is_some());
        setup.tiles_box.set_visible(self.conn.is_some());
        match &self.conn {
            Some(conn) => {
                let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
                setup.index_title.set_text(&query::meta(conn, "version").filter(|v| !v.is_empty()).unwrap_or_else(|| "Unnamed".to_owned()));
                setup.db_label.set_text(&format!("{}  ·  {:.0} MB", self.db_path.display(), size as f64 / 1_048_576.0));
                let vectors = MODELS.iter().position(|m| m.id == self.model.id).and_then(|i| self.model_states.get(i)).and_then(|s| s.1).unwrap_or(0);
                let values = [
                    count("SELECT count(*) FROM classes WHERE is_external = 0"),
                    count("SELECT count(*) FROM methods WHERE is_external = 0"),
                    count("SELECT count(*) FROM calls"),
                    count("SELECT count(*) FROM method_sources"),
                    count("SELECT count(*) FROM data_files"),
                    vectors as i64,
                ];
                for (tile, value) in setup.tiles.iter().zip(values) {
                    tile.set_text(&grouped(value));
                }
                if vectors == 0 {
                    setup.tiles[5].set_text("None");
                }
                setup.tiles[5].set_tooltip_text(Some(&format!("Chunks embedded with {}, what Ask searches", self.model.name)));
                setup.vectors_caption.set_text("Vectors");
            }
            None => {
                setup.index_title.set_text("No index yet");
                setup.db_label.set_text("Download a version below and build one from it.");
            }
        }

        let index = std::fs::metadata(&self.db_path).map_or(0, |m| m.len());
        let chats = std::fs::metadata(lantern_chat::store::path(&self.data_dir)).map_or(0, |m| m.len());
        setup.storage.set_text(&format!(
            "open index  {}\nversions    {}\nmodels      {}\nvectors     {}\nchats       {}",
            megabytes(index),
            megabytes(folder_size(&versions::root(&self.data_dir))),
            megabytes(folder_size(&download::models_dir(&self.data_dir))),
            megabytes(folder_size(&self.data_dir.join("embeddings"))),
            megabytes(chats),
        ));
    }

    /// Lets go of what the files to delete are open in, then deletes them off the UI thread.
    /// The button shows it is at work until `ResetDone`.
    pub(super) fn start_reset(&mut self, widgets: &mut Widgets, kind: Reset, sender: &ComponentSender<Self>) {
        // A loaded searcher holds the vector store and the models open.
        jobs::forget(&self.searcher);
        if kind == Reset::Everything {
            self.close_index();
        }
        self.job = Job::Reset;
        for (other, button, _) in &widgets.setup.resets {
            button.set_sensitive(false);
            if *other == kind {
                let spinner = gtk::Spinner::new();
                spinner.start();
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                line.append(&spinner);
                line.append(&view::label(if kind == Reset::Everything { "Resetting ..." } else { "Deleting ..." }, ""));
                button.set_child(Some(&line));
            }
        }
        let data_dir = self.data_dir.clone();
        sender.spawn_oneshot_command(move || Cmd::ResetDone(kind, reset_files(&data_dir, kind).map_err(|e| format!("{e:#}"))));
    }

    /// Puts the state back in line with what is left on disk.
    pub(super) fn after_reset(&mut self, widgets: &mut Widgets, kind: Reset, sender: &ComponentSender<Self>) {
        if kind == Reset::Everything {
            self.settings = Settings::default();
            self.load_settings(widgets);
            self.remote.clear();
            self.remote_asked = false;
            sender.input(Msg::ChatNew);
            self.refresh_chats(widgets);
            let fresh = self.data_dir.join("lantern.db");
            self.use_index(widgets, fresh, sender);
        }
        self.refresh_model_state();
        self.show_overview(widgets);
        self.show_versions(widgets, sender);
        // Everything gone is a first run again.
        if kind == Reset::Everything {
            self.show_onboarding(widgets, sender);
        }
    }
}

/// Deletes what `kind` names from the data folder. Nothing may hold those files open.
pub(super) fn reset_files(data_dir: &std::path::Path, kind: Reset) -> anyhow::Result<()> {
    let remove_dir = |dir: PathBuf| if dir.exists() { std::fs::remove_dir_all(&dir) } else { Ok(()) };
    match kind {
        Reset::Vectors => remove_dir(data_dir.join("embeddings"))?,
        Reset::Models => remove_dir(download::models_dir(data_dir))?,
        Reset::Everything => {
            for dir in [versions::root(data_dir), data_dir.join("embeddings"), download::models_dir(data_dir), data_dir.join("tools")] {
                remove_dir(dir)?;
            }
            for file in ["lantern.db", "chats.db", "settings.json"] {
                for suffix in ["", "-wal", "-shm", "-journal"] {
                    let path = data_dir.join(format!("{file}{suffix}"));
                    if path.exists() {
                        std::fs::remove_file(path)?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn megabytes(bytes: u64) -> String {
    format!("{:>8.1} MB", bytes as f64 / 1_048_576.0)
}

pub(super) fn folder_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            // Not following links, the model cache links snapshots to blobs and would count twice.
            Ok(meta) if meta.is_dir() => folder_size(&entry.path()),
            Ok(meta) if meta.is_file() => meta.len(),
            _ => 0,
        })
        .sum()
}
