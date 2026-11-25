//! Diff model (parsed + syntax highlighted) and its renderer: unified and side-by-side, line
//! numbers, intra-line emphasis and per-hunk staging buttons.

use crate::theme;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, Ui, UiBuilder, pos2,
    text::{LayoutJob, TextFormat},
    vec2,
};
use rustle_core::{FileDiff, PatchMode, SbsRow, intra_change, parse_diff, side_by_side};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

const MAX_HIGHLIGHT_LINES: usize = 8000;

#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub color: [u8; 3],
}

pub type LineSpans = Vec<Span>;

/// Old/new line number of each body line of a hunk.
type LineNums = Vec<(Option<u32>, Option<u32>)>;

#[derive(Debug, Clone, Copy)]
pub enum Row {
    File(usize),
    Hunk(usize, usize),
    /// file, hunk, line
    Line(usize, usize, usize),
    /// file, hunk, side-by-side row
    Sbs(usize, usize, usize),
}

pub struct DiffView {
    pub title: String,
    pub files: Vec<FileDiff>,
    /// `[file][hunk][line]` highlighted spans (empty when highlighting was skipped).
    spans: Vec<Vec<Vec<LineSpans>>>,
    sbs: Vec<Vec<Vec<SbsRow>>>,
    nums: Vec<Vec<LineNums>>,
    intra: HashMap<(usize, usize, usize), (usize, usize)>,
    rows_unified: Vec<Row>,
    rows_sbs: Vec<Row>,
    max_cols: usize,
}

static SYN: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();

fn syn() -> &'static (SyntaxSet, ThemeSet) {
    SYN.get_or_init(|| (SyntaxSet::load_defaults_newlines(), ThemeSet::load_defaults()))
}

fn spans_of(ranges: Vec<(syntect::highlighting::Style, &str)>) -> LineSpans {
    ranges
        .into_iter()
        .map(|(st, t)| Span {
            text: t.trim_end_matches(['\n', '\r']).replace('\t', "    "),
            color: [st.foreground.r, st.foreground.g, st.foreground.b],
        })
        .filter(|s| !s.text.is_empty())
        .collect()
}

