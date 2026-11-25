//! File browser in the sidebar (folder tree, name filter, content search) and the file viewer window.

use crate::diffview::{self, DiffAction, ViewExtras};
use crate::panels::{ctx_item, find_bar, status_color};
use crate::settings::Settings;
use crate::tab::{Act, AppAct, FileTree, Tab, ViewerState};
use crate::theme::{accent, amber, bold, nav_row_badges, purple, text_weak};
use eframe::egui::{self, Color32, Margin, RichText, Ui};
use std::collections::{HashMap, HashSet};

/// Shared context for drawing the tree.
struct TreeCtx<'a> {
    rev: &'a Option<String>,
    status: &'a HashMap<&'a str, char>,
    lfs: &'a HashSet<String>,
}

fn file_row(ui: &mut Ui, name: &str, path: &str, c: &TreeCtx<'_>, acts: &mut Vec<Act>) {
    let mut badges: Vec<(String, Color32)> = Vec::new();
    if let Some(l) = c.status.get(path) {
        badges.push((l.to_string(), status_color(*l)));
    }
    if c.lfs.contains(path) {
        badges.push(("LFS".into(), purple()));
    }
    let resp = nav_row_badges(ui, name, false, None, &badges).on_hover_text(path);
    if resp.clicked() {
        acts.push(Act::OpenFile {
            rev: c.rev.clone(),
            path: path.to_string(),
            line: None,
        });
    }
    resp.context_menu(|ui| {
        if ctx_item(ui, "View file", true) {
            acts.push(Act::OpenFile {
                rev: c.rev.clone(),
                path: path.to_string(),
                line: None,
            });
        }
        if ctx_item(ui, "File history", true) {
            acts.push(Act::History(path.to_string()));
        }
        if ctx_item(ui, "Blame", c.rev.is_none()) {
            acts.push(Act::Blame(path.to_string()));
        }
        if ctx_item(ui, "Copy path", true) {
            ui.ctx().copy_text(path.to_string());
        }
    });
}

fn tree_ui(ui: &mut Ui, node: &FileTree, prefix: &str, c: &TreeCtx<'_>, acts: &mut Vec<Act>) {
    for (name, child) in &node.dirs {
        let path = format!("{prefix}{name}");
        egui::CollapsingHeader::new(RichText::new(name).size(13.5))
            .id_salt(&path)
            .default_open(false)
            .show(ui, |ui| tree_ui(ui, child, &format!("{path}/"), c, acts));
    }
    for (name, full) in &node.files {
        file_row(ui, name, full, c, acts);
    }
}

