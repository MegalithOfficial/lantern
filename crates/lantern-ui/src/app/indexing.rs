//! The Index section of Setup: versions from Mojang, downloading, decompiling, building indexes and switching between them.

use super::*;

impl App {
    pub(super) fn on_indexing(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, root: &gtk::Window) {
        match message {
            Msg::ChoosePath(kind) => {
                let dialog = gtk::FileDialog::new();
                let sender = sender.clone();
                let done = move |result: Result<gio::File, glib::Error>| {
                    if let Some(path) = result.ok().and_then(|file| file.path()) {
                        sender.input(Msg::PathChosen(kind, path));
                    }
                };
                match kind {
                    PathKind::Jar => dialog.open(Some(root), gio::Cancellable::NONE, done),
                    _ => dialog.select_folder(Some(root), gio::Cancellable::NONE, done),
                }
            }
            Msg::PathChosen(kind, path) => {
                let text = path.display().to_string();
                match kind {
                    PathKind::Jar => widgets.setup.jar.set_text(&text),
                    PathKind::Src => {
                        widgets.setup.src.set_text(&text);
                        // Vineflower puts the data folder next to the sources.
                        let data = path.join("data");
                        if widgets.setup.data.text().is_empty() && data.is_dir() {
                            widgets.setup.data.set_text(&data.display().to_string());
                        }
                    }
                    PathKind::Data => widgets.setup.data.set_text(&text),
                }
            }
            Msg::BuildIndex => self.build_custom(widgets, &sender),
            Msg::RemoteRefresh => {
                self.remote_asked = true;
                widgets.setup.remote_status.set_text("Asking Mojang for the versions ...");
                let data_dir = self.data_dir.clone();
                sender.spawn_oneshot_command(move || Cmd::RemoteListed(versions::fetch_list(&data_dir).map_err(|e| format!("{e:#}"))));
            }
            Msg::RemoteFilter => self.show_versions(widgets, &sender),
            Msg::VersionDownload(id) => {
                let Some(remote) = self.remote.iter().find(|v| v.id == id).cloned() else { return };
                let decompile = widgets.setup.decompile.is_active() && self.java.is_some();
                self.start_version(widgets, &sender, id, Some(remote), decompile);
            }
            Msg::VersionDecompile(id) => self.start_version(widgets, &sender, id, None, true),
            Msg::VersionBuild(id) => {
                let Some(version) = self.installed.iter().find(|v| v.id == id) else { return };
                let Some(jar) = version.jar.clone() else { return };
                let inputs = Inputs { version: id, jar, src: version.src.clone(), data: version.data.clone() };
                self.start_index(widgets, &sender, inputs);
            }
            Msg::VersionUse(id) => {
                if self.job != Job::Idle {
                    return;
                }
                if let Some(index) = self.installed.iter().find(|v| v.id == id).and_then(|v| v.index.clone()) {
                    self.use_index(widgets, index, &sender);
                }
            }
            Msg::VersionOpen(id) => {
                let file = gio::File::for_path(versions::root(&self.data_dir).join(id));
                gtk::FileLauncher::new(Some(&file)).launch(Some(root), gio::Cancellable::NONE, |_| {});
            }
            Msg::VersionDelete(id) => {
                let active = self.installed.iter().any(|v| v.id == id && v.index.as_deref() == Some(self.db_path.as_path()));
                let detail = if active {
                    "Its index is the one open. The jar, the sources, the index and its vectors go, and another indexed version is opened if there is one."
                } else {
                    "The jar, the decompiled sources, the index and its vectors are removed from the data folder."
                };
                let dialog = gtk::AlertDialog::builder()
                    .message(format!("Delete {id}?"))
                    .detail(detail)
                    .buttons(["Cancel", "Delete"])
                    .cancel_button(0)
                    .default_button(0)
                    .modal(true)
                    .build();
                let sender = sender.clone();
                dialog.choose(Some(root), gio::Cancellable::NONE, move |answer| {
                    if answer == Ok(1) {
                        sender.input(Msg::VersionDeleteConfirmed(id));
                    }
                });
            }
            Msg::VersionDeleteConfirmed(id) => {
                if self.job != Job::Idle {
                    return;
                }
                let active = self.installed.iter().any(|v| v.id == id && v.index.as_deref() == Some(self.db_path.as_path()));
                if active {
                    self.close_index();
                }
                let result = versions::remove(&self.data_dir, &id);
                if active {
                    self.installed = versions::installed(&self.data_dir);
                    let next = self.installed.iter().find_map(|v| v.index.clone());
                    self.use_index(widgets, next.unwrap_or_else(|| self.data_dir.join("lantern.db")), &sender);
                }
                widgets.setup.version_status.set_text(&result.err().map(|e| format!("{e:#}")).unwrap_or_default());
                self.installed = versions::installed(&self.data_dir);
                self.show_versions(widgets, &sender);
                self.show_overview(widgets);
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_indexing_cmd(&mut self, widgets: &mut Widgets, message: Cmd, sender: ComponentSender<Self>) {
        match message {
            Cmd::Indexing(p) => {
                let name = self.building.as_ref().and_then(|p| p.parent()?.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let setup = &widgets.setup;
                let rate = self.rate(p.step.label(), p.done as u64);
                if p.total > 0 {
                    setup.index_bar.set_fraction(p.done as f64 / p.total as f64);
                    let mut text = format!("{name}: {}, {} of {} {}", p.step.label(), grouped(p.done as i64), grouped(p.total as i64), p.step.unit());
                    if let Some(rate) = rate {
                        text += &format!("  ·  {} per second{}", grouped(rate as i64), left(p.total.saturating_sub(p.done) as f64 / rate));
                    }
                    setup.index_status.set_text(&text);
                } else {
                    setup.index_status.set_text(&format!("{name}: {} ...", p.step.label()));
                }
                self.index_pulse = p.total == 0;
            }
            Cmd::Indexed(result) => {
                self.job = Job::Idle;
                self.step_started = None;
                widgets.setup.index_bar.set_fraction(0.0);
                let target = self.building.take().unwrap_or_else(|| self.db_path.clone());
                self.installed = versions::installed(&self.data_dir);
                match &result {
                    Ok(_) => self.use_index(widgets, target, &sender),
                    // The one open was not touched unless it was the one being rebuilt.
                    Err(_) if self.conn.is_none() => {
                        let path = self.db_path.clone();
                        self.use_index(widgets, path, &sender);
                    }
                    Err(_) => self.show_versions(widgets, &sender),
                }
                match result {
                    Ok(stats) => {
                        let seconds = self.build_started.take().map_or(0.0, |t| t.elapsed().as_secs_f64());
                        let mut text = format!(
                            "Built in {seconds:.1} s  ·  {} classes outside the jar  ·  {} unresolved calls  ·  {} overrides  ·  {} key fields",
                            grouped(stats.external_classes as i64),
                            grouped(stats.unresolved_calls as i64),
                            grouped(stats.overrides as i64),
                            grouped(stats.key_fields as i64),
                        );
                        if !stats.skipped.is_empty() {
                            text += &format!("  ·  {} classes could not be read", stats.skipped.len());
                        }
                        widgets.setup.index_status.set_text(&text);
                    }
                    Err(err) => widgets.setup.index_status.set_text(&err),
                }
            }
            Cmd::RemoteListed(result) => {
                let error = result.as_ref().err().cloned();
                match result {
                    Ok(remote) => {
                        self.remote = remote;
                        widgets.setup.remote_status.set_text("");
                        self.show_versions(widgets, &sender);
                    }
                    Err(err) if self.remote.is_empty() => widgets.setup.remote_status.set_text(&format!("Could not reach Mojang. {err}")),
                    Err(_) => widgets.setup.remote_status.set_text("Could not reach Mojang, this is the list from last time."),
                }
                let error = error.filter(|_| self.remote.is_empty());
                crate::pages::onboarding::fill_versions(&widgets.onboarding, &self.remote, &sender, error.as_deref());
            }
            Cmd::VersionProgress(p) => {
                let setup = &widgets.setup;
                let id = self.version_busy.clone().unwrap_or_default();
                let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
                if p.total > 0 {
                    setup.version_bar.set_fraction(p.done as f64 / p.total as f64);
                }
                let step = format!("{:?}", p.step);
                let rate = self.rate(&step, p.done);
                let remaining = |rate: f64| left(p.total.saturating_sub(p.done) as f64 / rate);
                let speed = rate.map_or(String::new(), |rate| format!("  ·  {:.1} MB/s{}", mb(rate as u64), remaining(rate)));
                setup.version_status.set_text(&match p.step {
                    versions::Step::Download => format!("Downloading {id}, {:.1} of {:.1} MB{speed}", mb(p.done), mb(p.total)),
                    versions::Step::Unpack => format!("Unpacking {id}"),
                    versions::Step::Vineflower => format!("Downloading Vineflower, {:.1} of {:.1} MB{speed}", mb(p.done), mb(p.total)),
                    versions::Step::Decompile => {
                        // Every class is counted twice, read and written.
                        let classes = rate.map_or(String::new(), |rate| format!("  ·  {:.0} classes per second{}", rate / 2.0, remaining(rate)));
                        format!(
                            "Decompiling {id} with Vineflower, {}%, {} of {} classes{classes}",
                            (p.done * 100).checked_div(p.total).unwrap_or(0),
                            grouped((p.done / 2) as i64),
                            grouped((p.total / 2) as i64)
                        )
                    }
                });
            }
            Cmd::VersionDone(result) => {
                self.job = Job::Idle;
                self.step_started = None;
                let id = self.version_busy.take().unwrap_or_default();
                self.installed = versions::installed(&self.data_dir);
                widgets.setup.version_bar.set_fraction(0.0);
                match result {
                    Ok(()) => {
                        widgets.setup.version_status.set_text("");
                        // A first version is what the empty app is waiting for.
                        if self.conn.is_none() {
                            sender.input(Msg::VersionBuild(id));
                        }
                    }
                    Err(err) => widgets.setup.version_status.set_text(&err),
                }
                self.show_versions(widgets, &sender);
                self.show_overview(widgets);
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    /// Prefills the index inputs with what the current index was built from.
    pub(super) fn load_inputs(&mut self, widgets: &Widgets) {
        let Some(conn) = &self.conn else {
            widgets.version_badge.set_visible(false);
            return;
        };
        let get = |key| query::meta(conn, key).unwrap_or_default();
        widgets.setup.version.set_text(&get("version"));
        widgets.version_badge.set_text(&get("version"));
        widgets.version_badge.set_visible(!get("version").is_empty());
        widgets.setup.jar.set_text(&get("jar"));
        widgets.setup.src.set_text(&get("src_root"));
        widgets.setup.data.set_text(&get("data_root"));
    }

    /// Downloads `fetch` when given, then decompiles `id` when asked to.
    pub(super) fn start_version(&mut self, widgets: &Widgets, sender: &ComponentSender<Self>, id: String, fetch: Option<Remote>, decompile: bool) {
        if self.job != Job::Idle {
            return;
        }
        self.job = Job::Version;
        self.version_busy = Some(id.clone());
        self.stop.store(false, Ordering::Relaxed);
        widgets.setup.version_bar.set_fraction(0.0);
        widgets.setup.version_status.set_text(&format!("Starting on {id} ..."));
        let (data_dir, stop, threads) = (self.data_dir.clone(), self.stop.clone(), lantern_embed::threads(&self.settings));
        sender.spawn_command(move |out| {
            let result = jobs::version(&data_dir, &id, fetch.as_ref(), decompile, threads, &stop, |p| {
                let _ = out.send(Cmd::VersionProgress(p));
            });
            let _ = out.send(Cmd::VersionDone(result.map_err(|e| format!("{e:#}"))));
        });
        self.show_versions(widgets, sender);
    }

    /// The versions in the data folder and what Mojang offers. Rebuilt when one of them or the
    /// job changes, not on every message.
    pub(super) fn show_versions(&self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        let setup = &widgets.setup;
        let idle = self.job == Job::Idle;
        let notes: Vec<Option<String>> = self.installed.iter().map(|v| v.index.as_deref().map(|index| self.index_note(index, &v.id))).collect();
        crate::pages::setup::fill_installed(setup, sender, &self.installed, &notes, &self.db_path, self.version_busy.as_deref(), idle);
        crate::pages::setup::fill_remote(setup, sender, &self.remote, &self.installed, idle);
        let decompiled: Vec<String> = self.installed.iter().filter(|v| v.src.is_some()).map(|v| v.id.clone()).collect();
        crate::pages::compare::set_versions(&widgets.compare, &decompiled);
        let working = self.job == Job::Version;
        setup.version_progress.set_visible(working || !setup.version_status.text().is_empty());
        setup.version_bar.set_visible(working);
        setup.version_stop.set_visible(working);
        setup.remote_status.set_visible(!setup.remote_status.text().is_empty());
        match &self.java {
            Some(java) => setup
                .java
                .set_text(&format!("Runs Vineflower with {}. A few minutes, and the sources are what the source view and embedding read.", java.display())),
            None => setup.java.set_text("No Java found. Install Java 21 or newer, without it only the jar gets indexed."),
        }
        setup.decompile.set_sensitive(self.java.is_some());
    }

    /// Builds from the paths of the Custom section.
    pub(super) fn build_custom(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>) {
        let path = |entry: &gtk::Entry| Some(entry.text().trim().to_owned()).filter(|t| !t.is_empty()).map(PathBuf::from);
        let Some(jar) = path(&widgets.setup.jar).filter(|p| p.is_file()) else {
            widgets.setup.index_status.set_text("The server jar does not exist.");
            return;
        };
        let inputs = Inputs { version: widgets.setup.version.text().trim().to_owned(), jar, src: path(&widgets.setup.src), data: path(&widgets.setup.data) };
        for dir in [&inputs.src, &inputs.data].into_iter().flatten() {
            if !dir.is_dir() {
                widgets.setup.index_status.set_text(&format!("{} is not a folder.", dir.display()));
                return;
            }
        }
        self.start_index(widgets, sender, inputs);
    }

    pub(super) fn start_index(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>, inputs: Inputs) {
        if self.job != Job::Idle || self.asking {
            return;
        }
        // Every version is built into its own folder. The open index stays usable meanwhile,
        // unless it is the one being rebuilt.
        let target = versions::index_path(&self.data_dir, &inputs.version);
        if target == self.db_path {
            self.close_index();
        }
        self.job = Job::Index;
        self.building = Some(target.clone());
        self.build_started = Some(Instant::now());
        self.step_started = None;
        widgets.setup.index_status.set_text(&format!("Indexing {} ...", inputs.version));
        self.show_versions(widgets, sender);
        let (data_dir, root) = (self.data_dir.clone(), versions::root(&self.data_dir));
        sender.spawn_command(move |out| {
            let mut last = Instant::now() - Duration::from_secs(1);
            let result = index::build_file(&target, &inputs, &mut |p| {
                if last.elapsed() >= Duration::from_millis(100) || p.total == 0 {
                    last = Instant::now();
                    let _ = out.send(Cmd::Indexing(p));
                }
            })
            .and_then(|stats| {
                if !inputs.jar.starts_with(&root) {
                    versions::mark_custom(&data_dir, &inputs.version)?;
                }
                Ok(stats)
            });
            let _ = out.send(Cmd::Indexed(result.map_err(|e| format!("{e:#}"))));
        });
    }

    /// Lets go of the open index, before its file is replaced or removed.
    pub(super) fn close_index(&mut self) {
        self.conn = None;
        crate::chat::markdown::set_index(None);
        jobs::forget(&self.searcher);
        if let Ok(mut chat) = self.chat.try_lock() {
            *chat = None;
        }
    }

    /// Opens `path` as the index everything works on and remembers it for the next start,
    /// the CLI and the MCP server.
    pub(super) fn use_index(&mut self, widgets: &mut Widgets, path: PathBuf, sender: &ComponentSender<Self>) {
        self.close_index();
        self.settings.version = match path.parent() {
            Some(dir) if path.starts_with(versions::root(&self.data_dir)) => dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            _ => String::new(),
        };
        self.save_settings(widgets);
        self.db_path = path;
        self.conn = db::open(&self.db_path).ok();
        crate::chat::markdown::set_index(Some(&self.db_path));
        widgets.setup.db_label.set_text(&self.db_path.display().to_string());
        self.installed = versions::installed(&self.data_dir);
        self.refresh_model_state();
        self.load_inputs(widgets);
        self.load_tree(widgets);
        self.show_overview(widgets);
        self.show_versions(widgets, sender);
    }

    /// What the index of a version holds, for its row: size, classes and whether it is embedded.
    pub(super) fn index_note(&self, index: &std::path::Path, version: &str) -> String {
        let size = std::fs::metadata(index).map_or(0, |m| m.len());
        let classes =
            db::open(index).ok().and_then(|conn| conn.query_row("SELECT count(*) FROM classes WHERE is_external = 0", [], |r| r.get::<_, i64>(0)).ok());
        let vectors = store::stored_count(&versions::vectors_dir(&self.data_dir, version), self.model).filter(|n| *n > 0);
        let mut parts = vec![format!("index {:.0} MB", size as f64 / 1_048_576.0)];
        parts.extend(classes.map(|n| format!("{} classes", grouped(n))));
        parts.push(vectors.map_or_else(|| "not embedded".to_owned(), |n| format!("{} vectors", grouped(n as i64))));
        parts.join("  ·  ")
    }

    /// The vectors of the open index.
    pub(super) fn vectors(&self) -> PathBuf {
        vectors_dir(self.conn.as_ref(), &self.data_dir)
    }

    /// Units per second of the step that runs, once it has run long enough to say.
    pub(super) fn rate(&mut self, step: &str, done: u64) -> Option<f64> {
        if self.step_started.as_ref().is_none_or(|(running, _, _)| running != step) {
            self.step_started = Some((step.to_owned(), Instant::now(), done));
        }
        let (_, started, base) = self.step_started.as_ref()?;
        let seconds = started.elapsed().as_secs_f64();
        (seconds > 1.5 && done > *base).then(|| (done - base) as f64 / seconds)
    }
}

pub(super) fn vectors_dir(conn: Option<&Connection>, data_dir: &std::path::Path) -> PathBuf {
    match conn {
        Some(conn) => lantern_embed::vectors_dir(conn, data_dir),
        None => versions::vectors_dir(data_dir, ""),
    }
}

/// `, about 3 min left`.
pub(super) fn left(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 1.0 {
        return String::new();
    }
    match seconds as u64 {
        s if s < 60 => format!(", {s} s left"),
        s if s < 3600 => format!(", about {} min left", s.div_ceil(60)),
        s => format!(", about {:.1} h left", s as f64 / 3600.0),
    }
}
