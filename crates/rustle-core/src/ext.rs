//! Further git operations: per-branch push/fast-forward, commit options (hooks, signing), remotes,
//! configuration, search, merge helpers, worktrees, submodules and LFS.

use crate::backend::CliBackend;
use crate::error::{GitError, Result};
use crate::model::{ConfigValue, GrepHit, LfsInfo, OpResult, Remote, SignatureInfo, Submodule, Worktree};
use crate::pathenc::{decode_path, encode_path};
use crate::runner::{CancelToken, RunOpts};
use std::collections::HashSet;
use std::path::Path;

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// stdout followed by stderr, trimmed; hooks write their messages to either.
fn combined(out: &std::process::Output) -> String {
    let mut s = lossy(&out.stdout);
    let e = lossy(&out.stderr);
    if !e.trim().is_empty() {
        if !s.trim().is_empty() {
            s.push('\n');
        }
        s.push_str(&e);
    }
    s.trim().to_string()
}

impl CliBackend {
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Id of the commit HEAD points to, if any.
    pub fn head_id(&self) -> Option<String> {
        let out = self.git(&["rev-parse", "--verify", "-q", "HEAD"]).ok()?;
        Some(lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
    }

    // ---------------------------------------------------------------- per-branch push / fast-forward

    /// Pushes a local branch to its upstream, or publishes it (`-u`) to the first remote.
    pub fn push_branch(&self, name: &str, cancel: Option<&CancelToken>) -> Result<()> {
        let upstream = self
            .branches()?
            .into_iter()
            .find(|b| !b.is_remote && b.name == name)
            .and_then(|b| b.upstream);
        match upstream {
            Some(up) => {
                let (remote, branch) = up
                    .split_once('/')
                    .ok_or_else(|| GitError::Parse(format!("unexpected upstream: {up}")))?;
                let spec = format!("refs/heads/{name}:refs/heads/{branch}");
                self.git_cancel(&["push", remote, &spec], cancel)?;
            }
            None => {
                let remote = self.first_remote()?;
                self.git_cancel(&["push", "-u", &remote, name], cancel)?;
            }
        }
        Ok(())
    }

    /// Fast-forwards a branch that is not checked out to its upstream (fails if it would not be
    /// a fast-forward).
    pub fn fast_forward_branch(&self, name: &str, cancel: Option<&CancelToken>) -> Result<()> {
        let up = self
            .branches()?
            .into_iter()
            .find(|b| !b.is_remote && b.name == name)
            .and_then(|b| b.upstream)
            .ok_or_else(|| GitError::Parse(format!("{name} has no upstream")))?;
        let (remote, branch) = up
            .split_once('/')
            .ok_or_else(|| GitError::Parse(format!("unexpected upstream: {up}")))?;
        self.git_cancel(&["fetch", remote, &format!("{branch}:{name}")], cancel)?;
        Ok(())
    }

    // ---------------------------------------------------------------- commit options

    /// Commits the index. `sign`: `Some(true)` forces `-S`, `Some(false)` forces `--no-gpg-sign`,
    /// `None` follows `commit.gpgsign`. Returns what hooks printed.
    pub fn commit_with(
        &self,
        message: &str,
        amend: bool,
        sign: Option<bool>,
        no_verify: bool,
    ) -> Result<String> {
        let mut a = args(&["commit", "-q"]);
        if amend {
            a.push("--amend".into());
        }
        match sign {
            Some(true) => a.push("-S".into()),
            Some(false) => a.push("--no-gpg-sign".into()),
            None => {}
        }
        if no_verify {
            a.push("--no-verify".into());
        }
        a.extend(["-m".into(), message.to_string()]);
        Ok(combined(&self.git(&a)?))
    }

    /// Whether `commit.gpgsign` is switched on.
    pub fn signing_default(&self) -> bool {
        self.git(&["config", "--type=bool", "--get", "commit.gpgsign"])
            .map(|o| lossy(&o.stdout).trim() == "true")
            .unwrap_or(false)
    }

    /// Signature status of a commit (verifies it, so only ask for one commit at a time).
    pub fn signature(&self, id: &str) -> Result<SignatureInfo> {
        let out = self.git(&["log", "-1", "--format=%G?%x1f%GS", id])?;
        let s = lossy(&out.stdout);
        let (status, signer) = s.trim_end().split_once('\x1f').unwrap_or((s.trim_end(), ""));
        Ok(SignatureInfo {
            status: status.chars().next().unwrap_or('N'),
            signer: signer.to_string(),
        })
    }

    // ---------------------------------------------------------------- remotes

    pub fn remotes(&self) -> Result<Vec<Remote>> {
        let out = self.git(&["remote", "-v"])?;
        let mut v: Vec<Remote> = Vec::new();
        for line in lossy(&out.stdout).lines() {
            let Some((name, rest)) = line.split_once('\t') else {
                continue;
            };
            let (url, kind) = rest.rsplit_once(' ').unwrap_or((rest, "(fetch)"));
            let r = match v.iter_mut().find(|r| r.name == name) {
                Some(r) => r,
                None => {
                    v.push(Remote {
                        name: name.to_string(),
                        fetch_url: String::new(),
                        push_url: String::new(),
                    });
                    v.last_mut().expect("just pushed")
                }
            };
            if kind == "(push)" {
                r.push_url = url.to_string();
            } else {
                r.fetch_url = url.to_string();
            }
        }
        Ok(v)
    }

    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.git(&["remote", "add", name, url])?;
        Ok(())
    }

