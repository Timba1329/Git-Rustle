use rustle_core::conflict::{Choice, conflict_count, parse_conflicts, resolve};
use rustle_core::{
    CliBackend, GitError, GitRunner, LogQuery, OpResult, Side, TodoAction, TodoItem, build_graph,
};
use std::path::Path;
use std::process::Command;

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

fn write(dir: &Path, name: &str, content: &str) {
    if let Some(parent) = dir.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(dir.join(name), content).unwrap();
}

fn commit_file(dir: &Path, name: &str, content: &str, msg: &str) {
    write(dir, name, content);
    git(dir, &["add", "--", name]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn open(dir: &Path) -> CliBackend {
    CliBackend::open(GitRunner::discover().unwrap(), dir).unwrap()
}

fn head_subjects(b: &CliBackend) -> Vec<String> {
    let q = LogQuery {
        current_branch_only: true,
        ..LogQuery::all(100)
    };
    b.log_query(&q).unwrap().into_iter().map(|c| c.subject).collect()
}

/// A repository `a` with a bare `origin`.
fn with_remote(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let remote = root.join("remote.git");
    let a = root.join("a");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&a).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    init(&a);
    commit_file(&a, "f.txt", "1\n", "one");
    git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    (a, remote)
}

// ------------------------------------------------------------------ per-branch push / fast-forward

#[test]
fn push_branch_publishes_and_fast_forward_updates_other_branches() {
    let root = tempfile::tempdir().unwrap();
    let (a, remote) = with_remote(root.path());
    let b = open(&a);
    b.push(false, None).unwrap();

    // A new branch is published with -u.
    git(&a, &["switch", "-q", "-c", "feat"]);
    commit_file(&a, "g.txt", "g\n", "on feat");
    b.push_branch("feat", None).unwrap();
    let feat = b
        .branches()
        .unwrap()
        .into_iter()
        .find(|x| x.name == "feat")
        .unwrap();
    assert_eq!(feat.upstream.as_deref(), Some("origin/feat"));
    assert_eq!((feat.ahead, feat.behind), (0, 0));

    // A further local commit is pushed to the existing upstream.
    commit_file(&a, "h.txt", "h\n", "more feat");
    b.push_branch("feat", None).unwrap();
    assert_eq!(
        b.branches()
            .unwrap()
            .iter()
            .find(|x| x.name == "feat")
            .unwrap()
            .ahead,
        0
    );

    // Someone else advances feat on the remote; the local feat is not checked out and is fast-forwarded.
    let c = root.path().join("c");
    git(
        root.path(),
        &["clone", "-q", remote.to_str().unwrap(), c.to_str().unwrap()],
    );
    git(&c, &["config", "user.name", "Other"]);
    git(&c, &["config", "user.email", "o@example.com"]);
    git(&c, &["config", "commit.gpgsign", "false"]);
    git(&c, &["switch", "-q", "feat"]);
    commit_file(&c, "i.txt", "i\n", "from other");
    git(&c, &["push", "-q", "origin", "feat"]);

    git(&a, &["switch", "-q", "main"]);
    b.fetch(None).unwrap();
    let feat = b
        .branches()
        .unwrap()
        .into_iter()
        .find(|x| x.name == "feat")
        .unwrap();
    assert_eq!((feat.ahead, feat.behind), (0, 1));
    b.fast_forward_branch("feat", None).unwrap();
    let feat = b
        .branches()
        .unwrap()
        .into_iter()
        .find(|x| x.name == "feat")
        .unwrap();
    assert_eq!((feat.ahead, feat.behind), (0, 0));

    // Fast-forwarding the checked-out branch is refused instead of doing something odd.
    git(&a, &["switch", "-q", "feat"]);
    assert!(b.fast_forward_branch("feat", None).is_err());
}

// ------------------------------------------------------------------ hooks and signing

#[test]
fn commit_returns_hook_output_and_honours_no_verify() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "1\n", "init");
    let hook = d.join(".git").join("hooks").join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho 'checking style' >&2\nexit 0\n").unwrap();
    let b = open(d);

    write(d, "a.txt", "2\n");
    b.stage(&["a.txt".into()]).unwrap();
    let out = b.commit_with("second", false, None, false).unwrap();
    assert!(out.contains("checking style"), "{out:?}");

    // A failing hook blocks the commit and its message is part of the error.
    std::fs::write(&hook, "#!/bin/sh\necho 'lint failed: no TODOs' >&2\nexit 1\n").unwrap();
    write(d, "a.txt", "3\n");
    b.stage(&["a.txt".into()]).unwrap();
    let err = b.commit_with("third", false, None, false).unwrap_err();
    assert!(err.to_string().contains("lint failed"), "{err}");
    assert_eq!(head_subjects(&b)[0], "second");

    // --no-verify skips the hook.
    let out = b.commit_with("third", false, None, true).unwrap();
    assert!(!out.contains("lint failed"));
    assert_eq!(head_subjects(&b)[0], "third");
}

