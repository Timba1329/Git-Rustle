//! Extra windows: branch comparison, repository dashboard, hooks, bisect, stash preview and toasts.

use crate::diffview::{self, DiffAction, DiffView, ViewExtras};
use crate::fmt;
use crate::panels::status_color;
use crate::tab::{Act, AppAct, Tab};
use crate::theme::{accent, amber, bold, green, red, text_weak};
use crate::worker::Request;
use eframe::egui::{self, Align2, RichText, Ui, vec2};
use rustle_core::{Commit, CommitFile};
use std::collections::HashSet;
use std::time::Duration;

pub struct CompareView {
    pub a: String,
    pub b: String,
    pub triple: bool,
    pub viewed: HashSet<String>,
    pub diff: Option<(String, DiffView)>,
    pub sel: HashSet<(usize, usize, usize)>,
    pub only_a: Vec<Commit>,
    pub only_b: Vec<Commit>,
    pub files: Vec<CommitFile>,
}

pub type StashView = (usize, DiffView, HashSet<(usize, usize, usize)>);

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / f64::from(1 << 20))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

fn commit_list(ui: &mut Ui, title: &str, list: &[Commit]) {
    ui.label(bold(
        format!(
            "{title}  ({}{})",
            list.len(),
            if list.len() >= 300 { "+" } else { "" }
        ),
        13.0,
    ));
    egui::ScrollArea::vertical()
        .id_salt(title)
        .max_height(220.0)
        .auto_shrink(false)
        .show(ui, |ui| {
            if list.is_empty() {
                ui.label(RichText::new("Nothing").color(text_weak()));
            }
            for c in list {
                let short: String = c.id.chars().take(7).collect();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(short).monospace().size(12.0).color(accent()));
                    ui.label(RichText::new(&c.subject).size(12.0));
                })
                .response
                .on_hover_text(format!("{} · {}", c.author_name, fmt::date(c.time, true)));
            }
        });
}

impl Tab {
    pub fn extra_windows(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        self.compare_window(ctx, acts);
        self.backups_window(ctx, acts);
        self.dashboard_window(ctx, acts);
        self.hooks_window(ctx, acts);
        self.bisect_window(ctx, acts);
        self.stash_window(ctx, acts);
        self.toast_ui(ctx);
    }

