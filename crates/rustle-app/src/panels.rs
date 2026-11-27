//! Drawing of the main panels of a tab: header, sidebar, commit graph and commit details.

use crate::diffview::{self, DiffAction, DiffView, ViewExtras};
use crate::branches::{btree_ui, build_btree, local_branch_row, remote_branch_row};
use crate::fmt;
use crate::imagediff;
use crate::settings::Settings;
use crate::theme;
use crate::tab::{Act, AppAct, Dialog, InputKind, RebaseRow, Tab};
use crate::theme::{
    accent, amber, bg_bar, bg_btn, bg_main, bg_side, bold, border, ghost, green, icon_button, nav_row,
    nav_row_badges, primary_button, purple, red, section_label, text, text_weak,
};
use crate::worker::{RepoData, Request};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Modifiers, Rect, RichText, Sense, Stroke,
    Ui, pos2, vec2,
};
use rustle_core::{Branch, Commit, CommitFile, GraphRow, OpState, RefKind, TodoAction};
use std::collections::HashSet;
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

pub fn ctx_item(ui: &mut Ui, label: &str, enabled: bool) -> bool {
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
fn first_parent_chain_to<'a>(commits: &'a [Commit], from: &str, to: &str) -> Option<Vec<&'a Commit>> {
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
pub fn branch_badges(b: &Branch) -> Vec<(String, Color32)> {
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
pub fn branch_tip(b: &Branch) -> String {
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

fn tool_btn(
    ui: &mut Ui,
    glyph: char,
    fallback: &str,
    count: u32,
    tip: &str,
    enabled: bool,
    color: Option<Color32>,
) -> egui::Response {
    let mut label = if theme::icons_ok() {
        glyph.to_string()
    } else {
        fallback.to_string()
    };
    if count > 0 {
        label = format!("{label} {count}");
    }
    let mut t = RichText::new(label).size(if theme::icons_ok() { 14.0 } else { 12.5 });
    if let Some(c) = color {
        t = t.color(c);
    }
    ui.add_enabled(enabled, egui::Button::new(t).min_size(vec2(30.0, 24.0)))
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn small(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.5))
}

/// "Find in diff" box with previous/next; returns the lowercase needle, the row of the current match
/// and a row to scroll to (only set right after the user moved).
pub fn find_bar(
    ui: &mut Ui,
    find: &mut String,
    pos: &mut usize,
    view: &DiffView,
    sbs: bool,
) -> (Option<String>, Option<usize>, Option<usize>) {
    let mut moved = false;
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(find)
                .hint_text("Find")
                .desired_width(150.0)
                .margin(Margin::symmetric(6, 2)),
        );
        if te.changed() {
            *pos = 0;
            moved = true;
        }
        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            *pos += 1;
            moved = true;
            te.request_focus();
        }
    });
    let needle = find.trim().to_lowercase();
    if needle.is_empty() {
        return (None, None, None);
    }
    let rows = view.find_rows(&needle, sbs);
    ui.horizontal(|ui| {
        if ui.add_enabled(!rows.is_empty(), small("◀")).clicked() {
            *pos = pos.checked_sub(1).unwrap_or(rows.len().saturating_sub(1));
            moved = true;
        }
        if ui.add_enabled(!rows.is_empty(), small("▶")).clicked() {
            *pos += 1;
            moved = true;
        }
        let shown = if rows.is_empty() {
            0
        } else {
            (*pos % rows.len()) + 1
        };
        ui.label(
            RichText::new(format!("{shown}/{}", rows.len()))
                .size(12.0)
                .color(text_weak()),
        );
    });
    if rows.is_empty() {
        return (Some(needle), None, None);
    }
    *pos %= rows.len();
    let row = rows[*pos];
    (Some(needle), Some(row), moved.then_some(row))
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
                                && let Some(dir) = rfd::FileDialog::new().pick_folder()
                            {
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
                                    let mut list: Vec<&Branch> =
                                        r.branches.iter().filter(|b| b.is_remote == remote).collect();
                                    if !remote {
                                        list.sort_by_key(|b| crate::branches::branch_rank(&b.name, &self.pins));
                                    }
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
                            ui.set_min_width(250.0);
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
                            if item(ui, "Reflog…") {
                                acts.push(Act::ShowReflog);
                            }
                            ui.separator();
                            if item(ui, "Repository settings…") {
                                acts.push(Act::OpenRepoSettings);
                            }
                            if item(ui, "Add worktree…") {
                                acts.push(Act::Dialog(Dialog::AddWorktree {
                                    path: String::new(),
                                    branch: String::new(),
                                    create: true,
                                }));
                            }
                            if item(ui, "Add submodule…") {
                                acts.push(Act::Dialog(Dialog::AddSubmodule {
                                    url: String::new(),
                                    path: String::new(),
                                }));
                            }
                            if item(ui, "Update all submodules") {
                                acts.push(Act::Send(Request::SubmoduleSync {
                                    path: None,
                                    remote: false,
                                }));
                            }
                            if item(ui, "Track with Git LFS…") {
                                acts.push(Act::Dialog(Dialog::TrackLfs {
                                    pattern: String::new(),
                                }));
                            }
                            if r.lfs.available && item(ui, "Pull LFS objects") {
                                acts.push(Act::Send(Request::LfsPull));
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
                        if tool_btn(
                            ui,
                            theme::IC_PUSH,
                            "Push",
                            st.ahead,
                            "Push the current branch",
                            !busy,
                            None,
                        )
                        .clicked()
                        {
                            acts.push(Act::Send(Request::Push { force: false }));
                        }
                        if tool_btn(
                            ui,
                            theme::IC_PULL,
                            "Pull",
                            st.behind,
                            "Pull from the upstream branch",
                            !busy,
                            None,
                        )
                        .clicked()
                        {
                            acts.push(Act::Send(Request::Pull));
                        }
                        let ago = self.last_fetch.elapsed().as_secs() / 60;
                        let mut tip = if ago == 0 {
                            "Fetch. Last fetch: less than a minute ago.".to_string()
                        } else {
                            format!("Fetch. Last fetch: {ago} min ago.")
                        };
                        let mut warn = None;
                        if let Some(e) = &self.auto_fetch_error {
                            tip = format!("The last automatic fetch failed:\n{e}");
                            warn = Some(amber());
                        }
                        if tool_btn(ui, theme::IC_FETCH, "Fetch", 0, &tip, !busy, warn).clicked() {
                            acts.push(Act::Send(Request::Fetch));
                        }
                        if tool_btn(ui, theme::IC_REFRESH, "Refresh", 0, "Reload the repository", !busy, None)
                            .clicked()
                        {
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
            // Long or multi-line errors (hook output, git messages) are shortened; the full text
            // is one click away.
            let long = is_err && (msg.contains('\n') || msg.chars().count() > 180);
            let shown = if long {
                let first = msg.lines().next().unwrap_or("");
                let mut s: String = first.chars().take(180).collect();
                if first.chars().count() > 180 || msg.lines().count() > 1 {
                    s.push('…');
                }
                s
            } else {
                msg.clone()
            };
            ui.add_space(6.0);
            Frame::new()
                .fill(color.gamma_multiply(0.12))
                .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
                .corner_radius(4)
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&shown).color(color));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, "✕", "Dismiss", true).clicked() {
                                if is_err {
                                    self.error = None;
                                } else {
                                    self.info = None;
                                }
                            }
                            if long && ui.add(small("Details")).clicked() {
                                acts.push(Act::ShowDetails);
                            }
                        });
                    });
                });
        }
    }

    // ------------------------------------------------------------------ sidebar

    #[allow(clippy::too_many_lines)]
    pub fn sidebar(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        if self.repo.is_none() {
            return;
        }
        let busy = self.busy();
        if let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) {
            let key = path.to_string_lossy().to_string();
            if self.pins_for != key {
                self.pins = crate::branches::load_pins(&path);
                self.pins_for = key;
            }
        }
        let pins = self.pins.clone();
        let frame = Frame::new().fill(bg_side()).inner_margin(Margin::symmetric(8, 8));
        egui::Panel::left("sidebar")
            .frame(frame)
            .show_separator_line(true)
            .default_size(230.0)
            .size_range(170.0..=460.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.sidebar_files, false, "Refs");
                    ui.selectable_value(&mut self.sidebar_files, true, "Files");
                });
                ui.add_space(4.0);
                if self.sidebar_files {
                    self.files_sidebar(ui, acts);
                    return;
                }
                let Some(r) = &self.repo else { return };
                let current = r.status.branch.clone();
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
                    let locals: Vec<&Branch> = r.branches.iter().filter(|b| !b.is_remote).collect();
                    let pinned_list: Vec<&Branch> = pins
                        .iter()
                        .filter_map(|n| locals.iter().copied().find(|b| &b.name == n))
                        .collect();
                    let mains: Vec<&Branch> = locals
                        .iter()
                        .copied()
                        .filter(|b| matches!(b.name.as_str(), "main" | "master") && !pins.contains(&b.name))
                        .collect();
                    if !pinned_list.is_empty() {
                        ui.label(heading("PINNED".into()));
                        for b in &pinned_list {
                            local_branch_row(ui, b, &b.name, busy, &current, true, acts);
                        }
                        ui.add_space(4.0);
                    }
                    for b in &mains {
                        local_branch_row(ui, b, &b.name, busy, &current, false, acts);
                    }
                    let rest: Vec<&Branch> = locals
                        .iter()
                        .copied()
                        .filter(|b| !pins.contains(&b.name) && !mains.iter().any(|m| m.name == b.name))
                        .collect();
                    let tree = build_btree(&rest);
                    btree_ui(ui, &tree, "l:", true, 0, &mut |ui, b, label| {
                        local_branch_row(ui, b, label, busy, &current, false, acts);
                    });

                    let remotes: Vec<&Branch> = r.branches.iter().filter(|b| b.is_remote).collect();
                    if !remotes.is_empty() {
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(heading(format!("REMOTES  {}", remotes.len())))
                            .default_open(true)
                            .show(ui, |ui| {
                                let tree = build_btree(&remotes);
                                btree_ui(ui, &tree, "r:", false, 0, &mut |ui, b, label| {
                                    remote_branch_row(ui, b, label, busy, &current, acts);
                                });
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
                    if r.worktrees.len() > 1 {
                        egui::CollapsingHeader::new(heading(format!("WORKTREES  {}", r.worktrees.len())))
                            .default_open(true)
                            .show(ui, |ui| {
                                for w in &r.worktrees {
                                    let is_cur = same_path(Path::new(&w.path), &r.path);
                                    let label = format!(
                                        "{}  ·  {}",
                                        folder_name(Path::new(&w.path)),
                                        w.branch.as_deref().unwrap_or("detached")
                                    );
                                    let dot = if w.is_main { green() } else { accent() };
                                    let resp = nav_row(ui, &label, is_cur, Some(dot)).on_hover_text(&w.path);
                                    if resp.double_clicked() && !is_cur {
                                        acts.push(Act::App(AppAct::OpenPath(PathBuf::from(&w.path))));
                                    }
                                    resp.context_menu(|ui| {
                                        if ctx_item(ui, "Open in new tab", !is_cur) {
                                            acts.push(Act::App(AppAct::OpenPath(PathBuf::from(&w.path))));
                                        }
                                        if ctx_item(ui, "Open in Explorer", true) {
                                            acts.push(Act::App(AppAct::Explorer(PathBuf::from(&w.path))));
                                        }
                                        if ctx_item(ui, "Remove…", !w.is_main && !is_cur) {
                                            acts.push(Act::Dialog(Dialog::Confirm {
                                                title: "Remove worktree".into(),
                                                text: format!(
                                                    "Remove the worktree at {}? Uncommitted changes in it are lost.",
                                                    w.path
                                                ),
                                                button: "Remove".into(),
                                                req: Some(Request::RemoveWorktree {
                                                    path: w.path.clone(),
                                                    force: true,
                                                }),
                                            }));
                                        }
                                    });
                                }
                                if r.worktrees.iter().any(|w| w.prunable)
                                    && ui.add(small("Prune stale worktrees")).clicked()
                                {
                                    acts.push(Act::Send(Request::PruneWorktrees));
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
                                    let resp = nav_row(ui, &s.path, false, Some(dot)).on_hover_text(match s.status {
                                        ' ' => "Up to date",
                                        '-' => "Not initialised",
                                        '+' => "Checked out at a different commit",
                                        _ => "Has conflicts",
                                    });
                                    if resp.double_clicked() && s.status != '-' {
                                        acts.push(Act::App(AppAct::OpenPath(r.path.join(&s.path))));
                                    }
                                    resp.context_menu(|ui| {
                                        if ctx_item(ui, "Open in new tab", s.status != '-') {
                                            acts.push(Act::App(AppAct::OpenPath(r.path.join(&s.path))));
                                        }
                                        let label = if s.status == '-' { "Initialise and update" } else { "Update" };
                                        if ctx_item(ui, label, !busy) {
                                            acts.push(Act::Send(Request::SubmoduleSync {
                                                path: Some(s.path.clone()),
                                                remote: false,
                                            }));
                                        }
                                        if ctx_item(ui, "Update to latest on its remote", !busy) {
                                            acts.push(Act::Send(Request::SubmoduleSync {
                                                path: Some(s.path.clone()),
                                                remote: true,
                                            }));
                                        }
                                    });
                                }
                            });
                    }
                    if !r.lfs.patterns.is_empty() {
                        egui::CollapsingHeader::new(heading(format!("GIT LFS  {}", r.lfs.patterns.len())))
                            .default_open(false)
                            .show(ui, |ui| {
                                for pat in &r.lfs.patterns {
                                    nav_row(ui, pat, false, Some(text_weak()));
                                }
                                if !r.lfs.available {
                                    ui.label(
                                        RichText::new("git-lfs is not installed").size(12.0).color(amber()),
                                    );
                                }
                                ui.horizontal(|ui| {
                                    if ui.add(small("Track…")).clicked() {
                                        acts.push(Act::Dialog(Dialog::TrackLfs {
                                            pattern: String::new(),
                                        }));
                                    }
                                    if r.lfs.available && ui.add_enabled(!busy, small("Pull objects")).clicked() {
                                        acts.push(Act::Send(Request::LfsPull));
                                    }
                                });
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
                // With uncommitted changes the graph gets an extra first row for them.
                let wip = r
                    .graph_wip
                    .as_ref()
                    .filter(|_| !r.status.files.is_empty() && !r.query.is_filtered());
                let graph = wip.unwrap_or(&r.graph);
                let off = usize::from(wip.is_some());
                let graph_w = graph.width.max(1) as f32 * LANE_W + 12.0;
                let mods: Modifiers = ui.input(|i| i.modifiers);
                let selected = self.selected;
                let multi: HashSet<&str> = self.multi.iter().map(String::as_str).collect();
                let compare = self.compare_id.clone();
                let head_id = r.head_id.clone();
                let mut last_visible = 0;
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                    ui,
                    ROW_H,
                    r.commits.len() + off,
                    |ui, range| {
                        last_visible = range.end;
                        for i in range {
                            if off == 1 && i == 0 {
                                wip_row(ui, r, graph_w, &graph.rows[0], acts);
                                continue;
                            }
                            let ci = i - off;
                            let c = &r.commits[ci];
                            let state = RowState {
                                selected: selected == Some(ci) || compare.as_deref() == Some(c.id.as_str()),
                                in_multi: multi.contains(c.id.as_str()),
                                mods,
                                draggable: self.reorderable.contains(&c.id),
                            };
                            commit_row(
                                ui,
                                r,
                                ci,
                                &graph.rows[i],
                                graph_w,
                                &state,
                                head_id.as_deref(),
                                &self.reorderable,
                                acts,
                            );
                        }
                    },
                );
                if r.has_more && !self.load_more_sent && last_visible + 40 >= r.commits.len() + off {
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
                            } else if self.multi.len() >= 2 {
                                RichText::new(format!("{} selected", self.multi.len()))
                                    .size(12.0)
                                    .color(accent())
                            } else {
                                RichText::new("Ctrl/Shift+click selects several commits")
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

    /// The panel that replaces the details when several commits are selected.
    fn multi_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let frame = Frame::new()
            .fill(bg_side())
            .inner_margin(Margin::symmetric(12, 8));
        let busy = self.busy();
        let squash = self.squash_dialog().map(|_| ());
        let sorted: Vec<String> = self
            .multi_sorted()
            .iter()
            .map(|(_, c)| format!("{}  {}", c.short_id(), c.subject))
            .collect();
        let n = sorted.len();
        egui::Panel::bottom("detail")
            .frame(frame)
            .show_separator_line(true)
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.label(bold(format!("{n} commits selected"), 14.5));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let tip = squash.as_ref().err().cloned().unwrap_or_default();
                    if ui
                        .add_enabled(!busy && squash.is_ok(), small("Squash…"))
                        .on_disabled_hover_text(tip)
                        .clicked()
                    {
                        acts.push(Act::SquashSelected);
                    }
                    if ui.add_enabled(!busy, small("Cherry-pick")).clicked() {
                        acts.push(Act::CherryPickSelected);
                    }
                    if ui.add_enabled(!busy, small("Revert")).clicked() {
                        acts.push(Act::RevertSelected);
                    }
                    if ui
                        .add_enabled(n == 2, small("Compare"))
                        .on_disabled_hover_text("Select exactly two commits")
                        .clicked()
                    {
                        acts.push(Act::CompareSelected);
                    }
                    if ui.add(small("Clear selection")).clicked() {
                        acts.push(Act::ClearMulti);
                    }
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for s in &sorted {
                        ui.label(RichText::new(s).size(12.5));
                    }
                });
            });
    }

    #[allow(clippy::too_many_lines)]
    pub fn detail_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, settings: &mut Settings) {
        if self.multi.len() >= 2 {
            self.multi_panel(ui, acts);
            return;
        }
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
                    ui.horizontal(|ui| {
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
                        if let Some((_, st, who)) = &self.signature {
                            let (label, color) = match st {
                                'G' => ("Signed", green()),
                                'U' => ("Signed, key not trusted", amber()),
                                'B' => ("Bad signature", red()),
                                'X' | 'Y' | 'R' => ("Signature expired or revoked", amber()),
                                'E' => ("Signature cannot be checked", amber()),
                                _ => ("", text_weak()),
                            };
                            if !label.is_empty() {
                                let l = if who.is_empty() {
                                    label.to_string()
                                } else {
                                    format!("{label}: {who}")
                                };
                                ui.label(RichText::new(l).size(12.0).color(color));
                            }
                        }
                    });
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
                let mut reload = false;
                ui.horizontal(|ui| {
                    reload |= ui.checkbox(&mut settings.side_by_side, "Side by side").changed();
                    reload |= ui
                        .checkbox(&mut settings.ignore_whitespace, "Ignore whitespace")
                        .changed();
                    if ui
                        .checkbox(&mut settings.show_whitespace, "Show whitespace")
                        .changed()
                    {
                        settings.save();
                    }
                    if ui.checkbox(&mut self.full_file, "Whole file").changed() {
                        reload = true;
                    }
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
                });
                if reload {
                    settings.save();
                    acts.push(Act::ReloadView);
                }
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
                                let total_a: u32 = self.commit_files.iter().filter_map(|f| f.added).sum();
                                let total_d: u32 = self.commit_files.iter().filter_map(|f| f.deleted).sum();
                                let all_sel = self.selected_cfile.as_deref() == Some("");
                                let r = all_rows(ui, self.commit_files.len(), total_a, total_d, all_sel);
                                if r.clicked() {
                                    acts.push(Act::SelectCommitFile(String::new()));
                                }
                                let paths: Vec<String> = self.commit_files.iter().map(|f| f.path.clone()).collect();
                                let tree = crate::tab::FileTree::build(&paths);
                                let by_path: std::collections::HashMap<&str, &CommitFile> =
                                    self.commit_files.iter().map(|f| (f.path.as_str(), f)).collect();
                                let rev = self.selected_id.clone();
                                commit_tree_ui(ui, &tree, "", &by_path, self.selected_cfile.as_deref(), &rev, acts);
                            });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        let binary = self.commit_diff.as_ref().is_some_and(DiffView::is_binary);
                        if binary {
                            match &mut self.commit_image {
                                Some(img) if img.has_any() => imagediff::show(ui, img),
                                Some(_) => {
                                    ui.label(RichText::new("Binary file — no preview.").color(text_weak()));
                                }
                                None if self
                                    .selected_cfile
                                    .as_deref()
                                    .is_some_and(imagediff::is_image_path) =>
                                {
                                    ui.label(RichText::new("Loading image…").color(text_weak()));
                                }
                                None => {
                                    ui.label(RichText::new("Binary file — no text diff.").color(text_weak()));
                                }
                            }
                            return;
                        }
                        match &self.commit_diff {
                            Some(d) if d.is_empty() => {
                                ui.label(RichText::new("No textual changes.").color(text_weak()));
                            }
                            Some(d) => {
                                let (needle, current, scroll) = find_bar(
                                    ui,
                                    &mut self.find,
                                    &mut self.find_pos,
                                    d,
                                    settings.side_by_side,
                                );
                                let extras = ViewExtras {
                                    find: needle.as_deref(),
                                    current_row: current,
                                    scroll_to: scroll,
                                    show_ws: settings.show_whitespace,
                                };
                                let mut none = Vec::<DiffAction>::new();
                                diffview::show(
                                    ui,
                                    "commit-diff",
                                    d,
                                    settings.side_by_side,
                                    &mut self.commit_sel,
                                    None,
                                    &extras,
                                    &mut none,
                                );
                            }
                            None => {
                                ui.label(RichText::new("Loading…").color(text_weak()));
                            }
                        }
                    });
                });
            });
    }
}

