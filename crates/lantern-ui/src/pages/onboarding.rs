//! The first run: pick versions, the models and optionally a chat model, then everything is
//! downloaded, decompiled, indexed and embedded in one go, with the progress of every step.

use std::{cell::RefCell, collections::BTreeSet, path::Path, rc::Rc, time::Instant};

use gtk::prelude::*;
use lantern_core::{
    settings::PROVIDERS,
    versions::{self, Remote},
};
use lantern_embed::models::{DEFAULT_MODEL, DEFAULT_RERANKER, MODELS, ModelSpec, RERANKERS, RerankerSpec};
use relm4::ComponentSender;

use crate::{
    app::{App, Msg},
    theme::icons::{self, colored},
    view::{button, label, scrolled, separator, window_controls, wrapping},
};

const STEPS: [&str; 4] = ["Versions", "Models", "Chat", "Review"];

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Next,
    Back,
    Filter,
    /// A version was ticked or unticked.
    Picked,
    /// Leave a failed setup for the app as it is.
    Leave,
    Retry,
}

pub use lantern_embed::setup::{Event, Task, Unit};

pub struct Widgets {
    pub root: gtk::Box,
    steps: gtk::Stack,
    /// Per step its number in a circle, its name and the line that leads to it.
    dots: Vec<(Dot, gtk::Label, Option<gtk::Box>)>,
    back: gtk::Button,
    pub next: gtk::Button,
    footer: gtk::Box,

    filter: gtk::SearchEntry,
    releases: gtk::ToggleButton,
    versions: gtk::Box,
    versions_status: gtk::Label,
    picked_label: gtk::Label,
    /// Ticked versions by id, kept when the list is filtered and drawn again.
    picked: Rc<RefCell<BTreeSet<String>>>,

    models: Vec<gtk::CheckButton>,
    rerankers: Vec<gtk::CheckButton>,

    provider: gtk::DropDown,
    address: gtk::Entry,
    key: gtk::PasswordEntry,
    model: gtk::Entry,

    review: gtk::Box,
    embed: gtk::Switch,

    tasks: gtk::Box,
    run_title: gtk::Label,
    run_bar: gtk::ProgressBar,
}

/// A step's circle: its number, or a check once it is done.
struct Dot {
    circle: gtk::Box,
    number: gtk::Label,
    check: gtk::Image,
}

/// Where the setup is.
#[derive(Default)]
pub struct State {
    step: usize,
    pub tasks: Vec<Task>,
    rows: Vec<(gtk::Box, gtk::Label)>,
    pub running: Option<usize>,
    pub failed: Option<usize>,
    /// Skip embedding was pressed, the embedding that is left is not done.
    pub skipping: bool,
    done: usize,
    /// When the phase that runs started and how far it was then, for rates.
    phase: Option<(String, Instant, u64)>,
}

fn action(sender: &ComponentSender<App>, text: &str, class: &str, action: Action) -> gtk::Button {
    button(sender, text, class, Msg::Onboarding(action))
}

fn section(title: &str, description: &str) -> gtk::Box {
    let head = gtk::Box::new(gtk::Orientation::Vertical, 4);
    head.append(&label(title, "group-title"));
    if !description.is_empty() {
        head.append(&wrapping(description, "group-description"));
    }
    head
}

fn framed(content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("group");
    frame.set_margin_top(8);
    frame.append(content);
    frame
}

/// A row with a radio or check in front of a name and what it is.
fn choice(check: &gtk::CheckButton, radio: bool, title: &str, details: &[&str]) -> gtk::Box {
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&label(title, "row-title"));
    for detail in details.iter().filter(|d| !d.is_empty()) {
        text.append(&wrapping(detail, "row-detail"));
    }
    check.set_valign(gtk::Align::Start);
    check.set_margin_top(2);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("setting");
    row.append(check);
    row.append(&text);
    // The whole row ticks the box, not only the box.
    let click = gtk::GestureClick::new();
    {
        let check = check.clone();
        click.connect_released(move |_, _, _, _| {
            if radio {
                check.set_active(true);
            } else {
                check.set_active(!check.is_active());
            }
        });
    }
    text.add_controller(click);
    row
}