    fn compare_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(v) = &mut self.compare_view else { return };
        let mut open = true;
        let title = if v.triple {
            format!("Review {} against {}", v.b, v.a)
        } else {
            format!("Compare {} ↔ {}", v.a, v.b)
        };
        egui::Window::new(bold(title, 15.0))
            .open(&mut open)
            .default_size([1100.0, 660.0])
            .show(ctx, |ui| {
                egui::CollapsingHeader::new(format!("Commits  ({} / {})", v.only_a.len(), v.only_b.len()))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.columns(2, |c| {
                            commit_list(&mut c[0], &format!("Only in {}", v.a), &v.only_a);
                            commit_list(&mut c[1], &format!("Only in {}", v.b), &v.only_b);
                        });
                    });
                let total = v.files.len();
                let seen = v.files.iter().filter(|f| v.viewed.contains(&f.path)).count();
                ui.horizontal(|ui| {
                    ui.label(bold(format!("{seen} of {total} files viewed"), 13.5));
                    ui.add(egui::ProgressBar::new(seen as f32 / total.max(1) as f32).desired_width(160.0));
                });
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        vec2(340.0, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(340.0);
                            egui::ScrollArea::vertical()
                                .id_salt("cmpfiles")
                                .auto_shrink(false)
                                .show(ui, |ui| {
                                    for f in &v.files {
                                        ui.horizontal(|ui| {
                                            let mut viewed = v.viewed.contains(&f.path);
                                            if ui
                                                .checkbox(&mut viewed, "")
                                                .on_hover_text("Mark as viewed")
                                                .changed()
                                            {
                                                if viewed {
                                                    v.viewed.insert(f.path.clone());
                                                } else {
                                                    v.viewed.remove(&f.path);
                                                }
                                            }
                                            ui.label(
                                                RichText::new(f.status)
                                                    .monospace()
                                                    .color(status_color(f.status)),
                                            );
                                            let current = v.diff.as_ref().is_some_and(|(p, _)| *p == f.path);
                                            let name = f.path.rsplit('/').next().unwrap_or(&f.path);
                                            if ui
                                                .selectable_label(current, name)
                                                .on_hover_text(&f.path)
                                                .clicked()
                                            {
                                                acts.push(Act::Send(Request::ReviewDiff {
                                                    a: v.a.clone(),
                                                    b: v.b.clone(),
                                                    triple: v.triple,
                                                    path: f.path.clone(),
                                                }));
                                            }
                                            if let (Some(a), Some(d)) = (f.added, f.deleted) {
                                                ui.label(
                                                    RichText::new(format!("+{a}")).color(green()).size(11.0),
                                                );
                                                ui.label(
                                                    RichText::new(format!("−{d}")).color(red()).size(11.0),
                                                );
                                            }
                                        });
                                    }
                                });
                        },
                    );
                    ui.separator();
                    ui.vertical(|ui| {
                        if let Some((_, dv)) = &v.diff {
                            let extras = ViewExtras {
                                find: None,
                                current_row: None,
                                scroll_to: None,
                                show_ws: false,
                            };
                            let mut none = Vec::<DiffAction>::new();
                            diffview::show(
                                ui,
                                "review-diff",
                                dv,
                                false,
                                &mut v.sel,
                                None,
                                &extras,
                                &mut none,
                            );
                        } else {
                            ui.label(RichText::new("Select a file to see its changes.").color(text_weak()));
                        }
                    });
                });
            });
        if !open {
            self.compare_view = None;
        }
    }

    fn backups_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(list) = &self.backups else { return };
        let busy = self.busy();
        let mut open = true;
        egui::Window::new(bold("Backups", 15.0))
            .open(&mut open)
            .default_size([560.0, 380.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Rustle saves HEAD and your tracked changes here before a hard reset or a discard.",
                    )
                    .size(12.0)
                    .color(text_weak()),
                );
                ui.add_space(4.0);
                if list.is_empty() {
                    ui.label("No backups yet.");
                }
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for b in list {
                        ui.horizontal(|ui| {
                            let kind = if b.refname.ends_with("-work") {
                                "changes"
                            } else {
                                "HEAD"
                            };
                            let label = b.refname.rsplit('/').next().unwrap_or("");
                            let label = label.split_once('-').map_or(label, |(_, l)| l);
                            ui.label(
                                RichText::new(fmt::date(b.time, true))
                                    .size(12.0)
                                    .color(text_weak()),
                            );
                            ui.label(format!("{label}  ({kind})"));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add_enabled(!busy, egui::Button::new("Delete").small())
                                    .clicked()
                                {
                                    acts.push(Act::Send(Request::DeleteBackup(b.refname.clone())));
                                }
                                let restore = if kind == "changes" {
                                    "Re-apply"
                                } else {
                                    "Branch from it"
                                };
                                if ui
                                    .add_enabled(!busy, egui::Button::new(restore).small())
                                    .clicked()
                                {
                                    acts.push(Act::Send(Request::RestoreBackup(b.refname.clone())));
                                }
                            });
                        });
                    }
                });
            });
        if !open {
            self.backups = None;
        }
    }

    fn dashboard_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        use crate::theme::{FS_TITLE, SP_L, card, section_label};
        let Some(d) = &self.dashboard else { return };
        let mut open = true;
        egui::Window::new(bold("Repository dashboard", 15.0))
            .open(&mut open)
            .default_size([640.0, 600.0])
            .vscroll(true)
            .show(ctx, |ui| {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (n, label) in [
                            (d.commits, "commits"),
                            (d.branches, "branches"),
                            (d.tracked_files, "tracked files"),
                        ] {
                            ui.label(bold(n.to_string(), FS_TITLE));
                            ui.label(RichText::new(label).color(text_weak()));
                            ui.add_space(SP_L);
                        }
                    });
                });
                section_label(ui, &format!("No activity for 30+ days  ({})", d.stale.len()));
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        if crate::theme::text_button(ui, "Delete merged branches…", true)
                            .on_hover_text("Review and delete branches that are fully merged")
                            .clicked()
                        {
                            acts.push(Act::Send(Request::MergedBranches { remote: false }));
                        }
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("stale")
                        .max_height(200.0)
                        .show(ui, |ui| {
                            if d.stale.is_empty() {
                                crate::theme::empty_state(ui, "No stale branches", "");
                            }
                            for b in &d.stale {
                                ui.horizontal(|ui| {
                                    ui.label(&b.name);
                                    ui.label(
                                        RichText::new(fmt::relative(b.unix)).size(12.0).color(text_weak()),
                                    );
                                    if b.merged {
                                        ui.label(RichText::new("merged").size(11.0).color(green()));
                                    } else {
                                        ui.label(RichText::new("not merged").size(11.0).color(amber()));
                                    }
                                });
                            }
                        });
                });
                section_label(ui, "Commits per week (last 12 weeks)");
                card(ui, |ui| {
                    let max = d
                        .authors
                        .iter()
                        .flat_map(|(_, w)| w.iter().copied())
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    if d.authors.is_empty() {
                        crate::theme::empty_state(ui, "No commits in the last 12 weeks", "");
                    }
                    for (who, weeks) in &d.authors {
                        ui.horizontal(|ui| {
                            ui.add_sized([150.0, 16.0], egui::Label::new(who.as_str()).truncate());
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(12.0 * 11.0, 18.0), egui::Sense::hover());
                            for (i, n) in weeks.iter().enumerate() {
                                let h = 2.0 + 16.0 * (*n as f32 / max as f32);
                                let x = rect.left() + i as f32 * 11.0;
                                let bar = egui::Rect::from_min_max(
                                    egui::pos2(x, rect.bottom() - h),
                                    egui::pos2(x + 9.0, rect.bottom()),
                                );
                                let c = if *n == 0 {
                                    text_weak().gamma_multiply(0.4)
                                } else {
                                    accent()
                                };
                                ui.painter().rect_filled(bar, 2.0, c);
                            }
                            resp.on_hover_text(format!("{} commits in 12 weeks", weeks.iter().sum::<u32>()));
                            ui.label(
                                RichText::new(weeks.iter().sum::<u32>().to_string())
                                    .size(12.0)
                                    .color(text_weak()),
                            );
                        });
                    }
                });
                section_label(ui, "Largest files");
                card(ui, |ui| {
                    for (path, size) in &d.big_files {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(size_text(*size))
                                    .monospace()
                                    .size(12.0)
                                    .color(accent()),
                            );
                            ui.label(path);
                        });
                    }
                });
            });
        if !open {
            self.dashboard = None;
        }
    }

    fn hooks_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if !self.hooks_open {
            return;
        }
        let Some(dir) = self.repo.as_ref().map(|r| r.git_dir.join("hooks")) else {
            self.hooks_open = false;
            return;
        };
        let mut hooks: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.ends_with(".sample"))
                    .collect()
            })
            .unwrap_or_default();
        hooks.sort();
        let mut open = true;
        let mut rename: Option<(String, String)> = None;
        egui::Window::new(bold("Git hooks", 15.0))
            .open(&mut open)
            .default_size([460.0, 320.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Disabling renames a hook to <name>.disabled. Hooks from core.hooksPath are not listed.")
                        .size(12.0)
                        .color(text_weak()),
                );
                ui.add_space(4.0);
                if hooks.is_empty() {
                    ui.label("No hooks installed.");
                }
                for h in &hooks {
                    ui.horizontal(|ui| {
                        if let Some(base) = h.strip_suffix(".disabled") {
                            ui.label(RichText::new(format!("{base}  (disabled)")).color(text_weak()));
                            if ui.small_button("Enable").clicked() {
                                rename = Some((h.clone(), base.to_string()));
                            }
                        } else {
                            ui.label(RichText::new(h).color(green()));
                            if ui.small_button("Disable").clicked() {
                                rename = Some((h.clone(), format!("{h}.disabled")));
                            }
                        }
                    });
                }
                ui.add_space(6.0);
                if ui.small_button("Open hooks folder").clicked() {
                    acts.push(Act::App(AppAct::Explorer(dir.clone())));
                }
            });
        if let Some((from, to)) = rename {
            let _ = std::fs::rename(dir.join(from), dir.join(to));
        }
        if !open {
            self.hooks_open = false;
        }
    }

    fn bisect_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some((text, active)) = self.bisect.clone() else {
            return;
        };
        let busy = self.busy();
        egui::Window::new(bold("Bisect", 15.0))
            .anchor(Align2::RIGHT_TOP, vec2(-16.0, 70.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                let shown: String = text.lines().take(4).collect::<Vec<_>>().join("\n");
                ui.label(RichText::new(shown).monospace().size(12.0));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if active {
                        for (label, mark) in [("Good", "good"), ("Bad", "bad"), ("Skip", "skip")] {
                            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                acts.push(Act::Send(Request::BisectMark(mark.into())));
                            }
                        }
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Finish")).clicked() {
                        acts.push(Act::Send(Request::BisectReset));
                    }
                });
            });
    }

    fn stash_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let side = self.stash_side_by_side;
        let Some((index, view, sel)) = &mut self.stash_view else {
            return;
        };
        let index = *index;
        let mut open = true;
        egui::Window::new(bold(format!("Stash preview — stash@{{{index}}}"), 15.0))
            .open(&mut open)
            .default_size([860.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        acts.push(Act::Send(Request::StashApply(index)));
                    }
                    if ui.button("Pop").clicked() {
                        acts.push(Act::Send(Request::StashPop(index)));
                    }
                });
                ui.separator();
                let extras = ViewExtras {
                    find: None,
                    current_row: None,
                    scroll_to: None,
                    show_ws: false,
                };
                let mut none = Vec::<DiffAction>::new();
                diffview::show(ui, "stash-diff", view, side, sel, None, &extras, &mut none);
            });
        if !open {
            self.stash_view = None;
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_secs(4) {
            self.toast = None;
            return;
        }
        let first: String = text.lines().next().unwrap_or("").chars().take(140).collect();
        let (icon, color) = if text.starts_with('⚠') {
            ("✕", red())
        } else if text.contains("conflict") || text.contains("not in the loaded") {
            ("!", amber())
        } else {
            ("✓", green())
        };
        let first = first.trim_start_matches('⚠').trim().to_string();
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .stroke(egui::Stroke::new(1.5, color))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).strong().color(color));
                            ui.label(first);
                        });
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}