    pub fn rename_remote(&self, old: &str, new: &str) -> Result<()> {
        self.git(&["remote", "rename", old, new])?;
        Ok(())
    }

    pub fn set_remote_url(&self, name: &str, url: &str) -> Result<()> {
        self.git(&["remote", "set-url", name, url])?;
        Ok(())
    }

    pub fn remove_remote(&self, name: &str) -> Result<()> {
        self.git(&["remote", "remove", name])?;
        Ok(())
    }

    /// Removes remote-tracking branches that no longer exist on the remote.
    pub fn prune_remote(&self, name: &str, cancel: Option<&CancelToken>) -> Result<()> {
        self.git_cancel(&["remote", "prune", name], cancel)?;
        Ok(())
    }

    // ---------------------------------------------------------------- configuration

    /// A configuration value and the scope it comes from.
    pub fn get_config(&self, key: &str) -> ConfigValue {
        match self.git(&["config", "--show-scope", "--get", key]) {
            Ok(o) => {
                let s = lossy(&o.stdout);
                let line = s.lines().next().unwrap_or("");
                match line.split_once('\t') {
                    Some((scope, value)) => ConfigValue {
                        value: Some(value.to_string()),
                        scope: Some(scope.to_string()),
                    },
                    None => ConfigValue::default(),
                }
            }
            Err(_) => ConfigValue::default(),
        }
    }

    /// Sets a value in the repository (`global = false`) or the user configuration.
    pub fn set_config(&self, key: &str, value: &str, global: bool) -> Result<()> {
        let scope = if global { "--global" } else { "--local" };
        self.git(&["config", scope, key, value])?;
        Ok(())
    }

    pub fn unset_config(&self, key: &str, global: bool) -> Result<()> {
        let scope = if global { "--global" } else { "--local" };
        match self.git(&["config", scope, "--unset", key]) {
            // Exit code 5: the key was not set in that scope.
            Err(GitError::Failed { code: Some(5), .. }) => Ok(()),
            r => r.map(|_| ()),
        }
    }

    // ---------------------------------------------------------------- several commits at once

    /// Cherry-picks the commits in the given order (oldest first).
    pub fn cherry_pick_many(&self, ids: &[String]) -> Result<OpResult> {
        let mut a = args(&["cherry-pick"]);
        a.extend(ids.iter().cloned());
        let r = self.git(&a);
        self.finish(r)
    }

