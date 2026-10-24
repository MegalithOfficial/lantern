//! Compare: comparing two versions and walking through what changed.

use super::*;

impl App {
    pub(super) fn on_compare(&mut self, widgets: &mut Widgets, message: Msg, sender: ComponentSender<Self>, _root: &gtk::Window) {
        match message {
            Msg::CompareRun => {
                let compare = &widgets.compare;
                let Some((old, new)) = crate::pages::compare::picked(compare) else { return };
                if self.comparing {
                    return;
                }
                if old == new {
                    compare.stack.set_visible_child_name("empty");
                    compare.empty_title.set_text("Pick two different versions");
                    compare.empty_help.set_text("The older one on the left of the arrow, the newer one on the right.");
                    return;
                }
                let roots =
                    lantern_core::compare::source_root(&self.data_dir, &old).and_then(|a| Ok((a, lantern_core::compare::source_root(&self.data_dir, &new)?)));
                let (old_root, new_root) = match roots {
                    Ok(roots) => roots,
                    Err(err) => {
                        compare.stack.set_visible_child_name("empty");
                        compare.empty_title.set_text("Can not compare these");
                        compare.empty_help.set_text(&format!("{err:#}"));
                        return;
                    }
                };
                self.comparing = true;
                compare.run.set_sensitive(false);
                compare.bar.set_fraction(0.0);
                compare.busy.set_text(&format!("Comparing {old} with {new} ..."));
                compare.stack.set_visible_child_name("busy");
                sender.spawn_command(move |out| {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let result = lantern_core::compare::changes(&old_root, &new_root, |done, total| {
                        if last.elapsed() >= Duration::from_millis(100) {
                            last = Instant::now();
                            let _ = out.send(Cmd::CompareProgress(done, total));
                        }
                    });
                    let _ = out.send(Cmd::Compared(result.map(|changes| (old, new, old_root, new_root, changes)).map_err(|e| format!("{e:#}"))));
                });
            }
            Msg::CompareFilter => crate::pages::compare::show_files(&mut self.compare, &widgets.compare),
            Msg::CompareActivated(row) => {
                if let Some(change) = crate::pages::compare::activate(&mut self.compare, &widgets.compare, row) {
                    crate::pages::compare::show_diff(&mut self.compare, &widgets.compare, change);
                }
            }
            Msg::CompareMode => crate::pages::compare::redraw(&mut self.compare, &widgets.compare),
            Msg::CompareClick(line) => crate::pages::compare::click(&mut self.compare, &widgets.compare, line),
            Msg::CompareStep(forward) => {
                if let Some(change) = crate::pages::compare::step(&mut self.compare, forward) {
                    crate::pages::compare::show_files(&mut self.compare, &widgets.compare);
                    crate::pages::compare::show_diff(&mut self.compare, &widgets.compare, change);
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }

    pub(super) fn on_compare_cmd(&mut self, widgets: &mut Widgets, message: Cmd, _sender: ComponentSender<Self>) {
        match message {
            Cmd::CompareProgress(done, total) => {
                let compare = &widgets.compare;
                compare.bar.set_fraction(done as f64 / total.max(1) as f64);
                if let Some((old, new)) = crate::pages::compare::picked(compare) {
                    compare.busy.set_text(&format!("Comparing {old} with {new}, {} of {} files", grouped(done as i64), grouped(total as i64)));
                }
            }
            Cmd::Compared(result) => {
                self.comparing = false;
                let compare = &widgets.compare;
                compare.run.set_sensitive(true);
                compare.stack.set_visible_child_name("empty");
                match result {
                    Ok((old, new, old_root, new_root, changes)) => {
                        compare.empty_title.set_text(&if changes.is_empty() {
                            format!("{old} and {new} are the same")
                        } else {
                            format!("{} files changed from {old} to {new}", grouped(changes.len() as i64))
                        });
                        compare.empty_help.set_text("Pick a file on the left, or step through them with the arrows.");
                        self.compare = crate::pages::compare::State::new((old, new, old_root, new_root), changes);
                        crate::pages::compare::show_files(&mut self.compare, compare);
                    }
                    Err(err) => {
                        compare.empty_title.set_text("The comparison failed");
                        compare.empty_help.set_text(&err);
                    }
                }
            }
            _ => unreachable!("{:?} is not handled here", message),
        }
    }
}
