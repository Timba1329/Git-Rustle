//! Drawing of the main panels of a tab: header, sidebar, commit graph and commit details.

use crate::diffview::{self, DiffAction};
use crate::fmt;
use crate::settings::Settings;
use crate::tab::{Act, AppAct, Dialog, InputKind, RebaseRow, Tab};
use crate::theme::{
    accent, amber, bg_bar, bg_btn, bg_main, bg_side, bold, border, ghost, green, icon_button, nav_row,
    nav_row_badges, primary_button, purple, red, section_label, text, text_weak,
};
use crate::worker::Request;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, pos2,
    vec2,
};
use rustle_core::{Branch, Commit, CommitFile, GraphRow, OpState, RefKind, TodoAction};
use std::path::{Path, PathBuf};

const LANE_W: f32 = 14.0;
const ROW_H: f32 = 26.0;

fn lane_color(i: usize) -> Color32 {
    const P: [Color32; 8] = [
        Color32::from_rgb(0x5b, 0x8d, 0xef),
        Color32::from_rgb(0xe5, 0x67, 0x6b),
        Color32::from_rgb(0x4f, 0xb8, 0x83),
        Color32::from_rgb(0xe0, 0xa8, 0x4f),
        Color32::from_rgb(0xa9, 0x8b, 0xdc),
        Color32::from_rgb(0x4d, 0xbf, 0xd0),
        Color32::from_rgb(0xe5, 0x6a, 0x9a),
        Color32::from_rgb(0xa8, 0x93, 0x86),
    ];
    P[i % P.len()]
}

pub fn folder_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    crate::recent::same_path(a, b)
}

/// A menu entry: closes the menu and returns `true` when clicked.
fn item(ui: &mut Ui, label: &str) -> bool {
    let clicked = nav_row(ui, label, false, None).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn ctx_item(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let clicked = ui
        .add_enabled(enabled, egui::Button::new(label).frame(false))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked
}

pub fn status_color(letter: char) -> Color32 {
    match letter {
        'A' | 'N' => green(),
        'D' => red(),
        'U' => Color32::from_rgb(0xf0, 0x8a, 0x5f),
        'R' | 'C' => Color32::from_rgb(0x4d, 0xbf, 0xd0),
        _ => amber(),
    }
}

/// Walks the first-parent chain from `from` (usually HEAD) down to `to`; returns the commits
/// after `to`, oldest first, or `None` when `to` is not on that chain.
fn first_parent_chain<'a>(commits: &'a [Commit], from: &str, to: &str) -> Option<Vec<&'a Commit>> {
    let mut out = Vec::new();
    let mut cur = from.to_string();
    for _ in 0..commits.len() + 1 {
        if cur == to {
            out.reverse();
            return Some(out);
        }
        let c = commits.iter().find(|c| c.id == cur)?;
        out.push(c);
        cur = c.parents.first()?.clone();
    }
    None
}

/// Small status badges for a local branch: commits to push/pull, or why there is no tracking.
fn branch_badges(b: &Branch) -> Vec<(String, Color32)> {
    if b.is_remote {
        return Vec::new();
    }
    if b.gone {
        return vec![("gone".into(), red())];
    }
    if b.upstream.is_none() {
        return vec![("local".into(), text_weak())];
    }
    let mut v = Vec::new();
    if b.ahead > 0 {
        v.push((format!("↑{}", b.ahead), green()));
    }
    if b.behind > 0 {
        v.push((format!("↓{}", b.behind), amber()));
    }
    v
}

/// Explains the badges of [`branch_badges`] in words.
fn branch_tip(b: &Branch) -> String {
    if b.is_remote {
        return String::new();
    }
    let Some(up) = &b.upstream else {
        return "Not published: this branch has no upstream. Push it to share it.".into();
    };
    if b.gone {
        return format!("The upstream {up} no longer exists on the remote.");
    }
    let plural = |n: u32| if n == 1 { "commit" } else { "commits" };
    match (b.ahead, b.behind) {
        (0, 0) => format!("Up to date with {up} (as of the last fetch)."),
        (a, 0) => format!("{a} {} to push to {up}.", plural(a)),
        (0, d) => format!("{d} {} to pull from {up}.", plural(d)),
        (a, d) => format!(
            "{a} {} to push and {d} {} to pull — diverged from {up}.",
            plural(a),
            plural(d)
        ),
    }
}

fn small(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.5))
}

impl Tab {
    // ------------------------------------------------------------------ header