fn all_rows(ui: &mut Ui, n: usize, a: u32, d: u32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 3.0, accent().gamma_multiply(0.16));
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, bg_btn());
    }
    let cy = rect.center().y;
    p.text(
        pos2(rect.left() + 6.0, cy),
        Align2::LEFT_CENTER,
        format!("All files ({n})"),
        FontId::proportional(13.0),
        text(),
    );
    paint_counts(&p, rect, Some(a), Some(d));
    resp.on_hover_text("Show the diff of all files in this commit")
}

fn paint_counts(p: &egui::Painter, rect: egui::Rect, a: Option<u32>, d: Option<u32>) {
    let cy = rect.center().y;
    let mut x = rect.right() - 6.0;
    let f = FontId::monospace(11.5);
    if a.is_none() && d.is_none() {
        p.text(pos2(x, cy), Align2::RIGHT_CENTER, "bin", f, text_weak());
        return;
    }
    let ds = format!("\u{2212}{}", d.unwrap_or(0));
    let r = p.text(pos2(x, cy), Align2::RIGHT_CENTER, ds, f.clone(), Color32::from_rgb(0xe5, 0x53, 0x4b));
    x = r.left() - 6.0;
    p.text(
        pos2(x, cy),
        Align2::RIGHT_CENTER,
        format!("+{}", a.unwrap_or(0)),
        f,
        Color32::from_rgb(0x3f, 0xb9, 0x50),
    );
}

