use crate::panels::ctx_item;
use crate::tab::{Act, Dialog, InputKind};
use crate::theme::{self, accent, bg_btn, bg_btn_hover, green, nav_row, nav_row_badges, text_weak};
use crate::worker::Request;
use eframe::egui::{self, Align2, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use rustle_core::Branch;
use std::collections::BTreeMap;

pub fn pins_file(repo: &std::path::Path) -> Option<std::path::PathBuf> {
    let g = repo.join(".git");
    g.is_dir().then(|| g.join("rustle-pins"))
}

pub fn load_pins(repo: &std::path::Path) -> Vec<String> {
    pins_file(repo)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn save_pins(repo: &std::path::Path, pins: &[String]) {
    if let Some(f) = pins_file(repo) {
        let _ = std::fs::write(f, pins.join("\n"));
    }
}

pub fn branch_rank(name: &str, pins: &[String]) -> (u8, usize) {
    if let Some(i) = pins.iter().position(|p| p == name) {
        (0, i)
    } else if matches!(name, "main" | "master") {
        (1, 0)
    } else {
        (2, 0)
    }
}

#[derive(Clone, Copy)]
pub struct TreeOpen {
    pub base: bool,
    pub epoch: u32,
    pub all: Option<bool>,
}

impl TreeOpen {
    pub fn open(self, depth: usize) -> bool {
        self.all.unwrap_or(self.base || depth == 0)
    }
}

#[derive(Default)]
pub struct BTree<'a> {
    dirs: BTreeMap<String, BTree<'a>>,
    items: Vec<(String, &'a Branch)>,
}

pub fn build_btree<'a>(list: &[&'a Branch]) -> BTree<'a> {
    let mut root = BTree::default();
    for b in list {
        let mut node = &mut root;
        let mut parts = b.name.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                node = node.dirs.entry(part.to_string()).or_default();
            } else {
                node.items.push((part.to_string(), b));
            }
        }
    }
    root
}

pub fn btree_ui(
    ui: &mut Ui,
    node: &BTree<'_>,
    prefix: &str,
    open: TreeOpen,
    depth: usize,
    row: &mut dyn FnMut(&mut Ui, &Branch, &str),
    mut dir_req: Option<&mut Option<String>>,
) {
    for (name, child) in &node.dirs {
        let id = format!("{prefix}{name}/");
        let r = egui::CollapsingHeader::new(RichText::new(name).size(13.0))
            .id_salt((&id, open.epoch))
            .default_open(open.open(depth))
            .show(ui, |ui| {
                btree_ui(ui, child, &id, open, depth + 1, row, dir_req.as_deref_mut());
            });
        if let Some(req) = dir_req.as_deref_mut() {
            r.header_response.context_menu(|ui| {
                if ctx_item(ui, "New branch in this folder…", true) {
                    *req = Some(id.get(2..).unwrap_or("").to_string());
                }
            });
        }
    }
    for (label, b) in &node.items {
        row(ui, b, label);
    }
}

fn pin_overlay(ui: &mut Ui, row: &egui::Response, pinned: bool, name: &str, acts: &mut Vec<Act>) {
    if !ui.rect_contains_pointer(row.rect) {
        return;
    }
    let rect = Rect::from_center_size(
        pos2(row.rect.right() - 13.0, row.rect.center().y),
        vec2(20.0, 20.0),
    );
    let r = ui.interact(rect, row.id.with("pin"), Sense::click());
    ui.painter()
        .rect_filled(rect, 4.0, if r.hovered() { bg_btn_hover() } else { bg_btn() });
    let glyph = if pinned {
        theme::icon(theme::IC_PIN_ON, "\u{2605}")
    } else {
        theme::icon(theme::IC_PIN, "\u{2606}")
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(13.0),
        if pinned || r.hovered() {
            accent()
        } else {
            text_weak()
        },
    );
    if r.clicked() {
        acts.push(Act::TogglePin(name.to_string()));
    }
    r.on_hover_text(if pinned { "Unpin" } else { "Pin to top" });
}

