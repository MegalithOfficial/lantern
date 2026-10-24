//! The live part of a chat turn, laid out like T3 Code does it: a "Working for 12s" line,
//! below it what the model thinks and which tools it runs, folded away into "Worked for 40s"
//! once the answer is there.

use std::time::Instant;

use gtk::prelude::*;
use relm4::ComponentSender;

use crate::{
    app::App,
    theme::icons::{self, colored},
    view::label,
};

/// New rows slide in instead of popping up.
fn reveal(child: &impl IsA<gtk::Widget>) -> gtk::Revealer {
    let revealer = gtk::Revealer::builder().child(child).transition_type(gtk::RevealerTransitionType::SlideDown).transition_duration(160).build();
    let later = revealer.clone();
    gtk::glib::idle_add_local_once(move || later.set_reveal_child(true));
    revealer
}

fn row_with_icon(name: &str, text: &gtk::Label) -> gtk::Box {
    let image = colored(name, icons::FAINT);
    image.set_pixel_size(13);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    line.append(&image);
    line.append(text);
    line
}

fn elapsed(since: Instant) -> String {
    let seconds = since.elapsed().as_secs();
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m {}s", seconds / 60, seconds % 60) }
}

struct Thinking {
    title: gtk::Label,
    /// Rendered Markdown, models think in lists and code like they answer in them.
    text: crate::chat::markdown::Document,
    body: gtk::Revealer,
    started: Instant,
    content: String,
    dirty: bool,
}

pub struct Turn {
    sender: ComponentSender<App>,
    header: gtk::Label,
    arrow: gtk::Label,
    work: gtk::Box,
    fold: gtk::Revealer,
    thinking: Option<Thinking>,
    tool: Option<gtk::Label>,
    /// The row in `tool` was put up when the call was announced and has no arguments yet.
    announced: bool,
    /// What the header says in front of the timer. "Working" only once the model really is.
    state: String,
    /// A turn that is read back from a saved chat: no timer, nothing is running.
    replay: bool,
    started: Instant,
    steps: usize,
}

impl Turn {
    /// A turn of a chat that was loaded from the store.
    pub fn replayed(chat_box: &gtk::Box, sender: &ComponentSender<App>) -> Self {
        let mut turn = Self::new(chat_box, sender);
        turn.replay = true;
        turn.header.set_text("Worked on it");
        turn.header.remove_css_class("live");
        turn
    }

    pub fn new(chat_box: &gtk::Box, sender: &ComponentSender<App>) -> Self {
        let header = label("Sending", "work-header");
        header.add_css_class("live");
        let arrow = label("›", "work-arrow");
        arrow.set_visible(false);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&header);
        line.append(&arrow);
        let toggle = gtk::Button::new();
        toggle.set_child(Some(&line));
        toggle.add_css_class("work-toggle");
        toggle.set_halign(gtk::Align::Start);

        let work = gtk::Box::new(gtk::Orientation::Vertical, 4);
        work.add_css_class("work");
        // Empty until the model thinks or calls something, the line on its left would show.
        work.set_visible(false);
        let fold =
            gtk::Revealer::builder().child(&work).transition_type(gtk::RevealerTransitionType::SlideDown).transition_duration(180).reveal_child(true).build();
        {
            let (fold, arrow) = (fold.clone(), arrow.clone());
            toggle.connect_clicked(move |_| {
                fold.set_reveal_child(!fold.reveals_child());
                arrow.set_text(if fold.reveals_child() { "⌄" } else { "›" });
            });
        }

