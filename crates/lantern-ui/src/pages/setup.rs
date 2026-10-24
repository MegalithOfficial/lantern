//! The Setup page: a section list on the left, one page of grouped rows per section.

use std::path::Path;

use gtk::prelude::*;
use lantern_core::{
    settings::PROVIDERS,
    versions::{Installed, Remote},
};
use lantern_embed::models::{MODELS, RERANKERS};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg, PathKind, Reset},
    theme::icons,
    view::{button, icon_button, label, scrolled, separator, wrapping},
};

/// One model in a picker: what it is, what state it is in and the one thing to do with it.
pub struct ModelRow {
    pub state: gtk::Label,
    pub active: gtk::Label,
    pub action: gtk::Button,
}

/// The sections of Setup, as the sidebar lists them.
pub const SECTIONS: &[(&str, &str)] =
    &[("Index", "folder-tree"), ("Models", "box"), ("Search", "search"), ("Chat", "message-square"), ("Storage", "download"), ("About", "tag")];

pub struct Setup {
    /// One page per section, switched from the sidebar.
    pub pages: gtk::Stack,
    pub db_label: gtk::Label,
    /// The version of the open index, or that there is none.
    pub index_title: gtk::Label,
    pub index_pill: gtk::Label,
    /// Classes, methods, calls, method sources, data files, vectors.
    pub tiles: Vec<gtk::Label>,
    pub tiles_box: gtk::Box,
    pub vectors_caption: gtk::Label,
    pub index_bar: gtk::ProgressBar,
    pub index_status: gtk::Label,

    /// The versions in the data folder, one row each, filled by `fill_installed`.
    pub installed: gtk::Box,
    pub version_progress: gtk::Box,
    pub version_bar: gtk::ProgressBar,
    pub version_status: gtk::Label,
    pub version_stop: gtk::Button,
    /// What Mojang offers, filled by `fill_remote`.
    pub remote: gtk::Box,
    pub remote_search: gtk::SearchEntry,
    /// On: releases only, off: snapshots too.
    pub remote_releases: gtk::ToggleButton,
    pub remote_status: gtk::Label,
    pub decompile: gtk::Switch,
    pub java: gtk::Label,

    pub version: gtk::Entry,
    pub jar: gtk::Entry,
    pub src: gtk::Entry,
    pub data: gtk::Entry,
    pub index_button: gtk::Button,

    pub models: Vec<ModelRow>,
    /// Row 0 is "Off".
    pub rerankers: Vec<ModelRow>,
    pub embed_button: gtk::Button,
    pub stop_button: gtk::Button,
    pub job_bar: gtk::ProgressBar,
    pub job_status: gtk::Label,

    pub threads: gtk::SpinButton,
    pub results: gtk::SpinButton,
    pub rerank_pool: gtk::SpinButton,

    pub chat_provider: gtk::DropDown,
    pub chat_note: gtk::Label,
    pub chat_models: gtk::DropDown,
    pub chat_models_status: gtk::Label,
    pub chat_url: gtk::Entry,
    pub chat_key: gtk::PasswordEntry,
    pub chat_model: gtk::Entry,
    pub chat_rounds: gtk::SpinButton,

    pub storage: gtk::Label,
    /// The buttons of Reset with their labels, for showing one of them at work.
    pub resets: Vec<(Reset, gtk::Button, &'static str)>,
    pub update_check: gtk::Button,
    pub update_status: gtk::Label,
    pub update_link: gtk::LinkButton,
}

/// A titled group of rows inside one rounded box, rows split by hairlines.
fn group(title: &str, description: &str, rows: &[&gtk::Widget]) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    outer.set_margin_bottom(22);
    if !title.is_empty() {
        outer.append(&label(title, "group-title"));
    }
    if !description.is_empty() {
        outer.append(&wrapping(description, "group-description"));
    }
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("group");
    frame.set_margin_top(4);
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            frame.append(&separator(gtk::Orientation::Horizontal));
        }
        frame.append(*row);
    }
    outer.append(&frame);
    outer
}

/// Name and explanation on the left, the control on the right.
fn row(title: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, "row-title"));
    if !description.is_empty() {
        text.append(&wrapping(description, "row-detail"));
    }
    control.set_valign(gtk::Align::Center);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("setting");
    row.append(&text);
    row.append(control);
    row.upcast()
}

