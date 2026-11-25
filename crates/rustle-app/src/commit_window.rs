//! The commit window: unstaged files on top, staged files below, message and actions at the
//! bottom of the left column, and the diff (or conflict versions) of the selected file on the right.

use crate::diffview::{self, DiffAction, StageCtx};
use crate::panels::status_color;
use crate::settings::Settings;
use crate::tab::{Act, AppAct, Dialog, Tab};
use crate::theme::{accent, amber, bg_btn, bg_btn_hover, bold, primary_button, red, text, text_weak};
use crate::worker::Request;
use eframe::egui::{self, Align, Align2, FontId, Layout, Margin, Rect, RichText, Sense, Ui, pos2, vec2};
use rustle_core::{FileChange, OpState, Side};

fn status_letter(c: &FileChange, staged: bool) -> char {
    if c.conflicted {
        'U'
    } else if staged {
        c.index
    } else if c.is_untracked() {
        'N'
    } else {
        c.worktree
    }
}

enum RowClick {
    None,
    Select,
    Toggle,
}

/// One compact file row: status letter, name, dimmed folder, and a stage/unstage button.
fn file_row(
    ui: &mut Ui,
    f: &FileChange,
    is_staged: bool,
    selected: bool,
    busy: bool,
) -> (RowClick, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 3.0, accent().gamma_multiply(0.16));
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, bg_btn());
    }
    let cy = rect.center().y;
    let letter = status_letter(f, is_staged);
    p.text(
        pos2(rect.left() + 12.0, cy),
        Align2::CENTER_CENTER,
        letter,
        FontId::monospace(12.0),
        status_color(letter),
    );

    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), f.path.as_str()),
    };
    let tp = p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(rect.right() - 30.0, rect.bottom()),
    ));
    let g = tp.layout_no_wrap(name.to_string(), FontId::proportional(13.5), text());
    let x = rect.left() + 28.0;
    let w = g.size().x;
    tp.galley(pos2(x, cy - g.size().y / 2.0), g, text());
    if !dir.is_empty() {
        tp.text(
            pos2(x + w + 7.0, cy),
            Align2::LEFT_CENTER,
            dir,
            FontId::proportional(12.0),
            text_weak(),
        );
    }

    let btn = Rect::from_center_size(pos2(rect.right() - 15.0, cy), vec2(22.0, 22.0));
    let id = ui.scope_id().with(("toggle", &f.path, is_staged));
    let br = ui.interact(btn, id, Sense::click());
    let hovered = br.hovered() && !busy;
    if hovered {
        p.rect_filled(btn, 4.0, bg_btn_hover());
    }
    p.text(
        btn.center(),
        Align2::CENTER_CENTER,
        if is_staged { "−" } else { "+" },
        FontId::proportional(16.0),
        if hovered { text() } else { text_weak() },
    );
    let toggled = br.clicked() && !busy;
    br.on_hover_text(if is_staged { "Unstage" } else { "Stage" });
    let click = if toggled {
        RowClick::Toggle
    } else if resp.clicked() && !hovered {
        RowClick::Select
    } else {
        RowClick::None
    };
    (click, resp)
}

fn section_header(ui: &mut Ui, title: &str, count: usize, add_buttons: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title.to_uppercase()).size(10.5).color(text_weak()));
        ui.label(RichText::new(count.to_string()).size(10.5).color(text_weak()));
        ui.with_layout(Layout::right_to_left(Align::Center), add_buttons);
    });
}

fn small(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.0))
}