#[test]
fn ssh_signing_roundtrip() {
    // Needs ssh-keygen (ships with Windows 10+ and Git for Windows); skip quietly without it.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let key = d.join("id_test");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .output();
    if !made.is_ok_and(|o| o.status.success()) {
        eprintln!("ssh-keygen not available; skipping");
        return;
    }
    let repo = d.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);
    let fwd = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let pubkey = std::fs::read_to_string(format!("{}.pub", key.display())).unwrap();
    let signers = d.join("allowed_signers");
    std::fs::write(&signers, format!("t@example.com {}", pubkey.trim())).unwrap();
    git(&repo, &["config", "gpg.format", "ssh"]);
    git(&repo, &["config", "user.signingkey", &fwd(&key)]);
    git(&repo, &["config", "gpg.ssh.allowedSignersFile", &fwd(&signers)]);

    let b = open(&repo);
    assert!(!b.signing_default());
    write(&repo, "a.txt", "1\n");
    b.stage(&["a.txt".into()]).unwrap();
    b.commit_with("signed", false, Some(true), false).unwrap();
    let raw = git(&repo, &["cat-file", "-p", "HEAD"]);
    assert!(raw.contains("gpgsig"), "{raw}");
    let sig = b.signature("HEAD").unwrap();
    assert_eq!(sig.status, 'G', "{sig:?}");

    // Unsigned commits report 'N'.
    write(&repo, "a.txt", "2\n");
    b.stage(&["a.txt".into()]).unwrap();
    b.commit_with("plain", false, Some(false), false).unwrap();
    assert_eq!(b.signature("HEAD").unwrap().status, 'N');

    git(&repo, &["config", "commit.gpgsign", "true"]);
    assert!(b.signing_default());
}

// ------------------------------------------------------------------ remotes and configuration

#[test]
fn remotes_can_be_managed() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    let b = open(dir.path());
    assert!(b.remotes().unwrap().is_empty());

    b.add_remote("origin", "https://example.com/a.git").unwrap();
    b.add_remote("backup", "https://example.com/b.git").unwrap();
    let r = b.remotes().unwrap();
    assert_eq!(r.len(), 2);
    let origin = r.iter().find(|x| x.name == "origin").unwrap();
    assert_eq!(origin.fetch_url, "https://example.com/a.git");
    assert_eq!(origin.push_url, "https://example.com/a.git");

    b.set_remote_url("backup", "https://example.com/c.git").unwrap();
    b.rename_remote("backup", "mirror").unwrap();
    let r = b.remotes().unwrap();
    assert_eq!(
        r.iter().find(|x| x.name == "mirror").unwrap().fetch_url,
        "https://example.com/c.git"
    );
    assert!(b.add_remote("origin", "x").is_err());

    b.remove_remote("mirror").unwrap();
    assert_eq!(b.remotes().unwrap().len(), 1);
}

#[test]
fn local_config_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    let b = open(dir.path());
    let v = b.get_config("rustle.test-key");
    assert!(v.value.is_none() && v.scope.is_none());

    b.set_config("rustle.test-key", "hello world", false).unwrap();
    let v = b.get_config("rustle.test-key");
    assert_eq!(v.value.as_deref(), Some("hello world"));
    assert_eq!(v.scope.as_deref(), Some("local"));

    b.unset_config("rustle.test-key", false).unwrap();
    assert!(b.get_config("rustle.test-key").value.is_none());
    // Unsetting something that is not set is not an error.
    b.unset_config("rustle.test-key", false).unwrap();
}