/// A row whose control is wide, like a path: the text goes on top.
fn tall_row(title: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
    row.add_css_class("setting");
    row.append(&label(title, "row-title"));
    if !description.is_empty() {
        row.append(&wrapping(description, "row-detail"));
    }
    row.append(control);
    row.upcast()
}

fn bare(widget: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 8);
    row.add_css_class("setting");
    row.append(widget);
    row.upcast()
}

fn model_row(sender: &ComponentSender<App>, title: &str, specs: &str, note: &str, message: Msg) -> (ModelRow, gtk::Widget) {
    let active = label("Active", "pill");
    active.set_valign(gtk::Align::Center);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    heading.append(&label(title, "row-title"));
    heading.append(&active);

    let state = label("", "row-detail");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&heading);
    if !specs.is_empty() {
        text.append(&wrapping(specs, "row-detail"));
    }
    if !note.is_empty() {
        text.append(&wrapping(note, "row-detail"));
    }
    text.append(&state);

    let action = button(sender, "Use", "", message);
    action.set_valign(gtk::Align::Center);
    action.set_width_request(96);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("setting");
    row.append(&text);
    row.append(&action);
    (ModelRow { state, active, action }, row.upcast())
}

fn path_row(sender: &ComponentSender<App>, kind: PathKind) -> (gtk::Entry, gtk::Box) {
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&entry);
    row.append(&button(sender, "Choose", "", Msg::ChoosePath(kind)));
    (entry, row)
}

fn spin(sender: &ComponentSender<App>, min: f64, max: f64, step: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    let sender = sender.clone();
    spin.connect_value_changed(move |_| sender.input(Msg::SettingsEdited));
    spin
}

fn page(groups: &[&gtk::Box]) -> gtk::ScrolledWindow {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_width_request(720);
    column.set_halign(gtk::Align::Center);
    column.set_margin_top(24);
    column.set_margin_bottom(24);
    column.set_margin_start(24);
    column.set_margin_end(24);
    for group in groups {
        column.append(*group);
    }
    // By default a page taller than the window gets its minimum height, which squeezes the
    // version list to nothing and rewraps every label, on each frame of a fold opening.
    let viewport = gtk::Viewport::builder().child(&column).vscroll_policy(gtk::ScrollablePolicy::Natural).build();
    scrolled(&viewport)
}