pub fn build(window: &gtk::Window, sender: &ComponentSender<App>) -> Widgets {
    // A bar to drag the window by, the window has no titlebar of its own.
    let brand = label("Lantern", "brand");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.add_css_class("topbar");
    bar.set_margin_start(8);
    bar.append(&crate::theme::brand::image(25));
    bar.append(&brand);
    bar.append(&spacer);
    bar.append(&window_controls(window));
    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&bar));

    let title = label("Set up Lantern", "onboard-title");
    let subtitle =
        wrapping("Pick the Minecraft versions to read and the models that search them. Everything is downloaded once and stays on this machine.", "muted");
    // A stepper: numbered circles joined by lines, the way a checkout shows where it is.
    let dots_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    dots_box.add_css_class("stepper");
    dots_box.set_margin_top(18);
    let dots: Vec<(Dot, gtk::Label, Option<gtk::Box>)> = STEPS
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let line = (index > 0).then(|| {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                line.add_css_class("step-line");
                line.set_hexpand(true);
                line.set_valign(gtk::Align::Center);
                dots_box.append(&line);
                line
            });
            let number = gtk::Label::new(Some(&(index + 1).to_string()));
            let check = colored("check", "#3fb950");
            check.set_pixel_size(13);
            check.set_visible(false);
            let circle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            circle.add_css_class("step-dot");
            circle.set_size_request(24, 24);
            circle.set_hexpand(false);
            circle.set_halign(gtk::Align::Center);
            circle.set_valign(gtk::Align::Center);
            for part in [number.upcast_ref::<gtk::Widget>(), check.upcast_ref()] {
                part.set_halign(gtk::Align::Center);
                part.set_valign(gtk::Align::Center);
                part.set_hexpand(true);
            }
            circle.append(&number);
            circle.append(&check);
            let name = label(name, "step-name");
            name.set_valign(gtk::Align::Center);
            dots_box.append(&circle);
            dots_box.append(&name);
            (Dot { circle, number, check }, name, line)
        })
        .collect();

    // Versions
    let filter = gtk::SearchEntry::new();
    filter.set_placeholder_text(Some("Filter versions"));
    filter.set_hexpand(true);
    {
        let sender = sender.clone();
        filter.connect_search_changed(move |_| sender.input(Msg::Onboarding(Action::Filter)));
    }
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("segmented");
    track.set_valign(gtk::Align::Center);
    let all = gtk::ToggleButton::with_label("All");
    let releases = gtk::ToggleButton::with_label("Releases");
    releases.set_group(Some(&all));
    releases.set_active(true);
    {
        let sender = sender.clone();
        releases.connect_toggled(move |_| sender.input(Msg::Onboarding(Action::Filter)));
    }
    track.append(&all);
    track.append(&releases);
    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    filter_bar.add_css_class("setting");
    filter_bar.append(&filter);
    filter_bar.append(&track);
    let versions = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let versions_scroll = gtk::ScrolledWindow::builder()
        .child(&versions)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(330)
        .build();
    let versions_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    versions_frame.append(&filter_bar);
    versions_frame.append(&separator(gtk::Orientation::Horizontal));
    versions_frame.append(&versions_scroll);
    let versions_status = wrapping("Asking Mojang for the versions ...", "row-detail");
    let picked_label = wrapping("", "row-detail");
    let java = match versions::find_java() {
        Some(java) => format!("Decompiled with Vineflower, which runs on {}.", java.display()),
        None => "No Java found. Install Java 21 or newer first, without it there is no source view and nothing to embed.".to_owned(),
    };
    let versions_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    versions_page.append(&section(
        "Versions",
        "One or more. Each is downloaded from Mojang, decompiled and indexed on its own, the newest opens when it is done. Only 26.1 and later, older servers are obfuscated.",
    ));
    versions_page.append(&framed(&versions_frame));
    versions_page.append(&versions_status);
    versions_page.append(&picked_label);
    versions_page.append(&wrapping(&java, "row-detail"));

    // Models
    let model_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let mut models: Vec<gtk::CheckButton> = Vec::new();
    for (index, spec) in MODELS.iter().enumerate() {
        let check = gtk::CheckButton::new();
        if let Some(first) = models.first() {
            check.set_group(Some(first));
        }
        check.set_active(spec.id == DEFAULT_MODEL);
        if index > 0 {
            model_rows.append(&separator(gtk::Orientation::Horizontal));
        }
        let specs = format!("{}M parameters  ·  {} MB  ·  {} token context  ·  {}", spec.params_millions, spec.download_mb, spec.context_tokens, spec.license);
        let title = if spec.id == DEFAULT_MODEL { format!("{}  ·  recommended", spec.name) } else { spec.name.to_owned() };
        model_rows.append(&choice(&check, true, &title, &[&specs, spec.note]));
        models.push(check);
    }
    let reranker_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let mut rerankers: Vec<gtk::CheckButton> = Vec::new();
    for (index, spec) in RERANKERS.iter().enumerate() {
        let check = gtk::CheckButton::new();
        if let Some(first) = rerankers.first() {
            check.set_group(Some(first));
        }
        check.set_active(spec.id == DEFAULT_RERANKER);
        if index > 0 {
            reranker_rows.append(&separator(gtk::Orientation::Horizontal));
        }
        let specs = format!("{}M parameters  ·  {} MB  ·  {}", spec.params_millions, spec.download_mb, spec.license);
        let title = if spec.id == DEFAULT_RERANKER { format!("{}  ·  recommended", spec.name) } else { spec.name.to_owned() };
        reranker_rows.append(&choice(&check, true, &title, &[&specs, spec.note]));
        rerankers.push(check);
    }
    let models_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    models_page.append(&section("Embedding model", "Turns methods and questions into vectors, so Ask finds code by what it does. Runs on the CPU."));
    models_page.append(&framed(&model_rows));
    let reranker_head = section("Reranker", "Reads the question next to every candidate and puts the ones that answer it first.");
    reranker_head.set_margin_top(18);
    models_page.append(&reranker_head);
    models_page.append(&framed(&reranker_rows));

    // Chat
    let names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
    let provider = gtk::DropDown::from_strings(&names);
    provider.set_width_request(220);
    provider.set_valign(gtk::Align::Center);
    let provider_note = wrapping(PROVIDERS[0].note, "row-detail");
    let address = gtk::Entry::new();
    address.set_text(PROVIDERS[0].base_url);
    let key = gtk::PasswordEntry::new();
    key.set_show_peek_icon(true);
    let model = gtk::Entry::new();
    model.set_placeholder_text(Some(PROVIDERS[0].example_model));
    {
        let (note, address, model) = (provider_note.clone(), address.clone(), model.clone());
        provider.connect_selected_notify(move |dropdown| {
            let provider = &PROVIDERS[dropdown.selected() as usize];
            note.set_text(provider.note);
            address.set_text(provider.base_url);
            model.set_placeholder_text(Some(provider.example_model));
        });
    }
    let provider_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    provider_text.set_hexpand(true);
    provider_text.append(&label("Provider", "row-title"));
    provider_text.append(&provider_note);
    let provider_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    provider_row.add_css_class("setting");
    provider_row.append(&provider_text);
    provider_row.append(&provider);
    let field = |title: &str, detail: &str, control: &gtk::Widget| {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.add_css_class("setting");
        row.append(&label(title, "row-title"));
        row.append(&wrapping(detail, "row-detail"));
        row.append(control);
        row
    };
    let chat_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chat_rows.append(&provider_row);
    for row in [
        field("Address", "The base URL up to and including /v1.", address.upcast_ref()),
        field("API key", "Kept in the settings file, readable by your user only.", key.upcast_ref()),
        field("Model", "As the provider names it. It has to support tool calling.", model.upcast_ref()),
    ] {
        chat_rows.append(&separator(gtk::Orientation::Horizontal));
        chat_rows.append(&row);
    }
    let chat_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    chat_page.append(&section("Chat, optional", "A model that answers questions by looking things up in Lantern. Leave it empty to skip, Setup has it later."));
    chat_page.append(&framed(&chat_rows));

    // Review
    let review = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let embed = gtk::Switch::new();
    embed.set_active(true);
    embed.set_valign(gtk::Align::Center);
    let embed_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    embed_text.set_hexpand(true);
    embed_text.append(&label("Embed every version", "row-title"));
    embed_text.append(&wrapping(
        "What Ask searches. Runs a while on the CPU per version, the newest first. It can be skipped while it runs and finished later from Setup.",
        "row-detail",
    ));
    let embed_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    embed_row.add_css_class("setting");
    embed_row.append(&embed_text);
    embed_row.append(&embed);
    let review_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    review_frame.append(&review);
    review_frame.append(&separator(gtk::Orientation::Horizontal));
    review_frame.append(&embed_row);
    let review_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    review_page.append(&section("Review", "This is what happens when you go on. It can take a few minutes per version."));
    review_page.append(&framed(&review_frame));

    // Run
    let run_title = label("Setting up ...", "row-title");
    let run_bar = gtk::ProgressBar::new();
    let tasks = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let run_head = gtk::Box::new(gtk::Orientation::Vertical, 8);
    run_head.add_css_class("setting");
    run_head.append(&run_title);
    run_head.append(&run_bar);
    let run_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    run_frame.append(&run_head);
    run_frame.append(&separator(gtk::Orientation::Horizontal));
    run_frame.append(&tasks);
    let run_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    run_page.append(&section("Setting up", "Lantern opens when everything is done."));
    run_page.append(&framed(&run_frame));

    let steps = gtk::Stack::new();
    steps.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    steps.set_transition_duration(200);
    steps.set_vhomogeneous(false);
    for (name, page) in [("0", &versions_page), ("1", &models_page), ("2", &chat_page), ("3", &review_page), ("run", &run_page)] {
        steps.add_named(page, Some(name));
    }

    let back = action(sender, "Back", "flat", Action::Back);
    let next = action(sender, "Next", "primary", Action::Next);
    next.set_width_request(120);
    let footer_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer_spacer.set_hexpand(true);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_margin_top(16);
    footer.append(&back);
    footer.append(&footer_spacer);
    footer.append(&next);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_width_request(760);
    column.set_halign(gtk::Align::Center);
    column.set_margin_top(36);
    column.set_margin_bottom(36);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.append(&title);
    column.append(&subtitle);
    column.append(&dots_box);
    column.append(&steps);
    column.append(&footer);
    steps.set_margin_top(18);
    // Same reason as the Setup pages: a taller page than the window would get its minimum.
    let viewport = gtk::Viewport::builder().child(&column).vscroll_policy(gtk::ScrollablePolicy::Natural).build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("onboarding");
    root.append(&handle);
    root.append(&scrolled(&viewport));

    let widgets = Widgets {
        root,
        steps,
        dots,
        back,
        next,
        footer,
        filter,
        releases,
        versions,
        versions_status,
        picked_label,
        picked: Rc::default(),
        models,
        rerankers,
        provider,
        address,
        key,
        model,
        review,
        embed,
        tasks,
        run_title,
        run_bar,
    };
    show_step(&State::default(), &widgets);
    widgets
}

