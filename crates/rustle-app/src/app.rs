//! Application shell: tabs, settings, about, clone dialog, shortcuts and window state.

use crate::recent::Recent;
use crate::settings::{Settings, ThemeMode};
use crate::tab::{Act, AppAct, Tab};
use crate::theme::{
    self, accent, amber, bg_bar, bg_btn, bg_btn_hover, bg_side, bold, icon_button, primary_button, red,
    section_label, text, text_weak,
};
use crate::update;
use crate::worker::Request;
use eframe::egui::{
    self, Align, Align2, FontId, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense, Ui, pos2, vec2,
};
use rustle_core::PullMode;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};

enum UpdateState {
    None,
    Available(update::Release),
    Installing(update::Release, f32),
    Failed(update::Release, String),
}

enum UpdateAct {
    Install(update::Release),
    Open(String),
    Later,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Opens a GitHub address in the default browser.
fn open_url(url: &str) {
    if url.starts_with("https://github.com/") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        spawn_hidden(c);
    }
}

struct CloneState {
    url: String,
    dest: String,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    settings: Settings,
    recent: Recent,
    show_settings: bool,
    show_about: bool,
    clone: Option<CloneState>,
    ident_name: String,
    ident_email: String,
    ident_wait: bool,
    update_tx: Sender<update::Msg>,
    update_rx: Receiver<update::Msg>,
    update: UpdateState,
    show_update: bool,
    checking_updates: bool,
    update_note: Option<String>,
    exe_path: Option<PathBuf>,
    was_focused: bool,
    win_inner: Option<egui::Vec2>,
    win_pos: Option<egui::Pos2>,
    win_max: bool,
}

fn spawn_hidden(mut cmd: Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.spawn();
}

fn open_explorer(p: &Path) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer").arg(p).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(p).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(p).spawn();
    }
}

