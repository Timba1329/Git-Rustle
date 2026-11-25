//! Colours (dark and light), fonts and small reusable widgets.

use crate::settings::ThemeMode;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, RichText,
    Sense, Stroke, TextStyle, Ui, pos2, vec2,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static LIGHT: AtomicBool = AtomicBool::new(false);
static BOLD_OK: AtomicBool = AtomicBool::new(false);

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

fn pick(dark: Color32, light: Color32) -> Color32 {
    if is_light() { light } else { dark }
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn bg_main() -> Color32 {
    pick(c(0x13, 0x15, 0x1a), c(0xff, 0xff, 0xff))
}
pub fn bg_side() -> Color32 {
    pick(c(0x17, 0x1a, 0x20), c(0xf5, 0xf6, 0xf8))
}
pub fn bg_bar() -> Color32 {
    pick(c(0x1a, 0x1d, 0x24), c(0xec, 0xef, 0xf3))
}
pub fn bg_input() -> Color32 {
    pick(c(0x0e, 0x10, 0x14), c(0xff, 0xff, 0xff))
}
pub fn bg_btn() -> Color32 {
    pick(c(0x21, 0x26, 0x2f), c(0xee, 0xf0, 0xf4))
}
pub fn bg_btn_hover() -> Color32 {
    pick(c(0x2b, 0x31, 0x3c), c(0xe1, 0xe5, 0xeb))
}
pub fn border() -> Color32 {
    pick(c(0x28, 0x2d, 0x37), c(0xd9, 0xdd, 0xe4))
}
pub fn text() -> Color32 {
    pick(c(0xd3, 0xd7, 0xde), c(0x1d, 0x21, 0x29))
}
pub fn text_weak() -> Color32 {
    pick(c(0x7d, 0x85, 0x96), c(0x6a, 0x72, 0x83))
}
pub fn accent() -> Color32 {
    pick(c(0x5b, 0x8d, 0xef), c(0x2a, 0x64, 0xd6))
}
pub fn green() -> Color32 {
    pick(c(0x4f, 0xb8, 0x83), c(0x1a, 0x8f, 0x5c))
}
pub fn red() -> Color32 {
    pick(c(0xe5, 0x67, 0x6b), c(0xcc, 0x3b, 0x3f))
}
pub fn amber() -> Color32 {
    pick(c(0xe0, 0xa8, 0x4f), c(0xb9, 0x7a, 0x12))
}
pub fn purple() -> Color32 {
    pick(c(0xa9, 0x8b, 0xdc), c(0x7d, 0x4f, 0xc4))
}

fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

fn load(fonts: &mut FontDefinitions, name: &str, files: &[&str], family: FontFamily) -> bool {
    for f in files {
        if let Ok(bytes) = std::fs::read(f) {
            fonts
                .font_data
                .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, name.to_string());
            return true;
        }
    }
    false
}

/// Bold text; falls back to the default font if Segoe UI Semibold/Bold is missing.
pub fn bold(text: impl Into<String>, size: f32) -> RichText {
    let t = RichText::new(text.into());
    if BOLD_OK.load(Ordering::Relaxed) {
        t.font(FontId::new(size, bold_family()))
    } else {
        t.size(size).strong()
    }
}

/// Installs fonts once at startup.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    load(
        &mut fonts,
        "ui",
        &["C:\\Windows\\Fonts\\segoeui.ttf"],
        FontFamily::Proportional,
    );
    load(
        &mut fonts,
        "mono",
        &[
            "C:\\Windows\\Fonts\\CascadiaMono.ttf",
            "C:\\Windows\\Fonts\\consola.ttf",
        ],
        FontFamily::Monospace,
    );
    // Semibold looks more refined than full bold for headings.
    if load(
        &mut fonts,
        "ui-bold",
        &[
            "C:\\Windows\\Fonts\\seguisb.ttf",
            "C:\\Windows\\Fonts\\segoeuib.ttf",
        ],
        bold_family(),
    ) {
        let fallback = fonts.families[&FontFamily::Proportional].clone();
        let fam = fonts.families.entry(bold_family()).or_default();
        fam.extend(fallback.into_iter().filter(|n| n != "ui-bold"));
        BOLD_OK.store(true, Ordering::Relaxed);
    }
    ctx.set_fonts(fonts);
}

