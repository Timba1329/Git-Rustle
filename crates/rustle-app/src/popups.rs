//! Modal dialogs and the blame / reflog windows.

use crate::fmt;
use crate::tab::{Act, Dialog, InputKind, Tab};
use crate::theme::{accent, amber, bold, danger_button, primary_button, red, text_weak};
use crate::worker::Request;
use eframe::egui::{self, Align, Align2, Layout, Margin, RichText, Sense, vec2};
use rustle_core::{ResetMode, TodoAction, TodoItem};

impl Tab {
    // ------------------------------------------------------------------ dialogs

    #[allow(clippy::too_many_lines)]
    pub fn dialogs(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let mut d = std::mem::replace(&mut self.dialog, Dialog::None);
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let keep = match &mut d {
            Dialog::None => false,
            Dialog::Confirm {
                title,
                text: body,
                button,
                req,
            } => {
                let mut keep = true;
                egui::Window::new(bold(title.as_str(), 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(body.as_str());
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if req.is_some() {
                                if danger_button(ui, button).clicked() {
                                    if let Some(r) = req.take() {
                                        acts.push(Act::Send(r));
                                    }
                                    keep = false;
                                }
                                if ui.button("Cancel").clicked() {
                                    keep = false;
                                }
                            } else if ui.button("OK").clicked() {
                                keep = false;
                            }
                        });
                    });
                keep
            }
            Dialog::Input {
                title,
                label,
                value,
                kind,
                check,
            } => {
                let mut keep = true;
                egui::Window::new(bold(title.as_str(), 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(360.0);
                        ui.label(label.as_str());
                        let te = ui.add(
                            egui::TextEdit::singleline(value)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        te.request_focus();
                        if let Some((l, v)) = check {
                            ui.checkbox(v, l.as_str());
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let optional = matches!(kind, InputKind::Stash);
                            let ok = !value.trim().is_empty() || optional;
                            if (primary_button(ui, "OK", ok).clicked() || (enter && ok)) && ok {
                                let v = value.trim().to_string();
                                let flag = check.as_ref().is_some_and(|(_, b)| *b);
                                acts.push(Act::Send(match kind {
                                    InputKind::BranchAt { id } => Request::CreateBranchAt {
                                        name: v,
                                        id: id.clone(),
                                        switch: flag,
                                    },
                                    InputKind::RenameBranch { old } => Request::RenameBranch {
                                        old: old.clone(),
                                        new: v,
                                    },
                                    InputKind::NewBranch => Request::CreateBranch(v),
                                    InputKind::Stash => Request::StashPush {
                                        message: v,
                                        include_untracked: flag,
                                    },
                                }));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                keep
            }
            Dialog::Tag {
                target,
                name,
                message,
            } => {
                let mut keep = true;
                egui::Window::new(bold("Create tag", 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(380.0);
                        ui.label("Tag name");
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        ui.label("Message (optional — makes it an annotated tag)");
                        ui.add(
                            egui::TextEdit::multiline(message)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !name.trim().is_empty();
                            if primary_button(ui, "Create", ok).clicked() {
                                let m = message.trim();
                                acts.push(Act::Send(Request::CreateTag {
                                    name: name.trim().to_string(),
                                    target: Some(target.clone()),
                                    message: (!m.is_empty()).then(|| m.to_string()),
                                }));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                keep
            }
            Dialog::Reset { id, mode } => {
                let mut keep = true;
                egui::Window::new(bold("Reset current branch", 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(400.0);
                        ui.label(format!("Move the current branch to {}.", &id[..id.len().min(7)]));
                        ui.add_space(6.0);
                        ui.radio_value(mode, ResetMode::Soft, "Soft — keep all changes staged");
                        ui.radio_value(mode, ResetMode::Mixed, "Mixed — keep changes in the working tree");
                        ui.radio_value(mode, ResetMode::Hard, "Hard — discard all changes");
                        if *mode == ResetMode::Hard {
                            ui.label(RichText::new("Uncommitted changes will be lost.").color(red()));
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let go = if *mode == ResetMode::Hard {
                                danger_button(ui, "Reset").clicked()
                            } else {
                                primary_button(ui, "Reset", true).clicked()
                            };
                            if go {
                                acts.push(Act::Send(Request::Reset {
                                    id: id.clone(),
                                    mode: *mode,
                                }));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                keep
            }
            Dialog::Rebase { base, rows } => {
                let mut keep = true;
                let mut swap: Option<(usize, usize)> = None;
                egui::Window::new(bold("Interactive rebase", 15.0))
                    .collapsible(false)
                    .resizable(true)
                    .default_size([620.0, 420.0])
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(
                            RichText::new(
                                "Oldest commit first. Squash/fixup melt a commit into the one above it.",
                            )
                            .color(text_weak()),
                        );
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                            let n = rows.len();
                            for (i, row) in rows.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt(("act", i))
                                        .width(90.0)
                                        .selected_text(action_name(row.action))
                                        .show_ui(ui, |ui| {
                                            for a in [
                                                TodoAction::Pick,
                                                TodoAction::Reword,
                                                TodoAction::Squash,
                                                TodoAction::Fixup,
                                                TodoAction::Drop,
                                            ] {
                                                ui.selectable_value(&mut row.action, a, action_name(a));
                                            }
                                        });
                                    ui.label(
                                        RichText::new(&row.id[..row.id.len().min(7)])
                                            .monospace()
                                            .color(text_weak()),
                                    );
                                    if row.action == TodoAction::Reword {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut row.message).desired_width(260.0),
                                        );
                                    } else {
                                        ui.label(&row.subject);
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .add_enabled(i + 1 < n, egui::Button::new("▼").small())
                                            .clicked()
                                        {
                                            swap = Some((i, i + 1));
                                        }
                                        if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                                            swap = Some((i, i - 1));
                                        }
                                    });
                                });
                            }
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let first_ok = !matches!(
                                rows.iter()
                                    .find(|r| r.action != TodoAction::Drop)
                                    .map(|r| r.action),
                                Some(TodoAction::Squash | TodoAction::Fixup)
                            );
                            if primary_button(ui, "Start rebase", first_ok).clicked() {
                                let items = rows
                                    .iter()
                                    .map(|r| TodoItem {
                                        action: r.action,
                                        id: r.id.clone(),
                                        message: (r.action == TodoAction::Reword).then(|| r.message.clone()),
                                    })
                                    .collect();
                                acts.push(Act::Send(Request::InteractiveRebase {
                                    base: base.clone(),
                                    items,
                                }));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                            if !first_ok {
                                ui.label(
                                    RichText::new("The first commit cannot be squashed.").color(amber()),
                                );
                            }
                        });
                    });
                if let Some((a, b)) = swap {
                    rows.swap(a, b);
                }
                keep
            }
        };
        if keep && matches!(self.dialog, Dialog::None) {
            self.dialog = d;
        }
    }

    // ------------------------------------------------------------------ blame & reflog

    #[allow(clippy::too_many_lines)]
    pub fn info_windows(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let mut close_blame = false;
        if let Some((path, lines)) = &self.blame {
            let mut open = true;
            egui::Window::new(bold(format!("Blame — {path}"), 15.0))
                .open(&mut open)
                .default_size([900.0, 560.0])
                .show(ctx, |ui| {
                    let row_h = 20.0;
                    let mut last = String::new();
                    egui::ScrollArea::both().auto_shrink(false).show_rows(
                        ui,
                        row_h,
                        lines.len(),
                        |ui, range| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for l in &lines[range] {
                                ui.horizontal(|ui| {
                                    let same = l.id == last;
                                    last.clone_from(&l.id);
                                    let id = if same {
                                        String::new()
                                    } else {
                                        l.id[..l.id.len().min(7)].to_string()
                                    };
                                    let author = if same { String::new() } else { l.author.clone() };
                                    ui.add_sized(
                                        [62.0, row_h],
                                        egui::Label::new(RichText::new(id).monospace().color(accent())),
                                    );
                                    ui.add_sized(
                                        [130.0, row_h],
                                        egui::Label::new(RichText::new(author).color(text_weak())).truncate(),
                                    );
                                    ui.add_sized(
                                        [40.0, row_h],
                                        egui::Label::new(
                                            RichText::new(l.number.to_string())
                                                .monospace()
                                                .color(text_weak()),
                                        ),
                                    );
                                    ui.label(RichText::new(l.text.replace('\t', "    ")).monospace());
                                });
                            }
                        },
                    );
                });
            close_blame = !open;
        }
        if close_blame {
            self.blame = None;
        }

        let mut close_reflog = false;
        if let Some(entries) = &self.reflog {
            let mut open = true;
            egui::Window::new(bold("Reflog", 15.0))
                .open(&mut open)
                .default_size([760.0, 480.0])
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Every place HEAD has been. Right-click an entry to go back to it.")
                            .color(text_weak()),
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        for e in entries {
                            let resp = ui
                                .horizontal(|ui| {
                                    ui.add_sized(
                                        [80.0, 20.0],
                                        egui::Label::new(
                                            RichText::new(&e.selector).monospace().color(accent()),
                                        ),
                                    );
                                    ui.add_sized(
                                        [60.0, 20.0],
                                        egui::Label::new(
                                            RichText::new(&e.id[..e.id.len().min(7)])
                                                .monospace()
                                                .color(text_weak()),
                                        ),
                                    );
                                    ui.add_sized(
                                        [110.0, 20.0],
                                        egui::Label::new(
                                            RichText::new(fmt::relative(e.time)).color(text_weak()),
                                        ),
                                    );
                                    ui.label(&e.message);
                                })
                                .response
                                .interact(Sense::click());
                            let id = e.id.clone();
                            resp.context_menu(|ui| {
                                if ui.button("Check out (detached)").clicked() {
                                    ui.close();
                                    acts.push(Act::Send(Request::CheckoutCommit(id.clone())));
                                }
                                if ui.button("Create branch here…").clicked() {
                                    ui.close();
                                    acts.push(Act::Dialog(Dialog::Input {
                                        title: "Create branch".into(),
                                        label: "Branch name".into(),
                                        value: String::new(),
                                        kind: InputKind::BranchAt { id: id.clone() },
                                        check: Some(("Switch to the new branch".into(), true)),
                                    }));
                                }
                                if ui.button("Reset current branch here…").clicked() {
                                    ui.close();
                                    acts.push(Act::Dialog(Dialog::Reset {
                                        id: id.clone(),
                                        mode: ResetMode::Mixed,
                                    }));
                                }
                            });
                        }
                    });
                });
            close_reflog = !open;
        }
        if close_reflog {
            self.reflog = None;
        }
    }

    /// Closes the top-most popup; returns `true` if something was closed.
    pub fn on_escape(&mut self) -> bool {
        if !matches!(self.dialog, Dialog::None) {
            self.dialog = Dialog::None;
            true
        } else if self.blame.is_some() {
            self.blame = None;
            true
        } else if self.reflog.is_some() {
            self.reflog = None;
            true
        } else if self.commit_open {
            self.commit_open = false;
            true
        } else {
            false
        }
    }
}

fn action_name(a: TodoAction) -> &'static str {
    match a {
        TodoAction::Pick => "pick",
        TodoAction::Reword => "reword",
        TodoAction::Squash => "squash",
        TodoAction::Fixup => "fixup",
        TodoAction::Drop => "drop",
    }
}
