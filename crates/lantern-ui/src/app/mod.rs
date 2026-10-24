mod browse;
mod chat;
mod compare;
mod indexing;
mod lookup;
mod models;
mod onboarding;
mod storage;

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gtk::{gio, glib, prelude::*};
use lantern_core::{
    db::{self, Connection},
    index::{self, Inputs, Stats},
    query::{self, Location, MethodRef},
    render::{Query, short, signature},
    settings::{self, Settings},
    versions::{self, Installed, Remote},
};
use lantern_embed::{
    Answer, BuildProgress, BuildReport, Phase, download,
    models::{MODELS, ModelSpec, RERANKERS, RerankerSpec, Source},
    store,
};
use relm4::{Component, ComponentParts, ComponentSender};

use self::indexing::*;
use crate::{
    browse::tree::{ClassTree, Target},
    jobs::{self, SharedSearcher},
    theme::icons,
    view::{self, AnswerRow, Widgets, fill, fill_answers, fill_calls, fill_icons, fill_tree},
};

#[derive(Clone, Copy, Debug)]
pub enum PathKind {
    Jar,
    Src,
    Data,
}

/// What Storage can delete.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reset {
    Vectors,
    Models,
    Everything,
}

#[derive(Clone, Debug)]
pub enum Msg {
    ClassSearch(String),
    ClassActivated(usize),
    MemberActivated(usize),
    CallerActivated(usize),
    CalleeActivated(usize),
    OpenFile,
    Ask,
    /// The search box was emptied: back to the members of the class.
    SearchCleared,
    PaneClosed(u64),
    PaneMoved(u64, u64),
    PaneFocused(u64),
    PaneSplit,
    FindInFiles(String),
    FindNext,
    ToggleSync(bool),
    /// A class dragged from the tree onto a pane, or beside them.
    OpenInPane(u64, String),
    OpenInNewPane(String),
    FocusSearch,
    AnswerActivated(usize),
    Lookup,
    /// A link in a lookup that runs another kind of lookup, by its position in `LOOKUPS`.
    LookupFor(usize, String),
    /// A file named in a lookup, opened in the default editor.
    OpenPath(String),
    /// A class in Browse, at a method and a line of its file. A site is turned into the line
    /// once it is clicked, finding it means reading the method's source.
    OpenAt {
        class: String,
        method: Option<i64>,
        line: Option<i64>,
        site: Option<query::Site>,
    },
    ChoosePath(PathKind),
    PathChosen(PathKind, PathBuf),
    BuildIndex,
    /// Ask Mojang for the versions again.
    RemoteRefresh,
    Onboarding(crate::pages::onboarding::Action),
    CompareRun,
    CompareFilter,
    CompareActivated(usize),
    /// Unified or split.
    CompareMode,
    /// The next changed file, or the one before.
    CompareStep(bool),
    /// A line of the diff was clicked, a hunk header opens what it hides.
    CompareClick(usize),
    RemoteFilter,
    VersionDownload(String),
    VersionDecompile(String),
    VersionBuild(String),
    /// Open the index of this version instead of the one that is open.
    VersionUse(String),
    VersionOpen(String),
    VersionDelete(String),
    /// Delete, after the dialog said yes.
    VersionDeleteConfirmed(String),
    /// Use the model, or download it first when it is not there.
    ModelAction(usize),
    /// The same for rerankers, 0 is "Off".
    RerankerAction(usize),
    SettingsEdited,
    CheckForUpdates,
    ProviderSelected(usize),
    LoadModels,
    ModelPicked(String),
    OpenDataFolder,
    Reset(Reset),
    /// After the dialog said yes.
    ResetConfirmed(Reset),
    Embed,
    Stop,
    ChatSend,
    ChatRegenerate,
    ChatCopy,
    Back,
    Forward,
    /// A `Class.method` link in an answer.
    OpenSymbol(String),
    ChatStop,
    ChatNew,
    /// Open a saved chat.
    ChatOpen(usize),
    ChatMenu(usize, f64, f64),
    ChatDelete(usize),
    Tick,
    Page(&'static str),
}

#[derive(Debug)]
pub enum Cmd {
    UpdateChecked(Result<lantern_core::updates::Status, String>),
    Indexed(Result<Stats, String>),
    RemoteListed(Result<Vec<Remote>, String>),
    CompareProgress(usize, usize),
    Compared(Result<(String, String, PathBuf, PathBuf, Vec<lantern_core::compare::FileChange>), String>),
    VersionProgress(versions::Progress),
    Indexing(index::Progress),
    ResetDone(Reset, Result<(), String>),
    Setup(crate::pages::onboarding::Event),
    VersionDone(Result<(), String>),
    Downloading(jobs::Download),
    Downloaded(Result<(), String>),
    Embedding(BuildProgress),
    Embedded(Result<BuildReport, String>),
    Answers(Result<Vec<Answer>, String>),
    LookedUp(Result<lantern_core::render::Doc, String>),
    Chat(lantern_chat::session::Event),
    ModelsLoaded(Result<Vec<String>, String>),
    ChatDone(Result<lantern_chat::client::Usage, String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Job {
    Idle,
    Index,
    /// Downloading or decompiling a Minecraft version.
    Version,
    /// Deleting what Storage was asked to delete.
    Reset,
    /// The first run setup, downloading and building everything.
    Onboarding,
    Download,
    Embed,
}

/// What to switch to once a download is through.
#[derive(Clone, Copy)]
enum Pending {
    Model(usize),
    Reranker(usize),
}

/// A place the view has been: page, class and method.
type Place = (&'static str, Option<String>, Option<i64>);

enum Member {
    Method(i64),
    Field(i64),
}

pub struct App {
    db_path: PathBuf,
    data_dir: PathBuf,
    /// None when there is no index, or while the open one is rebuilt or deleted.
    conn: Option<Connection>,
    searcher: SharedSearcher,
    stop: Arc<AtomicBool>,
    job: Job,
    asking: bool,
    model: &'static ModelSpec,
    /// When the embedding phase started and how much was done by then, for the rate.
    embed_started: Option<(Instant, usize)>,

    /// What the rows of the sidebar stand for, a package only in tree mode.
    classes: Vec<Target>,
    tree: ClassTree,
    searching: bool,
    members: Vec<Member>,
    callers: Vec<MethodRef>,
    callees: Vec<MethodRef>,
    answers: Vec<Answer>,
    /// The saved chats, newest first, and which of them is open.
    chats: Vec<lantern_chat::store::ChatInfo>,
    chat_id: Option<i64>,
    location: Option<Location>,
    page: &'static str,
    class: Option<String>,
    /// The method the source view is on, for going back to it.
    method: Option<i64>,
    /// Where Back leads: page, class and method of every place that was left by a jump.
    history: Vec<Place>,
    /// What Back left behind, for Forward. A new jump empties it, like in a browser.
    future: Vec<Place>,
    settings: Settings,
    chat: Arc<Mutex<Option<lantern_chat::session::Session>>>,
    chat_stop: Arc<AtomicBool>,
    chatting: bool,
    /// The block streamed so far. Rendered again on the next tick when it changed, tokens
    /// arrive faster than it is worth rebuilding the widgets.
    chat_text: String,
    chat_dirty: bool,
    /// The question of the last turn and the whole answer to it, for Regenerate and Copy.
    chat_question: String,
    chat_answer: String,
    /// A failed turn is not in the history, trying again must not take the one before it back.
    chat_failed: bool,
    /// Per model: on disk, and how many vectors it has. Looked up when it can have changed,
    /// not on every message, it means opening databases.
    model_states: Vec<(bool, Option<usize>)>,
    reranker_states: Vec<bool>,
    reranker: Option<&'static RerankerSpec>,
    pending: Option<Pending>,
    /// What Mojang offers, from the last fetch or the copy kept on disk.
    remote: Vec<Remote>,
    remote_asked: bool,
    installed: Vec<Installed>,
    /// The version a download or decompile works on.
    version_busy: Option<String>,
    /// Where the index being built goes. It becomes the open one when it is done.
    building: Option<PathBuf>,
    /// When the step that is running started and how far it was then, for rates.
    step_started: Option<(String, Instant, u64)>,
    java: Option<PathBuf>,
    /// The step of the build that runs has no count, the bar only shows that it is busy.
    index_pulse: bool,
    compare: crate::pages::compare::State,
    comparing: bool,
    build_started: Option<Instant>,
    checking_update: bool,
    onboarding: crate::pages::onboarding::State,
}

impl Component for App {
    type CommandOutput = Cmd;
    type Input = Msg;
    type Output = ();
    type Init = PathBuf;
    type Root = gtk::Window;
    type Widgets = Widgets;

    fn init_root() -> Self::Root {
        // SNAPSHOT-TMP: the size can be set for a capture
        let size = std::env::var("LANTERN_SIZE").ok().and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        let (width, height) = size.unwrap_or((1600, 960));
        gtk::Window::builder().title("Lantern").default_width(width).default_height(height).build()
    }

    fn init(db_path: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let data_dir = lantern_embed::data_dir(&db_path);
        let settings = Settings::load(&data_dir);
        let conn = db::open(&db_path).ok();
        let vectors = vectors_dir(conn.as_ref(), &data_dir);
        let (model, reranker) = lantern_embed::pick_models(&data_dir, &vectors, &settings);
        // An explicit choice stands even when there is nothing behind it yet, Setup shows why.
        let model = lantern_embed::models::find(&settings.embedding_model).unwrap_or(model);
        let (remote, installed) = (versions::cached_list(&data_dir), versions::installed(&data_dir));

        let mut model = Self {
            conn,
            db_path,
            data_dir,
            searcher: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            job: Job::Idle,
            asking: false,
            model,
            embed_started: None,
            classes: Vec::new(),
            tree: ClassTree::default(),
            searching: false,
            members: Vec::new(),
            callers: Vec::new(),
            callees: Vec::new(),
            answers: Vec::new(),
            chats: Vec::new(),
            chat_id: None,
            location: None,
            page: "browse",
            class: None,
            method: None,
            history: Vec::new(),
            future: Vec::new(),
            settings,
            chat: Arc::new(Mutex::new(None)),
            chat_stop: Arc::new(AtomicBool::new(false)),
            chatting: false,
            chat_text: String::new(),
            chat_dirty: false,
            chat_question: String::new(),
            chat_answer: String::new(),
            chat_failed: false,
            model_states: Vec::new(),
            reranker_states: Vec::new(),
            reranker,
            pending: None,
            remote,
            remote_asked: false,
            installed,
            version_busy: None,
            building: None,
            step_started: None,
            java: versions::find_java(),
            index_pulse: false,
            compare: Default::default(),
            comparing: false,
            build_started: None,
            checking_update: false,
            onboarding: Default::default(),
        };
        model.refresh_model_state();

        crate::chat::markdown::set_index(Some(&model.db_path));
        crate::theme::fonts::register();
        let mut widgets = view::build(&root, &sender);
        widgets.setup.db_label.set_text(&model.db_path.display().to_string());
        widgets.version_badge.set_visible(false);
        model.load_settings(&widgets);
        model.show_overview(&widgets);
        model.load_inputs(&widgets);
        model.show_versions(&widgets, &sender);
        model.load_tree(&widgets);
        model.refresh_chats(&widgets);
        model.sync_setup(&mut widgets);
        model.show_page(&widgets, "browse");
        // No index open but some on disk, from a setup that was closed halfway or a version
        // that was deleted: the newest of them.
        if model.conn.is_none() {
            if let Some(index) = model.installed.iter().find_map(|v| v.index.clone()) {
                model.use_index(&mut widgets, index, &sender);
            }
        }
        // Someone with an index from before there was a first run setup does not need it.
        if !model.settings.onboarded && (model.conn.is_some() || model.installed.iter().any(|v| v.index.is_some())) {
            model.settings.onboarded = true;
            model.save_settings(&widgets);
        }
        if !model.settings.onboarded {
            model.show_onboarding(&widgets, &sender);
        } else if model.conn.is_none() {
            model.show_page(&widgets, "setup");
            sender.input(Msg::RemoteRefresh);
        }

        let ticker = sender.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            ticker.input(Msg::Tick);
            glib::ControlFlow::Continue
        });

        // SNAPSHOT-TMP
        if let Some(out) = std::env::var_os("LANTERN_SNAPSHOT") {
            if let Ok(page) = std::env::var("LANTERN_PAGE") {
                if let Some((name, _)) = view::PAGES.iter().find(|(name, _)| *name == page) {
                    model.show_page(&widgets, name);
                }
            }
            if let Ok(class) = std::env::var("LANTERN_CLASS") {
                let method =
                    std::env::var("LANTERN_METHOD").ok().and_then(|m| query::find_methods(model.conn.as_ref()?, &class, &m).ok()?.first().map(|m| m.id));
                model.open_class(&widgets, &class, method, &sender);
            }
            if let Ok(symbol) = std::env::var("LANTERN_SYMBOL") {
                sender.input(Msg::OpenSymbol(symbol));
            }
            if let Some(index) = std::env::var("LANTERN_OPEN_CHAT").ok().and_then(|v| v.parse().ok()) {
                sender.input(Msg::ChatOpen(index));
            }
            for class in std::env::var("LANTERN_CLASS2").iter().flat_map(|list| list.split(',')) {
                widgets.panes.split(&sender);
                model.open_class(&widgets, class, None, &sender);
            }
            // SNAPSHOT-TMP: what drop targets a pane's text view is left with
            if std::env::var_os("LANTERN_DND_DEBUG").is_some() {
                if let Some(view) = widgets.panes.active_view() {
                    let controllers = view.observe_controllers();
                    let names: Vec<String> = (0..controllers.n_items()).filter_map(|i| controllers.item(i)).map(|c| c.type_().to_string()).collect();
                    eprintln!("text view controllers: {names:?}");
                }
            }
            if let Ok(class) = std::env::var("LANTERN_DROP") {
                sender.input(Msg::OpenInNewPane(class));
            }
            if let Ok(text) = std::env::var("LANTERN_LOOKUP") {
                let kind = std::env::var("LANTERN_LOOKUP_KIND").ok().and_then(|k| k.parse().ok()).unwrap_or(0);
                sender.input(Msg::LookupFor(kind, text));
            }
            if let Ok(needle) = std::env::var("LANTERN_FIND") {
                sender.input(Msg::FindInFiles(needle));
            }
            if let Ok(question) = std::env::var("LANTERN_CHAT") {
                widgets.chat_entry.set_text(&question);
                sender.input(Msg::ChatSend);
            }
            if let Some((x, y)) = std::env::var("LANTERN_MENU_AT").ok().and_then(|v| {
                let (x, y) = v.split_once(',')?;
                Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?))
            }) {
                let (chat_box, opener) = (widgets.chat_box.clone(), sender.clone());
                glib::timeout_add_local_once(Duration::from_millis(5000), move || crate::chat::chatmenu::popup(&chat_box, &opener, x, y));
            }
            if let Some(after) = std::env::var("LANTERN_STOP_AFTER").ok().and_then(|v| v.parse().ok()) {
                let stopper = sender.clone();
                glib::timeout_add_local_once(Duration::from_millis(after), move || stopper.input(Msg::ChatStop));
            }
            if let Ok(question) = std::env::var("LANTERN_ASK") {
                widgets.ask_entry.set_text(&question);
                sender.input(Msg::Ask);
            }
            let window = root.clone();
            glib::timeout_add_local_once(Duration::from_millis(std::env::var("LANTERN_WAIT").ok().and_then(|w| w.parse().ok()).unwrap_or(1500)), move || {
                let paintable = gtk::WidgetPaintable::new(Some(&window));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
                match (snapshot.to_node(), window.native().and_then(|n| n.renderer())) {
                    (Some(node), Some(renderer)) => {
                        let saved = renderer.render_texture(&node, None).save_to_png(&out);
                        eprintln!("snapshot {}x{}: {saved:?}", window.width(), window.height());
                    }
                    (node, renderer) => eprintln!("snapshot failed, node {} renderer {}", node.is_some(), renderer.is_some()),
                }
                window.close();
            });
        }

