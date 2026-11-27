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

#[test]
fn fixup_folds_staged_changes_into_an_older_commit() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "first");
    commit_file(d.path(), "b.txt", "1\n", "second");
    let first = git(d.path(), &["rev-parse", "HEAD~1"]).trim().to_string();

    std::fs::write(d.path().join("a.txt"), "1\nmore\n").unwrap();
    git(d.path(), &["add", "a.txt"]);
    let b = open(d.path());
    assert_eq!(b.fixup_into(&first).unwrap(), rustle_core::OpResult::Done);

    let subjects = git(d.path(), &["log", "--format=%s"]);
    assert_eq!(subjects.lines().collect::<Vec<_>>(), ["second", "first"]);
    let content = git(d.path(), &["show", "HEAD~1:a.txt"]);
    assert_eq!(content, "1\nmore\n");
}

#[test]
fn bisect_finds_the_first_bad_commit() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "f.txt", "ok\n", "c1");
    let good = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    commit_file(d.path(), "f.txt", "ok\nok\n", "c2");
    commit_file(d.path(), "f.txt", "bad\n", "c3 breaks it");
    commit_file(d.path(), "g.txt", "x\n", "c4");
    let bad = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();

    let b = open(d.path());
    let mut out = b.bisect_start(&bad, &good).unwrap();
    for _ in 0..4 {
        if out.contains("is the first bad commit") {
            break;
        }
        let broken = std::fs::read_to_string(d.path().join("f.txt")).unwrap() == "bad\n";
        out = b.bisect_mark(if broken { "bad" } else { "good" }).unwrap();
    }
    assert!(out.contains("c3 breaks it"), "{out}");
    b.bisect_reset().unwrap();
}

#[test]
fn tag_message_untrack_patch_stash_and_containing() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "first");
    git(d.path(), &["tag", "-a", "v1", "-m", "release one"]);
    let b = open(d.path());
    assert_eq!(b.tag_message("v1").unwrap(), "release one");

    // patch round trip
    let head = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
    let patch = d.path().join("out.patch");
    b.export_patch(&head, &patch).unwrap();
    assert!(
        std::fs::read_to_string(&patch)
            .unwrap()
            .contains("Subject: [PATCH] first")
    );

    // branches containing the commit
    assert_eq!(b.containing_branches(&head).unwrap(), ["main"]);

    // untrack keeps the file
    commit_file(d.path(), "secret.env", "x\n", "oops");
    b.untrack(&["secret.env".to_string()]).unwrap();
    assert!(d.path().join("secret.env").exists());
    assert!(git(d.path(), &["status", "--porcelain"]).contains("D  secret.env"));
    git(d.path(), &["reset", "-q", "--hard"]);

    // stash preview
    std::fs::write(d.path().join("a.txt"), "1\nstashed\n").unwrap();
    git(d.path(), &["stash", "push", "-q"]);
    assert!(b.stash_show(0).unwrap().contains("+stashed"));
}

#[test]
fn dashboard_reports_stale_branches_and_big_files() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "small.txt", "x\n", "base");
    std::fs::write(d.path().join("big.bin"), vec![b'a'; 5000]).unwrap();
    git(d.path(), &["add", "big.bin"]);
    git(d.path(), &["commit", "-q", "-m", "big"]);
    git(d.path(), &["branch", "old"]);

    let b = open(d.path());
    // stale_days = -1 makes every branch "older" than the cutoff
    let dash = b.dashboard(-1).unwrap();
    assert_eq!(dash.commits, 2);
    assert_eq!(dash.tracked_files, 2);
    assert_eq!(dash.big_files[0].0, "big.bin");
    assert!(dash.stale.iter().any(|s| s.name == "old" && s.merged));
    assert!(dash.stale.iter().all(|s| s.name != "main"));
}

#[test]
fn log_can_be_limited_by_date() {
    let d = tempfile::tempdir().unwrap();
    init(d.path());
    commit_file(d.path(), "a.txt", "1\n", "now");
    let b = open(d.path());
    let q = LogQuery {
        until: Some("2000-01-01".into()),
        ..LogQuery::all(10)
    };
    assert!(b.log_query(&q).unwrap().is_empty());
    let q = LogQuery {
        since: Some("2000-01-01".into()),
        ..LogQuery::all(10)
    };
    assert_eq!(b.log_query(&q).unwrap().len(), 1);
}