pub fn build(sender: &ComponentSender<App>) -> (Setup, gtk::Widget) {
    // Index
    let db_label = label("", "row-detail");
    db_label.add_css_class("mono");
    db_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    let index_title = label("No index yet", "index-title");
    let index_pill = label("Active", "pill");
    index_pill.add_css_class("green");
    index_pill.set_valign(gtk::Align::Center);
    let index_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    index_head.append(&index_title);
    index_head.append(&index_pill);

    // One tile per number, the way a dashboard shows them.
    let tiles_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    tiles_box.set_homogeneous(true);
    tiles_box.set_margin_top(6);
    let mut tiles = Vec::new();
    let mut vectors_caption = label("", "tile-caption");
    for caption in ["Classes", "Methods", "Calls", "Method sources", "Data files", "Vectors"] {
        let value = label("", "tile-value");
        let caption = label(caption, "tile-caption");
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
        tile.add_css_class("tile");
        tile.append(&value);
        tile.append(&caption);
        tiles_box.append(&tile);
        tiles.push(value);
        vectors_caption = caption;
    }

    let index_bar = gtk::ProgressBar::new();
    index_bar.set_margin_top(6);
    let index_status = wrapping("", "row-detail");
    let summary = gtk::Box::new(gtk::Orientation::Vertical, 4);
    summary.append(&index_head);
    summary.append(&db_label);
    summary.append(&tiles_box);
    summary.append(&index_bar);
    summary.append(&index_status);

    // Versions
    let installed = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let version_bar = gtk::ProgressBar::new();
    let version_status = wrapping("", "row-detail");
    let version_text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    version_text.set_hexpand(true);
    version_text.set_valign(gtk::Align::Center);
    version_text.append(&version_status);
    version_text.append(&version_bar);
    let version_stop = button(sender, "Stop", "danger", Msg::Stop);
    version_stop.set_valign(gtk::Align::Center);
    let version_line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    version_line.add_css_class("setting");
    version_line.append(&version_text);
    version_line.append(&version_stop);
    // The hairline belongs to the row, it goes away with it.
    let version_progress = gtk::Box::new(gtk::Orientation::Vertical, 0);
    version_progress.append(&separator(gtk::Orientation::Horizontal));
    version_progress.append(&version_line);
    let installed_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    installed_frame.append(&installed);
    installed_frame.append(&version_progress);

    // From Mojang
    let remote_search = gtk::SearchEntry::new();
    remote_search.set_placeholder_text(Some("Filter versions"));
    remote_search.set_hexpand(true);
    let (filter, remote_releases) = segmented(sender, "All", "Releases");
    {
        let sender = sender.clone();
        remote_search.connect_search_changed(move |_| sender.input(Msg::RemoteFilter));
    }
    let remote_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    remote_bar.append(&remote_search);
    remote_bar.append(&filter);
    remote_bar.append(&icon_button(sender, "refresh-cw", "", "flat", Msg::RemoteRefresh));
    let remote_status = wrapping("", "row-detail");
    let remote_head = gtk::Box::new(gtk::Orientation::Vertical, 8);
    remote_head.append(&remote_bar);
    remote_head.append(&remote_status);

    let remote = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let remote_scroll =
        gtk::ScrolledWindow::builder().child(&remote).hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).max_content_height(340).build();

    let decompile = gtk::Switch::new();
    decompile.set_active(true);
    let java = wrapping("", "row-detail");
    let decompile_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    decompile_text.set_hexpand(true);
    decompile_text.set_valign(gtk::Align::Center);
    decompile_text.append(&label("Decompile after download", "row-title"));
    decompile_text.append(&java);
    decompile.set_valign(gtk::Align::Center);
    let decompile_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    decompile_row.add_css_class("setting");
    decompile_row.append(&decompile_text);
    decompile_row.append(&decompile);

    // Files of your own, folded away
    let version = gtk::Entry::new();
    version.set_width_request(220);
    version.set_placeholder_text(Some("26.3"));
    let (jar, jar_row) = path_row(sender, PathKind::Jar);
    let (src, src_row) = path_row(sender, PathKind::Src);
    let (data, data_row) = path_row(sender, PathKind::Data);
    let index_button = button(sender, "Build index", "primary", Msg::BuildIndex);
    let custom_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for widget in [
        row("Version", "Shown in the sidebar and handed to tools.", &version),
        tall_row("Server jar", "The unpacked server jar, the one with the net/minecraft classes in it.", &jar_row),
        tall_row("Decompiled sources", "Optional. Without them there is no source view and nothing to embed.", &src_row),
        tall_row("Data folder", "Optional. The data/ folder with tags, loot tables and registries.", &data_row),
        row("Build", "Reads every class of the jar, resolves calls and cuts the sources into methods. Replaces the current index.", &index_button),
    ] {
        custom_rows.append(&separator(gtk::Orientation::Horizontal));
        custom_rows.append(&widget);
    }
    let custom = gtk::Revealer::builder().child(&custom_rows).transition_type(gtk::RevealerTransitionType::SlideDown).transition_duration(180).build();
    let custom_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    custom_frame.append(&disclosure(
        "Use files of your own",
        "A jar and sources prepared some other way, like a modded server or a remapped older version.",
        &custom,
    ));
    custom_frame.append(&custom);

    let index_page = page(&[
        &group("Index", "", &[&bare(&summary)]),
        &group(
            "Versions",
            "Each one keeps its own index and vectors in the data folder. Use switches Browse, Lookup, Ask, chat and the MCP server to it.",
            &[installed_frame.upcast_ref()],
        ),
        &group(
            "Get a version",
            "Straight from Mojang. Only 26.1 and later, older servers are obfuscated.",
            &[&bare(&remote_head), remote_scroll.upcast_ref(), decompile_row.upcast_ref()],
        ),
        &group("", "", &[custom_frame.upcast_ref()]),
    ]);

    // Models
    let mut models = Vec::new();
    let mut model_rows: Vec<gtk::Widget> = Vec::new();
    for (index, m) in MODELS.iter().enumerate() {
        let specs = format!(
            "{}M parameters  ·  {} MB  ·  {} token context  ·  {} dimensions stored  ·  {}",
            m.params_millions, m.download_mb, m.context_tokens, m.stored_dims, m.license
        );
        let (row, widget) = model_row(sender, m.name, &specs, m.note, Msg::ModelAction(index));
        models.push(row);
        model_rows.push(widget);
    }

    let threads = spin(sender, 1.0, std::thread::available_parallelism().map_or(8.0, |n| n.get() as f64), 1.0);
    let embed_button = button(sender, "Embed", "primary", Msg::Embed);
    let stop_button = button(sender, "Stop", "danger", Msg::Stop);
    let job_bar = gtk::ProgressBar::new();
    let job_status = wrapping("", "row-detail");
    let embed_text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    embed_text.set_hexpand(true);
    embed_text.set_valign(gtk::Align::Center);
    embed_text.append(&label("Embed the index", "row-title"));
    embed_text.append(&wrapping("Runs the active model over every method. Safe to stop, a later run only does what is missing.", "row-detail"));
    embed_text.append(&job_bar);
    embed_text.append(&job_status);
    let embed_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    embed_buttons.set_valign(gtk::Align::Center);
    embed_buttons.append(&stop_button);
    embed_buttons.append(&embed_button);
    let embed_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    embed_row.add_css_class("setting");
    embed_row.append(&embed_text);
    embed_row.append(&embed_buttons);
    model_rows.push(embed_row.upcast());

    let mut rerankers = Vec::new();
    let mut reranker_rows: Vec<gtk::Widget> = Vec::new();
    let (off, off_widget) = model_row(sender, "Off", "", "Results come straight from the embedding and full text search.", Msg::RerankerAction(0));
    rerankers.push(off);
    reranker_rows.push(off_widget);
    for (index, r) in RERANKERS.iter().enumerate() {
        let specs = format!("{}M parameters  ·  {} MB  ·  reads {} tokens per candidate  ·  {}", r.params_millions, r.download_mb, r.pair_tokens, r.license);
        let (row, widget) = model_row(sender, r.name, &specs, r.note, Msg::RerankerAction(index + 1));
        rerankers.push(row);
        reranker_rows.push(widget);
    }

    let model_refs: Vec<&gtk::Widget> = model_rows.iter().collect();
    let reranker_refs: Vec<&gtk::Widget> = reranker_rows.iter().collect();
    let models_page = page(&[
        &group("Embedding model", "Turns methods and questions into vectors. Runs on the CPU, nothing leaves this machine.", &model_refs),
        &group(
            "Reranker",
            "Reads the question next to every candidate and puts the ones that answer it first. Adds a few seconds to a question.",
            &reranker_refs,
        ),
    ]);

    // Search
    let results = spin(sender, 5.0, 60.0, 1.0);
    let rerank_pool = spin(sender, 10.0, 120.0, 5.0);
    let search_page = page(&[&group(
        "Search",
        "Used by Ask here, by `lantern ask` and by the MCP server.",
        &[
            &row("Results", "How many answers a question returns.", &results),
            &row("Reranker candidates", "How many results the reranker reads. More finds a little more and takes longer.", &rerank_pool),
            &row("Threads", "For embedding and reranking. Leave some cores for the rest of the machine.", &threads),
        ],
    )]);

    // Chat
    let names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
    let chat_provider = gtk::DropDown::from_strings(&names);
    chat_provider.set_width_request(220);
    {
        let sender = sender.clone();
        chat_provider.connect_selected_notify(move |dropdown| sender.input(Msg::ProviderSelected(dropdown.selected() as usize)));
    }
    let chat_note = wrapping("", "row-detail");
    let chat_url = gtk::Entry::new();
    chat_url.set_hexpand(true);
    let chat_key = gtk::PasswordEntry::new();
    chat_key.set_show_peek_icon(true);
    chat_key.set_hexpand(true);

    let chat_model = gtk::Entry::new();
    chat_model.set_hexpand(true);
    let model_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    model_line.append(&chat_model);
    model_line.append(&button(sender, "Load models", "", Msg::LoadModels));
    // Filled by Load models. Searchable, the big providers list hundreds.
    let chat_models = gtk::DropDown::from_strings(&[]);
    chat_models.set_enable_search(true);
    chat_models.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    chat_models.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), gtk::Expression::NONE, "string")));
    chat_models.set_visible(false);
    {
        let sender = sender.clone();
        chat_models.connect_selected_notify(move |dropdown| {
            if let Some(id) = dropdown.selected_item().and_downcast::<gtk::StringObject>() {
                sender.input(Msg::ModelPicked(id.string().into()));
            }
        });
    }
    let chat_models_status = wrapping("", "row-detail");
    chat_models_status.set_visible(false);
    let model_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    model_box.append(&model_line);
    model_box.append(&chat_models);
    model_box.append(&chat_models_status);

    let chat_rounds = spin(sender, 1.0, 40.0, 1.0);
    for entry in [&chat_url, &chat_model] {
        let sender = sender.clone();
        entry.connect_changed(move |_| sender.input(Msg::SettingsEdited));
    }
    {
        let sender = sender.clone();
        chat_key.connect_changed(move |_| sender.input(Msg::SettingsEdited));
    }
    let provider_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    provider_text.set_hexpand(true);
    provider_text.append(&label("Provider", "row-title"));
    provider_text.append(&chat_note);
    chat_provider.set_valign(gtk::Align::Center);
    let provider_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    provider_row.add_css_class("setting");
    provider_row.append(&provider_text);
    provider_row.append(&chat_provider);
    let chat_page = page(&[&group(
        "Chat",
        "A model that answers questions by using Lantern as its tools. Key and model are kept per provider.",
        &[
            provider_row.upcast_ref(),
            &tall_row("Address", "The base URL up to and including /v1. Filled in for the known providers.", &chat_url),
            &tall_row("API key", "Stored in the settings file, readable by your user only. LANTERN_API_KEY overrides it.", &chat_key),
            &tall_row("Model", "The model id as the provider names it. It has to support tool calling.", &model_box),
            &row("Tool rounds", "How many times the model may look things up before it has to answer.", &chat_rounds),
        ],
    )]);

    // Storage
    let storage = wrapping("", "row-title");
    storage.add_css_class("mono");
    let open_folder = button(sender, "Open folder", "", Msg::OpenDataFolder);
    let resets: Vec<(Reset, gtk::Button, &'static str)> = [(Reset::Vectors, "Delete"), (Reset::Models, "Delete"), (Reset::Everything, "Reset")]
        .into_iter()
        .map(|(kind, text)| (kind, button(sender, text, "danger", Msg::Reset(kind)), text))
        .collect();
    let reset = |kind: Reset| resets.iter().find(|(k, _, _)| *k == kind).map(|(_, b, _)| b.clone()).expect("every kind has a button");
    let storage_page = page(&[
        &group(
            "Storage",
            "",
            &[&bare(&storage), &row("Data folder", "The versions with their indexes, the models, the vectors, the chats and the settings.", &open_folder)],
        ),
        &group(
            "Reset",
            "Asks before it deletes anything. None of it can be undone.",
            &[
                &row("Vectors", "The embeddings of every version. Ask needs them made again, which takes a while.", &reset(Reset::Vectors)),
                &row("Models", "The downloaded embedding models and rerankers. They are fetched again when used.", &reset(Reset::Models)),
                &row(
                    "Everything",
                    "Versions, indexes, vectors, models, chats and settings, API keys included. Lantern starts over as on the first run.",
                    &reset(Reset::Everything),
                ),
            ],
        ),
    ]);

    let identity = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    identity.add_css_class("setting");
    identity.append(&crate::theme::brand::image(64));
    let identity_text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    identity_text.set_valign(gtk::Align::Center);
    identity_text.append(&label("Lantern", "about-title"));
    identity_text.append(&label(&format!("Version {}", lantern_core::VERSION), "row-detail"));
    identity.append(&identity_text);
    let repository = gtk::LinkButton::with_label(lantern_core::REPOSITORY, "GitHub");
    let license = gtk::LinkButton::with_label(&format!("{}/blob/master/LICENSE", lantern_core::REPOSITORY), "GPL-3.0-only");
    let update_check = button(sender, "Check for updates", "", Msg::CheckForUpdates);
    let update_status = wrapping("Checks GitHub for the latest stable release.", "row-detail");
    let update_link = gtk::LinkButton::with_label(&format!("{}/releases/latest", lantern_core::REPOSITORY), "Download update");
    update_link.set_visible(false);
    update_link.set_halign(gtk::Align::Start);
    let update_details = gtk::Box::new(gtk::Orientation::Vertical, 8);
    update_details.append(&update_status);
    update_details.append(&update_link);
    let about_page = page(&[
        &group("", "", &[identity.upcast_ref()]),
        &group("Updates", "", &[&row("Lantern updates", "", &update_check), &bare(&update_details)]),
        &group("Project", "", &[&row("Source code", "", &repository), &row("License", "", &license)]),
    ]);

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(100);
    let pages: [(&str, &gtk::ScrolledWindow); 6] =
        [("Index", &index_page), ("Models", &models_page), ("Search", &search_page), ("Chat", &chat_page), ("Storage", &storage_page), ("About", &about_page)];
    for (title, content) in pages {
        stack.add_named(content, Some(title));
    }

    let setup = Setup {
        pages: stack.clone(),
        db_label,
        index_title,
        index_pill,
        tiles,
        tiles_box,
        vectors_caption,
        index_bar,
        index_status,
        installed,
        version_progress,
        version_bar,
        version_status,
        version_stop,
        remote,
        remote_search,
        remote_releases,
        remote_status,
        decompile,
        java,
        version,
        jar,
        src,
        data,
        index_button,
        models,
        rerankers,
        embed_button,
        stop_button,
        job_bar,
        job_status,
        threads,
        results,
        rerank_pool,
        chat_provider,
        chat_note,
        chat_models,
        chat_models_status,
        chat_url,
        chat_key,
        chat_model,
        chat_rounds,
        storage,
        resets,
        update_check,
        update_status,
        update_link,
    };
    (setup, stack.upcast())
}