    #[allow(clippy::too_many_lines)]
    pub fn header(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, recent: &[PathBuf]) {
        let busy = self.busy();
        let frame = Frame::new().fill(bg_bar()).inner_margin(Margin::symmetric(10, 6));
        egui::Panel::top("header")
            .frame(frame)
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // Repository switcher.
                    let current = self.repo.as_ref().map(|r| r.path.clone());
                    let repo_name = current
                        .as_ref()
                        .map_or_else(|| "No repository".to_string(), |p| folder_name(p));
                    ghost(ui, |ui| {
                        ui.menu_button(bold(format!("{repo_name} ▾"), 14.0), |ui| {
                            ui.set_min_width(300.0);
                            section_label(ui, "Recent repositories");
                            if recent.is_empty() {
                                ui.label(RichText::new("No recent repositories").color(text_weak()));
                            }
                            for p in recent {
                                let is_cur = current.as_ref().is_some_and(|c| same_path(c, p));
                                let resp = nav_row(ui, &folder_name(p), is_cur, is_cur.then_some(green()))
                                    .on_hover_text(p.display().to_string());
                                if resp.clicked() {
                                    if !is_cur {
                                        acts.push(Act::App(AppAct::OpenPath(p.clone())));
                                    }
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if item(ui, "Open repository…")
                                && let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    acts.push(Act::App(AppAct::OpenPath(dir)));
                                }
                            if item(ui, "Clone repository…") {
                                acts.push(Act::App(AppAct::Clone));
                            }
                            if item(ui, "Create new repository…") {
                                acts.push(Act::App(AppAct::PickAndInit));
                            }
                        });
                    });

                    let Some(r) = &self.repo else {
                        if busy {
                            ui.spinner();
                        }
                        return;
                    };
                    let st = &r.status;

                    // Branch switcher.
                    let branch = st.branch.as_deref().unwrap_or("detached HEAD");
                    ghost(ui, |ui| {
                        ui.menu_button(RichText::new(format!("{branch} ▾")).color(accent()), |ui| {
                            ui.set_min_width(280.0);
                            ui.horizontal(|ui| {
                                let te = ui.add(
                                    egui::TextEdit::singleline(&mut self.new_branch)
                                        .hint_text("New branch…")
                                        .desired_width(180.0)
                                        .margin(Margin::symmetric(6, 4)),
                                );
                                let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let create = ui.add_enabled(!busy, small("Create")).clicked();
                                if (create || enter) && !busy && !self.new_branch.trim().is_empty() {
                                    acts.push(Act::NewBranch);
                                    ui.close();
                                }
                            });
                            ui.separator();
                            egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                                for (title, remote) in [("Local branches", false), ("Remote branches", true)] {
                                    let list: Vec<&Branch> =
                                        r.branches.iter().filter(|b| b.is_remote == remote).collect();
                                    if list.is_empty() {
                                        continue;
                                    }
                                    section_label(ui, title);
                                    for b in list {
                                        let dot = b.is_head.then_some(green());
                                        if nav_row_badges(ui, &b.name, b.is_head, dot, &branch_badges(b))
                                            .on_hover_text(branch_tip(b))
                                            .clicked()
                                        {
                                            if !b.is_head && !busy {
                                                acts.push(Act::Send(Request::Checkout(b.clone())));
                                            }
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        });
                    });
                    if let Some(u) = &st.upstream {
                        ui.label(RichText::new(u).size(12.0).color(text_weak()));
                    }
                    if busy {
                        ui.spinner();
                        if ui.add(small("Cancel")).clicked() {
                            self.worker.cancel();
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let n = st.files.len();
                        let commit = if n > 0 { format!("Commit  {n}") } else { "Commit".into() };
                        if primary_button(ui, &commit, true).clicked() {
                            acts.push(Act::OpenCommit);
                        }
                        ui.add_space(4.0);
                        ui.menu_button(RichText::new("⋯").size(15.0), |ui| {
                            ui.set_min_width(230.0);
                            let path = r.path.clone();
                            if item(ui, "Stash changes…") {
                                acts.push(Act::Dialog(Dialog::Input {
                                    title: "Stash changes".into(),
                                    label: "Message (optional)".into(),
                                    value: String::new(),
                                    kind: InputKind::Stash,
                                    check: Some(("Include untracked files".into(), true)),
                                }));
                            }
                            if item(ui, "Force push (with lease)…") {
                                acts.push(Act::Dialog(Dialog::Confirm {
                                    title: "Force push".into(),
                                    text: "Overwrite the remote branch with your local branch? The push is \
                                           refused if someone else pushed in the meantime (--force-with-lease)."
                                        .into(),
                                    button: "Force push".into(),
                                    req: Some(Request::Push { force: true }),
                                }));
                            }
                            if item(ui, "Update submodules") {
                                acts.push(Act::Send(Request::SubmoduleUpdate));
                            }
                            if item(ui, "Reflog…") {
                                acts.push(Act::ShowReflog);
                            }
                            ui.separator();
                            if item(ui, "Open in Explorer") {
                                acts.push(Act::App(AppAct::Explorer(path.clone())));
                            }
                            if item(ui, "Open in terminal") {
                                acts.push(Act::App(AppAct::Terminal(path.clone())));
                            }
                            if item(ui, "Open in editor") {
                                acts.push(Act::App(AppAct::Editor(path)));
                            }
                            ui.separator();
                            if item(ui, "New tab") {
                                acts.push(Act::App(AppAct::NewTab));
                            }
                            if item(ui, "Settings…") {
                                acts.push(Act::App(AppAct::Settings));
                            }
                            if item(ui, "About Rustle") {
                                acts.push(Act::App(AppAct::About));
                            }
                        });
                        let push = if st.ahead > 0 {
                            format!("Push  {}", st.ahead)
                        } else {
                            "Push".into()
                        };
                        if ui.add_enabled(!busy, small(&push)).clicked() {
                            acts.push(Act::Send(Request::Push { force: false }));
                        }
                        let pull = if st.behind > 0 {
                            format!("Pull  {}", st.behind)
                        } else {
                            "Pull".into()
                        };
                        if ui.add_enabled(!busy, small(&pull)).clicked() {
                            acts.push(Act::Send(Request::Pull));
                        }
                        let ago = self.last_fetch.elapsed().as_secs() / 60;
                        let mut tip = if ago == 0 {
                            "Last fetch: less than a minute ago.".to_string()
                        } else {
                            format!("Last fetch: {ago} min ago.")
                        };
                        let fetch_label = match &self.auto_fetch_error {
                            Some(e) => {
                                tip = format!("The last automatic fetch failed:
{e}");
                                RichText::new("Fetch  !").size(12.5).color(amber())
                            }
                            None => RichText::new("Fetch").size(12.5),
                        };
                        if ui
                            .add_enabled(!busy, egui::Button::new(fetch_label))
                            .on_hover_text(tip)
                            .clicked()
                        {
                            acts.push(Act::Send(Request::Fetch));
                        }
                        if ui.add_enabled(!busy, small("Refresh")).clicked() {
                            acts.push(Act::Send(Request::Refresh));
                        }
                    });
                });

                self.banners(ui, acts);
            });
    }

    fn banners(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let busy = self.busy();
        if let Some(r) = &self.repo {
            let op = r.status.op;
            if op != OpState::None {
                let name = match op {
                    OpState::Merge => "Merge",
                    OpState::Rebase => "Rebase",
                    OpState::CherryPick => "Cherry-pick",
                    OpState::Revert => "Revert",
                    OpState::None => "",
                };
                let conflicts = r.status.files.iter().filter(|f| f.conflicted).count();
                ui.add_space(6.0);
                Frame::new()
                    .fill(amber().gamma_multiply(0.12))
                    .stroke(Stroke::new(1.0, amber().gamma_multiply(0.45)))
                    .corner_radius(4)
                    .inner_margin(Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let text = if conflicts > 0 {
                                format!("{name} in progress — {conflicts} conflicted file(s)")
                            } else {
                                format!("{name} in progress — no conflicts left")
                            };
                            ui.label(RichText::new(text).color(amber()));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add_enabled(!busy, small("Abort")).clicked() {
                                    acts.push(Act::Dialog(Dialog::Confirm {
                                        title: format!("Abort {name}"),
                                        text: "Abort and return to the state before the operation started?"
                                            .into(),
                                        button: "Abort".into(),
                                        req: Some(Request::OpAbort),
                                    }));
                                }
                                if op == OpState::Rebase && ui.add_enabled(!busy, small("Skip")).clicked() {
                                    acts.push(Act::Send(Request::RebaseSkip));
                                }
                                if ui
                                    .add_enabled(!busy && conflicts == 0, small("Continue"))
                                    .clicked()
                                {
                                    acts.push(Act::Send(Request::OpContinue));
                                }
                                if conflicts > 0 && ui.add(small("Resolve conflicts")).clicked() {
                                    acts.push(Act::OpenCommit);
                                }
                            });
                        });
                    });
            }
        }

        let note = self
            .error
            .as_ref()
            .map(|e| (e.clone(), red()))
            .or_else(|| self.info.as_ref().map(|i| (i.clone(), green())));
        if let Some((msg, color)) = note {
            let is_err = self.error.is_some();
            ui.add_space(6.0);
            Frame::new()
                .fill(color.gamma_multiply(0.12))
                .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
                .corner_radius(4)
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&msg).color(color));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, "✕", "Dismiss", true).clicked() {
                                if is_err {
                                    self.error = None;
                                } else {
                                    self.info = None;
                                }
                            }
                        });
                    });
                });
        }
    }

    // ------------------------------------------------------------------ sidebar

    #[allow(clippy::too_many_lines)]
    pub fn sidebar(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let Some(r) = &self.repo else { return };
        let busy = self.busy();
        let current = r.status.branch.clone();
        let frame = Frame::new().fill(bg_side()).inner_margin(Margin::symmetric(8, 8));
        egui::Panel::left("sidebar")
            .frame(frame)
            .show_separator_line(true)
            .default_size(220.0)
            .size_range(170.0..=420.0)
            .show(ui, |ui| {
                let heading = |text: String| RichText::new(text).size(10.5).color(text_weak());
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(heading("BRANCHES".into()));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, "+", "New branch", !busy).clicked() {
                                acts.push(Act::Dialog(Dialog::Input {
                                    title: "New branch".into(),
                                    label: "Branch name".into(),
                                    value: String::new(),
                                    kind: InputKind::NewBranch,
                                    check: None,
                                }));
                            }
                        });
                    });
                    for b in r.branches.iter().filter(|b| !b.is_remote) {
                        let resp = nav_row_badges(
                            ui,
                            &b.name,
                            b.is_head,
                            b.is_head.then_some(green()),
                            &branch_badges(b),
                        );
                        if resp.double_clicked() && !b.is_head && !busy {
                            acts.push(Act::Send(Request::Checkout(b.clone())));
                        }
                        let mut tip = branch_tip(b);
                        if !b.is_head {
                            tip.push_str("\nDouble-click to check out.");
                        }
                        resp.clone().on_hover_text(tip);
                        resp.context_menu(|ui| {
                            if ctx_item(ui, "Check out", !b.is_head) {
                                acts.push(Act::Send(Request::Checkout(b.clone())));
                            }
                            if let Some(cur) = &current {
                                if ctx_item(ui, &format!("Merge into {cur}"), !b.is_head) {
                                    acts.push(Act::Send(Request::Merge(b.name.clone())));
                                }
                                if ctx_item(ui, &format!("Rebase {cur} onto this"), !b.is_head) {
                                    acts.push(Act::Send(Request::Rebase(b.name.clone())));
                                }
                            }
                            if ctx_item(ui, "Rename…", true) {
                                acts.push(Act::Dialog(Dialog::Input {
                                    title: "Rename branch".into(),
                                    label: "New name".into(),
                                    value: b.name.clone(),
                                    kind: InputKind::RenameBranch { old: b.name.clone() },
                                    check: None,
                                }));
                            }
                            if ctx_item(ui, "Delete…", !b.is_head) {
                                acts.push(Act::Dialog(Dialog::Confirm {
                                    title: "Delete branch".into(),
                                    text: format!(
                                        "Delete the local branch {}? Unmerged commits can be lost.",
                                        b.name
                                    ),
                                    button: "Delete".into(),
                                    req: Some(Request::DeleteBranch {
                                        name: b.name.clone(),
                                        force: true,
                                    }),
                                }));
                            }
                            if ctx_item(ui, "Copy name", true) {
                                ui.ctx().copy_text(b.name.clone());
                            }
                        });
                    }

                    let remotes: Vec<&Branch> = r.branches.iter().filter(|b| b.is_remote).collect();
                    if !remotes.is_empty() {
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(heading(format!("REMOTES  {}", remotes.len())))
                            .default_open(true)
                            .show(ui, |ui| {
                                for b in remotes {
                                    let resp = nav_row(ui, &b.name, false, None);
                                    if resp.double_clicked() && !busy {
                                        acts.push(Act::Send(Request::Checkout(b.clone())));
                                    }
                                    resp.clone()
                                        .on_hover_text("Double-click to check out as a local branch");
                                    resp.context_menu(|ui| {
                                        if ctx_item(ui, "Check out as local branch", true) {
                                            acts.push(Act::Send(Request::Checkout(b.clone())));
                                        }
                                        if let Some(cur) = &current {
                                            if ctx_item(ui, &format!("Merge into {cur}"), true) {
                                                acts.push(Act::Send(Request::Merge(b.name.clone())));
                                            }
                                            if ctx_item(ui, &format!("Rebase {cur} onto this"), true) {
                                                acts.push(Act::Send(Request::Rebase(b.name.clone())));
                                            }
                                            if ctx_item(ui, &format!("Set as upstream of {cur}"), true) {
                                                acts.push(Act::Send(Request::SetUpstream {
                                                    branch: cur.clone(),
                                                    upstream: b.name.clone(),
                                                }));
                                            }
                                        }
                                        if ctx_item(ui, "Delete on remote…", true) {
                                            acts.push(Act::Dialog(Dialog::Confirm {
                                                title: "Delete remote branch".into(),
                                                text: format!(
                                                    "Delete {} from the remote? This affects everyone.",
                                                    b.name
                                                ),
                                                button: "Delete".into(),
                                                req: Some(Request::DeleteRemoteBranch(b.name.clone())),
                                            }));
                                        }
                                        if ctx_item(ui, "Copy name", true) {
                                            ui.ctx().copy_text(b.name.clone());
                                        }
                                    });
                                }
                            });
                    }
                    if !r.tags.is_empty() {
                        egui::CollapsingHeader::new(heading(format!("TAGS  {}", r.tags.len())))
                            .default_open(false)
                            .show(ui, |ui| {
                                for t in &r.tags {
                                    let resp = nav_row(ui, t, false, Some(amber()));
                                    resp.context_menu(|ui| {
                                        if ctx_item(ui, "Check out (detached)", true) {
                                            acts.push(Act::Send(Request::CheckoutCommit(t.clone())));
                                        }
                                        if ctx_item(ui, "Push tag", true) {
                                            acts.push(Act::Send(Request::PushTag(t.clone())));
                                        }
                                        if ctx_item(ui, "Delete…", true) {
                                            acts.push(Act::Dialog(Dialog::Confirm {
                                                title: "Delete tag".into(),
                                                text: format!("Delete the local tag {t}?"),
                                                button: "Delete".into(),
                                                req: Some(Request::DeleteTag(t.clone())),
                                            }));
                                        }
                                        if ctx_item(ui, "Copy name", true) {
                                            ui.ctx().copy_text(t.clone());
                                        }
                                    });
                                }
                            });
                    }
                    if !r.stashes.is_empty() {
                        egui::CollapsingHeader::new(heading(format!("STASHES  {}", r.stashes.len())))
                            .default_open(true)
                            .show(ui, |ui| {
                                for s in &r.stashes {
                                    let resp = nav_row(
                                        ui,
                                        &format!("{}: {}", s.index, s.message),
                                        false,
                                        Some(purple()),
                                    );
                                    let idx = s.index;
                                    resp.context_menu(|ui| {
                                        if ctx_item(ui, "Apply", true) {
                                            acts.push(Act::Send(Request::StashApply(idx)));
                                        }
                                        if ctx_item(ui, "Pop (apply and drop)", true) {
                                            acts.push(Act::Send(Request::StashPop(idx)));
                                        }
                                        if ctx_item(ui, "Drop…", true) {
                                            acts.push(Act::Dialog(Dialog::Confirm {
                                                title: "Drop stash".into(),
                                                text: "Delete this stash? This cannot be undone.".into(),
                                                button: "Drop".into(),
                                                req: Some(Request::StashDrop(idx)),
                                            }));
                                        }
                                    });
                                }
                            });
                    }
                    if !r.submodules.is_empty() {
                        egui::CollapsingHeader::new(heading(format!("SUBMODULES  {}", r.submodules.len())))
                            .default_open(true)
                            .show(ui, |ui| {
                                for s in &r.submodules {
                                    let dot = match s.status {
                                        ' ' => green(),
                                        '-' => text_weak(),
                                        _ => amber(),
                                    };
                                    nav_row(ui, &s.path, false, Some(dot)).on_hover_text(match s.status {
                                        ' ' => "Up to date",
                                        '-' => "Not initialised",
                                        '+' => "Checked out at a different commit",
                                        _ => "Has conflicts",
                                    });
                                }
                            });
                    }
                });
            });
    }

    // ------------------------------------------------------------------ commit graph

    #[allow(clippy::too_many_lines)]
    pub fn graph_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(bg_main()))
            .show(ui, |ui| {
                if self.repo.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.30);
                        ui.label(bold("Rustle", 32.0));
                        ui.label(RichText::new("A fast, native Git client").color(text_weak()));
                        ui.add_space(16.0);
                        if primary_button(ui, "Open repository", !self.busy()).clicked()
                            && let Some(dir) = rfd::FileDialog::new().pick_folder()
                        {
                            acts.push(Act::App(AppAct::OpenPath(dir)));
                        }
                        if ui.button("Clone repository…").clicked() {
                            acts.push(Act::App(AppAct::Clone));
                        }
                        if self.busy() {
                            ui.add_space(10.0);
                            ui.spinner();
                        }
                    });
                    return;
                }

                self.filter_bar(ui, acts);
                let Some(r) = &self.repo else { return };
                if r.commits.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        let msg = if r.query.is_filtered() {
                            "No commits match your search."
                        } else {
                            "This repository has no commits yet."
                        };
                        ui.label(RichText::new(msg).color(text_weak()));
                    });
                    return;
                }
                let graph_w = r.graph.width.max(1) as f32 * LANE_W + 12.0;
                let ctrl = ui.input(|i| i.modifiers.command);
                let selected = self.selected;
                let compare = self.compare_id.clone();
                let head_id = r
                    .commits
                    .iter()
                    .find(|c| c.refs.iter().any(|l| l.is_head))
                    .map(|c| c.id.clone());
                let mut last_visible = 0;
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                    ui,
                    ROW_H,
                    r.commits.len(),
                    |ui, range| {
                        last_visible = range.end;
                        for i in range {
                            let is_sel =
                                selected == Some(i) || compare.as_deref() == Some(r.commits[i].id.as_str());
                            commit_row(ui, r, i, graph_w, is_sel, ctrl, head_id.as_deref(), acts);
                        }
                    },
                );
                if r.has_more && !self.load_more_sent && last_visible + 40 >= r.commits.len() {
                    self.load_more_sent = true;
                    acts.push(Act::Send(Request::LoadMore));
                }
            });
    }

    fn filter_bar(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        Frame::new()
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let enter = |r: &egui::Response, ui: &Ui| {
                        r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    };
                    let id = ui.make_persistent_id("search");
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .id(id)
                            .hint_text("Search messages  (Ctrl+F)")
                            .desired_width(230.0)
                            .margin(Margin::symmetric(6, 3)),
                    );
                    if std::mem::take(&mut self.focus_search) {
                        te.request_focus();
                    }
                    let mut apply = enter(&te, ui);
                    let ae = ui.add(
                        egui::TextEdit::singleline(&mut self.author)
                            .hint_text("Author")
                            .desired_width(120.0)
                            .margin(Margin::symmetric(6, 3)),
                    );
                    apply |= enter(&ae, ui);
                    if ui
                        .checkbox(&mut self.branch_only, "Current branch only")
                        .changed()
                    {
                        apply = true;
                    }
                    if let Some(p) = self.path_filter.clone() {
                        ui.label(RichText::new(format!("History of {p}")).color(accent()));
                    }
                    let filtered = self.repo.as_ref().is_some_and(|r| r.query.is_filtered())
                        || self.path_filter.is_some()
                        || self.branch_only
                        || !self.search.is_empty()
                        || !self.author.is_empty();
                    if filtered && ui.add(small("Clear")).clicked() {
                        acts.push(Act::ClearQuery);
                    } else if apply {
                        acts.push(Act::ApplyQuery);
                    }
                    if let Some(r) = &self.repo {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} commits{}",
                                    r.commits.len(),
                                    if r.has_more { "+" } else { "" }
                                ))
                                .size(12.0)
                                .color(text_weak()),
                            );
                            let hint = if self.compare_id.is_some() {
                                RichText::new("Comparing two commits").size(12.0).color(accent())
                            } else {
                                RichText::new("Ctrl+click a second commit to compare")
                                    .size(12.0)
                                    .color(text_weak())
                            };
                            ui.label(hint);
                        });
                    }
                });
            });
        let r = ui.available_rect_before_wrap();
        ui.painter()
            .hline(r.x_range(), r.top(), Stroke::new(1.0, border()));
    }

    // ------------------------------------------------------------------ commit details

    #[allow(clippy::too_many_lines)]
    pub fn detail_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, settings: &mut Settings) {
        let Some(id) = self.selected_id.clone() else {
            return;
        };
        let Some(r) = &self.repo else { return };
        let commit = r.commits.iter().find(|c| c.id == id).cloned();
        let frame = Frame::new()
            .fill(bg_side())
            .inner_margin(Margin::symmetric(12, 8));
        egui::Panel::bottom("detail")
            .frame(frame)
            .show_separator_line(true)
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| {
                if let Some(c) = &commit {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(bold(&c.subject, 14.5));
                    let cmp = self
                        .compare_id
                        .as_ref()
                        .map(|a| format!("  ·  compared with {}", &a[..a.len().min(7)]))
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(format!(
                            "{} <{}>  ·  {}  ·  {}{cmp}",
                            c.author_name,
                            c.author_email,
                            fmt::date(c.time, true),
                            c.short_id()
                        ))
                        .size(12.0)
                        .color(text_weak()),
                    );
                }
                if !self.commit_body.is_empty() {
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("body")
                        .max_height(64.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&self.commit_body).size(12.5));
                        });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let mut reload = false;
                    reload |= ui.checkbox(&mut settings.side_by_side, "Side by side").changed();
                    reload |= ui
                        .checkbox(&mut settings.ignore_whitespace, "Ignore whitespace")
                        .changed();
                    ui.label(RichText::new("Context").size(12.0).color(text_weak()));
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.context)
                                .range(0..=30)
                                .speed(0.2),
                        )
                        .changed()
                    {
                        reload = true;
                    }
                    if reload {
                        settings.save();
                        acts.push(Act::ReloadView);
                    }
                });
                ui.add_space(2.0);
                ui.separator();
                if self.commit_files.is_empty() {
                    ui.label(RichText::new("No file changes.").color(text_weak()));
                    return;
                }
                let files_w = 260.0;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(vec2(files_w, ui.available_height()), |ui| {
                        ui.set_width(files_w);
                        egui::ScrollArea::vertical()
                            .id_salt("cfiles")
                            .auto_shrink(false)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                for f in &self.commit_files {
                                    let sel = self.selected_cfile.as_deref() == Some(f.path.as_str());
                                    if commit_file_row(ui, f, sel).clicked() {
                                        acts.push(Act::SelectCommitFile(f.path.clone()));
                                    }
                                }
                            });
                    });
                    ui.separator();
                    ui.vertical(|ui| match &self.commit_diff {
                        Some(d) if d.is_binary() => {
                            ui.label(RichText::new("Binary file — no text diff.").color(text_weak()));
                        }
                        Some(d) if d.is_empty() => {
                            ui.label(RichText::new("No textual changes.").color(text_weak()));
                        }
                        Some(d) => {
                            let mut none = Vec::<DiffAction>::new();
                            diffview::show(
                                ui,
                                "commit-diff",
                                d,
                                settings.side_by_side,
                                &mut self.commit_sel,
                                None,
                                &mut none,
                            );
                        }
                        None => {
                            ui.label(RichText::new("Loading…").color(text_weak()));
                        }
                    });
                });
            });
    }
}

