use rustle_core::{CliBackend, GitRunner, LogQuery};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = GitRunner::discover().unwrap().run(args, Some(dir)).unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(dir: &Path, name: &str, content: &str, msg: &str) {
    std::fs::write(dir.join(name), content).unwrap();
    git(dir, &["add", "--", name]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn open(dir: &Path) -> CliBackend {
    CliBackend::open(GitRunner::discover().unwrap(), dir).unwrap()
}

#[test]
fn merged_branches_lists_only_merged_unprotected_ones() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "a\n", "base");
    git(d.path(), &["branch", "Tickets/done"]);
    git(d.path(), &["checkout", "-q", "-b", "Tickets/open"]);
    commit_file(d.path(), "b.txt", "b\n", "unmerged work");
    git(d.path(), &["checkout", "-q", "main"]);

    let b = open(d.path());
    let merged = b.merged_branches(false).unwrap();
    assert_eq!(merged, ["Tickets/done"]);

    let (ok, failed) = b.delete_branches(&merged, false);
    assert_eq!((ok, failed.len()), (1, 0));
    assert!(b.merged_branches(false).unwrap().is_empty());
}

#[test]
fn merge_preview_reports_conflicting_files() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "f.txt", "base\n", "base");
    git(d.path(), &["checkout", "-q", "-b", "feature"]);
    commit_file(d.path(), "f.txt", "feature\n", "feature change");
    git(d.path(), &["checkout", "-q", "main"]);
    commit_file(d.path(), "f.txt", "main\n", "main change");
    git(d.path(), &["branch", "clean"]);

    let b = open(d.path());
    assert_eq!(b.merge_preview("feature").unwrap(), ["f.txt"]);
    assert!(b.merge_preview("clean").unwrap().is_empty());
}

#[test]
fn undo_last_moves_back_after_a_commit_and_after_a_checkout() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "first");
    commit_file(d.path(), "a.txt", "2\n", "second");
    let b = open(d.path());

    b.undo_last().unwrap();
    let subjects: Vec<String> = b
        .log_query(&LogQuery {
            current_branch_only: true,
            ..LogQuery::all(10)
        })
        .unwrap()
        .into_iter()
        .map(|c| c.subject)
        .collect();
    assert_eq!(subjects, ["first"]);

    git(d.path(), &["checkout", "-q", "-b", "other"]);
    b.undo_last().unwrap();
    assert_eq!(git(d.path(), &["branch", "--show-current"]).trim(), "main");
}

#[test]
fn pull_request_url_for_github_and_gitlab_remotes() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "first");
    let b = open(d.path());

    git(
        d.path(),
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    );
    assert_eq!(
        b.pull_request_url("Tickets/I2C-1").unwrap(),
        "https://github.com/owner/repo/compare/Tickets/I2C-1?expand=1"
    );

    git(
        d.path(),
        &["remote", "set-url", "origin", "https://gitlab.com/group/proj.git"],
    );
    assert_eq!(
        b.pull_request_url("x").unwrap(),
        "https://gitlab.com/group/proj/-/merge_requests/new?merge_request[source_branch]=x"
    );

    git(
        d.path(),
        &["remote", "set-url", "origin", "https://example.org/a/b.git"],
    );
    assert!(b.pull_request_url("x").is_err());
}

#[test]
fn log_can_be_limited_to_chosen_branches() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "base");
    git(d.path(), &["checkout", "-q", "-b", "side"]);
    commit_file(d.path(), "b.txt", "1\n", "side work");
    git(d.path(), &["checkout", "-q", "main"]);
    commit_file(d.path(), "c.txt", "1\n", "main work");

    let b = open(d.path());
    let q = LogQuery {
        revs: vec!["side".into()],
        ..LogQuery::all(10)
    };
    let subjects: Vec<String> = b.log_query(&q).unwrap().into_iter().map(|c| c.subject).collect();
    assert_eq!(subjects, ["side work", "base"]);
}
