use crate::error::{GitError, Result};
use crate::model::{
    BlameLine, Branch, Commit, CommitFile, ConflictVersions, DiffOpts, FileChange, LogQuery, OpResult,
    OpState, PatchMode, PullMode, ReflogEntry, ResetMode, Side, StashEntry, Status, Submodule, TodoAction,
    TodoItem,
};
use crate::parse::log::{LOG_FORMAT, parse_log};
use crate::parse::status::parse_status;
use crate::pathenc::{decode_path, encode_path};
use crate::runner::{CancelToken, GitRunner, RunOpts};
use std::path::{Path, PathBuf};

const MAX_DIFF_BYTES: usize = 1_000_000;

pub struct CliBackend {
    pub(crate) runner: GitRunner,
    pub(crate) repo: PathBuf,
    pub(crate) git_dir: PathBuf,
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn text(bytes: &[u8]) -> String {
    if bytes.len() > MAX_DIFF_BYTES {
        let mut s = String::from_utf8_lossy(&bytes[..MAX_DIFF_BYTES]).into_owned();
        s.push_str("\n… (diff truncated)");
        s
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn with_paths(base: Vec<String>, paths: &[String]) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = base.into_iter().map(Into::into).collect();
    v.push("--".into());
    v.extend(paths.iter().map(|p| encode_path(p)));
    v
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// Parses `%(upstream:track,nobracket)`: `ahead 2, behind 1`, `ahead 2`, `behind 1`, `gone` or empty.
fn parse_track(s: &str) -> (u32, u32, bool) {
    let (mut ahead, mut behind) = (0, 0);
    for part in s.split(',').map(str::trim) {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind, s.trim() == "gone")
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

impl CliBackend {
    /// Opens a repo (also from a subdirectory); fails with `NotARepo` if it is not a git repo.
    pub fn open(runner: GitRunner, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let out = runner
            .run(
                &["rev-parse", "--show-toplevel", "--absolute-git-dir"],
                Some(path),
            )
            .map_err(|e| match e {
                GitError::Failed { .. } => GitError::NotARepo(path.to_path_buf()),
                e => e,
            })?;
        let s = lossy(&out.stdout);
        let mut lines = s.lines();
        let top = lines.next().unwrap_or("").trim();
        let git_dir = lines.next().unwrap_or("").trim();
        Ok(Self {
            runner,
            repo: PathBuf::from(top),
            git_dir: PathBuf::from(git_dir),
        })
    }

    /// Creates a new repository in `dir`.
    pub fn init_repo(runner: &GitRunner, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        runner.run(&["init", "-q"], Some(dir))?;
        Ok(())
    }

    /// Clones `url` into `dest` (which must not exist or be empty).
    pub fn clone_repo(
        runner: &GitRunner,
        url: &str,
        dest: &Path,
        cancel: Option<&CancelToken>,
    ) -> Result<()> {
        let args = ["clone", "-q", "--", url, &dest.to_string_lossy()];
        runner.run_opts(
            &args,
            None,
            &RunOpts {
                cancel,
                ..Default::default()
            },
        )?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.repo
    }

    pub fn runner(&self) -> &GitRunner {
        &self.runner
    }

    pub(crate) fn git<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Result<std::process::Output> {
        self.runner.run(args, Some(&self.repo))
    }

    pub(crate) fn git_cancel<S: AsRef<std::ffi::OsStr>>(
        &self,
        args: &[S],
        cancel: Option<&CancelToken>,
    ) -> Result<std::process::Output> {
        self.runner.run_opts(
            args,
            Some(&self.repo),
            &RunOpts {
                cancel,
                ..Default::default()
            },
        )
    }

    pub(crate) fn has_head(&self) -> bool {
        self.git(&["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
    }

    pub(crate) fn has_conflicts(&self) -> bool {
        self.status()
            .map(|s| s.files.iter().any(|f| f.conflicted))
            .unwrap_or(false)
    }

    /// Maps a failure caused by merge conflicts to `Ok(Conflicts)`.
    pub(crate) fn finish(&self, r: Result<std::process::Output>) -> Result<OpResult> {
        match r {
            Ok(_) => Ok(OpResult::Done),
            Err(GitError::Failed { .. }) if self.has_conflicts() => Ok(OpResult::Conflicts),
            Err(e) => Err(e),
        }
    }

    // ---------------------------------------------------------------- reading

    pub fn log(&self, limit: usize, skip: usize) -> Result<Vec<Commit>> {
        self.log_query(&LogQuery {
            limit,
            skip,
            ..Default::default()
        })
    }

    pub fn log_query(&self, q: &LogQuery) -> Result<Vec<Commit>> {
        // A repo without commits makes `git log` fail; treat that as an empty list.
        if !self.has_head() {
            return Ok(Vec::new());
        }
        let mut args = strs(&["log", "-z", "--topo-order", "--decorate=full"]);
        args.push(format!("-n{}", q.limit));
        args.push(format!("--skip={}", q.skip));
        args.push(LOG_FORMAT.to_string());
        if q.text.is_some() || q.author.is_some() {
            args.push("--fixed-strings".into());
            args.push("--regexp-ignore-case".into());
        }
        if let Some(t) = &q.text {
            args.push(format!("--grep={t}"));
        }
        if let Some(a) = &q.author {
            args.push(format!("--author={a}"));
        }
        if q.current_branch_only || q.path.is_some() {
            args.push("HEAD".into());
        } else {
            args.extend(strs(&["--branches", "--remotes", "--tags", "HEAD"]));
        }
        if let Some(p) = &q.path {
            args.push("--follow".into());
            args.push("--".into());
            args.push(p.clone());
        }
        parse_log(&self.git(&args)?.stdout)
    }

    pub fn status(&self) -> Result<Status> {
        let out = self.git(&[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ])?;
        let mut st = parse_status(&out.stdout)?;
        st.op = self.op_state();
        for (staged, args) in [(false, vec!["diff", "-z", "--numstat"]), (true, vec!["diff", "--cached", "-z", "--numstat"])] {
            if let Ok(o) = self.git(&args) {
                for (p, a, d) in Self::parse_numstat(&o.stdout) {
                    if let (Some(a), Some(d)) = (a, d) {
                        st.stats.insert((p, staged), (a, d));
                    }
                }
            }
        }
        Ok(st)
    }

    pub fn branches(&self) -> Result<Vec<Branch>> {
        let out = self.git(&[
            "for-each-ref",
            "--format=%(refname)%1f%(HEAD)%1f%(upstream:short)%1f%(upstream:track,nobracket)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut v = Vec::new();
        for line in lossy(&out.stdout).lines() {
            let f: Vec<&str> = line.split('\x1f').collect();
            if f.len() != 4 {
                continue;
            }
            let (name, is_remote) = if let Some(n) = f[0].strip_prefix("refs/heads/") {
                (n, false)
            } else if let Some(n) = f[0].strip_prefix("refs/remotes/") {
                (n, true)
            } else {
                continue;
            };
            if is_remote && name.ends_with("/HEAD") {
                continue;
            }
            let (ahead, behind, gone) = parse_track(f[3]);
            v.push(Branch {
                name: name.to_string(),
                is_head: f[1] == "*",
                is_remote,
                upstream: (!f[2].is_empty()).then(|| f[2].to_string()),
                ahead,
                behind,
                gone,
            });
        }
        Ok(v)
    }

    pub fn tags(&self) -> Result<Vec<String>> {
        let out = self.git(&["for-each-ref", "--format=%(refname:short)", "refs/tags"])?;
        Ok(lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub fn op_state(&self) -> OpState {
        let d = &self.git_dir;
        if d.join("MERGE_HEAD").exists() {
            OpState::Merge
        } else if d.join("rebase-merge").exists() || d.join("rebase-apply").exists() {
            OpState::Rebase
        } else if d.join("CHERRY_PICK_HEAD").exists() {
            OpState::CherryPick
        } else if d.join("REVERT_HEAD").exists() {
            OpState::Revert
        } else {
            OpState::None
        }
    }

    /// The full message of a commit.
    pub fn commit_message(&self, id: &str) -> Result<String> {
        let out = self.git(&["log", "-1", "--format=%B", id])?;
        Ok(lossy(&out.stdout).trim().to_string())
    }

    pub fn last_message(&self) -> Result<String> {
        self.commit_message("HEAD")
    }

    /// Contents of `commit.template`, if configured.
    pub fn commit_template(&self) -> Option<String> {
        let out = self.git(&["config", "--get", "commit.template"]).ok()?;
        let p = lossy(&out.stdout).trim().to_string();
        if p.is_empty() {
            return None;
        }
        let path = if let Some(rest) = p.strip_prefix("~/") {
            std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(rest))?
        } else {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() { pb } else { self.repo.join(pb) }
        };
        std::fs::read_to_string(path).ok()
    }

    fn diff_args(opts: DiffOpts) -> Vec<String> {
        let mut v = vec![
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            format!("-U{}", opts.context),
        ];
        if opts.ignore_whitespace {
            v.push("-w".into());
        }
        v
    }

    pub fn file_diff(&self, change: &FileChange, staged: bool, opts: DiffOpts) -> Result<String> {
        if change.is_untracked() {
            let bytes = std::fs::read(self.repo.join(encode_path(&change.path)))?;
            if bytes.contains(&0) {
                return Ok("(binary file)".to_string());
            }
            let body = text(&bytes);
            let n = body.lines().count();
            let mut s = format!(
                "diff --git a/{p} b/{p}\nnew file mode 100644\n--- /dev/null\n+++ b/{p}\n@@ -0,0 +1,{n} @@\n",
                p = change.path
            );
            for l in body.lines() {
                s.push('+');
                s.push_str(l);
                s.push('\n');
            }
            return Ok(s);
        }
        let mut args = vec!["diff".to_string()];
        args.extend(Self::diff_args(opts));
        if staged {
            args.push("--cached".into());
        }
        let out = self.git(&with_paths(args, std::slice::from_ref(&change.path)))?;
        Ok(text(&out.stdout))
    }

    fn name_status(out: &[u8]) -> Vec<CommitFile> {
        let toks: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|t| !t.is_empty())
            .map(decode_path)
            .collect();
        let mut files = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let st = toks[i].chars().next().unwrap_or('M');
            if (st == 'R' || st == 'C') && i + 2 < toks.len() {
                files.push(CommitFile {
                    status: st,
                    path: toks[i + 2].clone(),
                    orig_path: Some(toks[i + 1].clone()),
                    added: None,
                    deleted: None,
                });
                i += 3;
            } else if i + 1 < toks.len() {
                files.push(CommitFile {
                    status: st,
                    path: toks[i + 1].clone(),
                    orig_path: None,
                    added: None,
                    deleted: None,
                });
                i += 2;
            } else {
                break;
            }
        }
        files
    }

    /// Files changed by a commit (against its first parent for merges).
    pub fn commit_files(&self, id: &str) -> Result<Vec<CommitFile>> {
        let out = self.git(&[
            "diff-tree",
            "-z",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "--root",
            "-m",
            "--first-parent",
            id,
        ])?;
        let mut files = Self::name_status(&out.stdout);
        if let Ok(ns) = self.git(&[
            "diff-tree",
            "-z",
            "--no-commit-id",
            "--numstat",
            "-r",
            "-M",
            "--root",
            "-m",
            "--first-parent",
            id,
        ]) {
            Self::apply_numstat(&mut files, &ns.stdout);
        }
        Ok(files)
    }

    fn parse_numstat(out: &[u8]) -> Vec<(String, Option<u32>, Option<u32>)> {
        let toks: Vec<String> = out.split(|&b| b == 0).filter(|t| !t.is_empty()).map(decode_path).collect();
        let mut res = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let mut it = toks[i].splitn(3, '\t');
            let (a, d, p) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
            let (a, d) = (a.parse::<u32>().ok(), d.parse::<u32>().ok());
            let path = if p.is_empty() {
                let np = toks.get(i + 2).cloned().unwrap_or_default();
                i += 3;
                np
            } else {
                i += 1;
                p.to_string()
            };
            res.push((path, a, d));
        }
        res
    }

    fn apply_numstat(files: &mut [CommitFile], out: &[u8]) {
        for (path, a, d) in Self::parse_numstat(out) {
            if let Some(f) = files.iter_mut().find(|f| f.path == path) {
                f.added = a;
                f.deleted = d;
            }
        }
    }

    pub fn commit_file_diff(&self, id: &str, path: &str, opts: DiffOpts) -> Result<String> {
        let mut args = strs(&[
            "diff-tree",
            "-p",
            "--no-commit-id",
            "-r",
            "-M",
            "--root",
            "-m",
            "--first-parent",
        ]);
        args.extend(Self::diff_args(opts));
        args.push(id.to_string());
        let paths: Vec<String> = if path.is_empty() { vec![] } else { vec![path.to_string()] };
        let out = self.git(&with_paths(args, &paths))?;
        Ok(text(&out.stdout))
    }

    /// Files that differ between two commits.
    pub fn compare_files(&self, a: &str, b: &str) -> Result<Vec<CommitFile>> {
        let out = self.git(&["diff", "-z", "--name-status", "-M", a, b])?;
        let mut files = Self::name_status(&out.stdout);
        if let Ok(ns) = self.git(&["diff", "-z", "--numstat", "-M", a, b]) {
            Self::apply_numstat(&mut files, &ns.stdout);
        }
        Ok(files)
    }

    pub fn compare_file_diff(&self, a: &str, b: &str, path: &str, opts: DiffOpts) -> Result<String> {
        let mut args = vec!["diff".to_string(), "-M".to_string()];
        args.extend(Self::diff_args(opts));
        args.push(a.to_string());
        args.push(b.to_string());
        let paths: Vec<String> = if path.is_empty() { vec![] } else { vec![path.to_string()] };
        let out = self.git(&with_paths(args, &paths))?;
        Ok(text(&out.stdout))
    }

    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>> {
        let out = self.git(&["blame", "--porcelain", "--", path])?;
        let s = lossy(&out.stdout);
        let mut authors: std::collections::HashMap<String, String> = Default::default();
        let (mut cur_id, mut cur_line) = (String::new(), 0u32);
        let mut lines = Vec::new();
        for l in s.lines() {
            if let Some(t) = l.strip_prefix('\t') {
                lines.push(BlameLine {
                    id: cur_id.clone(),
                    author: authors.get(&cur_id).cloned().unwrap_or_default(),
                    number: cur_line,
                    text: t.to_string(),
                });
            } else if let Some(a) = l.strip_prefix("author ") {
                authors.insert(cur_id.clone(), a.to_string());
            } else {
                let mut it = l.split(' ');
                if let (Some(id), Some(_), Some(fin)) = (it.next(), it.next(), it.next())
                    && id.len() >= 40
                    && id.chars().all(|c| c.is_ascii_hexdigit())
                {
                    cur_id = id.to_string();
                    cur_line = fin.parse().unwrap_or(0);
                }
            }
        }
        Ok(lines)
    }

    pub fn reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        let out = self.git(&[
            "reflog".to_string(),
            format!("-n{limit}"),
            "--format=%H%x1f%gd%x1f%gs%x1f%at".to_string(),
        ])?;
        Ok(lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                (f.len() == 4).then(|| ReflogEntry {
                    id: f[0].to_string(),
                    selector: f[1].to_string(),
                    message: f[2].to_string(),
                    time: f[3].parse().unwrap_or(0),
                })
            })
            .collect())
    }

    // ---------------------------------------------------------------- index & working tree

    pub fn stage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            self.git(&["add", "-A"])?;
        } else {
            self.git(&with_paths(strs(&["add", "-A"]), paths))?;
        }
        Ok(())
    }

    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        let all = [".".to_string()];
        let paths = if paths.is_empty() { &all[..] } else { paths };
        if self.has_head() {
            self.git(&with_paths(strs(&["reset", "-q"]), paths))?;
        } else {
            self.git(&with_paths(strs(&["rm", "--cached", "-r", "-q"]), paths))?;
        }
        Ok(())
    }

