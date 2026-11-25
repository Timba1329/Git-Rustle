//! Parsing of unified diffs, building (partial) patches for staging/discarding hunks and lines,
//! and helpers for side-by-side and intra-line rendering. Pure functions, no git calls.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The full `@@ -a,b +c,d @@ section` line.
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    /// Section text after the second `@@` (function name etc.).
    pub section: String,
    /// Body lines including their prefix character (` `, `+`, `-` or `\`).
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    /// Header lines (`diff --git`, `index`, `---`, `+++`, ...).
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

impl FileDiff {
    /// Best-effort path of the new file, from the `+++` / `diff --git` line.
    pub fn path(&self) -> Option<String> {
        for l in &self.header {
            if let Some(p) = l.strip_prefix("+++ b/") {
                return Some(p.to_string());
            }
        }
        for l in &self.header {
            if let Some(p) = l.strip_prefix("--- a/") {
                return Some(p.to_string());
            }
        }
        self.header
            .first()
            .and_then(|l| l.rsplit_once(" b/"))
            .map(|(_, p)| p.to_string())
    }
}

fn parse_range(s: &str) -> Option<u32> {
    // "12,3" or "12"
    s.split(',').next()?.parse().ok()
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    Some((parse_range(old)?, parse_range(new)?, section.trim().to_string()))
}

/// Parses the output of `git diff` / `git show` (possibly several files).
pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            files.push(FileDiff {
                header: vec![line.to_string()],
                ..Default::default()
            });
            continue;
        }
        let Some(file) = files.last_mut() else { continue };
        if line.starts_with("@@ ")
            && let Some((old_start, new_start, section)) = parse_hunk_header(line)
        {
            file.hunks.push(Hunk {
                header: line.to_string(),
                old_start,
                new_start,
                section,
                lines: Vec::new(),
            });
            continue;
        }
        match file.hunks.last_mut() {
            Some(h) => h.lines.push(line.to_string()),
            None => {
                if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                    file.binary = true;
                }
                file.header.push(line.to_string());
            }
        }
    }
    files
}

impl Hunk {
    /// Old/new line numbers for every body line.
    pub fn line_numbers(&self) -> Vec<(Option<u32>, Option<u32>)> {
        let (mut o, mut n) = (self.old_start, self.new_start);
        self.lines
            .iter()
            .map(|l| match l.as_bytes().first() {
                Some(b'+') => {
                    n += 1;
                    (None, Some(n - 1))
                }
                Some(b'-') => {
                    o += 1;
                    (Some(o - 1), None)
                }
                Some(b'\\') => (None, None),
                _ => {
                    o += 1;
                    n += 1;
                    (Some(o - 1), Some(n - 1))
                }
            })
            .collect()
    }

    /// Whether line `i` is an added or removed line (selectable for line staging).
    pub fn is_change(&self, i: usize) -> bool {
        matches!(
            self.lines.get(i).and_then(|l| l.as_bytes().first()),
            Some(b'+' | b'-')
        )
    }
}

/// Builds a patch containing one hunk of `file`, optionally restricted to the selected body
/// lines (indices into `hunk.lines`).
///
/// `reverse` describes how the patch will be applied: `false` for staging (apply forward to the
/// index), `true` for unstaging or discarding (apply with `--reverse`). Unselected lines are kept
/// as context or dropped so that applying the patch only touches the selected lines.
///
/// Returns `None` when nothing selectable remains.
pub fn build_patch(
    file: &FileDiff,
    hunk: usize,
    selected: Option<&HashSet<usize>>,
    reverse: bool,
) -> Option<String> {
    let h = file.hunks.get(hunk)?;
    let mut body: Vec<String> = Vec::new();
    let mut last_kept = false;
    let (mut old_count, mut new_count) = (0u32, 0u32);
    let mut any_change = false;

    for (i, l) in h.lines.iter().enumerate() {
        let c = l.as_bytes().first().copied().unwrap_or(b' ');
        let text = l.get(1..).unwrap_or("");
        match c {
            b'\\' => {
                if last_kept {
                    body.push(l.clone());
                }
            }
            b'+' | b'-' => {
                let keep = selected.is_none_or(|s| s.contains(&i));
                if keep {
                    any_change = true;
                    body.push(l.clone());
                    if c == b'+' {
                        new_count += 1;
                    } else {
                        old_count += 1;
                    }
                    last_kept = true;
                } else {
                    // Turn into context or drop, depending on how the patch is applied.
                    let as_context = (c == b'-') != reverse;
                    if as_context {
                        body.push(format!(" {text}"));
                        old_count += 1;
                        new_count += 1;
                        last_kept = true;
                    } else {
                        last_kept = false;
                    }
                }
            }
            _ => {
                body.push(l.clone());
                old_count += 1;
                new_count += 1;
                last_kept = true;
            }
        }
    }
    if !any_change {
        return None;
    }

    let mut out = String::new();
    for l in &file.header {
        out.push_str(l);
        out.push('\n');
    }
    let section = if h.section.is_empty() {
        String::new()
    } else {
        format!(" {}", h.section)
    };
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@{}\n",
        h.old_start, old_count, h.new_start, new_count, section
    ));
    for l in body {
        out.push_str(&l);
        out.push('\n');
    }
    Some(out)
}

