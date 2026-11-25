use crate::error::{GitError, Result};
use crate::model::{Commit, RefKind, RefLabel};

/// Format string for `git log`; belongs with [`parse_log`]. Use together with `--decorate=full -z`.
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D";

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Parses the `%D` decoration (with `--decorate=full`), e.g.
/// `HEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main`.
pub fn parse_refs(s: &str) -> Vec<RefLabel> {
    let mut v = Vec::new();
    for part in s.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        let label = if let Some(r) = part.strip_prefix("HEAD -> ") {
            RefLabel {
                name: r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
                kind: RefKind::Local,
                is_head: true,
            }
        } else if part == "HEAD" {
            RefLabel {
                name: "HEAD".into(),
                kind: RefKind::Local,
                is_head: true,
            }
        } else if let Some(t) = part.strip_prefix("tag: ") {
            RefLabel {
                name: t.strip_prefix("refs/tags/").unwrap_or(t).to_string(),
                kind: RefKind::Tag,
                is_head: false,
            }
        } else if let Some(r) = part.strip_prefix("refs/heads/") {
            RefLabel {
                name: r.to_string(),
                kind: RefKind::Local,
                is_head: false,
            }
        } else if let Some(r) = part.strip_prefix("refs/remotes/") {
            if r.ends_with("/HEAD") {
                continue;
            }
            RefLabel {
                name: r.to_string(),
                kind: RefKind::Remote,
                is_head: false,
            }
        } else {
            continue;
        };
        v.push(label);
    }
    v
}

/// Parses the output of `git log -z <LOG_FORMAT>` (records separated by NUL).
pub fn parse_log(out: &[u8]) -> Result<Vec<Commit>> {
    out.split(|&b| b == 0)
        .filter(|r| !r.iter().all(u8::is_ascii_whitespace))
        .map(|rec| {
            let f: Vec<&[u8]> = rec.split(|&b| b == 0x1f).collect();
            if f.len() != 7 {
                return Err(GitError::Parse(format!("{} fields instead of 7", f.len())));
            }
            let time = lossy(f[4])
                .trim()
                .parse()
                .map_err(|_| GitError::Parse("invalid timestamp".into()))?;
            Ok(Commit {
                id: lossy(f[0]).trim().to_string(),
                parents: lossy(f[1]).split_whitespace().map(String::from).collect(),
                author_name: lossy(f[2]),
                author_email: lossy(f[3]),
                time,
                subject: lossy(f[5]),
                refs: parse_refs(&lossy(f[6])),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records() {
        let input = b"aaa\x1fbbb ccc\x1fAnn\x1fa@x.nl\x1f100\x1fEerste\x1fHEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main, refs/remotes/origin/HEAD\0bbb\x1f\x1fBob\x1fb@x.nl\x1f50\x1fRoot \xff\x1f\0";
        let c = parse_log(input).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].parents, vec!["bbb", "ccc"]);
        assert_eq!(c[0].time, 100);
        assert_eq!(c[0].refs.len(), 3);
        assert!(c[0].refs[0].is_head && c[0].refs[0].name == "main");
        assert_eq!(c[0].refs[1].kind, RefKind::Tag);
        assert_eq!(c[0].refs[2].name, "origin/main");
        assert!(c[1].parents.is_empty());
        assert!(c[1].subject.starts_with("Root"));
        assert!(c[1].refs.is_empty());
    }

    #[test]
    fn empty_is_ok() {
        assert!(parse_log(b"").unwrap().is_empty());
    }

    #[test]
    fn bad_record_errors() {
        assert!(parse_log(b"a\x1fb").is_err());
    }
}
