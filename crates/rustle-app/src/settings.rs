//! User settings, stored as `key=value` lines in `%APPDATA%\Rustle\settings.ini`.

use rustle_core::PullMode;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone)]
pub struct Settings {
    /// Explicit path to `git.exe`; empty = auto-detect.
    pub git_path: String,
    pub theme: ThemeMode,
    /// Follow the Windows light/dark setting.
    pub follow_system: bool,
    /// Index of the accent color preset (0 = default).
    pub accent: u8,
    pub high_contrast: bool,
    /// Version whose release notes were shown last.
    pub seen_version: String,
    /// Ticket tracker URL with `{id}` for the ticket key (empty = no links).
    pub ticket_url: String,
    /// Repositories open at exit, and the active tab.
    pub session: Vec<String>,
    pub session_active: usize,
    /// UI scale factor (1.0 = default).
    pub font_scale: f32,
    pub pull_mode: PullMode,
    pub ignore_whitespace: bool,
    pub context: u32,
    /// Fetch in the background every N minutes (0 = never).
    pub auto_fetch_minutes: u32,
    pub side_by_side: bool,
    /// Show spaces as dots in diffs.
    pub show_whitespace: bool,
    /// Reload automatically when files change on disk.
    pub watch_files: bool,
    /// Editor command used by "Open in editor", e.g. `code`.
    pub editor: String,
    /// Check GitHub for new releases (at most once a day).
    pub check_updates: bool,
    /// Optional `owner/repo` override; empty = the repository this build comes from.
    pub update_repo: String,
    /// Unix time of the last update check.
    pub last_update_check: i64,
    /// Release tag found by the last check that is newer than this build.
    pub pending_update: String,
    pub win_w: f32,
    pub win_h: f32,
    pub win_x: Option<f32>,
    pub win_y: Option<f32>,
    pub maximized: bool,
    /// Recently used commit messages, newest first.
    pub messages: Vec<String>,
    file: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            git_path: String::new(),
            theme: ThemeMode::Dark,
            follow_system: false,
            accent: 0,
            high_contrast: false,
            seen_version: String::new(),
            ticket_url: String::new(),
            session: Vec::new(),
            session_active: 0,
            font_scale: 1.0,
            pull_mode: PullMode::FfOnly,
            ignore_whitespace: false,
            context: 3,
            auto_fetch_minutes: 5,
            side_by_side: false,
            show_whitespace: false,
            watch_files: true,
            editor: "code".into(),
            check_updates: true,
            update_repo: String::new(),
            last_update_check: 0,
            pending_update: String::new(),
            win_w: 1180.0,
            win_h: 760.0,
            win_x: None,
            win_y: None,
            maximized: false,
            messages: Vec::new(),
            file: None,
        }
    }
}

fn default_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(base).join("Rustle").join("settings.ini"))
}

