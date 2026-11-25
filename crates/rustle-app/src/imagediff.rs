//! Before/after comparison of images: side by side, swipe and onion skin.

use crate::theme::{accent, bg_side, text, text_weak};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Ui,
    pos2, vec2,
};

const MAX_DIM: u32 = 4096;
const MAX_PIXELS: u64 = 60_000_000;

/// An image decoded on the worker thread.
pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Size of the encoded file.
    pub bytes: usize,
}

pub fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico")
    )
}

/// Decodes an image; very large images are scaled down so they fit in a texture.
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    if u64::from(img.width()) * u64::from(img.height()) > MAX_PIXELS {
        return Err("image is too large to preview".into());
    }
    let img = if img.width() > MAX_DIM || img.height() > MAX_DIM {
        img.thumbnail(MAX_DIM, MAX_DIM)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(DecodedImage {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
        bytes: bytes.len(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    SideBySide,
    Swipe,
    Onion,
}

struct Tex {
    handle: TextureHandle,
    size: [usize; 2],
    bytes: usize,
}

pub struct ImageDiffView {
    old: Option<Tex>,
    new: Option<Tex>,
    mode: Mode,
    split: f32,
    opacity: f32,
}

impl ImageDiffView {
    pub fn new(ctx: &egui::Context, old: Option<DecodedImage>, new: Option<DecodedImage>) -> Self {
        let make = |name: &str, d: DecodedImage| {
            let img = egui::ColorImage::from_rgba_unmultiplied([d.width, d.height], &d.rgba);
            Tex {
                handle: ctx.load_texture(name, img, TextureOptions::LINEAR),
                size: [d.width, d.height],
                bytes: d.bytes,
            }
        };
        Self {
            old: old.map(|d| make("image-old", d)),
            new: new.map(|d| make("image-new", d)),
            mode: Mode::SideBySide,
            split: 0.5,
            opacity: 0.5,
        }
    }

    /// Whether there is at least one image to show.
    pub fn has_any(&self) -> bool {
        self.old.is_some() || self.new.is_some()
    }

    fn info(t: &Option<Tex>) -> String {
        match t {
            Some(t) => format!("{} × {}  ·  {}", t.size[0], t.size[1], human(t.bytes)),
            None => "—".into(),
        }
    }
}

fn human(b: usize) -> String {
    if b >= 1_048_576 {
        format!("{:.1} MB", b as f32 / 1_048_576.0)
    } else if b >= 1024 {
        format!("{:.1} KB", b as f32 / 1024.0)
    } else {
        format!("{b} B")
    }
}

/// Largest rect of the image's aspect ratio that fits in `area`, centred.
fn fit(area: Rect, size: [usize; 2]) -> Rect {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    // Never enlarge small images more than four times.
    let scale = (area.width() / w).min(area.height() / h).clamp(0.01, 4.0);
    Rect::from_center_size(area.center(), vec2(w * scale, h * scale))
}

fn draw_checker(p: &egui::Painter, r: Rect) {
    let step = 10.0;
    let (a, b) = (Color32::from_gray(58), Color32::from_gray(74));
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = r.left();
        let mut col = row;
        while x < r.right() {
            let cell = Rect::from_min_max(
                pos2(x, y),
                pos2((x + step).min(r.right()), (y + step).min(r.bottom())),
            );
            p.rect_filled(cell, 0.0, if col % 2 == 0 { a } else { b });
            x += step;
            col += 1;
        }
        y += step;
        row += 1;
    }
}

fn paint(p: &egui::Painter, t: &Tex, r: Rect, uv: Rect, tint: Color32) {
    p.image(t.handle.id(), r, uv, tint);
}

const FULL: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

pub fn show(ui: &mut Ui, v: &mut ImageDiffView) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut v.mode, Mode::SideBySide, "Side by side");
        ui.selectable_value(&mut v.mode, Mode::Swipe, "Swipe");
        ui.selectable_value(&mut v.mode, Mode::Onion, "Onion skin");
        if v.mode == Mode::Onion {
            ui.add(egui::Slider::new(&mut v.opacity, 0.0..=1.0).show_value(false));
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Before: {}", ImageDiffView::info(&v.old)))
                .size(12.0)
                .color(text_weak()),
        );
        ui.label(
            RichText::new(format!("After: {}", ImageDiffView::info(&v.new)))
                .size(12.0)
                .color(text_weak()),
        );
        if let (Some(o), Some(n)) = (&v.old, &v.new) {
            let d = n.bytes as i64 - o.bytes as i64;
            let sign = if d >= 0 { "+" } else { "−" };
            ui.label(
                RichText::new(format!("({sign}{})", human(d.unsigned_abs() as usize)))
                    .size(12.0)
                    .color(text_weak()),
            );
        }
    });
    let avail = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(avail.size(), Sense::drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, bg_side());
    let label = |pos: Pos2, s: &str| {
        p.text(
            pos,
            Align2::CENTER_TOP,
            s,
            FontId::proportional(12.0),
            text_weak(),
        );
    };

    match v.mode {
        Mode::SideBySide => {
            let half = vec2(rect.width() / 2.0, rect.height());
            for (i, (t, name)) in [(&v.old, "Before"), (&v.new, "After")].into_iter().enumerate() {
                let cell =
                    Rect::from_min_size(pos2(rect.left() + i as f32 * half.x, rect.top()), half).shrink(8.0);
                let body = Rect::from_min_max(pos2(cell.left(), cell.top() + 18.0), cell.max);
                label(pos2(cell.center().x, cell.top()), name);
                match t {
                    Some(t) => {
                        let r = fit(body, t.size);
                        draw_checker(&p, r);
                        paint(&p, t, r, FULL, Color32::WHITE);
                    }
                    None => {
                        p.text(
                            body.center(),
                            Align2::CENTER_CENTER,
                            "none",
                            FontId::proportional(13.0),
                            text_weak(),
                        );
                    }
                }
            }
        }
        Mode::Swipe | Mode::Onion => {
            let body = Rect::from_min_max(
                pos2(rect.left() + 8.0, rect.top() + 8.0),
                rect.max - vec2(8.0, 8.0),
            );
            let size = v.new.as_ref().or(v.old.as_ref()).map_or([1, 1], |t| t.size);
            let r = fit(body, size);
            draw_checker(&p, r);
            if v.mode == Mode::Swipe {
                if let Some(o) = &v.old {
                    paint(&p, o, r, FULL, Color32::WHITE);
                }
                if let Some(n) = &v.new {
                    let x = r.left() + r.width() * v.split;
                    let clip = Rect::from_min_max(pos2(x, r.top()), r.max);
                    let uv = Rect::from_min_max(pos2(v.split, 0.0), pos2(1.0, 1.0));
                    paint(&p, n, clip, uv, Color32::WHITE);
                }
                let x = r.left() + r.width() * v.split;
                p.line_segment(
                    [pos2(x, r.top()), pos2(x, r.bottom())],
                    Stroke::new(2.0, accent()),
                );
                p.circle_filled(pos2(x, r.center().y), 7.0, accent());
                if resp.dragged()
                    && let Some(pp) = resp.interact_pointer_pos()
                {
                    v.split = ((pp.x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0);
                }
                label(pos2(r.center().x, rect.top() - 0.0), "Drag to compare");
            } else {
                if let Some(o) = &v.old {
                    paint(&p, o, r, FULL, Color32::WHITE);
                }
                if let Some(n) = &v.new {
                    paint(
                        &p,
                        n,
                        r,
                        FULL,
                        Color32::from_white_alpha((v.opacity * 255.0) as u8),
                    );
                }
            }
        }
    }
    if !v.has_any() {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No image data",
            FontId::proportional(14.0),
            text(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for p in img.pixels_mut() {
            *p = image::Rgba(rgba);
        }
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn detects_image_paths() {
        assert!(is_image_path("assets/Logo.PNG"));
        assert!(is_image_path("a.jpeg"));
        assert!(!is_image_path("notes.txt"));
        assert!(!is_image_path("png"));
    }

    #[test]
    fn decodes_a_png() {
        let d = decode(&png(4, 3, [10, 20, 30, 255])).unwrap();
        assert_eq!((d.width, d.height), (4, 3));
        assert_eq!(&d.rgba[..4], &[10, 20, 30, 255]);
        assert!(d.bytes > 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not an image").is_err());
    }

    #[test]
    fn large_images_are_scaled_down() {
        let d = decode(&png(5000, 10, [0, 0, 0, 255])).unwrap();
        assert!(d.width <= MAX_DIM as usize);
        assert!(d.height >= 1);
    }

    #[test]
    fn fit_keeps_aspect_ratio_and_stays_inside() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        let r = fit(area, [400, 400]);
        assert!(area.contains_rect(r));
        assert!((r.width() - r.height()).abs() < 0.5);
    }
}