    /// Throws away changes of one file. With `include_staged` the staged changes go too (the file
    /// returns to HEAD); otherwise only the working-tree changes are discarded.
    pub fn discard(&self, change: &FileChange, include_staged: bool) -> Result<()> {
        if change.is_untracked() {
            self.git(&with_paths(
                strs(&["clean", "-f", "-q"]),
                std::slice::from_ref(&change.path),
            ))?;
        } else if include_staged && matches!(change.index, 'A' | 'R' | 'C') {
            self.git(&with_paths(
                strs(&["rm", "-f", "-q"]),
                std::slice::from_ref(&change.path),
            ))?;
            if let Some(orig) = &change.orig_path.clone().filter(|_| change.index == 'R') {
                self.git(&with_paths(
                    strs(&["restore", "--source=HEAD", "--staged", "--worktree"]),
                    std::slice::from_ref(orig),
                ))?;
            }
        } else if include_staged {
            self.git(&with_paths(
                strs(&["restore", "--source=HEAD", "--staged", "--worktree"]),
                std::slice::from_ref(&change.path),
            ))?;
        } else {
            self.git(&with_paths(
                strs(&["restore", "--worktree"]),
                std::slice::from_ref(&change.path),
            ))?;
        }
        Ok(())
    }

    /// Applies a patch from [`crate::diffparse::build_patch`] to the index or working tree.
    pub fn apply_patch(&self, patch: &str, mode: PatchMode) -> Result<()> {
        let mut args = strs(&["apply", "--recount", "--whitespace=nowarn"]);
        match mode {
            PatchMode::Stage => args.push("--cached".into()),
            PatchMode::Unstage => {
                args.push("--cached".into());
                args.push("--reverse".into());
            }
            PatchMode::Discard => args.push("--reverse".into()),
        }
        args.push("-".into());
        self.runner.run_opts(
            &args,
            Some(&self.repo),
            &RunOpts {
                input: Some(patch.as_bytes()),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// Appends a pattern to the repository's `.gitignore`.
    pub fn add_to_gitignore(&self, pattern: &str) -> Result<()> {
        use std::io::Write;
        let p = self.repo.join(".gitignore");
        let existing = std::fs::read_to_string(&p).unwrap_or_default();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        writeln!(f, "{pattern}")?;
        Ok(())
    }

    // ---------------------------------------------------------------- commits

    pub fn commit(&self, message: &str) -> Result<()> {
        self.git(&["commit", "-q", "-m", message])?;
        Ok(())
    }

    /// Replaces the last commit with the staged changes and a new message.
    pub fn commit_amend(&self, message: &str) -> Result<()> {
        self.git(&["commit", "-q", "--amend", "-m", message])?;
        Ok(())
    }

    pub fn identity(&self) -> (Option<String>, Option<String>) {
        let get = |k: &str| {
            self.git(&["config", "--get", k])
                .ok()
                .map(|o| lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        (get("user.name"), get("user.email"))
    }

    /// Sets the global git identity.
    pub fn set_identity(&self, name: &str, email: &str) -> Result<()> {
        self.git(&["config", "--global", "user.name", name])?;
        self.git(&["config", "--global", "user.email", email])?;
        Ok(())
    }

    pub fn cherry_pick(&self, id: &str) -> Result<OpResult> {
        let r = self.git(&["cherry-pick", id]);
        self.finish(r)
    }

    pub fn revert(&self, id: &str) -> Result<OpResult> {
        let r = self.git(&["revert", "--no-edit", id]);
        self.finish(r)
    }

    pub fn reset(&self, id: &str, mode: ResetMode) -> Result<()> {
        let m = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.git(&["reset", m, id])?;
        Ok(())
    }

    // ---------------------------------------------------------------- branches & tags

    pub fn checkout(&self, branch: &Branch) -> Result<()> {
        if branch.is_remote {
            self.git(&["switch", "--track", &branch.name])?;
        } else {
            self.git(&["switch", &branch.name])?;
        }
        Ok(())
    }

    /// Checks out a commit (detached HEAD).
    pub fn checkout_commit(&self, id: &str) -> Result<()> {
        self.git(&["switch", "--detach", id])?;
        Ok(())
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        self.git(&["switch", "-c", name])?;
        Ok(())
    }

    /// Creates a branch at `id`, optionally switching to it.
    pub fn create_branch_at(&self, name: &str, id: &str, switch: bool) -> Result<()> {
        if switch {
            self.git(&["switch", "-c", name, id])?;
        } else {
            self.git(&["branch", name, id])?;
        }
        Ok(())
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        self.git(&["branch", "-m", old, new])?;
        Ok(())
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        self.git(&["branch", if force { "-D" } else { "-d" }, name])?;
        Ok(())
    }

    /// Deletes a branch on its remote; `name` is like `origin/feature`.
    pub fn delete_remote_branch(&self, name: &str) -> Result<()> {
        let (remote, branch) = name
            .split_once('/')
            .ok_or_else(|| GitError::Parse(format!("not a remote branch: {name}")))?;
        self.git(&["push", remote, "--delete", branch])?;
        Ok(())
    }

    pub fn set_upstream(&self, branch: &str, upstream: &str) -> Result<()> {
        self.git(&["branch", "--set-upstream-to", upstream, branch])?;
        Ok(())
    }

    pub fn create_tag(&self, name: &str, target: Option<&str>, message: Option<&str>) -> Result<()> {
        let mut args = strs(&["tag"]);
        if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
            args.extend(["-a".into(), "-m".into(), m.to_string()]);
        }
        args.push(name.to_string());
        if let Some(t) = target {
            args.push(t.to_string());
        }
        self.git(&args)?;
        Ok(())
    }

    pub fn delete_tag(&self, name: &str) -> Result<()> {
        self.git(&["tag", "-d", name])?;
        Ok(())
    }

    pub(crate) fn first_remote(&self) -> Result<String> {
        let out = self.git(&["remote"])?;
        lossy(&out.stdout)
            .lines()
            .next()
            .map(|s| s.trim().to_string())
            .ok_or(GitError::NoRemote)
    }

    pub fn push_tag(&self, name: &str, cancel: Option<&CancelToken>) -> Result<()> {
        let remote = self.first_remote()?;
        self.git_cancel(&["push", &remote, &format!("refs/tags/{name}")], cancel)?;
        Ok(())
    }

    // ---------------------------------------------------------------- remotes

    /// Fetch for background use: like [`fetch`](Self::fetch) but fully non-interactive, so it fails
    /// (instead of prompting) when credentials are needed.
    pub fn fetch_background(&self, cancel: Option<&CancelToken>) -> Result<()> {
        self.runner.run_opts(
            &["fetch", "--all", "--prune", "--quiet"],
            Some(&self.repo),
            &RunOpts {
                cancel,
                non_interactive: true,
                ..Default::default()
            },
        )?;
        Ok(())
    }

    pub fn fetch(&self, cancel: Option<&CancelToken>) -> Result<()> {
        self.git_cancel(&["fetch", "--all", "--prune"], cancel)?;
        Ok(())
    }

    pub fn pull(&self, mode: PullMode, cancel: Option<&CancelToken>) -> Result<OpResult> {
        let flag = match mode {
            PullMode::FfOnly => "--ff-only",
            PullMode::Merge => "--no-rebase",
            PullMode::Rebase => "--rebase",
        };
        let r = self.git_cancel(&["pull", flag], cancel);
        self.finish(r)
    }

    /// Pushes the current branch; sets the upstream when there is none yet. With `force_lease`
    /// the push uses `--force-with-lease`.
    pub fn push(&self, force_lease: bool, cancel: Option<&CancelToken>) -> Result<()> {
        let mut args = strs(&["push"]);
        if force_lease {
            args.push("--force-with-lease".into());
        }
        if self.status()?.upstream.is_none() {
            let remote = self.first_remote()?;
            args.extend(["-u".into(), remote, "HEAD".into()]);
        }
        self.git_cancel(&args, cancel)?;
        Ok(())
    }

    // ---------------------------------------------------------------- merge, rebase & friends

    pub fn merge(&self, branch: &str) -> Result<OpResult> {
        let r = self.git(&["merge", "--no-edit", branch]);
        self.finish(r)
    }

    pub fn rebase(&self, onto: &str) -> Result<OpResult> {
        let r = self.git(&["rebase", onto]);
        self.finish(r)
    }

    /// Interactive rebase of everything after `base`, driven by `items` (oldest first).
    pub fn interactive_rebase(&self, base: &str, items: &[TodoItem]) -> Result<OpResult> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("rustle-rebase-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let mut todo = String::new();
        for (i, it) in items.iter().enumerate() {
            match it.action {
                TodoAction::Pick => todo.push_str(&format!("pick {}\n", it.id)),
                TodoAction::Squash => todo.push_str(&format!("squash {}\n", it.id)),
                TodoAction::Fixup => todo.push_str(&format!("fixup {}\n", it.id)),
                TodoAction::Drop => todo.push_str(&format!("drop {}\n", it.id)),
                TodoAction::Reword => {
                    let msg = dir.join(format!("msg{i}.txt"));
                    std::fs::write(&msg, it.message.clone().unwrap_or_default())?;
                    todo.push_str(&format!(
                        "pick {}\nexec git commit --amend -q -F \"{}\"\n",
                        it.id,
                        fwd(&msg)
                    ));
                }
            }
        }
        let todo_path = dir.join("todo.txt");
        std::fs::write(&todo_path, todo)?;
        let editor = format!("cp \"{}\"", fwd(&todo_path));
        let r = self.runner.run_opts(
            &["rebase", "-i", base],
            Some(&self.repo),
            &RunOpts {
                env: &[("GIT_SEQUENCE_EDITOR", &editor)],
                ..Default::default()
            },
        );
        let res = self.finish(r);
        if matches!(res, Ok(OpResult::Done)) {
            let _ = std::fs::remove_dir_all(&dir);
        }
        res
    }

    /// Continues the operation that is in progress (after resolving conflicts).
    pub fn op_continue(&self) -> Result<OpResult> {
        let r = match self.op_state() {
            OpState::Merge => self.git(&["commit", "--no-edit", "-q"]),
            OpState::Rebase => self.git(&["rebase", "--continue"]),
            OpState::CherryPick => self.git(&["cherry-pick", "--continue"]),
            OpState::Revert => self.git(&["revert", "--continue"]),
            OpState::None => return Ok(OpResult::Done),
        };
        self.finish(r)
    }

    pub fn op_continue_msg(&self, message: &str) -> Result<OpResult> {
        if self.op_state() == OpState::Merge && !message.trim().is_empty() {
            let r = self.git(&["commit", "-q", "-m", message.trim()]);
            return self.finish(r);
        }
        self.op_continue()
    }

    pub fn op_abort(&self) -> Result<()> {
        match self.op_state() {
            OpState::Merge => self.git(&["merge", "--abort"])?,
            OpState::Rebase => self.git(&["rebase", "--abort"])?,
            OpState::CherryPick => self.git(&["cherry-pick", "--abort"])?,
            OpState::Revert => self.git(&["revert", "--abort"])?,
            OpState::None => return Ok(()),
        };
        Ok(())
    }

    /// Skips the current commit during a rebase.
    pub fn rebase_skip(&self) -> Result<OpResult> {
        let r = self.git(&["rebase", "--skip"]);
        self.finish(r)
    }

    // ---------------------------------------------------------------- conflicts

    pub fn conflict_versions(&self, path: &str) -> Result<ConflictVersions> {
        let stage = |n: u8| {
            self.git(&["show", &format!(":{n}:{path}")])
                .ok()
                .map(|o| lossy(&o.stdout))
        };
        Ok(ConflictVersions {
            base: stage(1),
            ours: stage(2),
            theirs: stage(3),
        })
    }

    /// Resolves a conflicted file by taking one side completely, and stages the result.
    pub fn resolve(&self, path: &str, side: Side) -> Result<()> {
        let s = match side {
            Side::Ours => "--ours",
            Side::Theirs => "--theirs",
        };
        self.git(&["checkout", s, "--", path])?;
        self.git(&["add", "--", path])?;
        Ok(())
    }

    // ---------------------------------------------------------------- stash

    pub fn stash_list(&self) -> Result<Vec<StashEntry>> {
        let out = self.git(&["stash", "list", "--format=%gd%x1f%s"])?;
        Ok(lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (sel, msg) = l.split_once('\x1f')?;
                let index = sel.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
                Some(StashEntry {
                    index,
                    message: msg.to_string(),
                })
            })
            .collect())
    }

    pub fn stash_push(&self, message: &str, include_untracked: bool) -> Result<()> {
        let mut args = strs(&["stash", "push"]);
        if include_untracked {
            args.push("-u".into());
        }
        if !message.trim().is_empty() {
            args.extend(["-m".into(), message.to_string()]);
        }
        self.git(&args)?;
        Ok(())
    }

    pub fn stash_apply(&self, index: usize) -> Result<OpResult> {
        let r = self.git(&["stash", "apply", &format!("stash@{{{index}}}")]);
        self.finish(r)
    }

    pub fn stash_pop(&self, index: usize) -> Result<OpResult> {
        let r = self.git(&["stash", "pop", &format!("stash@{{{index}}}")]);
        self.finish(r)
    }

    pub fn stash_drop(&self, index: usize) -> Result<()> {
        self.git(&["stash", "drop", &format!("stash@{{{index}}}")])?;
        Ok(())
    }

    // ---------------------------------------------------------------- submodules

    pub fn submodules(&self) -> Result<Vec<Submodule>> {
        if !self.repo.join(".gitmodules").exists() {
            return Ok(Vec::new());
        }
        let out = self.git(&["submodule", "status"])?;
        Ok(lossy(&out.stdout)
            .lines()
            .filter(|l| l.len() > 42)
            .map(|l| {
                let status = l.chars().next().unwrap_or(' ');
                let rest = &l[1..];
                let (id, tail) = rest.split_once(' ').unwrap_or((rest, ""));
                let path = tail.split(" (").next().unwrap_or(tail).trim();
                Submodule {
                    path: path.to_string(),
                    id: id.to_string(),
                    status,
                }
            })
            .collect())
    }

    pub fn submodule_update(&self, cancel: Option<&CancelToken>) -> Result<()> {
        self.git_cancel(&["submodule", "update", "--init", "--recursive"], cancel)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_track;

    #[test]
    fn parses_tracking_info() {
        assert_eq!(parse_track(""), (0, 0, false));
        assert_eq!(parse_track("ahead 2"), (2, 0, false));
        assert_eq!(parse_track("behind 13"), (0, 13, false));
        assert_eq!(parse_track("ahead 2, behind 1"), (2, 1, false));
        assert_eq!(parse_track("gone"), (0, 0, true));
    }
}