/// Two choices on one track. Returns the track and the second choice, which is on when the
/// first is off.
fn segmented(sender: &ComponentSender<App>, first: &str, second: &str) -> (gtk::Box, gtk::ToggleButton) {
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("segmented");
    track.set_valign(gtk::Align::Center);
    let a = gtk::ToggleButton::with_label(first);
    let b = gtk::ToggleButton::with_label(second);
    b.set_group(Some(&a));
    a.set_active(true);
    let sender = sender.clone();
    b.connect_toggled(move |_| sender.input(Msg::RemoteFilter));
    track.append(&a);
    track.append(&b);
    (track, b)
}

/// A row that folds `revealer` open and shut, with a chevron that says which.
fn disclosure(title: &str, description: &str, revealer: &gtk::Revealer) -> gtk::Button {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&label(title, "row-title"));
    text.append(&wrapping(description, "row-detail"));
    let chevron = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    chevron.set_valign(gtk::Align::Center);
    chevron.append(&icons::icon("chevron-right"));
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    line.append(&text);
    line.append(&chevron);
    let button = gtk::Button::builder().child(&line).css_classes(["disclosure"]).build();
    let revealer = revealer.clone();
    button.connect_clicked(move |_| {
        let open = !revealer.reveals_child();
        revealer.set_reveal_child(open);
        if let Some(old) = chevron.first_child() {
            chevron.remove(&old);
        }
        chevron.append(&icons::icon(if open { "chevron-down" } else { "chevron-right" }));
    });
    button
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn kind_name(kind: &str) -> &str {
    match kind {
        "release" => "Release",
        "snapshot" => "Snapshot",
        "" => "Version",
        "custom" => "Custom",
        other => other,
    }
}

