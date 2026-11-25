use rustle_core::{CliBackend, DiffOpts, GitError, GitRunner, OpResult, PullMode};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    GitRunner::discover().unwrap().run(args, Some(dir)).unwrap();
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    config(dir);
}

fn config(dir: &Path) {
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn open(dir: &Path) -> CliBackend {
    CliBackend::open(GitRunner::discover().unwrap(), dir).unwrap()
}

#[test]
fn stage_unstage_commit_flow() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let b = open(dir.path());
    std::fs::write(dir.path().join("a.txt"), "hallo\n").unwrap();

    // Untracked.
    let st = b.status().unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert!(st.files[0].is_untracked());
    assert!(
        b.file_diff(&st.files[0], false, DiffOpts::default())
            .unwrap()
            .contains("+hallo")
    );

    // Stage (before any commits exist), unstage, stage again.
    b.stage(&["a.txt".into()]).unwrap();
    assert!(b.status().unwrap().files[0].is_staged());
    b.unstage(&["a.txt".into()]).unwrap();
    assert!(b.status().unwrap().files[0].is_untracked());
    b.stage(&[]).unwrap();

    b.commit("eerste").unwrap();
    assert!(b.status().unwrap().files.is_empty());
    assert_eq!(b.log(10, 0).unwrap()[0].subject, "eerste");

    // Change: unstaged -> staged, with diff.
    std::fs::write(dir.path().join("a.txt"), "hallo\nwereld\n").unwrap();
    let st = b.status().unwrap();
    assert!(st.files[0].is_unstaged() && !st.files[0].is_staged());
    assert!(
        b.file_diff(&st.files[0], false, DiffOpts::default())
            .unwrap()
            .contains("+wereld")
    );
    b.stage(&st.files[0].paths()).unwrap();
    let st = b.status().unwrap();
    assert!(st.files[0].is_staged());
    assert!(
        b.file_diff(&st.files[0], true, DiffOpts::default())
            .unwrap()
            .contains("+wereld")
    );
    b.unstage(&[]).unwrap();
    assert!(!b.status().unwrap().files[0].is_staged());

    // Commit diff.
    let id = b.log(1, 0).unwrap()[0].id.clone();
    let files = b.commit_files(&id).unwrap();
    assert_eq!(files[0].path, "a.txt");
    assert!(
        b.commit_file_diff(&id, "a.txt", DiffOpts::default())
            .unwrap()
            .contains("+hallo")
    );
}

#[test]
fn works_from_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("x.txt"), "x").unwrap();
    let b = open(&dir.path().join("sub"));
    b.stage(&["sub/x.txt".into()]).unwrap();
    assert!(b.status().unwrap().files[0].is_staged());
}

#[test]
fn branches_create_and_checkout() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    let b = open(dir.path());
    b.create_branch("feat").unwrap();
    let list = b.branches().unwrap();
    assert!(list.iter().any(|x| x.name == "feat" && x.is_head));
    let main = list.iter().find(|x| x.name == "main").unwrap().clone();
    b.checkout(&main).unwrap();
    assert_eq!(b.status().unwrap().branch.as_deref(), Some("main"));
}

#[test]
fn push_pull_fetch_with_remote() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let a = root.path().join("a");
    let c = root.path().join("c");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&a).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    init_repo(&a);
    git(&a, &["commit", "-q", "--allow-empty", "-m", "een"]);
    git(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);

    // Pushing without an upstream sets the upstream.
    let ba = open(&a);
    ba.push(false, None).unwrap();
    let st = ba.status().unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert_eq!((st.ahead, st.behind), (0, 0));

    // A second clone pushes a commit.
    git(
        root.path(),
        &["clone", "-q", remote.to_str().unwrap(), c.to_str().unwrap()],
    );
    config(&c);
    git(&c, &["commit", "-q", "--allow-empty", "-m", "twee"]);
    open(&c).push(false, None).unwrap();

    // Fetch shows "behind", pull brings it in.
    ba.fetch(None).unwrap();
    assert_eq!(ba.status().unwrap().behind, 1);
    let main = |b: &CliBackend| {
        b.branches()
            .unwrap()
            .into_iter()
            .find(|x| x.name == "main")
            .unwrap()
    };
    assert_eq!((main(&ba).ahead, main(&ba).behind), (0, 1));
    assert_eq!(ba.pull(PullMode::FfOnly, None).unwrap(), OpResult::Done);
    let st = ba.status().unwrap();
    assert_eq!((st.ahead, st.behind), (0, 0));
    assert_eq!(ba.log(10, 0).unwrap()[0].subject, "twee");

    // Local commit -> ahead.
    git(&a, &["commit", "-q", "--allow-empty", "-m", "drie"]);
    assert_eq!(ba.status().unwrap().ahead, 1);
    assert_eq!((main(&ba).ahead, main(&ba).behind), (1, 0));

    // A branch without upstream has no tracking info; a deleted upstream is reported as gone.
    git(&a, &["branch", "local-only"]);
    let lo = ba
        .branches()
        .unwrap()
        .into_iter()
        .find(|x| x.name == "local-only")
        .unwrap();
    assert!(lo.upstream.is_none() && !lo.gone);
    git(&a, &["push", "-q", "-u", "origin", "local-only"]);
    git(&a, &["push", "-q", "origin", "--delete", "local-only"]);
    ba.fetch(None).unwrap();
    let lo = ba
        .branches()
        .unwrap()
        .into_iter()
        .find(|x| x.name == "local-only")
        .unwrap();
    assert!(lo.gone, "{lo:?}");
}

#[test]
fn push_without_remote_errors() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    assert!(matches!(
        open(dir.path()).push(false, None),
        Err(GitError::NoRemote)
    ));
}
