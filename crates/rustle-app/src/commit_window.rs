//! The commit window: unstaged files on top, staged files below, message and actions at the
//! bottom of the left column, and the diff (conflict editor, image diff) of the selected file on the
//! right.

use crate::diffview::{self, DiffAction, StageCtx, ViewExtras};
use crate::imagediff;
use crate::panels::{find_bar, status_icon};
use crate::settings::Settings;
use crate::tab::{Act, AppAct, ConflictEditor, Dialog, InputKind, Tab};
use crate::theme;
use crate::theme::{
    accent, amber, bg_btn, bg_btn_hover, bg_input, bold, border, green, primary_button, purple, red, text,
    text_weak,
};
use crate::worker::Request;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, pos2,
    vec2,
};
use rustle_core::conflict::{Choice, Segment};
use rustle_core::{FileChange, OpState, Side};
use std::collections::HashSet;

fn pending_message(repo: &std::path::Path) -> Option<String> {
    let git = repo.join(".git");
    for name in ["MERGE_MSG", "SQUASH_MSG"] {
        if let Ok(t) = std::fs::read_to_string(git.join(name)) {
            let m: Vec<&str> = t.lines().filter(|l| !l.starts_with('#')).collect();
            let m = m.join("\n").trim().to_string();
            if !m.is_empty() {
                return Some(m);
            }
        }
    }
    None
}

fn change_tree_ui(
    ui: &mut Ui,
    node: &crate::tab::FileTree,
    prefix: &str,
    salt: &str,
    by_path: &std::collections::HashMap<&str, &FileChange>,
    row: &mut dyn FnMut(&mut Ui, &FileChange),
) {
    for (name, child) in &node.dirs {
        let (mut label, mut cur, mut c) = (name.clone(), format!("{prefix}{name}"), child);
        while c.files.is_empty() && c.dirs.len() == 1 {
            let (n, ch) = c.dirs.iter().next().unwrap();
            label = format!("{label}/{n}");
            cur = format!("{cur}/{n}");
            c = ch;
        }
        egui::CollapsingHeader::new(RichText::new(label).size(13.0))
            .id_salt((salt, "cwdir", &cur))
            .default_open(true)
            .show(ui, |ui| {
                change_tree_ui(ui, c, &format!("{cur}/"), salt, by_path, row);
            });
    }
    for (_, full) in &node.files {
        if let Some(f) = by_path.get(full.as_str()) {
            row(ui, f);
        }
    }
}

/// A ticket key such as `I2C-180` found in a branch name, as a commit message prefix.
fn ticket_prefix(branch: &str) -> Option<String> {
    branch.split(['/', '_', ' ']).find_map(|tok| {
        let (a, b) = tok.split_once('-')?;
        let digits: String = b.chars().take_while(char::is_ascii_digit).collect();
        let ok = a.len() >= 2
            && a.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        (ok && !digits.is_empty()).then(|| format!("{a}-{digits}: "))
    })
}

fn display_order<'a>(list: &[&'a FileChange]) -> Vec<&'a FileChange> {
    let paths: Vec<String> = list.iter().map(|f| f.path.clone()).collect();
    let mut order = Vec::new();
    crate::tab::tree_paths(&crate::tab::FileTree::build(&paths), &mut order);
    order
        .iter()
        .filter_map(|p| list.iter().copied().find(|f| &f.path == p))
        .collect()
}

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

#[allow(clippy::too_many_arguments)]
/// One compact file row: status letter, name, dimmed folder, and a stage/unstage button.
fn file_row(
    ui: &mut Ui,
    f: &FileChange,
    is_staged: bool,
    selected: bool,
    busy: bool,
    is_lfs: bool,
    stat: Option<(u32, u32)>,
    tree: bool,
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
    status_icon(&p, pos2(rect.left() + 12.0, cy), letter);

    let right_pad = if is_lfs { 62.0 } else { 30.0 } + if stat.is_some() { 80.0 } else { 0.0 };
    if let Some((a, d)) = stat {
        let f = FontId::monospace(11.5);
        let r = p.text(
            pos2(rect.right() - 34.0, cy),
            Align2::RIGHT_CENTER,
            format!("−{d}"),
            f.clone(),
            Color32::from_rgb(0xe5, 0x53, 0x4b),
        );
        p.text(
            pos2(r.left() - 6.0, cy),
            Align2::RIGHT_CENTER,
            format!("+{a}"),
            f,
            Color32::from_rgb(0x3f, 0xb9, 0x50),
        );
    }
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((_, n)) if tree => (String::new(), n),
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), f.path.as_str()),
    };
    let tp = p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(rect.right() - right_pad, rect.bottom()),
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
    if is_lfs {
        let g = p.layout_no_wrap("LFS".into(), FontId::proportional(10.5), purple());
        let size = g.size() + vec2(8.0, 2.0);
        let br = Rect::from_min_size(pos2(rect.right() - 62.0 + 6.0, cy - size.y / 2.0), size);
        p.rect_filled(br, 3.0, purple().gamma_multiply(0.18));
        p.galley(br.min + vec2(4.0, 1.0), g, purple());
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
    egui::Button::new(RichText::new(label).size(12.0)).small()
}

