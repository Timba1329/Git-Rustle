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
    /// Lines of a removed/added pair that differ only in whitespace.
    ws_only: HashSet<(usize, usize, usize)>,
    /// A plain file view: no add/remove colours and a single line number column.
    pub plain: bool,
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
        let mut ws_only = HashSet::new();
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
                            if h.lines[a][1..]
                                .split_whitespace()
                                .eq(h.lines[b][1..].split_whitespace())
                            {
                                ws_only.insert((fi, hi, a));
                                ws_only.insert((fi, hi, b));
                            }
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
            ws_only,
            plain: false,
        }
    }

    /// A read-only view of a whole file with syntax highlighting and line numbers.
    pub fn build_file(path: &str, text: &str, dark: bool) -> Self {
        let n = text.lines().count();
        let mut diff =
            format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,{n} +1,{n} @@\n");
        for l in text.lines() {
            diff.push(' ');
            diff.push_str(l);
            diff.push('\n');
        }
        let mut v = Self::build(path.to_string(), &diff, dark);
        v.plain = true;
        v
    }

    fn row_text(&self, row: Row) -> String {
        match row {
            Row::Line(f, h, l) => self.files[f].hunks[h].lines[l]
                .get(1..)
                .unwrap_or("")
                .to_lowercase(),
            Row::Sbs(f, h, s) => {
                let r = &self.sbs[f][h][s];
                let side =
                    |c: &Option<rustle_core::SbsCell>| c.as_ref().map_or(String::new(), |c| c.text.clone());
                format!("{} {}", side(&r.left), side(&r.right)).to_lowercase()
            }
            Row::File(f) => self.files[f].path().unwrap_or_default().to_lowercase(),
            Row::Hunk(..) => String::new(),
        }
    }

    /// Row indices (in the unified or side-by-side layout) whose text contains `needle`
    /// (lowercase).
    pub fn find_rows(&self, needle: &str, side_by_side: bool) -> Vec<usize> {
        if needle.is_empty() {
            return Vec::new();
        }
        let rows = if side_by_side {
            &self.rows_sbs
        } else {
            &self.rows_unified
        };
        rows.iter()
            .enumerate()
            .filter(|(_, r)| self.row_text(**r).contains(needle))
            .map(|(i, _)| i)
            .take(5000)
            .collect()
    }

    /// First row of every block of added/removed lines, for "next change" navigation.
    pub fn change_rows(&self, side_by_side: bool) -> Vec<usize> {
        let rows = if side_by_side {
            &self.rows_sbs
        } else {
            &self.rows_unified
        };
        let changed = |r: Row| match r {
            Row::Line(f, h, l) => self.files[f].hunks[h].lines[l].starts_with(['+', '-']),
            Row::Sbs(f, h, s) => {
                let r = &self.sbs[f][h][s];
                match (&r.left, &r.right) {
                    (Some(a), Some(b)) => a.text != b.text,
                    _ => true,
                }
            }
            _ => false,
        };
        let mut out = Vec::new();
        let mut prev = false;
        for (i, r) in rows.iter().enumerate() {
            let c = changed(*r);
            if c && !prev {
                out.push(i);
            }
            prev = c;
        }
        out
    }

    /// Row of line `line` (1-based) of a plain file view.
    pub fn file_line_row(line: u32) -> usize {
        2 + line.saturating_sub(1) as usize
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

/// Optional extras for [`show`].
#[derive(Default)]
pub struct ViewExtras<'a> {
    /// Lowercase text to highlight.
    pub find: Option<&'a str>,
    /// Row of the current match (drawn stronger).
    pub current_row: Option<usize>,
    /// Scroll this row into view.
    pub scroll_to: Option<usize>,
    /// Show spaces as dots.
    pub show_ws: bool,
}

/// Columns of trailing whitespace `(start, end)` in `text`, if any.
fn trailing_ws(text: &str) -> Option<(usize, usize)> {
    let t = text.replace('\t', "    ");
    let kept = t.trim_end().chars().count();
    let all = t.chars().count();
    (all > kept).then_some((kept, all))
}

fn tinted(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a)
}