/// One row per version in the data folder. `active` is the index that is open, `busy` the
/// version a download or decompile is working on.
pub fn fill_installed(
    setup: &Setup,
    sender: &ComponentSender<App>,
    versions: &[Installed],
    notes: &[Option<String>],
    active: &Path,
    busy: Option<&str>,
    idle: bool,
) {
    clear(&setup.installed);
    if versions.is_empty() {
        setup.installed.append(&bare(&wrapping("Nothing downloaded yet. Pick a version below.", "row-detail")));
        return;
    }
    for (index, version) in versions.iter().enumerate() {
        if index > 0 {
            setup.installed.append(&separator(gtk::Orientation::Horizontal));
        }
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        heading.append(&label(&version.id, "row-title"));
        let is_active = version.index.as_deref() == Some(active);
        let pill = match (is_active, version.index.is_some()) {
            (true, _) => Some(("Active", "pill green")),
            (false, true) => Some(("Indexed", "pill")),
            _ => None,
        };
        if let Some((text, classes)) = pill {
            let pill = label(text, "");
            for class in classes.split(' ') {
                pill.add_css_class(class);
            }
            pill.set_valign(gtk::Align::Center);
            heading.append(&pill);
        }
        let mut detail = vec![kind_name(&version.kind).to_owned()];
        if !version.release_time.is_empty() {
            detail.push(version.release_time.get(..10).unwrap_or(&version.release_time).to_owned());
        }
        if version.jar.is_some() {
            detail.push(if version.src.is_some() { "jar and sources".to_owned() } else { "jar only, not decompiled".to_owned() });
        }
        detail.push(format!("{:.0} MB", version.bytes as f64 / 1_048_576.0));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&heading);
        text.append(&wrapping(&detail.join("  ·  "), "row-detail"));
        if let Some(Some(note)) = notes.get(index) {
            text.append(&wrapping(note, "row-detail"));
        }

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_valign(gtk::Align::Center);
        let working = busy == Some(version.id.as_str());
        if version.jar.is_some() && version.src.is_none() {
            let decompile = button(sender, "Decompile", "", Msg::VersionDecompile(version.id.clone()));
            decompile.set_sensitive(idle);
            actions.append(&decompile);
        }
        if version.jar.is_some() {
            let indexed = version.index.is_some();
            let build =
                button(sender, if indexed { "Rebuild" } else { "Build index" }, if indexed { "" } else { "primary" }, Msg::VersionBuild(version.id.clone()));
            build.set_sensitive(idle);
            actions.append(&build);
        }
        if version.index.is_some() && !is_active {
            let switch = button(sender, "Use", "primary", Msg::VersionUse(version.id.clone()));
            switch.set_tooltip_text(Some("Open this version's index everywhere: Browse, Lookup, Ask, chat and the MCP server"));
            switch.set_sensitive(idle);
            actions.append(&switch);
        }
        let open = icon_button(sender, "folder", "", "flat", Msg::VersionOpen(version.id.clone()));
        open.set_tooltip_text(Some("Open the folder"));
        actions.append(&open);
        let delete = icon_button(sender, "x", "", "flat", Msg::VersionDelete(version.id.clone()));
        delete.set_tooltip_text(Some("Delete"));
        delete.set_sensitive(!working);
        actions.append(&delete);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        row.add_css_class("setting");
        row.add_css_class("enter");
        row.append(&text);
        row.append(&actions);
        setup.installed.append(&row);
    }
}

