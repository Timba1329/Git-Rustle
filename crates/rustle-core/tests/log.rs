use rustle_core::{CliBackend, GitError, GitRunner, LogQuery, RefKind};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let runner = GitRunner::discover().expect("git is required for the tests");
    let mut full = vec![
        "-c",
        "user.name=Test",
        "-c",
        "user.email=t@example.com",
        "-c",
        "commit.gpgsign=false",
    ];
    full.extend_from_slice(args);
    runner.run(&full, Some(dir)).unwrap();
}

fn repo_with(messages: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    for m in messages {
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", m]);
    }
    dir
}

fn open(dir: &tempfile::TempDir) -> CliBackend {
    CliBackend::open(GitRunner::discover().unwrap(), dir.path()).unwrap()
}

#[test]
fn log_returns_commits_newest_first() {
    let dir = repo_with(&["een", "twee", "drie"]);
    let b = open(&dir);
    let log = b.log(100, 0).unwrap();
    let subjects: Vec<_> = log.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["drie", "twee", "een"]);
    assert_eq!(log[0].parents.len(), 1);
    assert!(log[2].parents.is_empty());
    assert_eq!(b.log(1, 1).unwrap()[0].subject, "twee");
}

#[test]
fn log_has_ref_labels() {
    let dir = repo_with(&["een", "twee"]);
    git(dir.path(), &["tag", "v1"]);
    git(dir.path(), &["branch", "zijtak", "HEAD~1"]);
    let log = open(&dir).log(10, 0).unwrap();
    let top = &log[0].refs;
    assert!(top.iter().any(|r| r.is_head && r.name == "main"));
    assert!(top.iter().any(|r| r.kind == RefKind::Tag && r.name == "v1"));
    assert!(log[1].refs.iter().any(|r| r.name == "zijtak"));
}

#[test]
fn log_includes_other_branches() {
    let dir = repo_with(&["basis"]);
    git(dir.path(), &["switch", "-q", "-c", "feat"]);
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "op feat"]);
    git(dir.path(), &["switch", "-q", "main"]);
    let log = open(&dir).log(10, 0).unwrap();
    assert!(log.iter().any(|c| c.subject == "op feat"));
}

#[test]
fn unicode_subject_survives() {
    let dir = repo_with(&["café 🦀"]);
    assert_eq!(open(&dir).log(10, 0).unwrap()[0].subject, "café 🦀");
}

#[test]
fn empty_repo_gives_empty_log() {
    let dir = repo_with(&[]);
    assert!(open(&dir).log(10, 0).unwrap().is_empty());
}

#[test]
fn non_repo_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let r = CliBackend::open(GitRunner::discover().unwrap(), dir.path());
    assert!(matches!(r, Err(GitError::NotARepo(_))));
}

#[test]
fn log_query_filters() {
    let dir = repo_with(&["alpha fix", "beta feature", "alpha again"]);
    let b = open(&dir);
    let q = LogQuery {
        text: Some("ALPHA".into()),
        ..LogQuery::all(10)
    };
    let r = b.log_query(&q).unwrap();
    assert_eq!(r.len(), 2);
    assert!(q.is_filtered());
    let q = LogQuery {
        author: Some("nobody".into()),
        ..LogQuery::all(10)
    };
    assert!(b.log_query(&q).unwrap().is_empty());
}