fn commit_file_row(ui: &mut Ui, f: &CommitFile, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 3.0, accent().gamma_multiply(0.16));
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, bg_btn());
    }
    let cy = rect.center().y;
    p.text(
        pos2(rect.left() + 10.0, cy),
        Align2::CENTER_CENTER,
        f.status,
        FontId::monospace(12.0),
        status_color(f.status),
    );
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), f.path.as_str()),
    };
    let tp = p.with_clip_rect(rect);
    let g = tp.layout_no_wrap(name.to_string(), FontId::proportional(13.0), text());
    let x = rect.left() + 24.0;
    let w = g.size().x;
    tp.galley(pos2(x, cy - g.size().y / 2.0), g, text());
    if !dir.is_empty() {
        tp.text(
            pos2(x + w + 6.0, cy),
            Align2::LEFT_CENTER,
            dir,
            FontId::proportional(11.5),
            text_weak(),
        );
    }
    resp.on_hover_text(&f.path)
}

fn draw_graph(p: &egui::Painter, rect: Rect, row: &GraphRow, is_merge: bool, bg: Color32) {
    let x = |l: usize| rect.left() + 6.0 + l as f32 * LANE_W + LANE_W / 2.0;
    let mid = rect.center().y;
    for s in &row.top {
        p.line_segment(
            [pos2(x(s.from), rect.top()), pos2(x(s.to), mid)],
            Stroke::new(1.6, lane_color(s.color)),
        );
    }
    for s in &row.bottom {
        p.line_segment(
            [pos2(x(s.from), mid), pos2(x(s.to), rect.bottom())],
            Stroke::new(1.6, lane_color(s.color)),
        );
    }
    let c = pos2(x(row.col), mid);
    let color = lane_color(row.color);
    if is_merge {
        p.circle(c, 4.0, bg, Stroke::new(1.8, color));
    } else {
        p.circle(c, 4.0, color, Stroke::new(1.5, bg));
    }
}