// ------------------------------------------------------------------ several commits at once

#[test]
fn cherry_pick_and_revert_many() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "base.txt", "b\n", "base");
    git(d, &["switch", "-q", "-c", "side"]);
    commit_file(d, "s1.txt", "1\n", "side one");
    commit_file(d, "s2.txt", "2\n", "side two");
    git(d, &["switch", "-q", "main"]);
    let b = open(d);
    let side = b.log_query(&LogQuery::all(10)).unwrap();
    let id = |s: &str| side.iter().find(|c| c.subject == s).unwrap().id.clone();

    assert_eq!(
        b.cherry_pick_many(&[id("side one"), id("side two")]).unwrap(),
        OpResult::Done
    );
    assert_eq!(&head_subjects(&b)[..2], ["side two", "side one"]);

    assert_eq!(
        b.revert_many(&["HEAD".into(), "HEAD~1".into()]).unwrap(),
        OpResult::Done
    );
    assert!(!d.join("s1.txt").exists() && !d.join("s2.txt").exists());
    assert!(head_subjects(&b)[0].starts_with("Revert"));
}

// ------------------------------------------------------------------ search and file browsing

#[test]
fn grep_and_file_listing() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "hello world\nsecond\n", "one");
    commit_file(d, "sub/b.txt", "x\nHello again\n", "two");
    write(d, "a.txt", "hello world\nsecond\nhello unstaged\n");
    let b = open(d);

    let hits = b.grep("hello", None, true, 100).unwrap();
    let paths: Vec<_> = hits.iter().map(|h| (h.path.as_str(), h.line)).collect();
    assert!(
        paths.contains(&("a.txt", 1)) && paths.contains(&("a.txt", 3)) && paths.contains(&("sub/b.txt", 2)),
        "{paths:?}"
    );
    assert!(hits.iter().any(|h| h.text == "hello unstaged"));

    // Case sensitivity, a specific revision (no unstaged text) and "no matches".
    assert_eq!(b.grep("hello", None, false, 100).unwrap().len(), 2);
    let at_head = b.grep("hello", Some("HEAD"), true, 100).unwrap();
    assert_eq!(at_head.len(), 2);
    assert!(at_head.iter().all(|h| !h.path.starts_with("HEAD:")));
    assert!(b.grep("nothing-like-this", None, true, 100).unwrap().is_empty());
    assert_eq!(b.grep("hello", None, true, 1).unwrap().len(), 1);

    assert_eq!(b.ls_files().unwrap(), ["a.txt", "sub/b.txt"]);
    assert_eq!(b.ls_tree("HEAD~1").unwrap(), ["a.txt"]);
    assert_eq!(
        b.blob("HEAD:sub/b.txt").unwrap(),
        b"x\nHello again\n"
            .to_vec()
            .iter()
            .copied()
            .filter(|c| *c != b'\r')
            .collect::<Vec<u8>>()
    );
    assert!(
        String::from_utf8(b.worktree_file("a.txt").unwrap())
            .unwrap()
            .contains("unstaged")
    );
    assert!(b.blob("HEAD:missing.txt").is_err());
}

// ------------------------------------------------------------------ conflicts