fn commit_tree_ui(
    ui: &mut Ui,
    node: &crate::tab::FileTree,
    prefix: &str,
    by_path: &std::collections::HashMap<&str, &CommitFile>,
    selected: Option<&str>,
    rev: &Option<String>,
    acts: &mut Vec<Act>,
) {
    for (name, child) in &node.dirs {
        let path = format!("{prefix}{name}");
        let (mut label, mut cur, mut c) = (name.clone(), path.clone(), child);
        while c.files.is_empty() && c.dirs.len() == 1 {
            let (n, ch) = c.dirs.iter().next().unwrap();
            label = format!("{label}/{n}");
            cur = format!("{cur}/{n}");
            c = ch;
        }
        egui::CollapsingHeader::new(RichText::new(label).size(13.0))
            .id_salt(("cdir", &cur))
            .default_open(true)
            .show(ui, |ui| {
                commit_tree_ui(ui, c, &format!("{cur}/"), by_path, selected, rev, acts);
            });
    }
    for (name, full) in &node.files {
        let Some(f) = by_path.get(full.as_str()) else { continue };
        let resp = commit_file_row(ui, f, selected == Some(f.path.as_str()), Some(name.as_str()));
        if resp.clicked() {
            acts.push(Act::SelectCommitFile(f.path.clone()));
        }
        resp.context_menu(|ui| {
            if ctx_item(ui, "View file at this commit", f.status != 'D') {
                acts.push(Act::OpenFile {
                    rev: rev.clone(),
                    path: f.path.clone(),
                    line: None,
                });
            }
            if ctx_item(ui, "File history", true) {
                acts.push(Act::History(f.path.clone()));
            }
            if ctx_item(ui, "Blame", f.status != 'D') {
                acts.push(Act::Blame(f.path.clone()));
            }
            if ctx_item(ui, "Copy path", true) {
                ui.ctx().copy_text(f.path.clone());
            }
        });
    }
}