/// Directory for settings, logs and the recent list.
pub fn data_dir() -> Option<PathBuf> {
    default_file().and_then(|f| f.parent().map(PathBuf::from))
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Settings {
    pub fn load() -> Self {
        Self::load_from(default_file())
    }

    pub(crate) fn load_from(file: Option<PathBuf>) -> Self {
        let mut s = Self {
            file: file.clone(),
            ..Self::default()
        };
        let Some(text) = file.and_then(|f| std::fs::read_to_string(f).ok()) else {
            return s;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "git_path" => s.git_path = unesc(v),
                "theme" => {
                    s.theme = if v == "light" {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    }
                }
                "accent" => s.accent = v.parse().unwrap_or(0),
                "high_contrast" => s.high_contrast = v == "1",
                "seen_version" => s.seen_version = unesc(v),
                "ticket_url" => s.ticket_url = unesc(v),
                "session" => s.session.push(unesc(v)),
                "session_active" => s.session_active = v.parse().unwrap_or(0),
                "follow_system" => s.follow_system = v == "1",
                "font_scale" => s.font_scale = v.parse().unwrap_or(1.0f32).clamp(0.7, 1.8),
                "pull_mode" => {
                    s.pull_mode = match v {
                        "merge" => PullMode::Merge,
                        "rebase" => PullMode::Rebase,
                        _ => PullMode::FfOnly,
                    }
                }
                "ignore_whitespace" => s.ignore_whitespace = v == "true",
                "context" => s.context = v.parse().unwrap_or(3).min(50),
                "auto_fetch_minutes" => s.auto_fetch_minutes = v.parse().unwrap_or(5).min(240),
                "side_by_side" => s.side_by_side = v == "true",
                "show_whitespace" => s.show_whitespace = v == "true",
                "watch_files" => s.watch_files = v != "false",
                "editor" => s.editor = unesc(v),
                "update_repo" => s.update_repo = unesc(v),
                "check_updates" => s.check_updates = v != "false",
                "last_update_check" => s.last_update_check = v.parse().unwrap_or(0),
                "pending_update" => s.pending_update = unesc(v),
                "win_w" => s.win_w = v.parse().unwrap_or(s.win_w).max(600.0),
                "win_h" => s.win_h = v.parse().unwrap_or(s.win_h).max(400.0),
                "win_x" => s.win_x = v.parse().ok(),
                "win_y" => s.win_y = v.parse().ok(),
                "maximized" => s.maximized = v == "true",
                "msg" => s.messages.push(unesc(v)),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(f) = &self.file else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut t = String::new();
        let mut put = |k: &str, v: String| t.push_str(&format!("{k}={v}\n"));
        put("git_path", esc(&self.git_path));
        put(
            "theme",
            if self.theme == ThemeMode::Light {
                "light"
            } else {
                "dark"
            }
            .into(),
        );
        put("accent", self.accent.to_string());
        put("high_contrast", if self.high_contrast { "1" } else { "0" }.into());
        put("seen_version", esc(&self.seen_version));
        put("ticket_url", esc(&self.ticket_url));
        for p in &self.session {
            put("session", esc(p));
        }
        put("session_active", self.session_active.to_string());
        put("follow_system", if self.follow_system { "1" } else { "0" }.into());
        put("font_scale", self.font_scale.to_string());
        put(
            "pull_mode",
            match self.pull_mode {
                PullMode::FfOnly => "ff-only",
                PullMode::Merge => "merge",
                PullMode::Rebase => "rebase",
            }
            .into(),
        );
        put("ignore_whitespace", self.ignore_whitespace.to_string());
        put("context", self.context.to_string());
        put("auto_fetch_minutes", self.auto_fetch_minutes.to_string());
        put("side_by_side", self.side_by_side.to_string());
        put("show_whitespace", self.show_whitespace.to_string());
        put("watch_files", self.watch_files.to_string());
        put("editor", esc(&self.editor));
        put("update_repo", esc(&self.update_repo));
        put("check_updates", self.check_updates.to_string());
        put("last_update_check", self.last_update_check.to_string());
        put("pending_update", esc(&self.pending_update));
        put("win_w", self.win_w.to_string());
        put("win_h", self.win_h.to_string());
        if let Some(x) = self.win_x {
            put("win_x", x.to_string());
        }
        if let Some(y) = self.win_y {
            put("win_y", y.to_string());
        }
        put("maximized", self.maximized.to_string());
        for m in &self.messages {
            put("msg", esc(m));
        }
        let _ = std::fs::write(f, t);
    }

    /// Remembers a commit message (most recent first, deduplicated, at most 15).
    pub fn remember_message(&mut self, msg: &str) {
        let m = msg.trim().to_string();
        if m.is_empty() {
            return;
        }
        self.messages.retain(|x| *x != m);
        self.messages.insert(0, m);
        self.messages.truncate(15);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("settings.ini");
        let mut s = Settings::load_from(Some(file.clone()));
        assert_eq!(s.context, 3);
        s.theme = ThemeMode::Light;
        s.pull_mode = PullMode::Rebase;
        s.side_by_side = true;
        s.auto_fetch_minutes = 12;
        s.check_updates = false;
        s.show_whitespace = true;
        s.watch_files = false;
        s.last_update_check = 1_700_000_000;
        s.pending_update = "v9.9.9".into();
        s.git_path = "C:\\Tools\\git.exe".into();
        s.win_x = Some(40.0);
        s.remember_message("first line\nsecond line");
        s.remember_message("other");
        s.remember_message("first line\nsecond line");
        s.save();

        let t = Settings::load_from(Some(file));
        assert_eq!(t.theme, ThemeMode::Light);
        assert_eq!(t.pull_mode, PullMode::Rebase);
        assert!(t.side_by_side);
        assert_eq!(t.auto_fetch_minutes, 12);
        assert!(!t.check_updates);
        assert!(t.show_whitespace && !t.watch_files);
        assert_eq!(t.last_update_check, 1_700_000_000);
        assert_eq!(t.pending_update, "v9.9.9");
        assert_eq!(t.git_path, "C:\\Tools\\git.exe");
        assert_eq!(t.win_x, Some(40.0));
        assert_eq!(t.messages, ["first line\nsecond line", "other"]);
    }

    #[test]
    fn bad_values_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.ini");
        std::fs::write(
            &file,
            "font_scale=banana\ncontext=9999\nauto_fetch_minutes=99999\nwin_w=1\n",
        )
        .unwrap();
        let s = Settings::load_from(Some(file));
        assert_eq!(s.font_scale, 1.0);
        assert_eq!(s.context, 50);
        assert_eq!(s.auto_fetch_minutes, 240);
        assert!(s.win_w >= 600.0);
    }
}