/// A few lines of unchanged text between conflicts.
fn context_text(ui: &mut Ui, t: &str) {
    let lines: Vec<&str> = t.lines().collect();
    let shown: Vec<String> = if lines.len() > 6 {
        let mut v: Vec<String> = lines[..2].iter().map(|s| (*s).to_string()).collect();
        v.push(format!("⋯ {} unchanged lines ⋯", lines.len() - 4));
        v.extend(lines[lines.len() - 2..].iter().map(|s| (*s).to_string()));
        v
    } else {
        lines.iter().map(|s| (*s).to_string()).collect()
    };
    for l in shown {
        ui.label(RichText::new(l).monospace().size(12.0).color(text_weak()));
    }
}

fn conflict_column(
    ui: &mut Ui,
    title: &str,
    body: &str,
    color: egui::Color32,
    selected: bool,
    clickable: bool,
) -> bool {
    let (fill, width, edge) = if selected {
        (0.24, 2.0, 0.95)
    } else {
        (0.10, 1.0, 0.35)
    };
    let r = Frame::new()
        .fill(color.gamma_multiply(fill))
        .stroke(Stroke::new(width, color.gamma_multiply(edge)))
        .corner_radius(4)
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(11.0).color(color));
            ui.label(
                RichText::new(if body.is_empty() { "(empty)" } else { body })
                    .monospace()
                    .size(12.0),
            );
        });
    if !clickable {
        return false;
    }
    let resp = r
        .response
        .interact(Sense::click())
        .on_hover_text("Click to use this side")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter()
            .rect_stroke(resp.rect, 4.0, Stroke::new(1.5, color), egui::StrokeKind::Outside);
    }
    resp.clicked()
}

fn next_unresolved(e: &ConflictEditor) -> Option<usize> {
    let total = e.choices.len();
    (1..=total)
        .map(|d| (e.focus + d) % total)
        .find(|&k| e.choices[k].is_none())
}

