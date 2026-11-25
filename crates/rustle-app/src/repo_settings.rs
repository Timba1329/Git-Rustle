//! "Repository settings": remotes and the most useful git configuration values.

use crate::tab::{Act, Dialog, InputKind, Tab};
use crate::theme::{amber, bold, danger_button, primary_button, section_label, text_weak};
use crate::worker::Request;
use eframe::egui::{self, Align2, Margin, RichText, vec2};

fn small(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.0)).small()
}

impl Tab {
    #[allow(clippy::too_many_lines)]
    pub fn repo_settings_window(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if !self.repo_settings_open {
            return;
        }
        let busy = self.busy();
        let mut open = true;
        egui::Window::new(bold("Repository settings", 15.0))
            .open(&mut open)
            .collapsible(false)
            .default_size([760.0, 600.0])
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    section_label(ui, "Remotes");
                    match &self.remotes {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Loading…");
                            });
                        }
                        Some(list) if list.is_empty() => {
                            ui.label(RichText::new("No remotes yet. Add one below to push and pull.").color(text_weak()));
                        }
                        Some(list) => {
                            for (i, rm) in list.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(bold(&rm.name, 13.5));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.remote_edit[i])
                                            .desired_width(300.0)
                                            .margin(Margin::symmetric(6, 3)),
                                    );
                                    let changed = self.remote_edit[i].trim() != rm.fetch_url;
                                    if ui
                                        .add_enabled(changed && !busy && !self.remote_edit[i].trim().is_empty(), small("Save URL"))
                                        .clicked()
                                    {
                                        acts.push(Act::Send(Request::SetRemoteUrl {
                                            name: rm.name.clone(),
                                            url: self.remote_edit[i].trim().to_string(),
                                        }));
                                    }
                                    if ui.add(small("Rename…")).clicked() {
                                        acts.push(Act::Dialog(Dialog::Input {
                                            title: "Rename remote".into(),
                                            label: "New name".into(),
                                            value: rm.name.clone(),
                                            kind: InputKind::RenameRemote { old: rm.name.clone() },
                                            check: None,
                                        }));
                                    }
                                    if ui.add_enabled(!busy, small("Prune")).on_hover_text("Forget branches that no longer exist on the remote").clicked() {
                                        acts.push(Act::Send(Request::PruneRemote(rm.name.clone())));
                                    }
                                    if ui.add(small("Remove…")).clicked() {
                                        acts.push(Act::Dialog(Dialog::Confirm {
                                            title: "Remove remote".into(),
                                            text: format!(
                                                "Remove the remote {}? Its remote-tracking branches disappear from this repository; nothing changes on the server.",
                                                rm.name
                                            ),
                                            button: "Remove".into(),
                                            req: Some(Request::RemoveRemote(rm.name.clone())),
                                        }));
                                    }
                                });
                                if rm.push_url != rm.fetch_url {
                                    ui.label(RichText::new(format!("pushes to {}", rm.push_url)).size(12.0).color(text_weak()));
                                }
                            }
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_remote.0)
                                .hint_text("name (origin)")
                                .desired_width(110.0)
                                .margin(Margin::symmetric(6, 3)),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_remote.1)
                                .hint_text("https://github.com/owner/repo.git")
                                .desired_width(300.0)
                                .margin(Margin::symmetric(6, 3)),
                        );
                        let ok = !self.new_remote.0.trim().is_empty() && !self.new_remote.1.trim().is_empty() && !busy;
                        if primary_button(ui, "Add remote", ok).clicked() {
                            acts.push(Act::Send(Request::AddRemote {
                                name: self.new_remote.0.trim().to_string(),
                                url: self.new_remote.1.trim().to_string(),
                            }));
                            self.new_remote = (String::new(), String::new());
                        }
                    });

                    ui.add_space(14.0);
                    section_label(ui, "Git configuration");
                    ui.label(
                        RichText::new("“Repository” changes only this repository, “All repositories” changes your user settings.")
                            .size(12.0)
                            .color(text_weak()),
                    );
                    ui.add_space(4.0);
                    match &self.config {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Loading…");
                            });
                        }
                        Some(rows) => {
                            egui::Grid::new("config-grid").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                                for (i, row) in rows.iter().enumerate() {
                                    ui.label(RichText::new(row.key).monospace().size(12.5));
                                    ui.add_sized(
                                        [210.0, 24.0],
                                        egui::TextEdit::singleline(&mut self.config_edit[i])
                                            .margin(Margin::symmetric(6, 3)),
                                    );
                                    let scope = row.value.scope.as_deref().map_or("not set".to_string(), |s| format!("from {s}"));
                                    ui.label(RichText::new(scope).size(12.0).color(text_weak()));
                                    ui.horizontal(|ui| {
                                        let value = self.config_edit[i].trim().to_string();
                                        let can = !busy && !value.is_empty();
                                        if ui.add_enabled(can, small("Repository")).clicked() {
                                            acts.push(Act::Send(Request::SetConfig { key: row.key.into(), value: value.clone(), global: false }));
                                            acts.push(Act::Send(Request::LoadConfig));
                                        }
                                        if ui.add_enabled(can, small("All repositories")).clicked() {
                                            acts.push(Act::Send(Request::SetConfig { key: row.key.into(), value, global: true }));
                                            acts.push(Act::Send(Request::LoadConfig));
                                        }
                                        if ui
                                            .add_enabled(!busy && row.value.scope.as_deref() == Some("local"), small("Unset"))
                                            .on_hover_text("Remove the repository-level value")
                                            .clicked()
                                        {
                                            acts.push(Act::Send(Request::UnsetConfig { key: row.key.into(), global: false }));
                                            acts.push(Act::Send(Request::LoadConfig));
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Booleans are written as true or false; pull.rebase also accepts merges or interactive.")
                                    .size(12.0)
                                    .color(amber()),
                            );
                        }
                    }
                    let _ = danger_button;
                });
            });
        if !open {
            self.repo_settings_open = false;
        }
    }
}
