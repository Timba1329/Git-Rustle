use rustle_core::{
    CancelToken, CliBackend, DiffOpts, GitError, GitRunner, LogQuery, OpResult, OpState, PatchMode, PullMode,
    ResetMode, RunOpts, Side, TodoAction, TodoItem, build_patch, parse_diff,
};
use std::collections::HashSet;
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

fn write(dir: &Path, name: &str, content: &str) {
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

fn subjects(b: &CliBackend) -> Vec<String> {
    b.log(100, 0).unwrap().into_iter().map(|c| c.subject).collect()
}

/// Subjects of the current branch only (the default log shows every branch).
fn head_subjects(b: &CliBackend) -> Vec<String> {
    let q = LogQuery {
        current_branch_only: true,
        ..LogQuery::all(100)
    };
    b.log_query(&q).unwrap().into_iter().map(|c| c.subject).collect()
}

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line{i}\n")).collect()
}

#[test]
fn discard_tracked_untracked_and_staged_new() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "orig\n", "init");
    let b = open(d);

    write(d, "f.txt", "changed\n");
    write(d, "new.txt", "x\n");
    write(d, "added.txt", "y\n");
    git(d, &["add", "added.txt"]);
    let st = b.status().unwrap();
    let get = |p: &str| st.files.iter().find(|f| f.path == p).unwrap().clone();

    b.discard(&get("f.txt"), false).unwrap();
    assert_eq!(
        std::fs::read_to_string(d.join("f.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "orig\n"
    );
    b.discard(&get("new.txt"), false).unwrap();
    assert!(!d.join("new.txt").exists());
    b.discard(&get("added.txt"), true).unwrap();
    assert!(!d.join("added.txt").exists());
    assert!(b.status().unwrap().files.is_empty());
}

#[test]
fn hunk_and_line_staging() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", &lines(30), "init");
    let b = open(d);
    let opts = DiffOpts::default();

    // Two separate hunks; stage only the first.
    let mut content = lines(30);
    content = content
        .replace("line2\n", "CHANGED2\n")
        .replace("line28\n", "CHANGED28\n");
    write(d, "f.txt", &content);
    let st = b.status().unwrap();
    let diff = parse_diff(&b.file_diff(&st.files[0], false, opts).unwrap());
    assert_eq!(diff[0].hunks.len(), 2);
    let patch = build_patch(&diff[0], 0, None, false).unwrap();
    b.apply_patch(&patch, PatchMode::Stage).unwrap();
    let staged = git(d, &["diff", "--cached"]);
    assert!(staged.contains("+CHANGED2") && !staged.contains("CHANGED28"));
    let unstaged = git(d, &["diff"]);
    assert!(unstaged.contains("+CHANGED28") && !unstaged.contains("CHANGED2\n+"));

    // Unstage that hunk again.
    let st = b.status().unwrap();
    let f = st.files.iter().find(|f| f.is_staged()).unwrap();
    let sdiff = parse_diff(&b.file_diff(f, true, opts).unwrap());
    let patch = build_patch(&sdiff[0], 0, None, true).unwrap();
    b.apply_patch(&patch, PatchMode::Unstage).unwrap();
    assert!(git(d, &["diff", "--cached"]).is_empty());

    // Discard a hunk (the remaining changes are both hunks again).
    let st = b.status().unwrap();
    let diff = parse_diff(&b.file_diff(&st.files[0], false, opts).unwrap());
    let patch = build_patch(&diff[0], 1, None, true).unwrap();
    b.apply_patch(&patch, PatchMode::Discard).unwrap();
    let rest = git(d, &["diff"]);
    assert!(rest.contains("+CHANGED2") && !rest.contains("CHANGED28"));
    git(d, &["checkout", "--", "f.txt"]);

    // Line level: add two lines, stage only the first.
    write(d, "f.txt", &lines(30).replace("line10\n", "line10\nX\nY\n"));
    let st = b.status().unwrap();
    let diff = parse_diff(&b.file_diff(&st.files[0], false, opts).unwrap());
    let h = &diff[0].hunks[0];
    let x = h.lines.iter().position(|l| l == "+X").unwrap();
    let sel: HashSet<usize> = [x].into_iter().collect();
    let patch = build_patch(&diff[0], 0, Some(&sel), false).unwrap();
    b.apply_patch(&patch, PatchMode::Stage).unwrap();
    let staged = git(d, &["diff", "--cached"]);
    assert!(staged.contains("+X") && !staged.contains("+Y"));

    // Unstage just that line.
    let st = b.status().unwrap();
    let f = st.files.iter().find(|f| f.is_staged()).unwrap();
    let sdiff = parse_diff(&b.file_diff(f, true, opts).unwrap());
    let x = sdiff[0].hunks[0].lines.iter().position(|l| l == "+X").unwrap();
    let sel: HashSet<usize> = [x].into_iter().collect();
    let patch = build_patch(&sdiff[0], 0, Some(&sel), true).unwrap();
    b.apply_patch(&patch, PatchMode::Unstage).unwrap();
    assert!(git(d, &["diff", "--cached"]).is_empty());
}