/// Applies colours and text sizes. Call again when the theme or font scale changes.
pub fn apply(ctx: &egui::Context, mode: ThemeMode, scale: f32) {
    LIGHT.store(mode == ThemeMode::Light, Ordering::Relaxed);
    let mut v = if is_light() {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    let r4 = CornerRadius::same(4);
    v.panel_fill = bg_main();
    v.window_fill = bg_side();
    v.extreme_bg_color = bg_input();
    v.faint_bg_color = bg_side();
    v.window_stroke = Stroke::new(1.0, border());
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = accent().gamma_multiply(0.22);
    v.selection.stroke = Stroke::new(1.0, accent());
    v.hyperlink_color = accent();
    v.override_text_color = Some(text());
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text());
    v.widgets.noninteractive.corner_radius = r4;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r4;
    }
    v.widgets.inactive.bg_fill = bg_btn();
    v.widgets.inactive.weak_bg_fill = bg_btn();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, border());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, text());
    v.widgets.hovered.bg_fill = bg_btn_hover();
    v.widgets.hovered.weak_bg_fill = bg_btn_hover();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, text_weak().gamma_multiply(0.7));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, text());
    v.widgets.active.bg_fill = accent().gamma_multiply(0.55);
    v.widgets.active.weak_bg_fill = accent().gamma_multiply(0.55);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent());
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.open.bg_fill = bg_btn_hover();
    v.widgets.open.weak_bg_fill = bg_btn_hover();
    let theme = if is_light() {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    ctx.set_theme(if is_light() {
        egui::ThemePreference::Light
    } else {
        egui::ThemePreference::Dark
    });
    ctx.set_visuals_of(theme, v);

    ctx.style_mut_of(theme, |s| {
        s.spacing.item_spacing = vec2(8.0, 5.0);
        s.spacing.button_padding = vec2(10.0, 3.0);
        s.spacing.interact_size.y = 24.0 * scale;
        s.spacing.window_margin = egui::Margin::same(12);
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(13.5 * scale));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(13.0 * scale));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.5 * scale));
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(16.0 * scale));
        s.text_styles
            .insert(TextStyle::Monospace, FontId::monospace(12.5 * scale));
    });
}

/// Runs `add` with flat "ghost" buttons: no fill and no border until hovered.
pub fn ghost<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_stroke = Stroke::NONE;
        add(ui)
    })
    .inner
}

/// Primary (filled) button in the accent colour.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let b = egui::Button::new(RichText::new(label).color(Color32::WHITE))
        .fill(if enabled {
            accent()
        } else {
            accent().gamma_multiply(0.35)
        })
        .stroke(Stroke::NONE);
    ui.add_enabled(enabled, b)
}

/// Destructive (red) button.
pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    let b = egui::Button::new(RichText::new(label).color(Color32::WHITE))
        .fill(red())
        .stroke(Stroke::NONE);
    ui.add(b)
}

/// Small flat square button with a single glyph (`+`, `−`, `✕`).
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: &str, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect, 4.0, bg_btn_hover());
    }
    let color = if !enabled {
        text_weak().gamma_multiply(0.4)
    } else if hovered {
        text()
    } else {
        text_weak()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(15.0),
        color,
    );
    resp.on_hover_text(tip)
}

/// Small heading in capitals above a list.
pub fn section_label(ui: &mut Ui, label: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(label.to_uppercase()).size(10.5).color(text_weak()));
    ui.add_space(1.0);
}

/// A sidebar row with an optional coloured dot; returns the click response.
pub fn nav_row(ui: &mut Ui, label: &str, active: bool, dot: Option<Color32>) -> Response {
    nav_row_badges(ui, label, active, dot, &[])
}

/// Like [`nav_row`], with small coloured badges (text, colour) right-aligned in the row.
pub fn nav_row_badges(
    ui: &mut Ui,
    label: &str,
    active: bool,
    dot: Option<Color32>,
    badges: &[(String, Color32)],
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let p = ui.painter_at(rect);
    if active {
        p.rect_filled(rect, 4.0, accent().gamma_multiply(0.16));
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, bg_btn());
    }
    if let Some(c) = dot {
        p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, c);
    }

    // Badges from the right edge inwards.
    let mut right = rect.right() - 6.0;
    for (t, color) in badges.iter().rev() {
        let g = p.layout_no_wrap(t.clone(), FontId::proportional(11.0), *color);
        let size = g.size() + vec2(10.0, 2.0);
        let br = Rect::from_min_size(pos2(right - size.x, rect.center().y - size.y / 2.0), size);
        p.rect_filled(br, 3.0, color.gamma_multiply(0.15));
        p.galley(br.min + vec2(5.0, 1.0), g, *color);
        right = br.left() - 4.0;
    }

    let color = if active {
        text()
    } else {
        text().gamma_multiply(0.92)
    };
    let tp = p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(right.max(rect.left() + 30.0), rect.bottom()),
    ));
    let galley = tp.layout_no_wrap(label.to_string(), FontId::proportional(13.5), color);
    let pos = pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0);
    tp.galley(pos, galley, color);
    resp
}