fn commit_file_row(ui: &mut Ui, f: &CommitFile, selected: bool, name_only: Option<&str>) -> egui::Response {
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
    let (dir, name) = match (name_only, f.path.rsplit_once('/')) {
        (Some(n), _) => (String::new(), n),
        (None, Some((d, n))) => (format!("{d}/"), n),
        (None, None) => (String::new(), f.path.as_str()),
    };
    paint_counts(&p, rect, f.added, f.deleted);
    let tp = p.with_clip_rect(rect.with_max_x(rect.right() - 70.0));
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

/// The row for uncommitted changes, drawn above HEAD.
fn wip_row(ui: &mut Ui, r: &RepoData, graph_w: f32, grow: &GraphRow, acts: &mut Vec<Act>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let p = ui.painter_at(rect);
    if resp.hovered() {
        p.rect_filled(rect, 0.0, bg_btn());
    }
    // Lines as usual, but a hollow node: this is not a commit (yet).
    let mut node_less = grow.clone();
    node_less.color = 7;
    draw_graph(&p, rect, &node_less, true, bg_main());

    let files = &r.status.files;
    let staged = files.iter().filter(|f| f.is_staged()).count();
    let unstaged = files.iter().filter(|f| f.is_unstaged() && !f.conflicted).count();
    let conflicts = files.iter().filter(|f| f.conflicted).count();
    let cy = rect.center().y;
    let mut x = rect.left() + graph_w;
    p.text(
        pos2(x + 2.0, cy),
        Align2::LEFT_CENTER,
        "Working changes",
        FontId::proportional(13.5),
        text_weak(),
    );
    x += 125.0;
    for (n, label, color) in [
        (staged, "staged", green()),
        (unstaged, "unstaged", amber()),
        (conflicts, "conflicted", red()),
    ] {
        if n == 0 {
            continue;
        }
        let g = p.layout_no_wrap(format!("{n} {label}"), FontId::proportional(11.5), color);
        let size = g.size() + vec2(10.0, 3.0);
        let br = Rect::from_min_size(pos2(x, cy - size.y / 2.0), size);
        p.rect_filled(br, 3.0, color.gamma_multiply(0.15));
        p.galley(br.min + vec2(5.0, 1.5), g, color);
        x += size.x + 5.0;
    }
    if resp.clicked() {
        acts.push(Act::OpenCommit);
    }
    resp.on_hover_text("Click to review and commit your changes");
}

/// What a commit row needs to know about the current selection and drag state.
struct RowState {
    selected: bool,
    in_multi: bool,
    mods: Modifiers,
    draggable: bool,
}

/// Draws one commit row and its context menu.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn commit_row(
    ui: &mut Ui,
    r: &RepoData,
    i: usize,
    grow: &GraphRow,
    graph_w: f32,
    state: &RowState,
    head_id: Option<&str>,
    reorderable: &HashSet<String>,
    acts: &mut Vec<Act>,
) {
    let c = &r.commits[i];
    let sense = if state.draggable {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
    let p = ui.painter_at(rect);
    if state.selected || state.in_multi {
        p.rect_filled(
            rect,
            0.0,
            accent().gamma_multiply(if state.in_multi { 0.22 } else { 0.14 }),
        );
        p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, ROW_H)), 0.0, accent());
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, bg_btn());
    }
    draw_graph(&p, rect, grow, c.parents.len() > 1, bg_main());

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

    // Drag a not-yet-pushed commit onto another one to move it above that commit.
    if state.draggable && resp.drag_started() {
        resp.dnd_set_drag_payload(c.id.clone());
    }
    if reorderable.contains(&c.id) && resp.dnd_hover_payload::<String>().is_some() {
        p.hline(rect.x_range(), rect.top() + 1.0, Stroke::new(2.0, accent()));
    }
    if let Some(from) = resp.dnd_release_payload::<String>()
        && reorderable.contains(&c.id)
    {
        acts.push(Act::DropCommit {
            from: (*from).clone(),
            to: c.id.clone(),
        });
    }

    if resp.clicked() {
        acts.push(Act::SelectCommit {
            idx: i,
            ctrl: state.mods.command,
            shift: state.mods.shift,
        });
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
            match first_parent_chain_to(&r.commits, from, &id) {
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
        if ctx_item(ui, "Browse files at this commit", true) {
            acts.push(Act::BrowseAt(Some(id.clone())));
        }
        if ctx_item(ui, "Copy hash", true) {
            ui.ctx().copy_text(id.clone());
        }
        if ctx_item(ui, "Copy subject", true) {
            ui.ctx().copy_text(c.subject.clone());
        }
    });
}