pub fn local_branch_row(
    ui: &mut Ui,
    b: &Branch,
    label: &str,
    busy: bool,
    current: &Option<String>,
    pinned: bool,
    acts: &mut Vec<Act>,
) {
    let resp = nav_row_badges(
        ui,
        label,
        b.is_head,
        b.is_head.then_some(green()),
        &crate::panels::branch_badges(b),
    );
    if resp.double_clicked() && !b.is_head && !busy {
        acts.push(Act::Send(Request::Checkout(b.clone())));
    }
    let mut tip = crate::panels::branch_tip(b);
    if !b.is_head {
        tip.push_str("\nDouble-click to check out.");
    }
    let resp = resp
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    pin_overlay(ui, &resp, pinned, &b.name, acts);
    resp.context_menu(|ui| {
        if ctx_item(ui, "Check out", !b.is_head) {
            acts.push(Act::Send(Request::Checkout(b.clone())));
        }
        if ctx_item(ui, if pinned { "Unpin" } else { "Pin to top" }, true) {
            acts.push(Act::TogglePin(b.name.clone()));
        }
        let has_up = b.upstream.is_some() && !b.gone;
        let push_label = if has_up { "Push" } else { "Publish branch" };
        if ctx_item(ui, push_label, !busy && (b.ahead > 0 || !has_up)) {
            acts.push(Act::Send(Request::PushBranch(b.name.clone())));
        }
        if ctx_item(
            ui,
            "Fast-forward to upstream",
            !busy && !b.is_head && has_up && b.behind > 0 && b.ahead == 0,
        ) {
            acts.push(Act::Send(Request::FastForward(b.name.clone())));
        }
        if ctx_item(ui, "Show only this branch in graph", true) {
            acts.push(Act::GraphOnly(Some(b.name.clone())));
        }
        if ctx_item(ui, "Open pull request…", true) {
            acts.push(Act::Send(Request::PullRequestUrl(b.name.clone())));
        }
        if ctx_item(ui, "New branch in this folder…", true) {
            let folder = b
                .name
                .rsplit_once('/')
                .map(|(d, _)| format!("{d}/"))
                .unwrap_or_default();
            acts.push(Act::Dialog(Dialog::Input {
                title: "New branch".into(),
                label: "Branch name".into(),
                value: folder,
                kind: InputKind::NewBranch,
                check: None,
            }));
        }
        ui.separator();
        if let Some(cur) = current {
            if ctx_item(ui, &format!("Compare with {cur}"), !b.is_head) {
                acts.push(Act::Send(Request::BranchCompare {
                    a: b.name.clone(),
                    b: cur.clone(),
                }));
            }
            if ctx_item(ui, &format!("Preview merge into {cur}"), !b.is_head) {
                acts.push(Act::Send(Request::MergePreview(b.name.clone())));
            }
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

pub fn remote_branch_row(
    ui: &mut Ui,
    b: &Branch,
    label: &str,
    busy: bool,
    current: &Option<String>,
    acts: &mut Vec<Act>,
) {
    let resp = nav_row(ui, label, false, None);
    if resp.double_clicked() && !busy {
        acts.push(Act::Send(Request::Checkout(b.clone())));
    }
    let resp = resp
        .on_hover_text(format!("{}\nDouble-click to check out as a local branch", b.name))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.context_menu(|ui| {
        if ctx_item(ui, "Check out as local branch", true) {
            acts.push(Act::Send(Request::Checkout(b.clone())));
        }
        if let Some(cur) = current {
            if ctx_item(ui, &format!("Compare with {cur}"), true) {
                acts.push(Act::Send(Request::BranchCompare {
                    a: b.name.clone(),
                    b: cur.clone(),
                }));
            }
            if ctx_item(ui, &format!("Preview merge into {cur}"), true) {
                acts.push(Act::Send(Request::MergePreview(b.name.clone())));
            }
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
                text: format!("Delete {} from the remote? This affects everyone.", b.name),
                button: "Delete".into(),
                req: Some(Request::DeleteRemoteBranch(b.name.clone())),
            }));
        }
        if ctx_item(ui, "Copy name", true) {
            ui.ctx().copy_text(b.name.clone());
        }
    });
}