#[test]
fn amend_changes_message_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "1\n", "one");
    let b = open(d);
    assert_eq!(b.last_message().unwrap(), "one");
    write(d, "b.txt", "2\n");
    b.stage(&["b.txt".into()]).unwrap();
    b.commit_amend("two").unwrap();
    assert_eq!(subjects(&b), ["two"]);
    assert_eq!(b.commit_files("HEAD").unwrap().len(), 2);
}

#[test]
fn merge_conflict_resolve_and_continue() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "base\n", "base");
    git(d, &["switch", "-q", "-c", "feat"]);
    commit_file(d, "f.txt", "feature\n", "feat change");
    git(d, &["switch", "-q", "main"]);
    commit_file(d, "f.txt", "mainline\n", "main change");
    let b = open(d);

    assert_eq!(b.merge("feat").unwrap(), OpResult::Conflicts);
    let st = b.status().unwrap();
    assert_eq!(st.op, OpState::Merge);
    assert!(st.files[0].conflicted);
    let v = b.conflict_versions("f.txt").unwrap();
    assert!(v.base.unwrap().contains("base"));
    assert!(v.ours.unwrap().contains("mainline"));
    assert!(v.theirs.unwrap().contains("feature"));

    b.resolve("f.txt", Side::Theirs).unwrap();
    assert_eq!(b.op_continue().unwrap(), OpResult::Done);
    assert_eq!(b.status().unwrap().op, OpState::None);
    assert_eq!(b.log(1, 0).unwrap()[0].parents.len(), 2);
    assert!(
        std::fs::read_to_string(d.join("f.txt"))
            .unwrap()
            .contains("feature")
    );
}

#[test]
fn merge_abort() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "base\n", "base");
    git(d, &["switch", "-q", "-c", "feat"]);
    commit_file(d, "f.txt", "feature\n", "feat change");
    git(d, &["switch", "-q", "main"]);
    commit_file(d, "f.txt", "mainline\n", "main change");
    let b = open(d);
    assert_eq!(b.merge("feat").unwrap(), OpResult::Conflicts);
    b.op_abort().unwrap();
    assert_eq!(b.status().unwrap().op, OpState::None);
    assert!(b.status().unwrap().files.is_empty());
}

#[test]
fn rebase_cherry_pick_revert_reset() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "a.txt", "1\n", "base");
    git(d, &["switch", "-q", "-c", "feat"]);
    commit_file(d, "b.txt", "b\n", "on feat");
    git(d, &["switch", "-q", "main"]);
    commit_file(d, "c.txt", "c\n", "on main");
    let b = open(d);

    // Rebase feat onto main -> linear history.
    git(d, &["switch", "-q", "feat"]);
    assert_eq!(b.rebase("main").unwrap(), OpResult::Done);
    assert_eq!(head_subjects(&b), ["on feat", "on main", "base"]);

    // Cherry-pick a commit onto another branch.
    git(d, &["switch", "-q", "main"]);
    commit_file(d, "d.txt", "d\n", "extra");
    git(d, &["switch", "-q", "feat"]);
    let extra = b
        .log_query(&LogQuery {
            text: Some("extra".into()),
            ..LogQuery::all(5)
        })
        .unwrap()[0]
        .id
        .clone();
    assert_eq!(b.cherry_pick(&extra).unwrap(), OpResult::Done);
    assert_eq!(head_subjects(&b)[0], "extra");

    // Revert it, then hard reset back two steps.
    assert_eq!(b.revert("HEAD").unwrap(), OpResult::Done);
    assert!(head_subjects(&b)[0].starts_with("Revert"));
    b.reset("HEAD~2", ResetMode::Hard).unwrap();
    assert_eq!(head_subjects(&b)[0], "on feat");
    assert!(!d.join("d.txt").exists());
}