        ComponentParts { model, widgets }
    }

    fn update_with_view(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, root: &Self::Root) {
        match message {
            Msg::Tick => {
                if self.chat_dirty {
                    self.render_chat(widgets, &sender);
                }
                if let Some(turn) = &mut widgets.chat_live {
                    turn.tick();
                }
                if self.job == Job::Index && self.index_pulse {
                    widgets.setup.index_bar.pulse();
                }
                return;
            }
            Msg::Page(name) => {
                self.show_page(widgets, name);
                if name == "setup" && !self.remote_asked {
                    sender.input(Msg::RemoteRefresh);
                }
            }
            Msg::ClassSearch(..)
            | Msg::ClassActivated(..)
            | Msg::MemberActivated(..)
            | Msg::CallerActivated(..)
            | Msg::CalleeActivated(..)
            | Msg::AnswerActivated(..)
            | Msg::OpenFile
            | Msg::Ask
            | Msg::PaneClosed(..)
            | Msg::PaneMoved(..)
            | Msg::PaneFocused(..)
            | Msg::OpenInPane(..)
            | Msg::OpenInNewPane(..)
            | Msg::PaneSplit
            | Msg::FindInFiles(..)
            | Msg::FindNext
            | Msg::ToggleSync(..)
            | Msg::SearchCleared
            | Msg::FocusSearch
            | Msg::OpenAt { .. }
            | Msg::OpenPath(..)
            | Msg::OpenSymbol(..)
            | Msg::Back
            | Msg::Forward => self.on_browse(widgets, message, sender, root),
            Msg::ChatOpen(..)
            | Msg::ChatMenu(..)
            | Msg::ChatDelete(..)
            | Msg::ChatSend
            | Msg::ChatRegenerate
            | Msg::ChatCopy
            | Msg::ChatStop
            | Msg::ChatNew => self.on_chat(widgets, message, sender, root),
            Msg::Lookup | Msg::LookupFor(..) => self.on_lookup(widgets, message, sender, root),
            Msg::ChoosePath(..)
            | Msg::PathChosen(..)
            | Msg::BuildIndex
            | Msg::RemoteRefresh
            | Msg::RemoteFilter
            | Msg::VersionDownload(..)
            | Msg::VersionDecompile(..)
            | Msg::VersionBuild(..)
            | Msg::VersionUse(..)
            | Msg::VersionOpen(..)
            | Msg::VersionDelete(..)
            | Msg::VersionDeleteConfirmed(..) => self.on_indexing(widgets, message, sender, root),
            Msg::CheckForUpdates | Msg::Reset(..) | Msg::ResetConfirmed(..) | Msg::OpenDataFolder => self.on_storage(widgets, message, sender, root),
            Msg::Onboarding(..) => self.on_onboarding(widgets, message, sender, root),
            Msg::CompareRun | Msg::CompareFilter | Msg::CompareActivated(..) | Msg::CompareMode | Msg::CompareClick(..) | Msg::CompareStep(..) => {
                self.on_compare(widgets, message, sender, root)
            }
            Msg::ModelAction(..)
            | Msg::RerankerAction(..)
            | Msg::SettingsEdited
            | Msg::ProviderSelected(..)
            | Msg::LoadModels
            | Msg::ModelPicked(..)
            | Msg::Embed
            | Msg::Stop => self.on_models(widgets, message, sender, root),
        }
        self.sync_setup(widgets);
    }