fn open_terminal(p: &Path) {
    #[cfg(windows)]
    {
        // Windows Terminal when available, otherwise a plain console window.
        if Command::new("wt").arg("-d").arg(p).spawn().is_err() {
            let _ = Command::new("cmd")
                .args(["/C", "start", "cmd.exe"])
                .current_dir(p)
                .spawn();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("x-terminal-emulator").current_dir(p).spawn();
    }
}

fn open_editor(editor: &str, p: &Path) {
    let editor = editor.trim();
    if editor.is_empty() {
        open_explorer(p);
        return;
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", editor]).arg(p);
        spawn_hidden(c);
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new(editor);
        c.arg(p);
        spawn_hidden(c);
    }
}

fn repo_name_from_url(url: &str) -> String {
    let t = url.trim().trim_end_matches(['/', '\\']);
    let t = t.strip_suffix(".git").unwrap_or(t);
    t.rsplit(['/', ':', '\\']).next().unwrap_or("repo").to_string()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings = Settings::load();
        theme::apply(&cc.egui_ctx, settings.theme, settings.font_scale);
        let recent = Recent::load();

        let mut first = Tab::new(&cc.egui_ctx, &settings);
        // `rustle <path>` opens that repository; otherwise reopen the most recent one.
        let start = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .or_else(|| recent.paths().first().cloned());
        if let Some(p) = start {
            first.open_path(p);
        }

        let (update_tx, update_rx) = channel();

        let mut app = Self {
            tabs: vec![first],
            active: 0,
            settings,
            recent,
            show_settings: false,
            show_about: false,
            clone: None,
            ident_name: String::new(),
            ident_email: String::new(),
            ident_wait: false,
            update_tx,
            update_rx,
            update: UpdateState::None,
            show_update: false,
            checking_updates: false,
            update_note: None,
            exe_path: std::env::current_exe().ok(),
            was_focused: true,
            win_inner: None,
            win_pos: None,
            win_max: false,
        };
        app.startup_update_check(&cc.egui_ctx);
        app
    }

    // ------------------------------------------------------------------ updates

    fn update_repo(&self) -> String {
        update::effective_repo(&self.settings.update_repo)
    }

    /// Shows a previously found update right away and checks again when a day has passed.
    fn startup_update_check(&mut self, ctx: &egui::Context) {
        if !self.settings.check_updates {
            return;
        }
        let repo = self.update_repo();
        let pending = self.settings.pending_update.clone();
        if !pending.is_empty() {
            if update::is_newer(&pending, CURRENT_VERSION) {
                self.update = UpdateState::Available(update::Release::from_tag(&repo, &pending));
            } else {
                // We are on (or past) that version now.
                self.settings.pending_update.clear();
            }
        }
        if now_secs() - self.settings.last_update_check >= 24 * 3600 {
            self.spawn_check(ctx);
        }
    }

    fn spawn_check(&mut self, ctx: &egui::Context) {
        let repo = self.update_repo();
        if repo.is_empty() || self.checking_updates {
            return;
        }
        self.checking_updates = true;
        self.update_note = None;
        let (tx, ctx) = (self.update_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(update::Msg::Checked(update::fetch_latest_tag(&repo)));
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context, rel: update::Release) {
        let Some(exe) = self.exe_path.clone() else {
            self.update = UpdateState::Failed(rel, "cannot find the running application".into());
            return;
        };
        let prefix = update::allowed_prefix(&self.update_repo());
        self.update = UpdateState::Installing(rel.clone(), 0.0);
        let (tx, ctx) = (self.update_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut last = 0.0f32;
            let result = update::install(&rel, &exe, &prefix, &mut |p| {
                if p - last >= 0.01 {
                    last = p;
                    let _ = tx.send(update::Msg::Progress(p));
                    ctx.request_repaint();
                }
            });
            let _ = tx.send(match result {
                Ok(()) => update::Msg::Installed,
                Err(e) => update::Msg::Failed(e),
            });
            ctx.request_repaint();
        });
    }

    fn restart(&mut self, ctx: &egui::Context) {
        if let Some(exe) = &self.exe_path {
            let mut c = Command::new(exe);
            c.args(std::env::args_os().skip(1));
            if c.spawn().is_ok() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        self.update = UpdateState::None;
        self.update_note = Some("Updated. Please restart Rustle.".into());
    }

    fn poll_updates(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.update_rx.try_recv() {
            match m {
                update::Msg::Checked(r) => {
                    self.checking_updates = false;
                    match r {
                        Ok(tag) => {
                            self.settings.last_update_check = now_secs();
                            if update::is_newer(&tag, CURRENT_VERSION) {
                                self.settings.pending_update = tag.clone();
                                if !matches!(self.update, UpdateState::Installing(..)) {
                                    let rel = update::Release::from_tag(&self.update_repo(), &tag);
                                    self.update = UpdateState::Available(rel);
                                }
                                self.update_note = Some(format!("Version {tag} is available."));
                            } else {
                                self.settings.pending_update.clear();
                                if matches!(self.update, UpdateState::Available(..)) {
                                    self.update = UpdateState::None;
                                }
                                self.update_note = Some("You are up to date.".into());
                            }
                            self.settings.save();
                        }
                        Err(e) => self.update_note = Some(format!("Could not check for updates: {e}")),
                    }
                }
                update::Msg::Progress(p) => {
                    if let UpdateState::Installing(_, cur) = &mut self.update {
                        *cur = p;
                    }
                }
                update::Msg::Installed => {
                    self.settings.pending_update.clear();
                    self.settings.save();
                    self.restart(ctx);
                }
                update::Msg::Failed(e) => {
                    if let UpdateState::Installing(r, _) =
                        std::mem::replace(&mut self.update, UpdateState::None)
                    {
                        self.update = UpdateState::Failed(r, e);
                    }
                }
            }
        }
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        if !self.show_update {
            return;
        }
        let mut open = true;
        let mut act: Option<UpdateAct> = None;
        egui::Window::new(bold("Update Rustle", 15.0))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                match &self.update {
                    UpdateState::Available(r) => {
                        ui.label(format!("Rustle {} is available. You have {CURRENT_VERSION}.", r.tag));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Rustle downloads the new version, checks its checksum, replaces itself and restarts.",
                            )
                            .size(12.5)
                            .color(text_weak()),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if primary_button(ui, "Update now", true).clicked() {
                                act = Some(UpdateAct::Install(r.clone()));
                            }
                            if ui.button("Release notes").clicked() {
                                act = Some(UpdateAct::Open(r.page_url.clone()));
                            }
                            if ui.button("Later").clicked() {
                                act = Some(UpdateAct::Later);
                            }
                        });
                    }
                    UpdateState::Installing(r, p) => {
                        ui.label(format!("Downloading Rustle {}…", r.tag));
                        ui.add_space(6.0);
                        ui.add(egui::ProgressBar::new(*p).show_percentage());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Rustle restarts when the update is installed.").size(12.5).color(text_weak()));
                    }
                    UpdateState::Failed(r, e) => {
                        ui.colored_label(red(), format!("The update failed: {e}"));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Try again").clicked() {
                                act = Some(UpdateAct::Install(r.clone()));
                            }
                            if ui.button("Open releases page").clicked() {
                                act = Some(UpdateAct::Open(r.page_url.clone()));
                            }
                            if ui.button("Close").clicked() {
                                act = Some(UpdateAct::Later);
                            }
                        });
                    }
                    UpdateState::None => {
                        ui.label("Rustle is up to date.");
                    }
                }
            });
        match act {
            Some(UpdateAct::Install(r)) => self.start_install(ctx, r),
            Some(UpdateAct::Open(url)) => open_url(&url),
            Some(UpdateAct::Later) => self.show_update = false,
            None => {}
        }
        if !open {
            self.show_update = false;
        }
    }

    fn new_tab(&mut self, ctx: &egui::Context) {
        self.tabs.push(Tab::new(ctx, &self.settings));
        self.active = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, ctx: &egui::Context, i: usize) {
        if self.tabs.len() == 1 {
            self.tabs[0] = Tab::new(ctx, &self.settings);
            return;
        }
        self.tabs.remove(i);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if i < self.active {
            self.active -= 1;
        }
    }

    /// The tab a new repository should open in: the active one if it is empty, else a new one.
    fn target_tab(&mut self, ctx: &egui::Context) -> &mut Tab {
        let t = &self.tabs[self.active];
        if t.repo.is_some() || t.busy() {
            self.new_tab(ctx);
        }
        &mut self.tabs[self.active]
    }

    fn open_path(&mut self, ctx: &egui::Context, p: PathBuf) {
        let existing = self.tabs.iter().position(|t| {
            t.repo
                .as_ref()
                .is_some_and(|r| crate::recent::same_path(&r.path, &p))
        });
        if let Some(i) = existing {
            self.active = i;
            return;
        }
        self.target_tab(ctx).open_path(p);
    }

    fn handle(&mut self, ctx: &egui::Context, acts: Vec<AppAct>) {
        for a in acts {
            match a {
                AppAct::OpenPath(p) => self.open_path(ctx, p),
                AppAct::NewTab => self.new_tab(ctx),
                AppAct::Settings => self.open_settings(),
                AppAct::About => self.show_about = true,
                AppAct::Clone => {
                    self.clone = Some(CloneState {
                        url: String::new(),
                        dest: self
                            .recent
                            .paths()
                            .first()
                            .and_then(|p| p.parent())
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    });
                }
                AppAct::PickAndInit => {
                    if let Some(dir) = rfd::FileDialog::new()
                        .set_title("Folder for the new repository")
                        .pick_folder()
                    {
                        let t = self.target_tab(ctx);
                        t.title = dir
                            .file_name()
                            .map_or("New repository".into(), |n| n.to_string_lossy().into_owned());
                        t.send(Request::InitRepo(dir));
                    }
                }
                AppAct::Explorer(p) => open_explorer(&p),
                AppAct::Terminal(p) => open_terminal(&p),
                AppAct::Editor(p) => open_editor(&self.settings.editor, &p),
                AppAct::RepoOpened(p) => self.recent.add(&p),
            }
        }
    }

    fn open_settings(&mut self) {
        self.show_settings = true;
        if self.tabs[self.active].repo.is_some() {
            self.ident_wait = true;
            self.tabs[self.active].identity = None;
            self.tabs[self.active].send(Request::LoadIdentity);
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, self.settings.theme, self.settings.font_scale);
        for t in &mut self.tabs {
            t.reload_diffs(&self.settings);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, acts: &mut Vec<AppAct>) {
        let (open, new_tab, close_tab, settings, find, refresh, next_tab, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::T),
                i.consume_key(Modifiers::COMMAND, Key::W),
                i.consume_key(Modifiers::COMMAND, Key::Comma),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.consume_key(Modifiers::CTRL, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if open && let Some(dir) = rfd::FileDialog::new().pick_folder() {
            acts.push(AppAct::OpenPath(dir));
        }
        if new_tab {
            acts.push(AppAct::NewTab);
        }
        if close_tab {
            let i = self.active;
            self.close_tab(ctx, i);
        }
        if settings {
            acts.push(AppAct::Settings);
        }
        if next_tab && self.tabs.len() > 1 {
            self.active = (self.active + 1) % self.tabs.len();
        }
        let tab = &mut self.tabs[self.active];
        if find && tab.repo.is_some() {
            tab.focus_search = true;
        }
        if refresh && tab.repo.is_some() && !tab.busy() {
            tab.send(Request::Refresh);
        }
        if esc {
            if self.show_settings {
                self.show_settings = false;
            } else if self.show_about {
                self.show_about = false;
            } else if self.clone.is_some() {
                self.clone = None;
            } else {
                tab.on_escape();
            }
        }
        // Up/Down walk through the commit list when nothing else has focus.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing && !tab.commit_open {
            let (up, down) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                )
            });
            let n = tab.repo.as_ref().map_or(0, |r| r.commits.len());
            if n > 0 && (up || down) {
                let idx = match (tab.selected, up) {
                    (None, _) => 0,
                    (Some(i), true) => i.saturating_sub(1),
                    (Some(i), false) => (i + 1).min(n - 1),
                };
                if tab.selected != Some(idx) {
                    acts.extend(tab.apply(vec![Act::SelectCommit { idx, ctrl: false }]));
                }
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut Ui, acts: &mut Vec<AppAct>) {
        let frame = Frame::new().fill(bg_side()).inner_margin(Margin {
            left: 8,
            right: 8,
            top: 4,
            bottom: 0,
        });
        let mut close: Option<usize> = None;
        egui::Panel::top("tabs")
            .frame(frame)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for i in 0..self.tabs.len() {
                        let t = &self.tabs[i];
                        let selected = i == self.active;
                        let label = if t.busy() {
                            format!("{} …", t.title)
                        } else {
                            t.title.clone()
                        };
                        let color = if selected { text() } else { text_weak() };
                        let galley = ui
                            .painter()
                            .layout_no_wrap(label, FontId::proportional(13.0), color);
                        let w = (galley.size().x + 16.0 + 26.0).max(100.0);
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
                        let hovered = resp.hovered();
                        let radius = egui::CornerRadius {
                            nw: 5,
                            ne: 5,
                            sw: 0,
                            se: 0,
                        };
                        if selected {
                            ui.painter().rect_filled(rect, radius, bg_bar());
                            ui.painter().rect_filled(
                                Rect::from_min_size(rect.min, vec2(rect.width(), 2.0)),
                                radius,
                                accent(),
                            );
                        } else if hovered {
                            ui.painter().rect_filled(rect, radius, bg_btn());
                        }
                        ui.painter().galley(
                            pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0 + 1.0),
                            galley,
                            color,
                        );
                        // Close button, visible on the active tab and on hover.
                        let cr = Rect::from_center_size(
                            pos2(rect.right() - 14.0, rect.center().y + 1.0),
                            vec2(18.0, 18.0),
                        );
                        let cresp = ui.interact(cr, ui.scope_id().with(("tabclose", i)), Sense::click());
                        if selected || hovered {
                            if cresp.hovered() {
                                ui.painter().rect_filled(cr, 4.0, bg_btn_hover());
                            }
                            ui.painter().text(
                                cr.center(),
                                Align2::CENTER_CENTER,
                                "✕",
                                FontId::proportional(11.0),
                                if cresp.hovered() { text() } else { text_weak() },
                            );
                        }
                        if cresp.clicked() || resp.middle_clicked() {
                            close = Some(i);
                        } else if resp.clicked() {
                            self.active = i;
                        }
                    }
                    if icon_button(ui, "+", "New tab (Ctrl+T)", true).clicked() {
                        acts.push(AppAct::NewTab);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &self.update {
                        UpdateState::Available(r) => {
                            if ui
                                .button(RichText::new(format!("Update available: {}", r.tag)).color(amber()))
                                .on_hover_text("Click to update Rustle")
                                .clicked()
                            {
                                self.show_update = true;
                            }
                        }
                        UpdateState::Installing(_, p) => {
                            ui.label(RichText::new(format!("Updating… {:.0}%", p * 100.0)).color(accent()));
                        }
                        UpdateState::Failed(..) => {
                            if ui.button(RichText::new("Update failed").color(red())).clicked() {
                                self.show_update = true;
                            }
                        }
                        UpdateState::None => {}
                    });
                });
            });
        if let Some(i) = close {
            let ctx = ui.ctx().clone();
            self.close_tab(&ctx, i);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn windows(&mut self, ctx: &egui::Context, acts: &mut Vec<AppAct>) {
        // ---------------------------------------------------------------- settings
        if self.show_settings {
            let mut open = true;
            let mut theme_changed = false;
            let mut git_changed = false;
            let mut pull_changed = false;
            let mut diff_changed = false;
            let has_repo = self.tabs[self.active].repo.is_some();
            let mut save_identity = false;
            let mut check_now = false;
            egui::Window::new(bold("Settings", 15.0))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_min_width(460.0);
                    let s = &mut self.settings;
                    section_label(ui, "Appearance");
                    ui.horizontal(|ui| {
                        theme_changed |= ui.radio_value(&mut s.theme, ThemeMode::Dark, "Dark").changed();
                        theme_changed |= ui.radio_value(&mut s.theme, ThemeMode::Light, "Light").changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Text size");
                        theme_changed |= ui
                            .add(egui::Slider::new(&mut s.font_scale, 0.8..=1.5).fixed_decimals(2))
                            .changed();
                    });

                    section_label(ui, "Git");
                    ui.horizontal(|ui| {
                        ui.label("git executable");
                        git_changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut s.git_path)
                                    .hint_text("Detect automatically")
                                    .desired_width(260.0),
                            )
                            .changed();
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("git", &["exe"]).pick_file()
                        {
                            s.git_path = p.display().to_string();
                            git_changed = true;
                        }
                    });
                    ui.label(RichText::new("Pull strategy").color(text_weak()));
                    ui.horizontal(|ui| {
                        pull_changed |= ui
                            .radio_value(&mut s.pull_mode, PullMode::FfOnly, "Fast-forward only")
                            .changed();
                        pull_changed |= ui
                            .radio_value(&mut s.pull_mode, PullMode::Merge, "Merge")
                            .changed();
                        pull_changed |= ui
                            .radio_value(&mut s.pull_mode, PullMode::Rebase, "Rebase")
                            .changed();
                    });

                    section_label(ui, "Diff");
                    ui.horizontal(|ui| {
                        diff_changed |= ui.checkbox(&mut s.side_by_side, "Side by side").changed();
                        diff_changed |= ui
                            .checkbox(&mut s.ignore_whitespace, "Ignore whitespace")
                            .changed();
                        ui.label("Context lines");
                        diff_changed |= ui
                            .add(egui::DragValue::new(&mut s.context).range(0..=30))
                            .changed();
                    });

                    section_label(ui, "Remote");
                    ui.horizontal(|ui| {
                        ui.label("Fetch in the background every");
                        if ui
                            .add(egui::DragValue::new(&mut s.auto_fetch_minutes).range(0..=240).suffix(" min"))
                            .changed()
                        {
                            s.save();
                        }
                        ui.label(RichText::new("(0 = off)").size(12.0).color(text_weak()));
                    });
                    ui.label(
                        RichText::new("Background fetches never ask for a password; if one is needed they fail quietly.")
                            .size(12.0)
                            .color(text_weak()),
                    );

                    section_label(ui, "Editor");
                    ui.horizontal(|ui| {
                        ui.label("Editor command");
                        ui.add(
                            egui::TextEdit::singleline(&mut s.editor)
                                .hint_text("code")
                                .desired_width(160.0),
                        );
                    });

                    section_label(ui, "Updates");
                    if ui
                        .checkbox(&mut s.check_updates, "Check for updates automatically (at most once a day)")
                        .changed()
                    {
                        s.save();
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.checking_updates, egui::Button::new("Check now")).clicked() {
                            check_now = true;
                        }
                        if self.checking_updates {
                            ui.spinner();
                        }
                        if let Some(n) = &self.update_note {
                            ui.label(RichText::new(n).size(12.5).color(text_weak()));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Update source");
                        ui.add(
                            egui::TextEdit::singleline(&mut s.update_repo)
                                .hint_text(update::default_repo())
                                .desired_width(220.0),
                        );
                    });
                    ui.label(
                        RichText::new(format!("Rustle {CURRENT_VERSION}. Leave the source empty for the official releases."))
                            .size(12.0)
                            .color(text_weak()),
                    );

                    section_label(ui, "Git identity (global)");
                    if has_repo {
                        ui.horizontal(|ui| {
                            ui.label("Name");
                            ui.add(egui::TextEdit::singleline(&mut self.ident_name).desired_width(180.0));
                            ui.label("E-mail");
                            ui.add(egui::TextEdit::singleline(&mut self.ident_email).desired_width(200.0));
                        });
                        if ui.button("Save identity").clicked() {
                            save_identity = true;
                        }
                    } else {
                        ui.label(
                            RichText::new("Open a repository to edit your identity.").color(text_weak()),
                        );
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open data folder").clicked()
                            && let Some(d) = crate::settings::data_dir()
                        {
                            open_explorer(&d);
                        }
                    });
                });
            if check_now {
                self.spawn_check(ctx);
            }
            if save_identity {
                let (name, email) = (
                    self.ident_name.trim().to_string(),
                    self.ident_email.trim().to_string(),
                );
                if !name.is_empty() && !email.is_empty() {
                    self.tabs[self.active].send(Request::SetIdentity { name, email });
                }
            }
            if theme_changed || diff_changed {
                self.apply_theme(ctx);
            }
            if git_changed {
                let p = self.settings.git_path.clone();
                for t in &mut self.tabs {
                    t.send(Request::SetGitPath(p.clone()));
                }
            }
            if pull_changed {
                let m = self.settings.pull_mode;
                for t in &mut self.tabs {
                    t.send(Request::SetPullMode(m));
                }
            }
            if theme_changed || git_changed || pull_changed || diff_changed {
                self.settings.save();
            }
            if !open {
                self.settings.save();
                self.show_settings = false;
            }
        }
        // Fill the identity fields when the answer arrives.
        if self.ident_wait
            && let Some((n, e)) = self.tabs[self.active].identity.take()
        {
            self.ident_name = n.unwrap_or_default();
            self.ident_email = e.unwrap_or_default();
            self.ident_wait = false;
        }

        // ---------------------------------------------------------------- about
        if self.show_about {
            let mut open = true;
            egui::Window::new(bold("About Rustle", 15.0))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_min_width(340.0);
                    ui.label(bold("Rustle", 24.0));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(text_weak()),
                    );
                    ui.add_space(8.0);
                    ui.label("A fast, native Git client written in Rust.");
                    ui.label("Built on egui and the git command line.");
                    ui.add_space(8.0);
                    ui.label(RichText::new("Licensed under the MIT license.").color(text_weak()));
                    if let Some(d) = crate::settings::data_dir() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("Settings and logs: {}", d.display()))
                                .size(12.0)
                                .color(text_weak()),
                        );
                    }
                });
            if !open {
                self.show_about = false;
            }
        }

        // ---------------------------------------------------------------- clone
        let mut start_clone: Option<(String, PathBuf)> = None;
        let mut cancel_clone = false;
        if let Some(c) = &mut self.clone {
            egui::Window::new(bold("Clone repository", 15.0))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.set_min_width(480.0);
                    ui.label("Repository URL");
                    ui.add(
                        egui::TextEdit::singleline(&mut c.url)
                            .hint_text("https://github.com/owner/repo.git")
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(8, 6)),
                    );
                    ui.add_space(4.0);
                    ui.label("Clone into folder");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut c.dest)
                                .desired_width(360.0)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_folder()
                        {
                            c.dest = p.display().to_string();
                        }
                    });
                    let name = repo_name_from_url(&c.url);
                    if !c.url.trim().is_empty() && !c.dest.trim().is_empty() {
                        ui.label(
                            RichText::new(format!("→ {}", Path::new(c.dest.trim()).join(&name).display()))
                                .size(12.0)
                                .color(text_weak()),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = !c.url.trim().is_empty() && !c.dest.trim().is_empty();
                        if primary_button(ui, "Clone", ok).clicked() {
                            start_clone =
                                Some((c.url.trim().to_string(), Path::new(c.dest.trim()).join(&name)));
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_clone = true;
                        }
                    });
                });
        }
        if cancel_clone {
            self.clone = None;
        }
        if let Some((url, dest)) = start_clone {
            self.clone = None;
            let t = self.target_tab(ctx);
            t.title = repo_name_from_url(&url);
            t.send(Request::Clone { url, dest });
        }
        let _ = acts;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut app_acts: Vec<AppAct> = Vec::new();

        // Track the window geometry for the next start.
        ctx.input(|i| {
            let v = i.viewport();
            if let Some(r) = v.inner_rect
                && !v.maximized.unwrap_or(false)
            {
                self.win_inner = Some(r.size());
            }
            if let Some(r) = v.outer_rect
                && !v.maximized.unwrap_or(false)
            {
                self.win_pos = Some(r.min);
            }
            self.win_max = v.maximized.unwrap_or(false);
        });

        self.poll_updates(&ctx);

        for t in &mut self.tabs {
            app_acts.extend(t.poll(&mut self.settings));
        }
        self.shortcuts(&ctx, &mut app_acts);

        // Drag a folder onto the window to open it.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for p in dropped {
            let dir = if p.is_dir() {
                Some(p)
            } else {
                p.parent().map(Path::to_path_buf)
            };
            if let Some(d) = dir {
                app_acts.push(AppAct::OpenPath(d));
            }
        }

        // Window active again: refresh the status (other tools may have changed files).
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused {
            let t = &mut self.tabs[self.active];
            if t.repo.is_some() && !t.busy() {
                t.send(Request::RefreshStatus);
            }
        }
        self.was_focused = focused;

        // Background fetch of the active tab: only while the window is in use, never while another
        // operation runs, and only when the interval has passed since the last fetch.
        let minutes = u64::from(self.settings.auto_fetch_minutes);
        if minutes > 0 {
            let interval = std::time::Duration::from_secs(minutes * 60);
            let t = &mut self.tabs[self.active];
            if focused && t.repo.is_some() && !t.busy() && t.last_fetch.elapsed() >= interval {
                t.send(Request::AutoFetch);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(20));
        }

        self.tab_bar(ui, &mut app_acts);

        let mut acts: Vec<Act> = Vec::new();
        let recent: Vec<PathBuf> = self.recent.paths().to_vec();
        {
            let tab = &mut self.tabs[self.active];
            tab.header(ui, &mut acts, &recent);
            tab.sidebar(ui, &mut acts);
            tab.detail_panel(ui, &mut acts, &mut self.settings);
            tab.graph_panel(ui, &mut acts);
            if tab.commit_open {
                tab.commit_popup(&ctx, &mut acts, &mut self.settings);
            }
            tab.dialogs(&ctx, &mut acts);
            tab.info_windows(&ctx, &mut acts);

            let reload = acts.iter().any(|a| matches!(a, Act::ReloadView));
            app_acts.extend(tab.apply(acts));
            if reload {
                tab.reload_diffs(&self.settings);
            }
        }
        self.windows(&ctx, &mut app_acts);
        self.update_window(&ctx);
        self.handle(&ctx, app_acts);
    }

    fn on_exit(&mut self) {
        let s = &mut self.settings;
        if let Some(sz) = self.win_inner {
            s.win_w = sz.x;
            s.win_h = sz.y;
        }
        if let Some(p) = self.win_pos {
            s.win_x = Some(p.x);
            s.win_y = Some(p.y);
        }
        s.maximized = self.win_max;
        s.save();
    }
}

#[cfg(test)]
mod tests {
    use super::repo_name_from_url;

    #[test]
    fn names_from_urls() {
        assert_eq!(repo_name_from_url("https://github.com/owner/repo.git"), "repo");
        assert_eq!(repo_name_from_url("git@github.com:owner/other.git"), "other");
        assert_eq!(repo_name_from_url("C:\\code\\local\\"), "local");
        assert_eq!(repo_name_from_url("https://host/x/y/"), "y");
    }
}