#[test]
fn stash_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "orig\n", "init");
    let b = open(d);
    write(d, "f.txt", "changed\n");
    write(d, "u.txt", "untracked\n");
    b.stash_push("wip", true).unwrap();
    assert!(b.status().unwrap().files.is_empty());
    let l = b.stash_list().unwrap();
    assert_eq!(l.len(), 1);
    assert!(l[0].message.contains("wip"));
    assert_eq!(b.stash_pop(0).unwrap(), OpResult::Done);
    assert_eq!(b.status().unwrap().files.len(), 2);
    assert!(b.stash_list().unwrap().is_empty());
    b.stash_push("again", false).unwrap();
    b.stash_drop(0).unwrap();
    assert!(b.stash_list().unwrap().is_empty());
}

#[test]
fn tags_and_branch_management() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "1\n", "one");
    commit_file(d, "f.txt", "2\n", "two");
    let b = open(d);

    b.create_tag("light", None, None).unwrap();
    b.create_tag("annot", Some("HEAD~1"), Some("release note"))
        .unwrap();
    let mut t = b.tags().unwrap();
    t.sort();
    assert_eq!(t, ["annot", "light"]);
    b.delete_tag("light").unwrap();
    assert_eq!(b.tags().unwrap(), ["annot"]);

    b.create_branch_at("old", "HEAD~1", false).unwrap();
    b.rename_branch("old", "older").unwrap();
    assert!(b.branches().unwrap().iter().any(|x| x.name == "older"));
    b.delete_branch("older", false).unwrap();
    assert!(!b.branches().unwrap().iter().any(|x| x.name == "older"));

    b.checkout_commit("HEAD~1").unwrap();
    assert_eq!(b.status().unwrap().branch, None);
}

#[test]
fn interactive_rebase_reword_squash_drop() {
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
    let base = id("c1");
    let items = vec![
        TodoItem {
            action: TodoAction::Reword,
            id: id("c2"),
            message: Some("renamed c2".into()),
        },
        TodoItem {
            action: TodoAction::Squash,
            id: id("c3"),
            message: None,
        },
        TodoItem {
            action: TodoAction::Drop,
            id: id("c4"),
            message: None,
        },
    ];
    assert_eq!(b.interactive_rebase(&base, &items).unwrap(), OpResult::Done);
    let s = subjects(&b);
    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!(s[0], "renamed c2");
    assert!(d.join("c.txt").exists());
    assert!(!d.join("d.txt").exists());
}

#[test]
fn clone_init_and_gitignore() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    init(&src);
    commit_file(&src, "f.txt", "1\n", "one");

    let runner = GitRunner::discover().unwrap();
    let dest = root.path().join("copy");
    CliBackend::clone_repo(&runner, src.to_str().unwrap(), &dest, None).unwrap();
    let b = open(&dest);
    assert_eq!(subjects(&b), ["one"]);

    let fresh = root.path().join("fresh");
    CliBackend::init_repo(&runner, &fresh).unwrap();
    let nb = open(&fresh);
    assert!(nb.log(5, 0).unwrap().is_empty());

    nb.add_to_gitignore("*.log").unwrap();
    nb.add_to_gitignore("target/").unwrap();
    write(&fresh, "x.log", "noise");
    let st = nb.status().unwrap();
    assert!(!st.files.iter().any(|f| f.path == "x.log"));
}

#[test]
fn blame_reflog_and_compare() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    init(d);
    commit_file(d, "f.txt", "a\nb\n", "one");
    commit_file(d, "f.txt", "a\nb\nc\n", "two");
    let b = open(d);

    let bl = b.blame("f.txt").unwrap();
    assert_eq!(bl.len(), 3);
    assert_eq!(bl[0].author, "Test");
    assert_ne!(bl[0].id, bl[2].id);

    assert!(!b.reflog(10).unwrap().is_empty());

    let r = b.compare_files("HEAD~1", "HEAD").unwrap();
    assert_eq!(r[0].path, "f.txt");
    let diff = b
        .compare_file_diff("HEAD~1", "HEAD", "f.txt", DiffOpts::default())
        .unwrap();
    assert!(diff.contains("+c"));

    let hist = b
        .log_query(&LogQuery {
            path: Some("f.txt".into()),
            ..LogQuery::all(10)
        })
        .unwrap();
    assert_eq!(hist.len(), 2);
}