/// Back to the first step, for a start or after Reset.
pub fn restart(state: &mut State, widgets: &Widgets) {
    *state = State::default();
    widgets.picked.borrow_mut().clear();
    reset_footer(widgets);
    show_step(state, widgets);
}

fn show_step(state: &State, widgets: &Widgets) {
    let running = state.step >= STEPS.len();
    widgets.steps.set_visible_child_name(&if running { "run".to_owned() } else { state.step.to_string() });
    for (index, (dot, name, line)) in widgets.dots.iter().enumerate() {
        let class = match index.cmp(&state.step) {
            std::cmp::Ordering::Less => Some("done"),
            std::cmp::Ordering::Equal => Some("active"),
            std::cmp::Ordering::Greater => None,
        };
        dot.number.set_visible(class != Some("done"));
        dot.check.set_visible(class == Some("done"));
        for widget in [dot.circle.upcast_ref::<gtk::Widget>(), name.upcast_ref()].into_iter().chain(line.as_ref().map(|l| l.upcast_ref())) {
            widget.remove_css_class("active");
            widget.remove_css_class("done");
            // The line into a step lights up once the step is reached.
            if let Some(class) = class {
                widget.add_css_class(class);
            }
        }
    }
    widgets.back.set_visible(state.step > 0 && !running);
    widgets.next.set_label(match state.step {
        2 if widgets.key.text().is_empty() && widgets.model.text().trim().is_empty() => "Skip",
        3 => "Set up",
        _ => "Next",
    });
    widgets.footer.set_visible(!running);
    widgets.next.set_sensitive(state.step != 0 || !widgets.picked.borrow().is_empty());
}