    fn update_cmd_with_view(&mut self, widgets: &mut Widgets, message: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        let finished = Some(matches!(message, Cmd::Indexed(_) | Cmd::Downloaded(_) | Cmd::Embedded(_) | Cmd::VersionDone(_)));
        match message {
            Cmd::UpdateChecked(..) | Cmd::ResetDone(..) => self.on_storage_cmd(widgets, message, sender),
            Cmd::Indexing(..) | Cmd::Indexed(..) | Cmd::RemoteListed(..) | Cmd::VersionProgress(..) | Cmd::VersionDone(..) => {
                self.on_indexing_cmd(widgets, message, sender)
            }
            Cmd::CompareProgress(..) | Cmd::Compared(..) => self.on_compare_cmd(widgets, message, sender),
            Cmd::Setup(..) => self.on_onboarding_cmd(widgets, message, sender),
            Cmd::Downloading(..) | Cmd::Downloaded(..) | Cmd::Embedding(..) | Cmd::Embedded(..) | Cmd::ModelsLoaded(..) => {
                self.on_models_cmd(widgets, message, sender)
            }
            Cmd::Answers(..) => self.on_browse_cmd(widgets, message, sender),
            Cmd::Chat(..) | Cmd::ChatDone(..) => self.on_chat_cmd(widgets, message, sender),
            Cmd::LookedUp(..) => self.on_lookup_cmd(widgets, message, sender),
        }
        if matches!(finished, Some(true)) {
            self.show_overview(widgets);
        }
        self.sync_setup(widgets);
    }
}

