//! Rustle doubles as the `GIT_ASKPASS` / `SSH_ASKPASS` helper: git (or ssh) starts it with the
//! prompt as its first argument and `RUSTLE_ASKPASS=1`. It shows a small dialog and prints the
//! answer to stdout (exit code 1 when cancelled).

use crate::settings::ThemeMode;
use crate::theme;
use eframe::egui;
use std::io::Write;
use std::sync::{Arc, Mutex};

pub fn is_askpass() -> bool {
    std::env::var_os("RUSTLE_ASKPASS").is_some()
}

fn is_secret(prompt: &str) -> bool {
    let p = prompt.to_lowercase();
    ["password", "passphrase", "pin", "token"]
        .iter()
        .any(|k| p.contains(k))
}

fn finish(answer: Option<String>) -> ! {
    match answer {
        Some(v) => {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{v}");
            let _ = out.flush();
            std::process::exit(0);
        }
        None => std::process::exit(1),
    }
}

/// Runs the dialog; never returns.
pub fn run() -> ! {
    let prompt = std::env::args().nth(1).unwrap_or_else(|| "Enter value".into());
    // Test hook (also handy for scripted runs): answer without showing a window.
    if let Some(v) = std::env::var_os("RUSTLE_ASKPASS_AUTO") {
        finish(Some(v.to_string_lossy().into_owned()));
    }
    let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let secret = is_secret(&prompt);
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 180.0])
            .with_resizable(false)
            .with_always_on_top()
            .with_title("Rustle — authentication"),
        centered: true,
        ..Default::default()
    };
    let r2 = Arc::clone(&result);
    let _ = eframe::run_native(
        "Rustle askpass",
        opts,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx, ThemeMode::Dark, 1.0);
            Ok(Box::new(Ask {
                prompt,
                secret,
                value: String::new(),
                result: r2,
                first: true,
            }))
        }),
    );
    let answer = result.lock().ok().and_then(|g| g.clone());
    finish(answer)
}

struct Ask {
    prompt: String,
    secret: bool,
    value: String,
    result: Arc<Mutex<Option<String>>>,
    first: bool,
}

impl eframe::App for Ask {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut done = false;
        let mut cancel = false;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label(theme::bold("Git needs your input", 15.0));
            ui.add_space(4.0);
            ui.label(&self.prompt);
            ui.add_space(8.0);
            let te = ui.add(
                egui::TextEdit::singleline(&mut self.value)
                    .password(self.secret)
                    .desired_width(f32::INFINITY),
            );
            if self.first {
                te.request_focus();
                self.first = false;
            }
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = true;
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "OK", true).clicked() {
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if done && let Ok(mut r) = self.result.lock() {
            *r = Some(self.value.clone());
        }
        if done || cancel {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_secret;

    #[test]
    fn detects_secret_prompts() {
        assert!(is_secret("Password for 'https://user@github.com': "));
        assert!(is_secret("Enter passphrase for key '/home/u/.ssh/id_ed25519': "));
        assert!(!is_secret("Username for 'https://github.com': "));
    }
}