#[test]
fn pull_modes_merge_and_rebase() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let a = root.path().join("a");
    let c = root.path().join("c");
    for p in [&remote, &a] {
        std::fs::create_dir_all(p).unwrap();
    }
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    init(&a);
    commit_file(&a, "f.txt", "1\n", "one");
    git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let ba = open(&a);
    ba.push(false, None).unwrap();
    git(
        root.path(),
        &["clone", "-q", remote.to_str().unwrap(), c.to_str().unwrap()],
    );
    git(&c, &["config", "user.name", "Test"]);
    git(&c, &["config", "user.email", "t@example.com"]);
    git(&c, &["config", "commit.gpgsign", "false"]);

    // Diverge: a and c both commit.
    commit_file(&c, "c.txt", "c\n", "from c");
    open(&c).push(false, None).unwrap();
    commit_file(&a, "a.txt", "a\n", "from a");
    ba.fetch(None).unwrap();

    // ff-only fails, rebase succeeds.
    assert!(matches!(
        ba.pull(PullMode::FfOnly, None),
        Err(GitError::Failed { .. })
    ));
    assert_eq!(ba.pull(PullMode::Rebase, None).unwrap(), OpResult::Done);
    assert_eq!(subjects(&ba)[..2], ["from a", "from c"]);
}

#[test]
fn cancel_kills_the_process() {
    let runner = GitRunner::discover().unwrap();
    let token = CancelToken::new();
    let t2 = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        t2.cancel();
    });
    let start = std::time::Instant::now();
    let r = runner.run_opts(
        &["-c", "alias.slp=!sleep 10", "slp"],
        None,
        &RunOpts {
            cancel: Some(&token),
            ..Default::default()
        },
    );
    assert!(matches!(r, Err(GitError::Cancelled)), "{r:?}");
    assert!(start.elapsed().as_secs() < 5);
}

#[test]
fn background_fetch_is_non_interactive() {
    // The askpass helper must be invisible to non-interactive runs, and present otherwise.
    let runner = GitRunner::discover().unwrap().with_askpass("C:/some/helper.exe");
    let args = ["-c", "alias.e=!echo [$GIT_ASKPASS][$SSH_ASKPASS_REQUIRE]", "e"];
    let normal = runner.run(&args, None).unwrap();
    assert!(String::from_utf8_lossy(&normal.stdout).contains("helper.exe"));
    let quiet = runner
        .run_opts(
            &args,
            None,
            &RunOpts {
                non_interactive: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&quiet.stdout).trim(), "[][]");
}

#[test]
fn background_fetch_updates_tracking_and_fails_quietly() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let a = root.path().join("a");
    let c = root.path().join("c");
    for p in [&remote, &a] {
        std::fs::create_dir_all(p).unwrap();
    }
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    init(&a);
    commit_file(&a, "f.txt", "1\n", "one");
    git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    let ba = CliBackend::open(
        GitRunner::discover().unwrap().with_askpass("C:/some/helper.exe"),
        &a,
    )
    .unwrap();
    ba.push(false, None).unwrap();

    git(
        root.path(),
        &["clone", "-q", remote.to_str().unwrap(), c.to_str().unwrap()],
    );
    git(&c, &["config", "user.name", "Test"]);
    git(&c, &["config", "user.email", "t@example.com"]);
    git(&c, &["config", "commit.gpgsign", "false"]);
    commit_file(&c, "c.txt", "c\n", "from c");
    open(&c).push(false, None).unwrap();

    ba.fetch_background(None).unwrap();
    let main = ba
        .branches()
        .unwrap()
        .into_iter()
        .find(|b| b.name == "main")
        .unwrap();
    assert_eq!((main.ahead, main.behind), (0, 1));

    // An unreachable remote is an error (the caller ignores it), never a prompt or a hang.
    git(
        &a,
        &[
            "remote",
            "set-url",
            "origin",
            root.path().join("missing.git").to_str().unwrap(),
        ],
    );
    assert!(matches!(ba.fetch_background(None), Err(GitError::Failed { .. })));
}
