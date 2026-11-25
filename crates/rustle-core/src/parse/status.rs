use crate::error::{GitError, Result};
use crate::model::{FileChange, Status};

fn lossy(b: &[u8]) -> String {
    crate::pathenc::decode_path(b)
}

fn bad(line: &str) -> GitError {
    GitError::Parse(format!("invalid status line: {line}"))
}

fn xy(s: &str) -> Option<(char, char)> {
    let mut c = s.chars();
    Some((c.next()?, c.next()?))
}

/// Parset `git status --porcelain=v2 --branch -z`.
pub fn parse_status(out: &[u8]) -> Result<Status> {
    let mut st = Status::default();
    let mut it = out.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = it.next() {
        let s = lossy(rec);
        if let Some(h) = s.strip_prefix("# ") {
            if let Some(v) = h.strip_prefix("branch.head ") {
                if v != "(detached)" {
                    st.branch = Some(v.to_string());
                }
            } else if let Some(v) = h.strip_prefix("branch.upstream ") {
                st.upstream = Some(v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                for part in v.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        match s.as_bytes().first() {
            Some(b'1') => {
                let p: Vec<&str> = s.splitn(9, ' ').collect();
                if p.len() != 9 {
                    return Err(bad(&s));
                }
                let (index, worktree) = xy(p[1]).ok_or_else(|| bad(&s))?;
                st.files.push(FileChange {
                    path: p[8].to_string(),
                    orig_path: None,
                    index,
                    worktree,
                    conflicted: false,
                });
            }
            Some(b'2') => {
                let p: Vec<&str> = s.splitn(10, ' ').collect();
                if p.len() != 10 {
                    return Err(bad(&s));
                }
                let (index, worktree) = xy(p[1]).ok_or_else(|| bad(&s))?;
                let orig = it.next().map(lossy);
                st.files.push(FileChange {
                    path: p[9].to_string(),
                    orig_path: orig,
                    index,
                    worktree,
                    conflicted: false,
                });
            }
            Some(b'u') => {
                let p: Vec<&str> = s.splitn(11, ' ').collect();
                if p.len() != 11 {
                    return Err(bad(&s));
                }
                let (index, worktree) = xy(p[1]).ok_or_else(|| bad(&s))?;
                st.files.push(FileChange {
                    path: p[10].to_string(),
                    orig_path: None,
                    index,
                    worktree,
                    conflicted: true,
                });
            }
            Some(b'?') => st.files.push(FileChange {
                path: s[2..].to_string(),
                orig_path: None,
                index: '?',
                worktree: '?',
                conflicted: false,
            }),
            _ => {}
        }
    }
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_status() {
        let h = "0000000000000000000000000000000000000000";
        let input = format!(
            "# branch.oid {h}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
             1 M. N... 100644 100644 100644 {h} {h} src/a.rs\0\
             1 .M N... 100644 100644 100644 {h} {h} dir/spatie bestand.txt\0\
             2 R. N... 100644 100644 100644 {h} {h} R100 nieuw.txt\0oud.txt\0\
             u UU N... 100644 100644 100644 100644 {h} {h} {h} conflict.txt\0\
             ? los.txt\0"
        );
        let st = parse_status(input.as_bytes()).unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.files.len(), 5);
        assert!(st.files[0].is_staged() && !st.files[0].is_unstaged());
        assert!(!st.files[1].is_staged() && st.files[1].is_unstaged());
        assert_eq!(st.files[1].path, "dir/spatie bestand.txt");
        assert_eq!(st.files[2].orig_path.as_deref(), Some("oud.txt"));
        assert!(st.files[3].conflicted);
        assert!(st.files[4].is_untracked() && st.files[4].is_unstaged());
    }

    #[test]
    fn detached_head() {
        let st = parse_status(b"# branch.head (detached)\0").unwrap();
        assert_eq!(st.branch, None);
    }
}