impl Tab {
    /// The "Files" side of the sidebar.
    pub fn files_sidebar(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        if self.files.is_none() && !self.files_loading {
            acts.push(Act::LoadFiles);
        }
        if let Some(rev) = &self.files_rev {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("At commit {}", &rev[..rev.len().min(7)]))
                        .size(12.0)
                        .color(accent()),
                );
                if ui.small_button("Working tree").clicked() {
                    acts.push(Act::BrowseAt(None));
                }
            });
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.grep_mode, false, "Names");
            ui.selectable_value(&mut self.grep_mode, true, "Content");
        });
        ui.add_space(2.0);

        if self.grep_mode {
            let te = ui.add(
                egui::TextEdit::singleline(&mut self.grep_text)
                    .hint_text("Search in files, then press Enter")
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(6, 3)),
            );
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                acts.push(Act::GrepFiles);
            }
            ui.add_space(4.0);
            match &self.grep {
                None => {
                    ui.label(
                        RichText::new("Case-insensitive text search.")
                            .size(12.0)
                            .color(text_weak()),
                    );
                }
                Some((pattern, hits)) if hits.is_empty() => {
                    ui.label(RichText::new(format!("No matches for “{pattern}”.")).color(text_weak()));
                }
                Some((pattern, hits)) => {
                    ui.label(
                        RichText::new(format!(
                            "{} match(es) for “{pattern}”{}",
                            hits.len(),
                            if hits.len() >= 1000 { " (first 1000)" } else { "" }
                        ))
                        .size(12.0)
                        .color(text_weak()),
                    );
                    let rev = self.files_rev.clone();
                    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                        ui,
                        36.0,
                        hits.len(),
                        |ui, range| {
                            for h in &hits[range] {
                                let resp = ui
                                    .vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        ui.label(
                                            RichText::new(format!("{}:{}", h.path, h.line))
                                                .size(11.5)
                                                .color(accent()),
                                        );
                                        let t: String = h.text.trim().chars().take(90).collect();
                                        ui.label(RichText::new(t).monospace().size(12.0));
                                    })
                                    .response
                                    .interact(egui::Sense::click());
                                if resp.clicked() {
                                    acts.push(Act::OpenFile {
                                        rev: rev.clone(),
                                        path: h.path.clone(),
                                        line: Some(h.line),
                                    });
                                }
                            }
                        },
                    );
                }
            }
            return;
        }

        ui.add(
            egui::TextEdit::singleline(&mut self.file_filter)
                .hint_text("Filter files…")
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(6, 3)),
        );
        ui.add_space(4.0);
        let Some(files) = &self.files else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading files…").color(text_weak()));
            });
            return;
        };
        let status: HashMap<&str, char> = self
            .repo
            .as_ref()
            .map(|r| {
                r.status
                    .files
                    .iter()
                    .map(|f| {
                        (
                            f.path.as_str(),
                            if f.is_staged() {
                                f.index
                            } else if f.is_untracked() {
                                'N'
                            } else {
                                f.worktree
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let empty = HashSet::new();
        let lfs = self.repo.as_ref().map_or(&empty, |r| &r.lfs.files);
        let c = TreeCtx {
            rev: &self.files_rev,
            status: &status,
            lfs,
        };
        let filter = self.file_filter.trim().to_lowercase();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if filter.is_empty() {
                if let Some(tree) = &self.files_tree {
                    tree_ui(ui, tree, "", &c, acts);
                }
            } else {
                let matches: Vec<&String> = files
                    .iter()
                    .filter(|p| p.to_lowercase().contains(&filter))
                    .take(400)
                    .collect();
                if matches.is_empty() {
                    ui.label(RichText::new("No files match.").color(text_weak()));
                }
                for p in &matches {
                    file_row(ui, p, p, &c, acts);
                }
                if matches.len() == 400 {
                    ui.label(
                        RichText::new("Showing the first 400 matches.")
                            .size(12.0)
                            .color(amber()),
                    );
                }
            }
        });
    }

    /// The window that shows one file with syntax highlighting.
    pub fn viewer_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>, settings: &Settings) {
        let repo_path = self.repo.as_ref().map(|r| r.path.clone());
        let Some(v) = &mut self.viewer else { return };
        let mut open = true;
        let title = match &v.rev {
            Some(r) => format!("{} @ {}", v.path, &r[..r.len().min(7)]),
            None => v.path.clone(),
        };
        let (rev, path) = (v.rev.clone(), v.path.clone());
        egui::Window::new(bold(title, 14.0))
            .id(egui::Id::unique("file-viewer"))
            .open(&mut open)
            .default_size([900.0, 620.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("History").clicked() {
                        acts.push(Act::History(path.clone()));
                    }
                    if rev.is_none() {
                        if ui.small_button("Blame").clicked() {
                            acts.push(Act::Blame(path.clone()));
                        }
                        if let Some(rp) = &repo_path
                            && ui.small_button("Open in editor").clicked()
                        {
                            acts.push(Act::App(AppAct::Editor(rp.join(&path))));
                        }
                    }
                });
                ui.separator();
                match &v.state {
                    ViewerState::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Loading…");
                        });
                    }
                    ViewerState::Failed(e) => {
                        ui.colored_label(crate::theme::red(), e);
                    }
                    ViewerState::Binary(size) => {
                        ui.label(
                            RichText::new(format!("Binary or very large file ({size} bytes) — no preview."))
                                .color(text_weak()),
                        );
                    }
                    ViewerState::Ready(view) => {
                        let (needle, current, scroll) =
                            find_bar(ui, &mut v.find, &mut v.find_pos, view, false);
                        let jump = v.jump.take();
                        let extras = ViewExtras {
                            find: needle.as_deref(),
                            current_row: current.or(jump),
                            scroll_to: scroll.or(jump),
                            show_ws: settings.show_whitespace,
                        };
                        let mut sel = HashSet::new();
                        let mut none = Vec::<DiffAction>::new();
                        diffview::show(ui, "viewer", view, false, &mut sel, None, &extras, &mut none);
                    }
                }
            });
        if !open {
            self.viewer = None;
        }
    }
}