#[test]
fn conflict_editor_flow() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "top\nshared\nbottom\n", "base");
    git(d, &["switch", "-q", "-c", "feat"]);
    commit_file(d, "f.txt", "top\nfeature line\nbottom\n", "feat");
    git(d, &["switch", "-q", "main"]);
    commit_file(d, "f.txt", "top\nmain line\nbottom\n", "main");
    let b = open(d);
    assert_eq!(b.merge("feat").unwrap(), OpResult::Conflicts);

    let merged = b.conflict_merge("f.txt").unwrap();
    let segs = parse_conflicts(&merged);
    assert_eq!(conflict_count(&segs), 1);
    let rustle_core::conflict::Segment::Conflict(block) = &segs[1] else {
        panic!("{segs:?}")
    };
    assert!(block.ours.contains("main line") && block.theirs.contains("feature line"));
    assert!(block.base.as_deref().unwrap().contains("shared"));

    // Resolve by taking both and a custom edit would work the same way.
    let resolved = resolve(&segs, &[Some(Choice::Both)]);
    assert!(resolved.contains("main line") && resolved.contains("feature line"));
    assert!(!resolved.contains("<<<<<<<"));
    b.write_resolved("f.txt", &resolved).unwrap();
    assert!(!b.status().unwrap().files.iter().any(|f| f.conflicted));
    assert_eq!(b.op_continue().unwrap(), OpResult::Done);
    let content = std::fs::read_to_string(d.join("f.txt")).unwrap();
    assert!(content.contains("main line") && content.contains("feature line"));
    let _ = Side::Ours;
}

// ------------------------------------------------------------------ history editing with several commits

#[test]
fn squash_with_fixup_and_reorder_via_interactive_rebase() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "1\n", "c1");
    commit_file(d, "b.txt", "2\n", "c2");
    commit_file(d, "c.txt", "3\n", "c3");
    commit_file(d, "d.txt", "4\n", "c4");
    let b = open(d);
    let log = b.log(10, 0).unwrap();
    let id = |s: &str| log.iter().find(|c| c.subject == s).unwrap().id.clone();

    // Squash c2 and c3 into one commit with a new message; c4 is replayed on top.
    let items = vec![
        TodoItem {
            action: TodoAction::Reword,
            id: id("c2"),
            message: Some("c2+c3 squashed".into()),
        },
        TodoItem {
            action: TodoAction::Fixup,
            id: id("c3"),
            message: None,
        },
        TodoItem {
            action: TodoAction::Pick,
            id: id("c4"),
            message: None,
        },
    ];
    assert_eq!(b.interactive_rebase(&id("c1"), &items).unwrap(), OpResult::Done);
    assert_eq!(head_subjects(&b), ["c4", "c2+c3 squashed", "c1"]);
    assert!(d.join("b.txt").exists() && d.join("c.txt").exists());

    // Reorder the last two commits.
    let log = b.log(10, 0).unwrap();
    let id = |s: &str| log.iter().find(|c| c.subject == s).unwrap().id.clone();
    let items = vec![
        TodoItem {
            action: TodoAction::Pick,
            id: id("c4"),
            message: None,
        },
        TodoItem {
            action: TodoAction::Pick,
            id: id("c2+c3 squashed"),
            message: None,
        },
    ];
    assert_eq!(b.interactive_rebase(&id("c1"), &items).unwrap(), OpResult::Done);
    assert_eq!(head_subjects(&b), ["c2+c3 squashed", "c4", "c1"]);
}

// ------------------------------------------------------------------ worktrees, submodules, LFS

#[test]
fn worktrees_can_be_added_listed_and_removed() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    init(&main);
    commit_file(&main, "a.txt", "1\n", "one");
    let b = open(&main);

    let wt = root.path().join("wt-feature");
    b.add_worktree(wt.to_str().unwrap(), "feature", true).unwrap();
    git(&main, &["branch", "other"]);
    let wt2 = root.path().join("wt-other");
    b.add_worktree(wt2.to_str().unwrap(), "other", false).unwrap();

    let list = b.worktrees().unwrap();
    assert_eq!(list.len(), 3);
    assert!(list[0].is_main && list[0].branch.as_deref() == Some("main"));
    assert!(
        list.iter()
            .any(|w| w.branch.as_deref() == Some("feature") && !w.is_main)
    );
    assert!(wt.join("a.txt").exists());

    b.remove_worktree(wt.to_str().unwrap(), false).unwrap();
    b.remove_worktree(wt2.to_str().unwrap(), true).unwrap();
    assert_eq!(b.worktrees().unwrap().len(), 1);
    b.prune_worktrees().unwrap();
}