    /// Reverts the commits in the given order (newest first is the safest).
    pub fn revert_many(&self, ids: &[String]) -> Result<OpResult> {
        let mut a = args(&["revert", "--no-edit"]);
        a.extend(ids.iter().cloned());
        let r = self.git(&a);
        self.finish(r)
    }

    // ---------------------------------------------------------------- search and file browsing

    /// Searches for a fixed string in the working tree (`rev = None`) or in a commit.
    pub fn grep(
        &self,
        pattern: &str,
        rev: Option<&str>,
        ignore_case: bool,
        limit: usize,
    ) -> Result<Vec<GrepHit>> {
        let mut a = args(&["grep", "-z", "-n", "-I", "--no-color", "-F"]);
        if ignore_case {
            a.push("-i".into());
        }
        a.extend(["-e".into(), pattern.to_string()]);
        if let Some(r) = rev {
            a.push(r.to_string());
        }
        let out = match self.git(&a) {
            Ok(o) => o,
            // Exit code 1 means "no matches".
            Err(GitError::Failed { code: Some(1), .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let prefix = rev.map(|r| format!("{r}:"));
        let mut hits = Vec::new();
        for line in out.stdout.split(|&b| b == b'\n') {
            let mut it = line.splitn(3, |&b| b == 0);
            let (Some(path), Some(no), Some(text)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            let mut path = decode_path(path);
            if let Some(p) = prefix.as_ref().and_then(|p| path.strip_prefix(p.as_str())) {
                path = p.to_string();
            }
            hits.push(GrepHit {
                path,
                line: lossy(no).parse().unwrap_or(0),
                text: lossy(text),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// Tracked files of the working tree.
    pub fn ls_files(&self) -> Result<Vec<String>> {
        let out = self.git(&["ls-files", "-z"])?;
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(decode_path)
            .collect())
    }

    /// All files in a commit.
    pub fn ls_tree(&self, rev: &str) -> Result<Vec<String>> {
        let out = self.git(&["ls-tree", "-r", "-z", "--name-only", rev])?;
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(decode_path)
            .collect())
    }

    /// Contents of a blob: `HEAD:path`, `<id>:path`, `:path` (index) and so on.
    pub fn blob(&self, spec: &str) -> Result<Vec<u8>> {
        Ok(self.git(&["cat-file", "blob", spec])?.stdout)
    }

    /// Contents of a file in the working tree.
    pub fn worktree_file(&self, path: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.repo.join(encode_path(path)))?)
    }

    // ---------------------------------------------------------------- conflicts

    /// The conflicted file as a three-way merge with diff3 markers, built from the index stages
    /// (the working-tree file is left alone). Fails for files that are not valid UTF-8.
    pub fn conflict_merge(&self, path: &str) -> Result<String> {
        let stage = |n: u8| {
            self.git(&["show", &format!(":{n}:{path}")])
                .map(|o| o.stdout)
                .unwrap_or_default()
        };
        let (base, ours, theirs) = (stage(1), stage(2), stage(3));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("rustle-merge-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let result = (|| {
            let (fo, fb, ft) = (dir.join("ours"), dir.join("base"), dir.join("theirs"));
            std::fs::write(&fo, &ours)?;
            std::fs::write(&fb, &base)?;
            std::fs::write(&ft, &theirs)?;
            let a = [
                "merge-file".to_string(),
                "-p".into(),
                "--diff3".into(),
                "-L".into(),
                "ours".into(),
                "-L".into(),
                "base".into(),
                "-L".into(),
                "theirs".into(),
                fo.to_string_lossy().into_owned(),
                fb.to_string_lossy().into_owned(),
                ft.to_string_lossy().into_owned(),
            ];
            let out = self.runner.run_opts(
                &a,
                Some(&self.repo),
                &RunOpts {
                    max_ok_exit: Some(127),
                    ..Default::default()
                },
            )?;
            String::from_utf8(out.stdout)
                .map_err(|_| GitError::Parse("the file is not valid UTF-8 text".into()))
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Writes the resolved content of a conflicted file and stages it.
    pub fn write_resolved(&self, path: &str, content: &str) -> Result<()> {
        std::fs::write(self.repo.join(encode_path(path)), content)?;
        self.git(&[std::ffi::OsString::from("add"), "--".into(), encode_path(path)])?;
        Ok(())
    }

    // ---------------------------------------------------------------- worktrees

    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let out = self.git(&["worktree", "list", "--porcelain"])?;
        let text = lossy(&out.stdout);
        let mut v = Vec::new();
        for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
            let mut w = Worktree {
                path: String::new(),
                head: String::new(),
                branch: None,
                is_main: v.is_empty(),
                locked: false,
                prunable: false,
            };
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    w.path = p.to_string();
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    w.head = h.to_string();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
                } else if line.starts_with("locked") {
                    w.locked = true;
                } else if line.starts_with("prunable") {
                    w.prunable = true;
                }
            }
            v.push(w);
        }
        Ok(v)
    }

    /// Adds a worktree at `path`; with `create` a new branch `branch` is made.
    pub fn add_worktree(&self, path: &str, branch: &str, create: bool) -> Result<()> {
        if create {
            self.git(&["worktree", "add", "-b", branch, path])?;
        } else {
            self.git(&["worktree", "add", path, branch])?;
        }
        Ok(())
    }

    pub fn remove_worktree(&self, path: &str, force: bool) -> Result<()> {
        if force {
            self.git(&["worktree", "remove", "--force", path])?;
        } else {
            self.git(&["worktree", "remove", path])?;
        }
        Ok(())
    }

    pub fn prune_worktrees(&self) -> Result<()> {
        self.git(&["worktree", "prune"])?;
        Ok(())
    }

    // ---------------------------------------------------------------- submodules

    /// `git submodule update --init [--remote] --recursive [-- path]`.
    pub fn submodule_sync(
        &self,
        path: Option<&str>,
        remote: bool,
        cancel: Option<&CancelToken>,
    ) -> Result<()> {
        let mut a = args(&["submodule", "update", "--init", "--recursive"]);
        if remote {
            a.push("--remote".into());
        }
        if let Some(p) = path {
            a.push("--".into());
            a.push(p.to_string());
        }
        self.git_cancel(&a, cancel)?;
        Ok(())
    }

    pub fn add_submodule(&self, url: &str, path: &str, cancel: Option<&CancelToken>) -> Result<()> {
        self.git_cancel(&["submodule", "add", "--", url, path], cancel)?;
        Ok(())
    }

    /// Whether a submodule has local changes or a different commit than recorded.
    pub fn submodule_is_modified(&self, s: &Submodule) -> bool {
        s.status == '+' || s.status == 'U'
    }

    // ---------------------------------------------------------------- Git LFS

    /// LFS usage: patterns from `.gitattributes`, and (when `git lfs` is installed) the stored files.
    pub fn lfs_info(&self) -> LfsInfo {
        let attrs = std::fs::read_to_string(self.repo.join(".gitattributes")).unwrap_or_default();
        let patterns: Vec<String> = attrs
            .lines()
            .filter(|l| l.contains("filter=lfs"))
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect();
        // Only spawn `git lfs` when the repository actually uses it.
        let available = !patterns.is_empty() && self.git(&["lfs", "version"]).is_ok();
        let files = if available && !patterns.is_empty() {
            self.git(&["lfs", "ls-files", "-n"])
                .map(|o| {
                    lossy(&o.stdout)
                        .lines()
                        .map(str::to_string)
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default()
        } else {
            HashSet::new()
        };
        LfsInfo {
            available,
            patterns,
            files,
        }
    }

    pub fn lfs_track(&self, pattern: &str) -> Result<()> {
        self.git(&["lfs", "track", pattern])?;
        Ok(())
    }

    pub fn lfs_pull(&self, cancel: Option<&CancelToken>) -> Result<()> {
        self.git_cancel(&["lfs", "pull"], cancel)?;
        Ok(())
    }
}