impl App {
    fn sync_setup(&self, widgets: &mut Widgets) {
        widgets.chat_empty.set_visible(widgets.chat_box.first_child().is_none());
        widgets.ask_empty.set_visible(self.answers.is_empty());
        widgets.chat_send.set_visible(!self.chatting);
        widgets.chat_stop.set_visible(self.chatting);
        if !self.chatting && widgets.chat_usage.text().is_empty() {
            widgets.chat_usage.set_text(&if self.settings.chat.endpoint().model.is_empty() {
                "No model yet, Setup has a Chat section".to_owned()
            } else {
                self.chat_label()
            });
        }
        let setup = &widgets.setup;
        let idle = self.job == Job::Idle;
        setup.index_bar.set_visible(self.job == Job::Index);
        setup.job_bar.set_visible(matches!(self.job, Job::Download | Job::Embed));
        setup.job_status.set_visible(!setup.job_status.text().is_empty());
        setup.index_status.set_visible(!setup.index_status.text().is_empty());
        setup.index_button.set_sensitive(idle && !self.asking);
        setup.stop_button.set_visible(self.job == Job::Embed);

        for (index, (row, (downloaded, stored))) in setup.models.iter().zip(&self.model_states).enumerate() {
            let active = MODELS[index].id == self.model.id;
            row.active.set_visible(active);
            row.action.set_visible(!active || !downloaded);
            row.action.set_label(if *downloaded { "Use" } else { "Download" });
            row.action.set_sensitive(idle);
            row.state.set_text(&match (downloaded, stored) {
                (false, _) => "Not downloaded".to_owned(),
                (true, Some(n)) if *n > 0 => format!("Downloaded  ·  {n} chunks embedded"),
                (true, _) => "Downloaded  ·  nothing embedded yet".to_owned(),
            });
        }
        let active_model = MODELS.iter().position(|m| m.id == self.model.id).and_then(|i| self.model_states.get(i));
        setup.embed_button.set_sensitive(idle && self.conn.is_some() && active_model.is_some_and(|state| state.0));

        let chosen = self.reranker.and_then(|r| RERANKERS.iter().position(|o| o.id == r.id)).map_or(0, |i| i + 1);
        for (index, row) in setup.rerankers.iter().enumerate() {
            let downloaded = index == 0 || self.reranker_states.get(index - 1).copied().unwrap_or(false);
            let active = index == chosen;
            row.active.set_visible(active);
            row.action.set_visible(!active || !downloaded);
            row.action.set_label(if downloaded { "Use" } else { "Download" });
            row.action.set_sensitive(idle);
            row.state.set_visible(index > 0);
            row.state.set_text(if downloaded { "Downloaded" } else { "Not downloaded" });
        }

        widgets.ask_entry.set_tooltip_text(Some(&match self.active_reranker() {
            Some(r) => format!("{}  ·  {}", self.model.name, r.name),
            None => self.model.name.to_owned(),
        }));
    }
}

fn grouped(n: i64) -> String {
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