/// Draws one commit row and its context menu.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn commit_row(
    ui: &mut Ui,
    r: &crate::worker::RepoData,
    i: usize,
    graph_w: f32,
    selected: bool,
    ctrl: bool,
    head_id: Option<&str>,
    acts: &mut Vec<Act>,
) {
    let c = &r.commits[i];
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 0.0, accent().gamma_multiply(0.14));
        p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, ROW_H)), 0.0, accent());
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, bg_side());
    }
    draw_graph(&p, rect, &r.graph.rows[i], c.parents.len() > 1, bg_main());

    let cy = rect.center().y;
    let right = rect.right() - 14.0;
    let (date_w, author_w, hash_w) = (84.0, 140.0, 58.0);
    let narrow = rect.width() - graph_w < 540.0;
    let author_left = right - date_w - 14.0 - author_w;
    let hash_left = author_left - 14.0 - hash_w;
    let text_right = if narrow {
        right - date_w - 12.0
    } else {
        hash_left - 12.0
    };

    let tp = p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(text_right.max(rect.left()), rect.bottom()),
    ));
    let mut x = rect.left() + graph_w;
    for l in &c.refs {
        let color = match (l.kind, l.is_head) {
            (_, true) => green(),
            (RefKind::Local, _) => accent(),
            (RefKind::Remote, _) => purple(),
            (RefKind::Tag, _) => amber(),
        };
        let galley = tp.layout_no_wrap(l.name.clone(), FontId::proportional(11.5), color);
        let size = galley.size() + vec2(10.0, 3.0);
        let br = Rect::from_min_size(pos2(x, cy - size.y / 2.0), size);
        tp.rect_filled(br, 3.0, color.gamma_multiply(0.15));
        tp.galley(br.min + vec2(5.0, 1.5), galley, color);
        x += size.x + 5.0;
    }
    tp.text(
        pos2(x + 2.0, cy),
        Align2::LEFT_CENTER,
        &c.subject,
        FontId::proportional(13.5),
        text(),
    );

    p.text(
        pos2(right, cy),
        Align2::RIGHT_CENTER,
        fmt::relative(c.time),
        FontId::proportional(12.0),
        text_weak(),
    );
    if !narrow {
        p.text(
            pos2(hash_left, cy),
            Align2::LEFT_CENTER,
            c.short_id(),
            FontId::monospace(11.5),
            text_weak(),
        );
        let ap = p.with_clip_rect(Rect::from_min_max(
            pos2(author_left, rect.top()),
            pos2(author_left + author_w, rect.bottom()),
        ));
        ap.text(
            pos2(author_left, cy),
            Align2::LEFT_CENTER,
            &c.author_name,
            FontId::proportional(12.5),
            text_weak(),
        );
    }
    if resp.clicked() {
        acts.push(Act::SelectCommit { idx: i, ctrl });
    }
    let on_head_line = head_id.is_some();
    let id = c.id.clone();
    resp.context_menu(|ui| {
        if ctx_item(ui, "Check out this commit", true) {
            acts.push(Act::Send(Request::CheckoutCommit(id.clone())));
        }
        if ctx_item(ui, "Create branch here…", true) {
            acts.push(Act::Dialog(Dialog::Input {
                title: "Create branch".into(),
                label: "Branch name".into(),
                value: String::new(),
                kind: InputKind::BranchAt { id: id.clone() },
                check: Some(("Switch to the new branch".into(), true)),
            }));
        }
        if ctx_item(ui, "Create tag here…", true) {
            acts.push(Act::Dialog(Dialog::Tag {
                target: id.clone(),
                name: String::new(),
                message: String::new(),
            }));
        }
        ui.separator();
        if ctx_item(ui, "Cherry-pick", true) {
            acts.push(Act::Send(Request::CherryPick(id.clone())));
        }
        if ctx_item(ui, "Revert", true) {
            acts.push(Act::Send(Request::Revert(id.clone())));
        }
        if ctx_item(ui, "Reset current branch to here…", true) {
            acts.push(Act::Dialog(Dialog::Reset {
                id: id.clone(),
                mode: rustle_core::ResetMode::Mixed,
            }));
        }
        if ctx_item(ui, "Interactive rebase from here…", on_head_line) {
            let from = head_id.unwrap_or_default();
            match first_parent_chain(&r.commits, from, &id) {
                Some(chain) if !chain.is_empty() => {
                    let rows = chain
                        .iter()
                        .map(|c| RebaseRow {
                            action: TodoAction::Pick,
                            id: c.id.clone(),
                            subject: c.subject.clone(),
                            message: c.subject.clone(),
                        })
                        .collect();
                    acts.push(Act::Dialog(Dialog::Rebase {
                        base: id.clone(),
                        rows,
                    }));
                }
                _ => acts.push(Act::Dialog(Dialog::Confirm {
                    title: "Interactive rebase".into(),
                    text: "Pick a commit that is an ancestor of the current branch (older than HEAD).".into(),
                    button: "OK".into(),
                    req: None,
                })),
            }
        }
        ui.separator();
        if ctx_item(ui, "Copy hash", true) {
            ui.ctx().copy_text(id.clone());
        }
        if ctx_item(ui, "Copy subject", true) {
            ui.ctx().copy_text(c.subject.clone());
        }
    });
}