#[test]
fn submodules_can_be_added_and_updated() {
    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("lib");
    let app = root.path().join("app");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    init(&lib);
    commit_file(&lib, "l.txt", "lib\n", "lib one");
    init(&app);
    // Local-path submodules are blocked by default since a 2022 git security fix; allow them for
    // this test process only (other tests are unaffected: it merely permits the file transport).
    // SAFETY: only this test touches these variables.
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "protocol.file.allow");
        std::env::set_var("GIT_CONFIG_VALUE_0", "always");
    }
    commit_file(&app, "a.txt", "app\n", "app one");
    let b = open(&app);

    b.add_submodule(lib.to_str().unwrap(), "libs/lib", None).unwrap();
    git(&app, &["commit", "-q", "-m", "add submodule"]);
    let subs = b.submodules().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].path, "libs/lib");
    assert_eq!(subs[0].status, ' ');
    assert!(app.join("libs/lib/l.txt").exists());

    // The library moves on; updating to the remote head makes the submodule "modified".
    commit_file(&lib, "l2.txt", "more\n", "lib two");
    b.submodule_sync(Some("libs/lib"), true, None).unwrap();
    let subs = b.submodules().unwrap();
    assert_eq!(subs[0].status, '+');
    assert!(b.submodule_is_modified(&subs[0]));
    assert!(app.join("libs/lib/l2.txt").exists());
}

#[test]
fn lfs_tracking_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "1\n", "one");
    let b = open(d);
    let info = b.lfs_info();
    assert!(info.patterns.is_empty() && info.files.is_empty());
    if !info.available {
        eprintln!("git-lfs not installed; skipping the rest");
        return;
    }
    b.lfs_track("*.bin").unwrap();
    let info = b.lfs_info();
    assert_eq!(info.patterns, ["*.bin"]);
    let attrs = std::fs::read_to_string(d.join(".gitattributes")).unwrap();
    assert!(attrs.contains("filter=lfs"));
}

// ------------------------------------------------------------------ graph with a working-tree row

#[test]
fn working_tree_row_sits_on_the_head_lane() {
    use rustle_core::Commit;
    let c = |id: &str, parents: &[&str]| Commit {
        id: id.into(),
        parents: parents.iter().map(|s| s.to_string()).collect(),
        author_name: String::new(),
        author_email: String::new(),
        time: 0,
        subject: String::new(),
        refs: vec![],
    };
    // Another branch tip sits above HEAD in the list.
    let commits = vec![c("other", &["base"]), c("head", &["base"]), c("base", &[])];
    let mut with_wip = vec![c("WIP", &["head"])];
    with_wip.extend(commits.clone());
    let plain = build_graph(&commits);
    let wip = build_graph(&with_wip);
    // Without the row, `other` takes lane 0 and `head` lane 1.
    assert_eq!(plain.rows[0].col, 0);
    assert_eq!(plain.rows[1].col, 1);
    // With the row, the WIP node leads into HEAD's lane.
    assert_eq!(wip.rows.len(), 4);
    let wip_lane = wip.rows[0].col;
    let head_lane = wip.rows[2].col;
    assert_eq!(
        wip_lane, head_lane,
        "the WIP node must connect straight down to HEAD"
    );
}

// ------------------------------------------------------------------ misc

#[test]
fn head_id_and_error_for_unreachable_paths() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    let b = open(d);
    assert!(b.head_id().is_none());
    commit_file(d, "a.txt", "1\n", "one");
    let id = b.head_id().unwrap();
    assert_eq!(id.len(), 40);
    assert!(matches!(b.push_branch("nope", None), Err(GitError::NoRemote)));
}

#[cfg(unix)]
#[test]
fn non_utf8_file_names_survive_stage_and_status() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "base.txt", "1\n", "one");
    let name = std::ffi::OsStr::from_bytes(b"bad-\xff-name.txt");
    std::fs::write(d.join(name), "x").unwrap();
    let b = open(d);
    let st = b.status().unwrap();
    let f = st.files.iter().find(|f| f.path.starts_with("bad-")).unwrap();
    assert!(rustle_core::pathenc::has_escaped_bytes(&f.path));
    b.stage(&f.paths()).unwrap();
    assert!(b.status().unwrap().files.iter().any(|f| f.is_staged()));
}