/// The three-way conflict editor: pick ours, theirs, both, the base, or write your own per block.
fn conflict_editor_ui(ui: &mut Ui, e: &mut ConflictEditor, busy: bool, acts: &mut Vec<Act>) {
    let total = e.choices.len();
    let resolved = e.choices.iter().filter(|c| c.is_some()).count();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{resolved} of {total} resolved")).color(if resolved == total {
                green()
            } else {
                amber()
            }),
        );
        if total > 0 {
            if ui.add(small("◀")).on_hover_text("Previous conflict").clicked() {
                e.jump = Some((e.focus + total - 1) % total);
            }
            if ui.add(small("▶")).on_hover_text("Next conflict").clicked() {
                e.jump = Some((e.focus + 1) % total);
            }
            if resolved < total
                && ui
                    .add(small("Next unresolved"))
                    .on_hover_text("Jump to the next conflict without a choice")
                    .clicked()
            {
                e.jump = next_unresolved(e);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary_button(ui, "Save and mark resolved", resolved == total && !busy).clicked() {
                acts.push(Act::ConflictWrite);
            }
            if ui.add(small("All theirs")).clicked() {
                e.choices.iter_mut().for_each(|c| *c = Some(Choice::Theirs));
            }
            if ui.add(small("All ours")).clicked() {
                e.choices.iter_mut().for_each(|c| *c = Some(Choice::Ours));
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut k = 0;
        for seg in &e.segs {
            match seg {
                Segment::Text(t) => context_text(ui, t),
                Segment::Conflict(b) => {
                    ui.add_space(6.0);
                    let focused = e.focus == k;
                    let head = ui.horizontal(|ui| {
                        let state = if e.choices[k].is_some() {
                            "resolved"
                        } else {
                            "unresolved"
                        };
                        let mut title =
                            RichText::new(format!("Conflict {} of {total}  ·  {state}", k + 1)).size(13.0);
                        title = if e.choices[k].is_some() {
                            title.color(green())
                        } else if focused {
                            title.color(accent())
                        } else {
                            title.color(amber())
                        };
                        ui.label(title);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let choice = &mut e.choices[k];
                            let is = |c: &Option<Choice>, t: &Choice| c.as_ref() == Some(t);
                            let before = choice.clone();
                            if choice.is_some() && ui.add(small("✕")).on_hover_text("Clear choice").clicked()
                            {
                                *choice = None;
                            }
                            if ui
                                .selectable_label(matches!(choice, Some(Choice::Custom(_))), "Edit")
                                .on_hover_text("Write the result yourself")
                                .clicked()
                            {
                                if e.custom[k].is_empty() {
                                    e.custom[k] = b.ours.clone();
                                }
                                *choice = Some(Choice::Custom(String::new()));
                            }
                            if b.base.is_some()
                                && ui.selectable_label(is(choice, &Choice::Base), "Base").clicked()
                            {
                                *choice = Some(Choice::Base);
                            }
                            if ui
                                .selectable_label(is(choice, &Choice::BothReversed), "Theirs + ours")
                                .on_hover_text("Theirs first, then ours")
                                .clicked()
                            {
                                *choice = Some(Choice::BothReversed);
                            }
                            if ui
                                .selectable_label(is(choice, &Choice::Both), "Ours + theirs")
                                .on_hover_text("Ours first, then theirs")
                                .clicked()
                            {
                                *choice = Some(Choice::Both);
                            }
                            if ui
                                .selectable_label(is(choice, &Choice::Theirs), "Theirs")
                                .clicked()
                            {
                                *choice = Some(Choice::Theirs);
                            }
                            if ui.selectable_label(is(choice, &Choice::Ours), "Ours").clicked() {
                                *choice = Some(Choice::Ours);
                            }
                            if *choice != before {
                                e.focus = k;
                            }
                        });
                    });
                    if e.jump == Some(k) {
                        head.response.scroll_to_me(Some(Align::TOP));
                        e.focus = k;
                        e.jump = None;
                    }
                    let cols = if b.base.is_some() { 3 } else { 2 };
                    let cur = e.choices[k].clone();
                    let mut clicked: Option<Choice> = None;
                    ui.columns(cols, |c| {
                        if conflict_column(
                            &mut c[0],
                            &format!("Ours — {}", b.ours_label),
                            &b.ours,
                            accent(),
                            cur == Some(Choice::Ours),
                            true,
                        ) {
                            clicked = Some(Choice::Ours);
                        }
                        let mut next = 1;
                        if let Some(base) = &b.base {
                            if conflict_column(
                                &mut c[1],
                                "Base",
                                base,
                                text_weak(),
                                cur == Some(Choice::Base),
                                true,
                            ) {
                                clicked = Some(Choice::Base);
                            }
                            next = 2;
                        }
                        if conflict_column(
                            &mut c[next],
                            &format!("Theirs — {}", b.theirs_label),
                            &b.theirs,
                            purple(),
                            cur == Some(Choice::Theirs),
                            true,
                        ) {
                            clicked = Some(Choice::Theirs);
                        }
                    });
                    if let Some(ch) = clicked {
                        e.choices[k] = Some(ch);
                        e.focus = k;
                    }
                    match e.choices[k].clone() {
                        Some(Choice::Custom(_)) => {
                            ui.add(
                                egui::TextEdit::multiline(&mut e.custom[k])
                                    .code_editor()
                                    .desired_rows(4)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        Some(ch) => {
                            let res =
                                rustle_core::conflict::resolve(&[Segment::Conflict(b.clone())], &[Some(ch)]);
                            ui.add_space(2.0);
                            conflict_column(ui, "Result", &res, green(), false, false);
                        }
                        None => {}
                    }
                    k += 1;
                }
            }
        }
    });
}