impl Tab {
    #[allow(clippy::too_many_lines)]
    pub fn commit_popup(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>, settings: &mut Settings) {
        let busy = self.busy();
        let (files, op, repo_path, branch) = {
            let Some(r) = &self.repo else { return };
            (
                r.status.files.clone(),
                r.status.op,
                r.path.clone(),
                r.status.branch.clone().unwrap_or_else(|| "detached HEAD".into()),
            )
        };
        let conflicted: Vec<&FileChange> = files.iter().filter(|f| f.conflicted).collect();
        let staged: Vec<&FileChange> = files.iter().filter(|f| f.is_staged()).collect();
        let unstaged: Vec<&FileChange> = files
            .iter()
            .filter(|f| f.is_unstaged() && !f.conflicted)
            .collect();
        let mut open = self.commit_open;
        let mut close = false;

        egui::Window::new(bold(format!("Commit to {branch}"), 14.0))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([1080.0, 660.0])
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let total = ui.available_size();
                let left_w = (total.x * 0.36).clamp(320.0, 460.0);
                ui.horizontal_top(|ui| {
                    // ---------------------------------------------------------- left column
                    ui.allocate_ui_with_layout(vec2(left_w, total.y), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(left_w);
                        ui.spacing_mut().item_spacing.y = 3.0;

                        if !conflicted.is_empty() {
                            section_header(ui, "Conflicts", conflicted.len(), |_| {});
                            for f in &conflicted {
                                let selected = self.selected_file.as_ref().is_some_and(|(p, _)| *p == f.path);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.label(RichText::new("U").monospace().color(status_color('U')));
                                    if ui.selectable_label(selected, &f.path).clicked() {
                                        acts.push(Act::SelectFile((*f).clone(), false));
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.add_enabled(!busy, small("Resolved")).clicked() {
                                            acts.push(Act::Send(Request::Stage(vec![f.path.clone()])));
                                        }
                                        if ui.add_enabled(!busy, small("Theirs")).clicked() {
                                            acts.push(Act::Send(Request::Resolve {
                                                path: f.path.clone(),
                                                side: Side::Theirs,
                                            }));
                                        }
                                        if ui.add_enabled(!busy, small("Ours")).clicked() {
                                            acts.push(Act::Send(Request::Resolve {
                                                path: f.path.clone(),
                                                side: Side::Ours,
                                            }));
                                        }
                                        if ui.add(small("Editor")).clicked() {
                                            acts.push(Act::App(AppAct::Editor(repo_path.join(&f.path))));
                                        }
                                    });
                                });
                            }
                            ui.add_space(6.0);
                        }

                        // Height left for the two file lists.
                        let conflict_h = if conflicted.is_empty() {
                            0.0
                        } else {
                            26.0 + 24.0 * conflicted.len() as f32
                        };
                        let list_total = (total.y - 215.0 - conflict_h).max(180.0);
                        let usable = list_total - 64.0;
                        let (unstaged_h, staged_h) = match (unstaged.is_empty(), staged.is_empty()) {
                            (false, false) => (usable / 2.0, usable / 2.0),
                            (true, false) => (36.0, usable - 36.0),
                            (false, true) => (usable - 36.0, 36.0),
                            (true, true) => (36.0, 36.0),
                        };

                        for (title, list, is_staged, h) in [
                            ("Unstaged", &unstaged, false, unstaged_h),
                            ("Staged", &staged, true, staged_h),
                        ] {
                            section_header(ui, title, list.len(), |ui| {
                                let label = if is_staged { "Unstage all" } else { "Stage all" };
                                if ui.add_enabled(!list.is_empty() && !busy, small(label)).clicked() {
                                    acts.push(Act::Send(if is_staged {
                                        Request::Unstage(vec![])
                                    } else {
                                        Request::Stage(vec![])
                                    }));
                                }
                            });
                            egui::ScrollArea::vertical()
                                .id_salt(title)
                                .auto_shrink([false, false])
                                .max_height(h.max(30.0))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    if list.is_empty() {
                                        ui.label(RichText::new("No files").size(12.5).color(text_weak()));
                                    }
                                    for f in list.iter().copied() {
                                        let selected =
                                            self.selected_file.as_ref() == Some(&(f.path.clone(), is_staged));
                                        let (click, resp) = file_row(ui, f, is_staged, selected, busy);
                                        match click {
                                            RowClick::Select => {
                                                acts.push(Act::SelectFile(f.clone(), is_staged))
                                            }
                                            RowClick::Toggle => acts.push(Act::Send(if is_staged {
                                                Request::Unstage(f.paths())
                                            } else {
                                                Request::Stage(f.paths())
                                            })),
                                            RowClick::None => {}
                                        }
                                        row_menu(&resp, f, is_staged, &repo_path, acts, &mut close);
                                    }
                                });
                            ui.add_space(4.0);
                        }

                        ui.separator();
                        ui.add(
                            egui::TextEdit::multiline(&mut self.message)
                                .hint_text("Commit message (first line is the title)")
                                .desired_rows(4)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        let subject_len = self.message.lines().next().unwrap_or("").chars().count();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let mut amend = self.amend;
                            if ui.checkbox(&mut amend, "Amend last commit").changed() {
                                acts.push(Act::ToggleAmend(amend));
                            }
                            if !settings.messages.is_empty() {
                                egui::ComboBox::from_id_salt("recent-msgs")
                                    .selected_text("Recent")
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        for m in &settings.messages {
                                            let first = m.lines().next().unwrap_or("");
                                            let short: String = first.chars().take(48).collect();
                                            if ui.selectable_label(false, short).on_hover_text(m).clicked() {
                                                self.message = m.clone();
                                            }
                                        }
                                    });
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let color = if subject_len > 72 { amber() } else { text_weak() };
                                ui.label(RichText::new(format!("{subject_len}/72")).size(12.0).color(color));
                            });
                        });
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            if op != OpState::None {
                                let name = match op {
                                    OpState::Merge => "merge",
                                    OpState::Rebase => "rebase",
                                    OpState::CherryPick => "cherry-pick",
                                    OpState::Revert => "revert",
                                    OpState::None => "",
                                };
                                let can = conflicted.is_empty() && !busy;
                                if primary_button(ui, &format!("Continue {name}"), can).clicked() {
                                    acts.push(Act::Send(Request::OpContinue));
                                }
                            } else {
                                let can = (!staged.is_empty() || self.amend)
                                    && !self.message.trim().is_empty()
                                    && !busy;
                                let label = if self.amend {
                                    "Amend commit".to_string()
                                } else {
                                    format!(
                                        "Commit  {} {}",
                                        staged.len(),
                                        if staged.len() == 1 { "file" } else { "files" }
                                    )
                                };
                                let shortcut =
                                    ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                                if (primary_button(ui, &label, can).clicked() || shortcut) && can {
                                    acts.push(Act::Commit);
                                }
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                            if busy {
                                ui.spinner();
                            }
                            ui.label(RichText::new("Ctrl+Enter").size(11.5).color(text_weak()));
                        });
                        if let Some(e) = &self.error {
                            ui.colored_label(red(), e);
                        }
                    });

                    ui.separator();

                    // ---------------------------------------------------------- right column
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), total.y),
                        Layout::top_down(Align::Min),
                        |ui| {
                            let title = self
                                .conflict
                                .as_ref()
                                .map(|(p, _)| format!("{p} — conflict"))
                                .or_else(|| self.file_diff.as_ref().map(|d| d.title.clone()))
                                .unwrap_or_else(|| "No file selected".into());
                            ui.horizontal(|ui| {
                                ui.label(bold(title, 13.0));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let mut reload = false;
                                    reload |= ui
                                        .checkbox(&mut settings.ignore_whitespace, "Ignore whitespace")
                                        .changed();
                                    reload |=
                                        ui.checkbox(&mut settings.side_by_side, "Side by side").changed();
                                    if reload {
                                        settings.save();
                                        acts.push(Act::ReloadView);
                                    }
                                });
                            });
                            ui.separator();
                            self.right_pane(ui, acts, settings);
                        },
                    );
                });
            });
        self.commit_open = open && !close;
    }

    fn right_pane(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, settings: &Settings) {
        if let Some((_, v)) = &self.conflict {
            ui.columns(3, |c| {
                for (i, (title, body)) in [
                    ("Base", &v.base),
                    ("Ours (current)", &v.ours),
                    ("Theirs (incoming)", &v.theirs),
                ]
                .into_iter()
                .enumerate()
                {
                    c[i].label(RichText::new(title).size(11.5).color(text_weak()));
                    egui::ScrollArea::both()
                        .id_salt(("conflict", i))
                        .auto_shrink(false)
                        .show(&mut c[i], |ui| {
                            ui.label(
                                RichText::new(
                                    body.as_deref().unwrap_or("(file does not exist on this side)"),
                                )
                                .monospace(),
                            );
                        });
                }
            });
            return;
        }
        let Some(d) = &self.file_diff else {
            ui.label(RichText::new("Select a file to see its changes.").color(text_weak()));
            return;
        };
        if d.is_binary() {
            ui.label(RichText::new("Binary file — no text diff.").color(text_weak()));
            return;
        }
        if d.is_empty() {
            ui.label(RichText::new("No textual changes.").color(text_weak()));
            return;
        }
        let Some(r) = &self.repo else { return };
        let staged_view = self.selected_file.as_ref().is_some_and(|(_, s)| *s);
        let untracked = self
            .selected_file
            .as_ref()
            .and_then(|(p, _)| r.status.files.iter().find(|f| &f.path == p))
            .is_some_and(FileChange::is_untracked);
        let ctx = (!settings.ignore_whitespace).then_some(StageCtx {
            staged: staged_view,
            can_discard: !untracked,
        });
        let mut da = Vec::<DiffAction>::new();
        diffview::show(
            ui,
            "file-diff",
            d,
            settings.side_by_side,
            &mut self.file_sel,
            ctx,
            &mut da,
        );
        acts.extend(da.into_iter().map(Act::Diff));
    }
}

