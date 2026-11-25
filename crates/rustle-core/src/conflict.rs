//! Parsing and resolving files with merge-conflict markers (standard and diff3 style).

/// One conflicting region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictBlock {
    pub ours: String,
    /// Common ancestor; only present in diff3 style markers.
    pub base: Option<String>,
    pub theirs: String,
    pub ours_label: String,
    pub theirs_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Conflict(ConflictBlock),
}

/// How a conflict block is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    /// Ours followed by theirs.
    Both,
    Base,
    Custom(String),
}

fn marker<'a>(line: &'a str, m: &str) -> Option<&'a str> {
    let l = line.trim_end_matches(['\r', '\n']);
    if l == m {
        Some("")
    } else {
        l.strip_prefix(m).and_then(|r| r.strip_prefix(' '))
    }
}

/// Splits `text` into plain text and conflict blocks. An unterminated conflict is kept as text.
pub fn parse_conflicts(text: &str) -> Vec<Segment> {
    #[derive(PartialEq)]
    enum St {
        Text,
        Ours,
        Base,
        Theirs,
    }
    let mut segs: Vec<Segment> = Vec::new();
    let mut plain = String::new();
    let mut st = St::Text;
    let (mut ours, mut base, mut theirs) = (String::new(), String::new(), String::new());
    let (mut has_base, mut ours_label, mut raw) = (false, String::new(), String::new());

    for line in text.split_inclusive('\n') {
        match st {
            St::Text => {
                if let Some(label) = marker(line, "<<<<<<<") {
                    if !plain.is_empty() {
                        segs.push(Segment::Text(std::mem::take(&mut plain)));
                    }
                    ours_label = label.to_string();
                    raw = line.to_string();
                    ours.clear();
                    base.clear();
                    theirs.clear();
                    has_base = false;
                    st = St::Ours;
                } else {
                    plain.push_str(line);
                }
            }
            St::Ours | St::Base | St::Theirs => {
                raw.push_str(line);
                if st == St::Ours && marker(line, "|||||||").is_some() {
                    has_base = true;
                    st = St::Base;
                } else if st != St::Theirs && marker(line, "=======").is_some() {
                    st = St::Theirs;
                } else if st == St::Theirs && marker(line, ">>>>>>>").is_some() {
                    segs.push(Segment::Conflict(ConflictBlock {
                        ours: std::mem::take(&mut ours),
                        base: has_base.then(|| std::mem::take(&mut base)),
                        theirs: std::mem::take(&mut theirs),
                        ours_label: std::mem::take(&mut ours_label),
                        theirs_label: marker(line, ">>>>>>>").unwrap_or("").to_string(),
                    }));
                    st = St::Text;
                } else {
                    match st {
                        St::Ours => ours.push_str(line),
                        St::Base => base.push_str(line),
                        _ => theirs.push_str(line),
                    }
                }
            }
        }
    }
    if st != St::Text {
        // Unterminated: give the raw text back unchanged.
        plain.push_str(&raw);
    }
    if !plain.is_empty() {
        segs.push(Segment::Text(plain));
    }
    segs
}

/// Number of conflict blocks.
pub fn conflict_count(segs: &[Segment]) -> usize {
    segs.iter().filter(|s| matches!(s, Segment::Conflict(_))).count()
}

/// Builds the resolved text; `choices` has one entry per conflict block, in order. Blocks without a
/// choice keep their markers so nothing is lost.
pub fn resolve(segs: &[Segment], choices: &[Option<Choice>]) -> String {
    let mut out = String::new();
    let mut i = 0;
    for s in segs {
        match s {
            Segment::Text(t) => out.push_str(t),
            Segment::Conflict(b) => {
                match choices.get(i).and_then(Option::as_ref) {
                    Some(Choice::Ours) => out.push_str(&b.ours),
                    Some(Choice::Theirs) => out.push_str(&b.theirs),
                    Some(Choice::Both) => {
                        out.push_str(&b.ours);
                        out.push_str(&b.theirs);
                    }
                    Some(Choice::Base) => out.push_str(b.base.as_deref().unwrap_or("")),
                    Some(Choice::Custom(t)) => out.push_str(t),
                    None => {
                        out.push_str(&format!("<<<<<<< {}\n{}", b.ours_label, b.ours));
                        if let Some(base) = &b.base {
                            out.push_str(&format!("||||||| base\n{base}"));
                        }
                        out.push_str(&format!("=======\n{}>>>>>>> {}\n", b.theirs, b.theirs_label));
                    }
                }
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF3: &str = "top\n<<<<<<< ours\nX\n||||||| base\nb\n=======\nY\n>>>>>>> theirs\nbottom\n";

    #[test]
    fn parses_diff3_blocks() {
        let s = parse_conflicts(DIFF3);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], Segment::Text("top\n".into()));
        let Segment::Conflict(b) = &s[1] else { panic!() };
        assert_eq!((b.ours.as_str(), b.theirs.as_str()), ("X\n", "Y\n"));
        assert_eq!(b.base.as_deref(), Some("b\n"));
        assert_eq!(
            (b.ours_label.as_str(), b.theirs_label.as_str()),
            ("ours", "theirs")
        );
        assert_eq!(s[2], Segment::Text("bottom\n".into()));
    }

    #[test]
    fn parses_standard_blocks_without_base() {
        let s = parse_conflicts("<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> feature\n");
        let Segment::Conflict(b) = &s[0] else { panic!() };
        assert!(b.base.is_none());
        assert_eq!(b.theirs_label, "feature");
        assert_eq!(conflict_count(&s), 1);
    }

    #[test]
    fn resolves_each_choice() {
        let s = parse_conflicts(DIFF3);
        let r = |c| resolve(&s, &[Some(c)]);
        assert_eq!(r(Choice::Ours), "top\nX\nbottom\n");
        assert_eq!(r(Choice::Theirs), "top\nY\nbottom\n");
        assert_eq!(r(Choice::Both), "top\nX\nY\nbottom\n");
        assert_eq!(r(Choice::Base), "top\nb\nbottom\n");
        assert_eq!(r(Choice::Custom("mine\n".into())), "top\nmine\nbottom\n");
    }

    #[test]
    fn unresolved_blocks_keep_their_markers() {
        let s = parse_conflicts(DIFF3);
        let out = resolve(&s, &[None]);
        assert_eq!(out, DIFF3);
        assert_eq!(parse_conflicts(&out), s);
    }

    #[test]
    fn handles_multiple_blocks_and_crlf() {
        let t = "a\r\n<<<<<<< o\r\n1\r\n=======\r\n2\r\n>>>>>>> t\r\nmid\r\n<<<<<<< o\r\n3\r\n=======\r\n4\r\n>>>>>>> t\r\n";
        let s = parse_conflicts(t);
        assert_eq!(conflict_count(&s), 2);
        assert_eq!(
            resolve(&s, &[Some(Choice::Ours), Some(Choice::Theirs)]),
            "a\r\n1\r\nmid\r\n4\r\n"
        );
    }

    #[test]
    fn unterminated_conflict_is_returned_as_text() {
        let t = "x\n<<<<<<< ours\nA\n=======\nB\n";
        let s = parse_conflicts(t);
        assert_eq!(conflict_count(&s), 0);
        assert_eq!(resolve(&s, &[]), t);
    }
}