fn highlight(files: &[FileDiff], dark: bool) -> Vec<Vec<Vec<LineSpans>>> {
    let total: usize = files.iter().flat_map(|f| &f.hunks).map(|h| h.lines.len()).sum();
    let plain = || {
        files
            .iter()
            .map(|f| f.hunks.iter().map(|h| vec![Vec::new(); h.lines.len()]).collect())
            .collect()
    };
    if total > MAX_HIGHLIGHT_LINES {
        return plain();
    }
    let (ss, ts) = syn();
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let Some(theme) = ts.themes.get(theme_name) else {
        return plain();
    };
    files
        .iter()
        .map(|f| {
            let ext = f
                .path()
                .and_then(|p| p.rsplit_once('.').map(|(_, e)| e.to_string()));
            let syntax = ext
                .as_deref()
                .and_then(|e| ss.find_syntax_by_extension(e))
                .unwrap_or_else(|| ss.find_syntax_plain_text());
            f.hunks
                .iter()
                .map(|h| {
                    // Separate state for the old and the new side of the hunk.
                    let mut old = HighlightLines::new(syntax, theme);
                    let mut new = HighlightLines::new(syntax, theme);
                    h.lines
                        .iter()
                        .map(|l| {
                            let c = l.as_bytes().first().copied().unwrap_or(b' ');
                            if c == b'\\' {
                                return Vec::new();
                            }
                            let code = format!("{}\n", l.get(1..).unwrap_or(""));
                            let r_old = (c != b'+').then(|| old.highlight_line(&code, ss));
                            let r_new = (c != b'-').then(|| new.highlight_line(&code, ss));
                            match (c, r_new, r_old) {
                                (b'-', _, Some(Ok(r))) | (_, Some(Ok(r)), _) => spans_of(r),
                                _ => Vec::new(),
                            }
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

impl DiffView {
    pub fn build(title: String, text: &str, dark: bool) -> Self {
        let files = parse_diff(text);
        let spans = highlight(&files, dark);
        let mut sbs = Vec::new();
        let mut nums = Vec::new();
        let mut intra = HashMap::new();
        let mut rows_unified = Vec::new();
        let mut rows_sbs = Vec::new();
        let mut max_cols = 0usize;

        for (fi, f) in files.iter().enumerate() {
            rows_unified.push(Row::File(fi));
            rows_sbs.push(Row::File(fi));
            let mut f_sbs = Vec::new();
            let mut f_nums = Vec::new();
            for (hi, h) in f.hunks.iter().enumerate() {
                rows_unified.push(Row::Hunk(fi, hi));
                rows_sbs.push(Row::Hunk(fi, hi));
                for (li, l) in h.lines.iter().enumerate() {
                    rows_unified.push(Row::Line(fi, hi, li));
                    max_cols = max_cols.max(l.chars().count());
                }
                // Intra-line emphasis for paired removed/added runs.
                let mut i = 0;
                while i < h.lines.len() {
                    if h.lines[i].starts_with('-') {
                        let rs = i;
                        while i < h.lines.len() && h.lines[i].starts_with('-') {
                            i += 1;
                        }
                        let as_ = i;
                        while i < h.lines.len() && h.lines[i].starts_with('+') {
                            i += 1;
                        }
                        for k in 0..(as_ - rs).min(i - as_) {
                            let (a, b) = (rs + k, as_ + k);
                            let (ra, rb) = intra_change(&h.lines[a][1..], &h.lines[b][1..]);
                            intra.insert((fi, hi, a), ra);
                            intra.insert((fi, hi, b), rb);
                        }
                    } else {
                        i += 1;
                    }
                }
                let rows = side_by_side(h);
                for ri in 0..rows.len() {
                    rows_sbs.push(Row::Sbs(fi, hi, ri));
                }
                f_sbs.push(rows);
                f_nums.push(h.line_numbers());
            }
            sbs.push(f_sbs);
            nums.push(f_nums);
        }
        Self {
            title,
            files,
            spans,
            sbs,
            nums,
            intra,
            rows_unified,
            rows_sbs,
            max_cols,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|f| f.hunks.is_empty())
    }

    pub fn is_binary(&self) -> bool {
        self.files.iter().any(|f| f.binary)
    }
}

/// What the viewer is used for; `None` means read-only.
#[derive(Debug, Clone, Copy)]
pub struct StageCtx {
    /// The diff shows staged changes (buttons unstage) instead of unstaged ones.
    pub staged: bool,
    pub can_discard: bool,
}

#[derive(Debug, Clone)]
pub enum DiffAction {
    Patch {
        file: usize,
        hunk: usize,
        /// `None` = the whole hunk.
        lines: Option<HashSet<usize>>,
        mode: PatchMode,
    },
}

fn tinted(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a)
}

struct Style {
    font: FontId,
    char_w: f32,
    row_h: f32,
}

#[allow(clippy::too_many_arguments)]
fn draw_code(
    p: &egui::Painter,
    x: f32,
    cy: f32,
    plain: &str,
    spans: &LineSpans,
    st: &Style,
    emph: Option<((usize, usize), Color32)>,
) {
    if let Some(((a, b), color)) = emph
        && b > a
    {
        let r = Rect::from_min_max(
            pos2(x + a as f32 * st.char_w, cy - st.row_h / 2.0 + 1.0),
            pos2(x + b as f32 * st.char_w, cy + st.row_h / 2.0 - 1.0),
        );
        p.rect_filled(r, 2.0, color);
    }
    let mut job = LayoutJob::default();
    if spans.is_empty() {
        job.append(
            &plain.replace('\t', "    "),
            0.0,
            TextFormat::simple(st.font.clone(), theme::text()),
        );
    } else {
        for s in spans {
            job.append(
                &s.text,
                0.0,
                TextFormat::simple(
                    st.font.clone(),
                    Color32::from_rgb(s.color[0], s.color[1], s.color[2]),
                ),
            );
        }
    }
    let galley = p.layout_job(job);
    p.galley(pos2(x, cy - galley.size().y / 2.0), galley, theme::text());
}

/// Draws the diff. Returns nothing; staging requests are pushed to `acts`.
#[allow(clippy::too_many_lines)]
pub fn show(
    ui: &mut Ui,
    id: &str,
    v: &DiffView,
    side_by_side: bool,
    sel: &mut HashSet<(usize, usize, usize)>,
    stage: Option<StageCtx>,
    acts: &mut Vec<DiffAction>,
) {
    let font = FontId::monospace(13.0);
    let char_w = ui
        .painter()
        .layout_no_wrap("0".into(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let st = Style {
        font,
        char_w,
        row_h: 22.0,
    };
    let rows = if side_by_side {
        &v.rows_sbs
    } else {
        &v.rows_unified
    };
    let gutter = 6.0 * char_w + 10.0;
    let code_w = v.max_cols as f32 * char_w + 24.0;
    let half_w = (gutter + code_w).max(ui.available_width() / 2.0);
    let content_w = if side_by_side {
        half_w * 2.0
    } else {
        2.0 * gutter + code_w + 14.0
    };
    let add_bg = tinted(theme::green(), 0.16);
    let del_bg = tinted(theme::red(), 0.16);
    let add_emph = tinted(theme::green(), 0.38);
    let del_emph = tinted(theme::red(), 0.38);
    let sel_bg = tinted(theme::accent(), 0.30);

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::both().id_salt(id).auto_shrink(false).show_rows(
            ui,
            st.row_h,
            rows.len(),
            |ui, range| {
                let w = content_w.max(ui.available_width());
                let vis_right = ui.clip_rect().right();
                for ri in range {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, st.row_h), Sense::click());
                    let p = ui.painter_at(rect);
                    let cy = rect.center().y;
                    match rows[ri] {
                        Row::File(fi) => {
                            p.rect_filled(rect, 0.0, theme::bg_btn());
                            let f = &v.files[fi];
                            let name = f.path().unwrap_or_else(|| "file".into());
                            let label = if f.binary {
                                format!("{name}  (binary)")
                            } else {
                                name
                            };
                            p.text(
                                pos2(rect.left() + 10.0, cy),
                                Align2::LEFT_CENTER,
                                label,
                                FontId::proportional(13.0),
                                theme::text(),
                            );
                        }
                        Row::Hunk(fi, hi) => {
                            p.rect_filled(rect, 0.0, tinted(theme::accent(), 0.13));
                            let h = &v.files[fi].hunks[hi];
                            p.text(
                                pos2(rect.left() + 10.0, cy),
                                Align2::LEFT_CENTER,
                                &h.header,
                                st.font.clone(),
                                theme::accent(),
                            );
                            if let Some(sc) = stage {
                                let n_sel = sel.iter().filter(|(f, hh, _)| *f == fi && *hh == hi).count();
                                let r = Rect::from_min_max(
                                    pos2(rect.left(), rect.top()),
                                    pos2(vis_right - 8.0, rect.bottom()),
                                );
                                ui.scope_builder(
                                    UiBuilder::new()
                                        .max_rect(r)
                                        .layout(Layout::right_to_left(Align::Center)),
                                    |ui| {
                                        let sels = |hi_: usize| -> HashSet<usize> {
                                            sel.iter()
                                                .filter(|(f, hh, _)| *f == fi && *hh == hi_)
                                                .map(|(_, _, l)| *l)
                                                .collect()
                                        };
                                        let small =
                                            |t: &str| egui::Button::new(RichText::new(t).size(12.0)).small();
                                        let (whole, part, mode) = if sc.staged {
                                            ("Unstage hunk", "Unstage selected", PatchMode::Unstage)
                                        } else {
                                            ("Stage hunk", "Stage selected", PatchMode::Stage)
                                        };
                                        if sc.can_discard && !sc.staged {
                                            if ui.add(small("Discard hunk")).clicked() {
                                                acts.push(DiffAction::Patch {
                                                    file: fi,
                                                    hunk: hi,
                                                    lines: None,
                                                    mode: PatchMode::Discard,
                                                });
                                            }
                                            if n_sel > 0 && ui.add(small("Discard selected")).clicked() {
                                                acts.push(DiffAction::Patch {
                                                    file: fi,
                                                    hunk: hi,
                                                    lines: Some(sels(hi)),
                                                    mode: PatchMode::Discard,
                                                });
                                            }
                                        }
                                        if ui.add(small(whole)).clicked() {
                                            acts.push(DiffAction::Patch {
                                                file: fi,
                                                hunk: hi,
                                                lines: None,
                                                mode,
                                            });
                                        }
                                        if n_sel > 0 && ui.add(small(part)).clicked() {
                                            acts.push(DiffAction::Patch {
                                                file: fi,
                                                hunk: hi,
                                                lines: Some(sels(hi)),
                                                mode,
                                            });
                                        }
                                    },
                                );
                            }
                        }
                        Row::Line(fi, hi, li) => {
                            let h = &v.files[fi].hunks[hi];
                            let line = &h.lines[li];
                            let c = line.as_bytes().first().copied().unwrap_or(b' ');
                            let nums = v.nums[fi][hi][li];
                            let selected = sel.contains(&(fi, hi, li));
                            let bg = match c {
                                b'+' => Some(add_bg),
                                b'-' => Some(del_bg),
                                _ => None,
                            };
                            if let Some(bg) = bg {
                                p.rect_filled(rect, 0.0, bg);
                            }
                            if selected {
                                p.rect_filled(rect, 0.0, sel_bg);
                            }
                            let num = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
                            let gy = pos2(rect.left() + gutter - 8.0, cy);
                            p.text(
                                gy,
                                Align2::RIGHT_CENTER,
                                num(nums.0),
                                st.font.clone(),
                                theme::text_weak(),
                            );
                            p.text(
                                pos2(gy.x + gutter, cy),
                                Align2::RIGHT_CENTER,
                                num(nums.1),
                                st.font.clone(),
                                theme::text_weak(),
                            );
                            let x = rect.left() + 2.0 * gutter + 4.0;
                            let marker = match c {
                                b'+' => "+",
                                b'-' => "-",
                                _ => " ",
                            };
                            let mcolor = match c {
                                b'+' => theme::green(),
                                b'-' => theme::red(),
                                _ => theme::text_weak(),
                            };
                            p.text(pos2(x, cy), Align2::LEFT_CENTER, marker, st.font.clone(), mcolor);
                            let emph = v
                                .intra
                                .get(&(fi, hi, li))
                                .map(|r| (*r, if c == b'+' { add_emph } else { del_emph }));
                            let spans = v.spans.get(fi).and_then(|f| f.get(hi)).and_then(|h| h.get(li));
                            let empty = Vec::new();
                            draw_code(
                                &p,
                                x + st.char_w * 2.0,
                                cy,
                                line.get(1..).unwrap_or(""),
                                spans.unwrap_or(&empty),
                                &st,
                                emph,
                            );
                            if stage.is_some()
                                && h.is_change(li)
                                && resp.clicked()
                                && !sel.remove(&(fi, hi, li))
                            {
                                sel.insert((fi, hi, li));
                            }
                        }
                        Row::Sbs(fi, hi, si) => {
                            let h = &v.files[fi].hunks[hi];
                            let row = &v.sbs[fi][hi][si];
                            let mid = rect.left() + half_w;
                            p.line_segment(
                                [pos2(mid, rect.top()), pos2(mid, rect.bottom())],
                                Stroke::new(1.0, theme::border()),
                            );
                            let mut clicked_cell = None;
                            for (cell, left) in [(&row.left, true), (&row.right, false)] {
                                let x0 = if left { rect.left() } else { mid };
                                let cell_rect = Rect::from_min_max(
                                    pos2(x0, rect.top()),
                                    pos2(x0 + half_w, rect.bottom()),
                                );
                                let Some(cell) = cell else {
                                    p.rect_filled(cell_rect, 0.0, tinted(theme::text_weak(), 0.06));
                                    continue;
                                };
                                let cp = p.with_clip_rect(cell_rect);
                                match cell.kind {
                                    rustle_core::LineKind::Added => {
                                        cp.rect_filled(cell_rect, 0.0, add_bg);
                                    }
                                    rustle_core::LineKind::Removed => {
                                        cp.rect_filled(cell_rect, 0.0, del_bg);
                                    }
                                    rustle_core::LineKind::Context => {}
                                }
                                if sel.contains(&(fi, hi, cell.line_index)) {
                                    cp.rect_filled(cell_rect, 0.0, sel_bg);
                                }
                                cp.text(
                                    pos2(x0 + gutter - 8.0, cy),
                                    Align2::RIGHT_CENTER,
                                    cell.number.map_or(String::new(), |n| n.to_string()),
                                    st.font.clone(),
                                    theme::text_weak(),
                                );
                                let emph = v
                                    .intra
                                    .get(&(fi, hi, cell.line_index))
                                    .map(|r| (*r, if left { del_emph } else { add_emph }));
                                let spans = v
                                    .spans
                                    .get(fi)
                                    .and_then(|f| f.get(hi))
                                    .and_then(|hh| hh.get(cell.line_index));
                                let empty = Vec::new();
                                draw_code(
                                    &cp,
                                    x0 + gutter + 2.0,
                                    cy,
                                    &cell.text,
                                    spans.unwrap_or(&empty),
                                    &st,
                                    emph,
                                );
                                if resp.clicked()
                                    && resp
                                        .interact_pointer_pos()
                                        .is_some_and(|pp| cell_rect.contains(pp))
                                {
                                    clicked_cell = Some(cell.line_index);
                                }
                            }
                            if let (Some(li), Some(_)) = (clicked_cell, stage)
                                && h.is_change(li)
                                && !sel.remove(&(fi, hi, li))
                            {
                                sel.insert((fi, hi, li));
                            }
                        }
                    }
                }
            },
        );
    });
}