/// The version list, from what Mojang said. The newest release is ticked at the start.
pub fn fill_versions(widgets: &Widgets, remote: &[Remote], sender: &ComponentSender<App>, error: Option<&str>) {
    while let Some(child) = widgets.versions.first_child() {
        widgets.versions.remove(&child);
    }
    widgets.versions_status.set_visible(remote.is_empty() || error.is_some());
    if let Some(error) = error {
        widgets.versions_status.set_text(&format!("Could not reach Mojang. {error}"));
    }
    if widgets.picked.borrow().is_empty() {
        if let Some(newest) = remote.iter().find(|v| v.kind == "release") {
            widgets.picked.borrow_mut().insert(newest.id.clone());
        }
    }
    let needle = widgets.filter.text().trim().to_lowercase();
    let releases = widgets.releases.is_active();
    let shown = remote.iter().filter(|v| !releases || v.kind == "release").filter(|v| needle.is_empty() || v.id.to_lowercase().contains(&needle));
    for (index, version) in shown.enumerate() {
        if index > 0 {
            widgets.versions.append(&separator(gtk::Orientation::Horizontal));
        }
        let check = gtk::CheckButton::new();
        check.set_active(widgets.picked.borrow().contains(&version.id));
        {
            let (picked, id, sender) = (widgets.picked.clone(), version.id.clone(), sender.clone());
            check.connect_toggled(move |check| {
                if check.is_active() {
                    picked.borrow_mut().insert(id.clone());
                } else {
                    picked.borrow_mut().remove(&id);
                }
                sender.input(Msg::Onboarding(Action::Picked));
            });
        }
        let kind = if version.kind == "release" { "Release" } else { "Snapshot" };
        let row = choice(&check, false, &version.id, &[&format!("{kind}  ·  {}", version.date())]);
        row.add_css_class("compact");
        widgets.versions.append(&row);
    }
    show_picked(widgets);
}

