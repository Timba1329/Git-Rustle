#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod askpass;
mod branches;
mod commands;
mod commit_window;
mod diffview;
mod files_panel;
mod fmt;
mod imagediff;
mod panels;
mod popups;
mod recent;
mod repo_settings;
mod settings;
mod tab;
mod theme;
mod update;
mod watcher;
mod worker;

use settings::Settings;

fn init_logging() {
    let Some(dir) = settings::data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join("rustle.log");
    // Keep the log small: start fresh when it grew past 2 MB.
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > 2_000_000) {
        let _ = std::fs::remove_file(&log);
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&log) else {
        return;
    };
    let level = if std::env::var_os("RUSTLE_LOG").is_some_and(|v| v == "debug") {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_max_level(level)
        .try_init();

    let crash = dir.join("crash.log");
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let text = format!("{info}\n{bt}\n---\n");
        tracing::error!("panic: {info}");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash)
            .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
        default(info);
    }));
}

fn icon() -> Option<eframe::egui::IconData> {
    const RGBA: &[u8] = include_bytes!("../assets/icon-64.rgba");
    (RGBA.len() == 64 * 64 * 4).then(|| eframe::egui::IconData {
        rgba: RGBA.to_vec(),
        width: 64,
        height: 64,
    })
}

fn main() -> eframe::Result {
    // git / ssh start us as their password prompt helper.
    if askpass::is_askpass() {
        askpass::run();
    }
    init_logging();
    if let Ok(exe) = std::env::current_exe() {
        update::cleanup_old(&exe);
    }
    tracing::info!("Rustle {} starting", env!("CARGO_PKG_VERSION"));

    let s = Settings::load();
    let mut vp = eframe::egui::ViewportBuilder::default()
        .with_inner_size([s.win_w, s.win_h])
        .with_min_inner_size([760.0, 480.0])
        .with_maximized(s.maximized)
        .with_title("Rustle");
    if let (Some(x), Some(y)) = (s.win_x, s.win_y) {
        vp = vp.with_position([x, y]);
    }
    if let Some(i) = icon() {
        vp = vp.with_icon(i);
    }
    let options = eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    };
    eframe::run_native("Rustle", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