/// The versions Mojang offers, narrowed by the filter and the snapshot switch.
pub fn fill_remote(setup: &Setup, sender: &ComponentSender<App>, versions: &[Remote], installed: &[Installed], idle: bool) {
    clear(&setup.remote);
    let needle = setup.remote_search.text().trim().to_lowercase();
    let snapshots = !setup.remote_releases.is_active();
    let shown: Vec<&Remote> =
        versions.iter().filter(|v| snapshots || v.kind == "release").filter(|v| needle.is_empty() || v.id.to_lowercase().contains(&needle)).collect();
    if shown.is_empty() && !versions.is_empty() {
        setup.remote.append(&bare(&wrapping("No version matches.", "row-detail")));
    }
    for (index, v) in shown.into_iter().enumerate() {
        if index > 0 {
            setup.remote.append(&separator(gtk::Orientation::Horizontal));
        }
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        heading.append(&label(&v.id, "row-title"));
        if v.kind == "release" {
            let pill = label("Release", "pill");
            pill.set_valign(gtk::Align::Center);
            heading.append(&pill);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&heading);
        // A release already says so in its pill.
        let detail = if v.kind == "release" { v.date().to_owned() } else { format!("{}  ·  {}", kind_name(&v.kind), v.date()) };
        text.append(&label(&detail, "row-detail"));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        row.add_css_class("setting");
        row.add_css_class("compact");
        row.append(&text);
        // Only an index left, the jar and sources were deleted: it can be downloaded again.
        if installed.iter().any(|i| i.id == v.id && i.jar.is_some()) {
            let done = label("Downloaded", "row-detail");
            done.set_valign(gtk::Align::Center);
            row.append(&done);
        } else {
            let download = icon_button(sender, "download", "Download", "", Msg::VersionDownload(v.id.clone()));
            download.set_valign(gtk::Align::Center);
            download.set_sensitive(idle);
            row.append(&download);
        }
        setup.remote.append(&row);
    }
}