/// Char ranges (start, end) that differ between two lines, via common prefix and suffix.
pub fn intra_change(old: &str, new: &str) -> ((usize, usize), (usize, usize)) {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    ((prefix, a.len() - suffix), (prefix, b.len() - suffix))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbsCell {
    pub number: Option<u32>,
    pub text: String,
    pub kind: LineKind,
    /// Index into `Hunk::lines`.
    pub line_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SbsRow {
    pub left: Option<SbsCell>,
    pub right: Option<SbsCell>,
}

/// Lays a hunk out in two columns: removed lines left, added lines right, paired in order.
pub fn side_by_side(h: &Hunk) -> Vec<SbsRow> {
    let nums = h.line_numbers();
    let mut rows = Vec::new();
    let mut removed: Vec<SbsCell> = Vec::new();
    let mut added: Vec<SbsCell> = Vec::new();

    let flush = |rows: &mut Vec<SbsRow>, removed: &mut Vec<SbsCell>, added: &mut Vec<SbsCell>| {
        let n = removed.len().max(added.len());
        let mut r = std::mem::take(removed).into_iter();
        let mut a = std::mem::take(added).into_iter();
        for _ in 0..n {
            rows.push(SbsRow {
                left: r.next(),
                right: a.next(),
            });
        }
    };

    for (i, l) in h.lines.iter().enumerate() {
        let text = l.get(1..).unwrap_or("").to_string();
        match l.as_bytes().first() {
            Some(b'-') => removed.push(SbsCell {
                number: nums[i].0,
                text,
                kind: LineKind::Removed,
                line_index: i,
            }),
            Some(b'+') => added.push(SbsCell {
                number: nums[i].1,
                text,
                kind: LineKind::Added,
                line_index: i,
            }),
            Some(b'\\') => {}
            _ => {
                flush(&mut rows, &mut removed, &mut added);
                rows.push(SbsRow {
                    left: Some(SbsCell {
                        number: nums[i].0,
                        text: text.clone(),
                        kind: LineKind::Context,
                        line_index: i,
                    }),
                    right: Some(SbsCell {
                        number: nums[i].1,
                        text,
                        kind: LineKind::Context,
                        line_index: i,
                    }),
                });
            }
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/f.txt b/f.txt
index 111..222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,5 @@ fn x
 a
-b
+B
+B2
 c
 d
@@ -10,2 +11,2 @@
 x
-y
+z
";

    #[test]
    fn parses_hunks_and_numbers() {
        let f = &parse_diff(DIFF)[0];
        assert_eq!(f.path().as_deref(), Some("f.txt"));
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].section, "fn x");
        assert_eq!(f.hunks[1].old_start, 10);
        let n = f.hunks[0].line_numbers();
        assert_eq!(n[0], (Some(1), Some(1)));
        assert_eq!(n[1], (Some(2), None));
        assert_eq!(n[2], (None, Some(2)));
        assert_eq!(n[3], (None, Some(3)));
        assert_eq!(n[4], (Some(3), Some(4)));
    }

    #[test]
    fn whole_hunk_patch_recounts() {
        let f = &parse_diff(DIFF)[0];
        let p = build_patch(f, 1, None, false).unwrap();
        assert!(p.contains("@@ -10,2 +11,2 @@\n"));
        assert!(p.starts_with("diff --git"));
        assert!(!p.contains("fn x"));
    }

    #[test]
    fn partial_stage_drops_unselected_adds_and_keeps_unselected_removes() {
        let f = &parse_diff(DIFF)[0];
        // Select only the second added line "+B2" (index 3).
        let sel: HashSet<usize> = [3].into_iter().collect();
        let p = build_patch(f, 0, Some(&sel), false).unwrap();
        // "-b" becomes context, "+B" is dropped, "+B2" is kept.
        assert!(p.contains("\n b\n"));
        assert!(!p.contains("\n+B\n"));
        assert!(p.contains("\n+B2\n"));
        assert!(p.contains("@@ -1,4 +1,5 @@ fn x"));
    }

    #[test]
    fn partial_unstage_keeps_unselected_adds_as_context() {
        let f = &parse_diff(DIFF)[0];
        let sel: HashSet<usize> = [1].into_iter().collect(); // only "-b"
        let p = build_patch(f, 0, Some(&sel), true).unwrap();
        assert!(p.contains("\n-b\n"));
        assert!(p.contains("\n B\n"));
        assert!(p.contains("\n B2\n"));
    }

    #[test]
    fn nothing_selected_gives_none() {
        let f = &parse_diff(DIFF)[0];
        let sel: HashSet<usize> = HashSet::new();
        assert!(build_patch(f, 0, Some(&sel), false).is_none());
    }

    #[test]
    fn intra_line_ranges() {
        let ((a0, a1), (b0, b1)) = intra_change("let x = 10;", "let x = 25;");
        assert_eq!((a0, a1), (8, 10));
        assert_eq!((b0, b1), (8, 10));
        let ((a0, a1), (b0, b1)) = intra_change("same", "same");
        assert_eq!((a0, a1, b0, b1), (4, 4, 4, 4));
    }

    #[test]
    fn side_by_side_pairs_changes() {
        let f = &parse_diff(DIFF)[0];
        let rows = side_by_side(&f.hunks[0]);
        // context a, (-b | +B), (· | +B2), context c, context d
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1].left.as_ref().unwrap().text, "b");
        assert_eq!(rows[1].right.as_ref().unwrap().text, "B");
        assert!(rows[2].left.is_none());
        assert_eq!(rows[2].right.as_ref().unwrap().text, "B2");
    }

    #[test]
    fn binary_is_detected() {
        let t = "diff --git a/i.png b/i.png\nindex 1..2 100644\nBinary files a/i.png and b/i.png differ\n";
        assert!(parse_diff(t)[0].binary);
    }
}
