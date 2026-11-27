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
                                    InputKind::RenameRemote { old } => Request::RenameRemote {
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
            Dialog::Squash {
                base,
                plan,
                message,
                count,
                warning,
            } => {
                let mut keep = true;
                egui::Window::new(bold(format!("Squash {count} commits"), 15.0))
                    .collapsible(false)
                    .resizable(true)
                    .default_size([540.0, 380.0])
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label("The selected commits become one commit with the message below. Commits after them are replayed on top.");
                        if let Some(w) = warning {
                            ui.add_space(4.0);
                            ui.colored_label(amber(), w.as_str());
                        }
                        ui.add_space(6.0);
                        ui.add(
                            egui::TextEdit::multiline(message)
                                .desired_rows(8)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if primary_button(ui, "Squash", !message.trim().is_empty()).clicked() {
                                let mut first = true;
                                let items = plan
                                    .iter()
                                    .map(|(id, selected)| {
                                        if !*selected {
                                            TodoItem { action: TodoAction::Pick, id: id.clone(), message: None }
                                        } else if std::mem::take(&mut first) {
                                            TodoItem {
                                                action: TodoAction::Reword,
                                                id: id.clone(),
                                                message: Some(message.trim().to_string()),
                                            }
                                        } else {
                                            TodoItem { action: TodoAction::Fixup, id: id.clone(), message: None }
                                        }
                                    })
                                    .collect();
                                acts.push(Act::Send(Request::InteractiveRebase { base: base.clone(), items }));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                keep
            }
            Dialog::AddWorktree { path, branch, create } => {
                let mut keep = true;
                let branches: Vec<String> = self
                    .repo
                    .as_ref()
                    .map(|r| {
                        r.branches
                            .iter()
                            .filter(|b| !b.is_remote)
                            .map(|b| b.name.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                egui::Window::new(bold("Add worktree", 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(440.0);
                        ui.label("A worktree is a second working folder for the same repository, with its own branch checked out.");
                        ui.add_space(6.0);
                        ui.label("Folder");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(path).desired_width(320.0).margin(Margin::symmetric(6, 4)));
                            if ui.button("Browse…").clicked()
                                && let Some(p) = rfd::FileDialog::new().pick_folder()
                            {
                                *path = p.display().to_string();
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.radio_value(create, true, "New branch");
                            ui.radio_value(create, false, "Existing branch");
                        });
                        if *create {
                            ui.add(
                                egui::TextEdit::singleline(branch)
                                    .hint_text("name of the new branch")
                                    .desired_width(f32::INFINITY)
                                    .margin(Margin::symmetric(6, 4)),
                            );
                        } else {
                            egui::ComboBox::from_id_salt("wt-branch")
                                .selected_text(if branch.is_empty() { "choose a branch" } else { branch.as_str() })
                                .show_ui(ui, |ui| {
                                    for b in &branches {
                                        ui.selectable_value(branch, b.clone(), b);
                                    }
                                });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !path.trim().is_empty() && !branch.trim().is_empty();
                            if primary_button(ui, "Add", ok).clicked() {
                                acts.push(Act::Send(Request::AddWorktree {
                                    path: path.trim().to_string(),
                                    branch: branch.trim().to_string(),
                                    create: *create,
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
            Dialog::AddSubmodule { url, path } => {
                let mut keep = true;
                egui::Window::new(bold("Add submodule", 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(460.0);
                        ui.label("Repository URL");
                        ui.add(
                            egui::TextEdit::singleline(url)
                                .hint_text("https://github.com/owner/library.git")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(6, 4)),
                        );
                        ui.label("Path inside this repository");
                        ui.add(
                            egui::TextEdit::singleline(path)
                                .hint_text("libs/library")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(6, 4)),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !url.trim().is_empty() && !path.trim().is_empty();
                            if primary_button(ui, "Add", ok).clicked() {
                                acts.push(Act::Send(Request::AddSubmodule {
                                    url: url.trim().to_string(),
                                    path: path.trim().to_string(),
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
            Dialog::TrackLfs { pattern } => {
                let mut keep = true;
                egui::Window::new(bold("Track with Git LFS", 15.0))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.set_min_width(380.0);
                        ui.label("File pattern to store in Git LFS");
                        let te = ui.add(
                            egui::TextEdit::singleline(pattern)
                                .hint_text("*.psd")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(6, 4)),
                        );
                        te.request_focus();
                        ui.label(
                            RichText::new("This edits .gitattributes; commit it together with the files.")
                                .size(12.0)
                                .color(text_weak()),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let ok = !pattern.trim().is_empty();
                            if (primary_button(ui, "Track", ok).clicked() || (enter && ok)) && ok {
                                acts.push(Act::Send(Request::LfsTrack(pattern.trim().to_string())));
                                keep = false;
                            }
                            if ui.button("Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
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
        // Output of commit hooks and long error messages.
        let mut close_hook = false;
        if let Some(t) = &self.hook_output {
            let mut open = true;
            egui::Window::new(bold("Output of the commit hooks", 15.0))
                .open(&mut open)
                .default_size([640.0, 320.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                        ui.label(RichText::new(t).monospace().size(12.0));
                    });
                });
            close_hook = !open;
        }
        if close_hook {
            self.hook_output = None;
        }
        let mut close_details = false;
        if let Some(t) = &self.details {
            let mut open = true;
            egui::Window::new(bold("Details", 15.0))
                .open(&mut open)
                .default_size([700.0, 360.0])
                .show(ctx, |ui| {
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(t.clone());
                    }
                    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                        ui.label(RichText::new(t).monospace().size(12.0));
                    });
                });
            close_details = !open;
        }
        if close_details {
            self.details = None;
        }
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
        } else if self.hook_output.is_some() {
            self.hook_output = None;
            true
        } else if self.details.is_some() {
            self.details = None;
            true
        } else if self.viewer.is_some() {
            self.viewer = None;
            true
        } else if self.repo_settings_open {
            self.repo_settings_open = false;
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