/// Right-click menu of a file row.
fn row_menu(
    resp: &egui::Response,
    f: &FileChange,
    is_staged: bool,
    repo_path: &std::path::Path,
    acts: &mut Vec<Act>,
    close: &mut bool,
) {
    resp.context_menu(|ui| {
        let untracked = f.is_untracked();
        let label = if untracked {
            "Delete file…"
        } else if is_staged {
            "Discard all changes…"
        } else {
            "Discard changes…"
        };
        if ui.button(label).clicked() {
            ui.close();
            acts.push(Act::Dialog(Dialog::Confirm {
                title: "Discard changes".into(),
                text: format!("Discard the changes to {}? This cannot be undone.", f.path),
                button: "Discard".into(),
                req: Some(Request::Discard {
                    change: f.clone(),
                    include_staged: is_staged,
                }),
            }));
        }
        if untracked {
            if ui.button("Add to .gitignore").clicked() {
                ui.close();
                acts.push(Act::Send(Request::AddToGitignore(f.path.clone())));
            }
            if let Some((_, ext)) = f.path.rsplit_once('.')
                && ui.button(format!("Add *.{ext} to .gitignore")).clicked()
            {
                ui.close();
                acts.push(Act::Send(Request::AddToGitignore(format!("*.{ext}"))));
            }
        } else {
            if ui.button("File history").clicked() {
                ui.close();
                acts.push(Act::History(f.path.clone()));
                *close = true;
            }
            if ui.button("Blame").clicked() {
                ui.close();
                acts.push(Act::Blame(f.path.clone()));
            }
        }
        if ui.button("Open in editor").clicked() {
            ui.close();
            acts.push(Act::App(AppAct::Editor(repo_path.join(&f.path))));
        }
    });
}