fn show_picked(widgets: &Widgets) {
    let picked = widgets.picked.borrow();
    widgets.picked_label.set_text(&match picked.len() {
        0 => "Pick at least one.".to_owned(),
        _ => format!(
            "{}: {}",
            if picked.len() == 1 { "Picked" } else { "Picked, the newest opens first" },
            picked.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
    });
    widgets.next.set_sensitive(!picked.is_empty() || widgets.steps.visible_child_name().as_deref() != Some("0"));
}

/// The chat settings as entered, None when left empty.
pub fn chat(widgets: &Widgets) -> Option<(usize, String, String, String)> {
    let (key, model) = (widgets.key.text().to_string(), widgets.model.text().trim().to_owned());
    (!key.is_empty() || !model.is_empty()).then(|| (widgets.provider.selected() as usize, widgets.address.text().trim().to_owned(), key, model))
}

pub fn embedding(widgets: &Widgets) -> &'static ModelSpec {
    widgets.models.iter().position(|c| c.is_active()).map_or(&MODELS[0], |i| &MODELS[i])
}

pub fn reranker(widgets: &Widgets) -> &'static RerankerSpec {
    widgets.rerankers.iter().position(|c| c.is_active()).map_or(&RERANKERS[0], |i| &RERANKERS[i])
}

/// Next or Back on the pages before the run. Returns true when the run should start.
pub fn step(state: &mut State, widgets: &Widgets, forward: bool, remote: &[Remote], data_dir: &Path) -> bool {
    if forward && state.step == 3 {
        return true;
    }
    state.step = if forward { state.step + 1 } else { state.step.saturating_sub(1) };
    if state.step == 3 {
        fill_review(widgets, remote, data_dir);
    }
    show_step(state, widgets);
    false
}