struct Style {
    font: FontId,
    char_w: f32,
    row_h: f32,
    show_ws: bool,
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
    trail: Option<(usize, usize)>,
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
    if let Some((a, b)) = trail {
        let r = Rect::from_min_max(
            pos2(x + a as f32 * st.char_w, cy - st.row_h / 2.0 + 1.0),
            pos2(x + b as f32 * st.char_w, cy + st.row_h / 2.0 - 1.0),
        );
        p.rect_filled(r, 2.0, tinted(theme::red(), 0.45));
    }
    let dots = |t: &str| -> String {
        if st.show_ws {
            t.replace(' ', "·")
        } else {
            t.to_string()
        }
    };
    let mut job = LayoutJob::default();
    if spans.is_empty() {
        job.append(
            &dots(&plain.replace('\t', "    ")),
            0.0,
            TextFormat::simple(st.font.clone(), theme::text()),
        );
    } else {
        for s in spans {
            job.append(
                &dots(&s.text),
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
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    id: &str,
    v: &DiffView,
    side_by_side: bool,
    sel: &mut HashSet<(usize, usize, usize)>,
    stage: Option<StageCtx>,
    extras: &ViewExtras<'_>,
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
        show_ws: extras.show_ws,
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
        let mut area = egui::ScrollArea::both().id_salt(id).auto_shrink(false);
        if let Some(r) = extras.scroll_to {
            area = area.vertical_scroll_offset((r as f32 * st.row_h - 80.0).max(0.0));
        }
        area.show_rows(ui, st.row_h, rows.len(), |ui, range| {
            let w = content_w.max(ui.available_width());
            let vis_right = ui.clip_rect().right();
            for ri in range {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, st.row_h), Sense::click());
                let p = ui.painter_at(rect);
                let cy = rect.center().y;
                if let Some(needle) = extras.find
                    && !matches!(rows[ri], Row::Hunk(..))
                    && v.row_text(rows[ri]).contains(needle)
                {
                    let a = if extras.current_row == Some(ri) {
                        0.55
                    } else {
                        0.22
                    };
                    p.rect_filled(rect, 0.0, tinted(theme::amber(), a));
                }
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
                        if !v.plain {
                            p.text(
                                gy,
                                Align2::RIGHT_CENTER,
                                num(nums.0),
                                st.font.clone(),
                                theme::text_weak(),
                            );
                        }
                        p.text(
                            pos2(gy.x + gutter, cy),
                            Align2::RIGHT_CENTER,
                            num(nums.1),
                            st.font.clone(),
                            theme::text_weak(),
                        );
                        if v.ws_only.contains(&(fi, hi, li)) {
                            p.text(
                                pos2(vis_right - 14.0, cy),
                                Align2::RIGHT_CENTER,
                                "ws",
                                FontId::proportional(11.0),
                                theme::amber(),
                            );
                        }
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
                            if c == b'+' {
                                trailing_ws(line.get(1..).unwrap_or(""))
                            } else {
                                None
                            },
                        );
                        if stage.is_some() && h.is_change(li) && resp.clicked() && !sel.remove(&(fi, hi, li))
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
                            let cell_rect =
                                Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + half_w, rect.bottom()));
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
                                if cell.kind == rustle_core::LineKind::Added {
                                    trailing_ws(&cell.text)
                                } else {
                                    None
                                },
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
        });
    });
}

/// F3 / Shift+F3: the row of the next or previous change in the diff, when one of them was pressed.
pub fn change_nav(ctx: &egui::Context, d: &DiffView, side_by_side: bool, idx: &mut usize) -> Option<usize> {
    let rows = d.change_rows(side_by_side);
    if rows.is_empty() {
        return None;
    }
    let (prev, next) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3),
            i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
        )
    });
    if next {
        *idx = (*idx + 1) % rows.len();
    } else if prev {
        *idx = (*idx + rows.len() - 1) % rows.len();
    } else {
        return None;
    }
    rows.get(*idx).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_rows_marks_the_start_of_each_change_block() {
        let text = "diff --git a/f b/f
--- a/f
+++ b/f
@@ -1,6 +1,6 @@
 a
-b
+B
 c
 d
-e
+E
 f
";
        let v = DiffView::build("t".into(), text, true);
        let rows = v.change_rows(false);
        assert_eq!(rows.len(), 2);
        assert!(rows[0] < rows[1]);
    }

    const DIFF: &str = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n keep\n-let  x = 1;\n+let x = 1;\n tail  \n";

    #[test]
    fn finds_rows_in_both_layouts() {
        let v = DiffView::build("t".into(), DIFF, true);
        let unified = v.find_rows("let", false);
        assert_eq!(unified.len(), 2);
        let sbs = v.find_rows("let", true);
        assert_eq!(sbs.len(), 1, "the removed/added pair shares one side-by-side row");
        assert!(v.find_rows("", false).is_empty());
        assert!(v.find_rows("zzz", false).is_empty());
    }

    #[test]
    fn marks_whitespace_only_changes() {
        let v = DiffView::build("t".into(), DIFF, true);
        assert_eq!(v.ws_only.len(), 2);
    }

    #[test]
    fn trailing_whitespace_columns() {
        assert_eq!(trailing_ws("abc  "), Some((3, 5)));
        assert_eq!(trailing_ws("abc"), None);
        assert_eq!(trailing_ws("a\t"), Some((1, 5)));
    }

    #[test]
    fn plain_file_view_has_one_context_hunk() {
        let v = DiffView::build_file("src/a.rs", "fn main() {}\nlet x = 1;\n", true);
        assert!(v.plain);
        assert_eq!(v.files.len(), 1);
        assert_eq!(v.files[0].hunks[0].lines.len(), 2);
        assert!(v.files[0].hunks[0].lines.iter().all(|l| l.starts_with(' ')));
        // File header, hunk header, then the first line.
        assert_eq!(DiffView::file_line_row(1), 2);
        assert_eq!(v.find_rows("let", false), vec![DiffView::file_line_row(2)]);
    }
}