        chat_box.append(&reveal(&toggle));
        chat_box.append(&fold);
        Self {
            sender: sender.clone(),
            header,
            arrow,
            work,
            fold,
            thinking: None,
            tool: None,
            announced: false,
            state: "Sending".to_owned(),
            replay: false,
            started: Instant::now(),
            steps: 0,
        }
    }

    /// Called a few times a second: the timer, and thinking text that arrived since.
    pub fn tick(&mut self) {
        if self.replay {
            return;
        }
        self.header.set_text(&format!("{} {}", self.state, elapsed(self.started)));
        if let Some(thinking) = &mut self.thinking {
            if thinking.dirty {
                thinking.dirty = false;
                thinking.text.update(&thinking.content, &self.sender);
            }
        }
    }

    /// A request is out and the provider has not said anything yet.
    pub fn waiting(&mut self, provider: &str) {
        self.state = format!("Waiting for {}", if provider == "Custom" { "the server" } else { provider });
        self.tick();
    }

    pub fn retrying(&mut self, status: u16, seconds: u64) {
        let why = match status {
            429 => "rate limited".to_owned(),
            503 | 529 => "overloaded".to_owned(),
            other => format!("answered {other}"),
        };
        self.state = format!("The provider is {why}, trying again in {seconds}s, waiting");
        self.tick();
    }

    /// Something arrived: from here on the model is at work.
    fn working(&mut self) {
        if self.state != "Working for" {
            self.state = "Working for".to_owned();
            self.tick();
        }
    }

    pub fn thinking(&mut self, piece: &str) {
        self.working();
        let thinking = self.thinking.get_or_insert_with(|| {
            let title = label("Thinking", "work-row");
            title.add_css_class("live");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 8);
            text.add_css_class("thinking");
            let body = gtk::Revealer::builder()
                .child(&text)
                .transition_type(gtk::RevealerTransitionType::SlideDown)
                .transition_duration(160)
                .reveal_child(true)
                .build();
            let toggle = gtk::Button::new();
            toggle.set_child(Some(&row_with_icon("brain", &title)));
            toggle.add_css_class("work-toggle");
            toggle.set_halign(gtk::Align::Start);
            {
                let body = body.clone();
                toggle.connect_clicked(move |_| body.set_reveal_child(!body.reveals_child()));
            }
            self.work.set_visible(true);
            self.work.append(&reveal(&toggle));
            self.work.append(&body);
            Thinking { title, text: crate::chat::markdown::Document::new(&text), body, started: Instant::now(), content: String::new(), dirty: false }
        });
        thinking.content.push_str(piece);
        thinking.dirty = true;
    }

    /// The answer started to arrive.
    pub fn text(&mut self) {
        self.working();
        self.end_thinking();
    }

    /// The model stopped thinking and started to talk or to call a tool.
    pub fn end_thinking(&mut self) {
        if let Some(thinking) = self.thinking.take() {
            let mut thinking = thinking;
            thinking.text.update(&thinking.content, &self.sender);
            thinking.title.set_text(&format!("Thought for {}", elapsed(thinking.started)));
            thinking.title.remove_css_class("live");
            thinking.body.set_reveal_child(false);
        }
    }

    /// Text the model wrote before it went on to call a tool is part of the work, not the answer.
    pub fn absorb(&mut self, block: &gtk::Box) {
        if let Some(parent) = block.parent().and_downcast::<gtk::Box>() {
            parent.remove(block);
        }
        block.add_css_class("narration");
        self.work.set_visible(true);
        self.work.append(block);
    }

    /// The model is writing a call. With several calls in one reply only the first is
    /// announced, the others show up as they run.
    pub fn tool_coming(&mut self, name: &str) {
        if self.tool.is_none() {
            self.tool_started(name);
            self.announced = true;
        }
    }

    pub fn tool_started(&mut self, text: &str) {
        self.working();
        self.end_thinking();
        if std::mem::take(&mut self.announced) {
            if let Some(row) = &self.tool {
                row.set_text(text);
                return;
            }
        }
        self.steps += 1;
        let row = label(text, "work-row");
        row.add_css_class("mono");
        row.add_css_class("live");
        self.work.set_visible(true);
        self.work.append(&reveal(&row_with_icon("wrench", &row)));
        self.tool = Some(row);
    }

    pub fn tool_finished(&mut self, result: &str, failed: bool) {
        if let Some(row) = self.tool.take() {
            row.set_text(&format!("{}  ·  {result}", row.text()));
            row.remove_css_class("live");
            if failed {
                row.add_css_class("failed");
            }
        }
    }

    /// The answer is complete: fold the work away behind its summary.
    pub fn finish(mut self, failed: bool, stopped: bool) {
        self.end_thinking();
        if let Some(row) = self.tool.take() {
            row.remove_css_class("live");
        }
        let steps = match self.steps {
            0 => String::new(),
            1 => "  ·  1 lookup".to_owned(),
            n => format!("  ·  {n} lookups"),
        };
        if self.replay {
            self.header.set_text(&format!("Worked on it{steps}"));
        } else {
            self.header.set_text(&format!(
                "{} {}{steps}",
                if stopped {
                    "Stopped after"
                } else if failed {
                    "Failed after"
                } else {
                    "Worked for"
                },
                elapsed(self.started)
            ));
        }
        self.header.remove_css_class("live");
        let has_work = self.work.first_child().is_some();
        self.arrow.set_visible(has_work);
        // After a failure the last thing that happened is worth seeing.
        self.fold.set_reveal_child(failed && has_work);
        self.arrow.set_text(if failed && has_work { "⌄" } else { "›" });
    }
}
