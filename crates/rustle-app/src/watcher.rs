//! Watches a repository for changes made outside Rustle (editor saves, other git tools).
//!
//! Events are debounced and sorted into two kinds: changes to files in the working tree (and the
//! index) only need a new `git status`, changes to refs, HEAD or an operation in progress need a
//! full reload.

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, DebouncedEventBatch, Debouncer, new_debouncer};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Files or the index changed.
    Worktree,
    /// Refs, HEAD or the state of a merge/rebase changed.
    Refs,
}

/// Lowercase, forward-slash form used to compare paths.
pub fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Folders whose churn is never interesting for `git status`.
const NOISY: [&str; 5] = ["target", "node_modules", ".gradle", "__pycache__", ".idea"];

/// Sorts a changed path (normalised with [`norm`]) into a [`Change`], or ignores it.
pub fn classify(root: &str, git_dir: &str, path: &str) -> Option<Change> {
    if let Some(rel) = path.strip_prefix(git_dir).and_then(|r| r.strip_prefix('/')) {
        let first = rel.split('/').next().unwrap_or("");
        return match first {
            "objects" | "logs" | "hooks" | "info" | "lfs" | "" => None,
            f if f.ends_with(".lock") => None,
            "index" => Some(Change::Worktree),
            "refs" | "head" | "packed-refs" | "merge_head" | "cherry_pick_head" | "revert_head"
            | "rebase-merge" | "rebase-apply" | "orig_head" | "fetch_head" => Some(Change::Refs),
            _ => None,
        };
    }
    let rel = path
        .strip_prefix(root)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(path);
    let mut parts = rel.split('/');
    if parts.any(|c| c == ".git" || NOISY.contains(&c)) {
        return None;
    }
    Some(Change::Worktree)
}

pub struct RepoWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl RepoWatcher {
    /// Starts watching `root` (and `git_dir` when it lives elsewhere, as in a linked worktree).
    pub fn new(
        root: &Path,
        git_dir: &Path,
        tx: Sender<Change>,
        ctx: eframe::egui::Context,
    ) -> Result<Self, String> {
        let (root_n, git_n) = (norm(root), norm(git_dir));
        let mut debouncer = new_debouncer(Duration::from_millis(400), move |res: DebounceEventResult| {
            let Ok(batch) = res else { return };
            let (mut wt, mut refs) = (false, false);
            match batch {
                DebouncedEventBatch::Events(events) => {
                    for e in events {
                        match classify(&root_n, &git_n, &norm(&e.path)) {
                            Some(Change::Refs) => refs = true,
                            Some(Change::Worktree) => wt = true,
                            None => {}
                        }
                    }
                }
                // Events were missed: reload everything.
                DebouncedEventBatch::Rescan => refs = true,
                #[allow(unreachable_patterns)]
                _ => refs = true,
            }
            if refs {
                let _ = tx.send(Change::Refs);
            } else if wt {
                let _ = tx.send(Change::Worktree);
            }
            if refs || wt {
                ctx.request_repaint();
            }
        })
        .map_err(|e| e.to_string())?;
        debouncer
            .watcher()
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
        if !norm(git_dir).starts_with(&norm(root)) {
            debouncer
                .watcher()
                .watch(git_dir, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            _debouncer: debouncer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn classifies_paths() {
        let (r, g) = ("c:/repo", "c:/repo/.git");
        assert_eq!(classify(r, g, "c:/repo/src/main.rs"), Some(Change::Worktree));
        assert_eq!(classify(r, g, "c:/repo/.git/index"), Some(Change::Worktree));
        assert_eq!(classify(r, g, "c:/repo/.git/head"), Some(Change::Refs));
        assert_eq!(classify(r, g, "c:/repo/.git/refs/heads/main"), Some(Change::Refs));
        assert_eq!(classify(r, g, "c:/repo/.git/packed-refs"), Some(Change::Refs));
        assert_eq!(
            classify(r, g, "c:/repo/.git/rebase-merge/done"),
            Some(Change::Refs)
        );
        // Noise is ignored.
        assert_eq!(classify(r, g, "c:/repo/.git/objects/ab/cdef"), None);
        assert_eq!(classify(r, g, "c:/repo/.git/index.lock"), None);
        assert_eq!(classify(r, g, "c:/repo/.git/logs/HEAD"), None);
        assert_eq!(classify(r, g, "c:/repo/target/debug/app.exe"), None);
        assert_eq!(classify(r, g, "c:/repo/web/node_modules/x/index.js"), None);
    }

    #[test]
    fn linked_worktree_with_git_dir_elsewhere() {
        let (r, g) = ("c:/work/wt", "c:/main/.git/worktrees/wt");
        assert_eq!(
            classify(r, g, "c:/main/.git/worktrees/wt/head"),
            Some(Change::Refs)
        );
        assert_eq!(classify(r, g, "c:/work/wt/a.txt"), Some(Change::Worktree));
    }

    #[test]
    fn reports_real_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let (tx, rx) = channel();
        let _w = RepoWatcher::new(
            dir.path(),
            &dir.path().join(".git"),
            tx,
            eframe::egui::Context::default(),
        )
        .expect("watcher");
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).ok(),
            Some(Change::Worktree)
        );

        std::fs::create_dir_all(dir.path().join(".git/refs/heads")).unwrap();
        std::fs::write(dir.path().join(".git/refs/heads/main"), "abc").unwrap();
        // Drain until the refs event arrives (a worktree event for the new folder may come first).
        let mut got_refs = false;
        while let Ok(c) = rx.recv_timeout(Duration::from_secs(5)) {
            if c == Change::Refs {
                got_refs = true;
                break;
            }
        }
        assert!(got_refs);
    }
}