/// The versions picked, newest first.
pub fn picked(widgets: &Widgets, remote: &[Remote]) -> Vec<Remote> {
    let picked = widgets.picked.borrow();
    remote.iter().filter(|v| picked.contains(&v.id)).cloned().collect()
}

fn fill_review(widgets: &Widgets, remote: &[Remote], data_dir: &Path) {
    while let Some(child) = widgets.review.first_child() {
        widgets.review.remove(&child);
    }
    let versions = picked(widgets, remote);
    let (embedding, reranker) = (embedding(widgets), reranker(widgets));
    let chat = chat(widgets).map_or_else(
        || "Not set up, Setup has it later".to_owned(),
        |(provider, _, _, model)| format!("{}  ·  {}", PROVIDERS[provider].name, if model.is_empty() { "no model yet" } else { &model }),
    );
    let downloads = |downloaded: bool, mb: u32| if downloaded { "already downloaded".to_owned() } else { format!("{mb} MB") };
    let rows = [
        ("Versions", format!("{}  ·  about 60 MB each, then decompiled and indexed", versions.iter().map(|v| v.id.as_str()).collect::<Vec<_>>().join(", "))),
        (
            "Embedding model",
            format!("{}  ·  {}", embedding.name, downloads(lantern_embed::download::is_downloaded(embedding.source(), data_dir), embedding.download_mb)),
        ),
        ("Reranker", format!("{}  ·  {}", reranker.name, downloads(lantern_embed::download::is_downloaded(reranker.source(), data_dir), reranker.download_mb))),
        ("Chat", chat),
    ];
    for (index, (title, value)) in rows.into_iter().enumerate() {
        if index > 0 {
            widgets.review.append(&separator(gtk::Orientation::Horizontal));
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.append(&label(title, "row-title"));
        text.append(&wrapping(&value, "row-detail"));
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("setting");
        row.append(&text);
        widgets.review.append(&row);
    }
}

/// The tasks for what was picked, in the order they run.
pub fn plan(widgets: &Widgets, remote: &[Remote], data_dir: &Path) -> Vec<Task> {
    lantern_embed::setup::plan(data_dir, &picked(widgets, remote), embedding(widgets), reranker(widgets), widgets.embed.is_active())
}

/// Switches to the run page with one row per task.
pub fn start(state: &mut State, widgets: &Widgets, tasks: Vec<Task>) {
    while let Some(child) = widgets.tasks.first_child() {
        widgets.tasks.remove(&child);
    }
    state.rows.clear();
    for (index, task) in tasks.iter().enumerate() {
        if index > 0 {
            widgets.tasks.append(&separator(gtk::Orientation::Horizontal));
        }
        let icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        icon.set_size_request(18, 18);
        icon.set_valign(gtk::Align::Start);
        icon.set_margin_top(1);
        icon.append(&colored("minus", icons::FAINT));
        let detail = wrapping("Waiting", "row-detail");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.append(&label(&task.title(), "row-title"));
        text.append(&detail);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("setting");
        row.add_css_class("compact");
        row.append(&icon);
        row.append(&text);
        widgets.tasks.append(&row);
        state.rows.push((icon, detail));
    }
    state.tasks = tasks;
    state.step = STEPS.len();
    state.running = None;
    state.failed = None;
    state.skipping = false;
    state.done = 0;
    show_step(state, widgets);
    widgets.run_bar.set_fraction(0.0);
}

fn set_icon(holder: &gtk::Box, widget: &impl IsA<gtk::Widget>) {
    while let Some(child) = holder.first_child() {
        holder.remove(&child);
    }
    holder.append(widget);
}

/// Shows what the setup job reports. Returns true once everything went through.
pub fn event(state: &mut State, widgets: &Widgets, event: Event, sender: &ComponentSender<App>) -> bool {
    let total = state.tasks.len().max(1);
    match event {
        Event::Started(task) => {
            state.running = Some(task);
            state.phase = None;
            if let Some((icon, detail)) = state.rows.get(task) {
                let spinner = gtk::Spinner::new();
                spinner.start();
                set_icon(icon, &spinner);
                detail.set_text("Starting ...");
            }
            widgets.run_title.set_text(&format!("{}  ·  step {} of {}", state.tasks[task].title(), task + 1, total));
            // Embedding takes long and can be left for later.
            if matches!(state.tasks[task], Task::Embed(..)) {
                widgets.footer.set_visible(true);
                widgets.back.set_visible(false);
                widgets.next.set_label("Skip embedding");
                widgets.next.set_sensitive(true);
            }
        }
        Event::Progress { task, phase, done, total: of, unit } => {
            let Some((_, detail)) = state.rows.get(task) else { return false };
            let fraction = if of > 0 { done as f64 / of as f64 } else { 0.0 };
            widgets.run_bar.set_fraction((state.done as f64 + fraction) / total as f64);
            if state.phase.as_ref().is_none_or(|(running, _, _)| *running != phase) {
                state.phase = Some((phase.clone(), Instant::now(), done));
            }
            let (_, started, base) = state.phase.as_ref().expect("set right above");
            let seconds = started.elapsed().as_secs_f64();
            let rate = (seconds > 1.5 && done > *base).then(|| (done - base) as f64 / seconds);
            let left = |rate: f64| remaining(of.saturating_sub(done) as f64 / rate);
            detail.set_text(&match unit {
                Unit::Bytes => {
                    let mb = |b: u64| b as f64 / 1_048_576.0;
                    let speed = rate.map_or(String::new(), |r| format!("  ·  {:.1} MB/s{}", mb(r as u64), left(r)));
                    format!("{phase}, {:.1} of {:.1} MB{speed}", mb(done), mb(of))
                }
                Unit::Count(what) => {
                    let speed = rate.map_or(String::new(), |r| format!("  ·  {} per second{}", grouped(r as u64), left(r)));
                    format!("{phase}, {} of {} {what}{speed}", grouped(done), grouped(of))
                }
                Unit::None => format!("{phase} ..."),
            });
        }
        Event::Done(task, result) => {
            let Some((icon, detail)) = state.rows.get(task) else { return false };
            state.running = None;
            match result {
                Ok(()) if state.skipping && matches!(state.tasks[task], Task::Embed(..)) => {
                    state.done = task + 1;
                    set_icon(icon, &colored("minus", icons::MUTED));
                    detail.set_text("Skipped, Setup finishes it later");
                }
                Ok(()) => {
                    state.done = task + 1;
                    set_icon(icon, &colored("check", "#3fb950"));
                    detail.set_text("Done");
                    widgets.run_bar.set_fraction(state.done as f64 / total as f64);
                }
                Err(err) => {
                    state.failed = Some(task);
                    set_icon(icon, &colored("x", "#f85149"));
                    detail.add_css_class("chat-error");
                    detail.set_text(&err);
                    widgets.run_title.set_text(&format!("{} failed", state.tasks[task].title()));
                    // Try that one again, or go on with what is there.
                    while let Some(child) = widgets.footer.first_child() {
                        widgets.footer.remove(&child);
                    }
                    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    spacer.set_hexpand(true);
                    widgets.footer.append(&action(sender, "Open Lantern anyway", "flat", Action::Leave));
                    widgets.footer.append(&spacer);
                    widgets.footer.append(&action(sender, "Try again", "primary", Action::Retry));
                    widgets.footer.set_visible(true);
                }
            }
        }
        Event::Finished => {
            widgets.run_title.set_text("Done");
            widgets.run_bar.set_fraction(1.0);
            return true;
        }
    }
    false
}

/// Puts the footer back the way the steps have it, after a failure changed it.
pub fn reset_footer(widgets: &Widgets) {
    while let Some(child) = widgets.footer.first_child() {
        widgets.footer.remove(&child);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    widgets.footer.append(&widgets.back);
    widgets.footer.append(&spacer);
    widgets.footer.append(&widgets.next);
    widgets.footer.set_visible(false);
}

fn remaining(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 1.0 {
        return String::new();
    }
    match seconds as u64 {
        s if s < 60 => format!(", {s} s left"),
        s if s < 3600 => format!(", about {} min left", s.div_ceil(60)),
        s => format!(", about {:.1} h left", s as f64 / 3600.0),
    }
}

fn grouped(n: u64) -> String {
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