impl Tab {
    #[allow(clippy::too_many_lines)]
    pub fn commit_popup(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>, settings: &mut Settings) {
        let busy = self.busy();
        let (files, op, repo_path, branch, lfs, stats) = {
            let Some(r) = &mut self.repo else { return };
            (
                r.status.files.clone(),
                r.status.op,
                r.path.clone(),
                r.status.branch.clone().unwrap_or_else(|| "detached HEAD".into()),
                // Moved out for the duration of the window instead of cloned every frame.
                std::mem::take(&mut r.lfs.files),
                r.status.stats.clone(),
            )
        };
        if op == OpState::None {
            self.msg_prefilled = false;
        } else if !self.msg_prefilled {
            self.msg_prefilled = true;
            if self.message.trim().is_empty()
                && let Some(m) = pending_message(&repo_path)
            {
                self.message = m;
            }
        }
        if op == OpState::None && !self.prefix_done && !self.amend {
            self.prefix_done = true;
            if self.message.trim().is_empty()
                && let Some(p) = ticket_prefix(&branch)
            {
                self.msg_prefix = p.clone();
                self.message = p;
            }
        }
        let conflicted: Vec<&FileChange> = files.iter().filter(|f| f.conflicted).collect();
        let staged: Vec<&FileChange> = files.iter().filter(|f| f.is_staged()).collect();
        let unstaged: Vec<&FileChange> = files
            .iter()
            .filter(|f| f.is_unstaged() && !f.conflicted)
            .collect();
        if !ctx.egui_wants_keyboard_input()
            && let Some((path, is_staged)) = self.selected_file.clone()
        {
            let (space, down, up) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                )
            });
            let order = display_order(if is_staged { &staged } else { &unstaged });
            if let Some(pos) = order.iter().position(|f| f.path == path) {
                if down && pos + 1 < order.len() {
                    acts.push(Act::SelectFile(order[pos + 1].clone(), is_staged));
                }
                if up && pos > 0 {
                    acts.push(Act::SelectFile(order[pos - 1].clone(), is_staged));
                }
                if space && !busy {
                    let f = order[pos];
                    let next = order
                        .get(pos + 1)
                        .or_else(|| pos.checked_sub(1).and_then(|p| order.get(p)));
                    acts.push(Act::Send(if is_staged {
                        Request::Unstage(f.paths())
                    } else {
                        Request::Stage(f.paths())
                    }));
                    if let Some(n) = next {
                        acts.push(Act::SelectFile((*n).clone(), is_staged));
                    }
                }
            }
        }
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
                                    let (ir, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                                    status_icon(ui.painter(), ir.center(), 'U');
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
                        let list_total = (total.y - 245.0 - conflict_h).max(180.0);
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
                                if !is_staged
                                    && ui
                                        .add_enabled(!list.is_empty() && !busy, small("Stash all"))
                                        .on_hover_text("Stash all changes…")
                                        .clicked()
                                {
                                    acts.push(Act::Dialog(Dialog::Input {
                                        title: "Stash changes".into(),
                                        label: "Message (optional)".into(),
                                        value: String::new(),
                                        kind: InputKind::Stash,
                                        check: Some(("Include untracked files".into(), true)),
                                    }));
                                }
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
                                    let paths: Vec<String> = list.iter().map(|f| f.path.clone()).collect();
                                    let tree = crate::tab::FileTree::build(&paths);
                                    let by_path: std::collections::HashMap<&str, &FileChange> =
                                        list.iter().map(|f| (f.path.as_str(), *f)).collect();
                                    let selected_file = self.selected_file.clone();
                                    change_tree_ui(ui, &tree, "", title, &by_path, &mut |ui, f| {
                                        let selected =
                                            selected_file.as_ref() == Some(&(f.path.clone(), is_staged));
                                        let (click, resp) = file_row(
                                            ui,
                                            f,
                                            is_staged,
                                            selected,
                                            busy,
                                            lfs.contains(&f.path),
                                            stats.get(&(f.path.clone(), is_staged)).copied(),
                                            true,
                                        );
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
                                    });
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
                        if self.message.lines().nth(1).is_some_and(|l| !l.trim().is_empty()) {
                            ui.label(
                                RichText::new("Leave a blank line after the title.")
                                    .size(11.5)
                                    .color(amber()),
                            );
                        }
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
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            ui.checkbox(&mut self.sign, "Sign commit").on_hover_text(
                                "Uses your GPG or SSH signing key (see user.signingkey and gpg.format)",
                            );
                            ui.checkbox(&mut self.no_verify, "Skip hooks").on_hover_text(
                                "Passes --no-verify: pre-commit and commit-msg hooks are not run",
                            );
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
                                    acts.push(Act::Send(Request::OpContinueMsg(self.message.clone())));
                                }
                            } else {
                                let can = (!staged.is_empty() || self.amend)
                                    && !self.message.trim().is_empty()
                                    && self.message.trim() != self.msg_prefix.trim()
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
                                if !self.amend
                                    && primary_button(
                                        ui,
                                        &format!(
                                            "Commit & Push  {}",
                                            theme::icon(theme::IC_PUSH, "\u{2191}")
                                        ),
                                        can,
                                    )
                                    .on_hover_text("Commit and push the current branch")
                                    .clicked()
                                    && can
                                {
                                    acts.push(Act::CommitPush);
                                }
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                            if busy {
                                ui.spinner();
                            }
                            ui.label(
                                RichText::new("Ctrl+Enter commit · Space stage/unstage · ↑↓ next file")
                                    .size(11.5)
                                    .color(text_weak()),
                            );
                        });
                        if let Some(e) = &self.error {
                            let first: String = e.lines().next().unwrap_or("").chars().take(160).collect();
                            ui.colored_label(red(), first);
                        }
                    });

                    ui.separator();

                    // ---------------------------------------------------------- right column
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), total.y),
                        Layout::top_down(Align::Min),
                        |ui| {
                            let title = self
                                .conflict_edit
                                .as_ref()
                                .map(|e| format!("{} — conflict", e.path))
                                .or_else(|| self.conflict.as_ref().map(|(p, _)| format!("{p} — conflict")))
                                .or_else(|| self.file_diff.as_ref().map(|d| d.title.clone()))
                                .unwrap_or_else(|| "No file selected".into());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(bold(title, 13.0));
                                let mut reload = false;
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
                                reload |= ui.checkbox(&mut self.full_file, "Whole file").changed();
                                if reload {
                                    settings.save();
                                    acts.push(Act::ReloadView);
                                }
                            });
                            ui.separator();
                            self.right_pane(ui, acts, settings, busy);
                        },
                    );
                });
            });
        if let Some(r) = &mut self.repo {
            r.lfs.files = lfs;
        }
        self.commit_open = open && !close;
    }

    fn right_pane(&mut self, ui: &mut Ui, acts: &mut Vec<Act>, settings: &Settings, busy: bool) {
        if let Some(e) = &mut self.conflict_edit {
            conflict_editor_ui(ui, e, busy, acts);
            return;
        }
        if let Some((_, v)) = &self.conflict {
            ui.label(
                RichText::new(
                    "This file can't be merged here (binary or not UTF-8). Pick a side on the left.",
                )
                .size(12.0)
                .color(text_weak()),
            );
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
            match &mut self.work_image {
                Some(img) if img.has_any() => imagediff::show(ui, img),
                Some(_) => {
                    ui.label(RichText::new("Binary file — no preview.").color(text_weak()));
                }
                None if self
                    .selected_file
                    .as_ref()
                    .is_some_and(|(p, _)| imagediff::is_image_path(p)) =>
                {
                    ui.label(RichText::new("Loading image…").color(text_weak()));
                }
                None => {
                    ui.label(RichText::new("Binary file — no text diff.").color(text_weak()));
                }
            }
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
        let (needle, current, scroll) =
            find_bar(ui, &mut self.find, &mut self.find_pos, d, settings.side_by_side);
        let extras = ViewExtras {
            find: needle.as_deref(),
            current_row: current,
            scroll_to: scroll,
            show_ws: settings.show_whitespace,
        };
        let mut da = Vec::<DiffAction>::new();
        diffview::show(
            ui,
            "file-diff",
            d,
            settings.side_by_side,
            &mut self.file_sel,
            ctx,
            &extras,
            &mut da,
        );
        acts.extend(da.into_iter().map(Act::Diff));
        let _ = (bg_input, border);
        let _: Option<&HashSet<String>> = None;
        let _ = Rect::NOTHING;
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
        if ui.button("Stash this file").clicked() {
            ui.close();
            acts.push(Act::Send(Request::StashFiles {
                paths: f.paths(),
                message: String::new(),
            }));
        }
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
        if ui.button("View file").clicked() {
            ui.close();
            acts.push(Act::OpenFile {
                rev: None,
                path: f.path.clone(),
                line: None,
            });
        }
        if ui.button("Open in editor").clicked() {
            ui.close();
            acts.push(Act::App(AppAct::Editor(repo_path.join(&f.path))));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::ticket_prefix;

    #[test]
    fn ticket_prefix_comes_from_the_branch_name() {
        assert_eq!(ticket_prefix("Tickets/I2C-180").as_deref(), Some("I2C-180: "));
        assert_eq!(
            ticket_prefix("feature/HUB-141-abonnementen").as_deref(),
            Some("HUB-141: ")
        );
        assert_eq!(ticket_prefix("main"), None);
        assert_eq!(ticket_prefix("fix/typo"), None);
    }
}
