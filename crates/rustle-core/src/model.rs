#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

/// A branch/tag label next to a commit in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    pub is_head: bool,
}

/// A commit as shown in the log. Text fields are lossily converted to UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// Unix timestamp (seconds).
    pub time: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

impl Commit {
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(7)]
    }
}

/// A changed file from `git status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Original path for a rename/copy.
    pub orig_path: Option<String>,
    /// Status in the index (`.` = unchanged, `?` = untracked).
    pub index: char,
    /// Status in the working tree (`.` = unchanged, `?` = untracked).
    pub worktree: char,
    pub conflicted: bool,
}

impl FileChange {
    pub fn is_untracked(&self) -> bool {
        self.index == '?'
    }

    pub fn is_staged(&self) -> bool {
        !self.conflicted && self.index != '.' && self.index != '?'
    }

    pub fn is_unstaged(&self) -> bool {
        self.conflicted || self.worktree != '.'
    }

    /// Paths to pass to git to (un)stage this file.
    pub fn paths(&self) -> Vec<String> {
        let mut v = vec![self.path.clone()];
        if let Some(o) = &self.orig_path {
            v.push(o.clone());
        }
        v
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileChange>,
    /// A merge/rebase/cherry-pick/revert that is in progress.
    pub op: OpState,
    /// (path, staged) -> (added, deleted) line counts.
    pub stats: std::collections::HashMap<(String, bool), (u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Short name, e.g. `main` or `origin/main`.
    pub name: String,
    pub is_head: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    /// Commits on this branch that its upstream does not have yet (to push).
    pub ahead: u32,
    /// Commits on the upstream that this branch does not have yet (to pull).
    pub behind: u32,
    /// The upstream branch no longer exists on the remote.
    pub gone: bool,
}

/// A multi-step operation that is in progress in the repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpState {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// Result of an operation that can stop on conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpResult {
    Done,
    Conflicts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub path: String,
    pub id: String,
    /// `' '` up to date, `'-'` not initialised, `'+'` different commit, `'U'` conflicts.
    pub status: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    pub id: String,
    pub selector: String,
    pub message: String,
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub id: String,
    pub author: String,
    pub number: u32,
    pub text: String,
}

/// A file changed by a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub status: char,
    pub path: String,
    pub orig_path: Option<String>,
    pub added: Option<u32>,
    pub deleted: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictVersions {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PullMode {
    #[default]
    FfOnly,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub action: TodoAction,
    pub id: String,
    /// New message for `Reword`.
    pub message: Option<String>,
}

/// How a patch is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    /// Apply to the index.
    Stage,
    /// Reverse-apply to the index.
    Unstage,
    /// Reverse-apply to the working tree.
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOpts {
    pub ignore_whitespace: bool,
    pub context: u32,
}

impl Default for DiffOpts {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            context: 3,
        }
    }
}

/// Filters for the commit log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    pub limit: usize,
    pub skip: usize,
    /// Match in the commit message (case-insensitive, literal).
    pub text: Option<String>,
    pub author: Option<String>,
    /// Only commits touching this path (follows renames).
    pub path: Option<String>,
    /// Only the current branch instead of all branches, remotes and tags.
    pub current_branch_only: bool,
    /// Only these refs (branch names); empty = no restriction.
    pub revs: Vec<String>,
    /// `--since` / `--until` dates (anything git understands, e.g. 2026-11-01).
    pub since: Option<String>,
    pub until: Option<String>,
    /// Only commits that add or remove this text (`git log -S`).
    pub pickaxe: Option<String>,
}

impl LogQuery {
    pub fn all(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }

    /// Whether any filter narrows the log (which makes the graph meaningless).
    pub fn is_filtered(&self) -> bool {
        self.text.is_some()
            || self.author.is_some()
            || self.path.is_some()
            || self.since.is_some()
            || self.until.is_some()
            || self.pickaxe.is_some()
    }
}

/// A configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// Result of `git log --format=%G?`: `G` good, `B` bad, `U` good but untrusted, `X`/`Y`/`R` expired or
/// revoked, `E` cannot be checked, `N` not signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub status: char,
    pub signer: String,
}

/// A value from the git configuration together with the scope it comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigValue {
    pub value: Option<String>,
    /// `system`, `global`, `local`, `worktree` or `command`.
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub head: String,
    /// Short branch name; `None` for a detached HEAD.
    pub branch: Option<String>,
    pub is_main: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Git LFS usage of a repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LfsInfo {
    /// The `git lfs` command is installed.
    pub available: bool,
    /// Patterns tracked through `.gitattributes`.
    pub patterns: Vec<String>,
    /// Files currently stored in LFS.
    pub files: std::collections::HashSet<String>,
}

/// A branch and how long ago its tip was committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchAge {
    pub name: String,
    pub unix: i64,
    pub merged: bool,
}

/// Overview numbers for the repository dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dashboard {
    pub commits: usize,
    pub branches: usize,
    pub tracked_files: usize,
    /// Branches without recent activity, oldest first.
    pub stale: Vec<BranchAge>,
    /// Largest tracked files: (path, bytes).
    pub big_files: Vec<(String, u64)>,
    /// Commits per week (oldest first, 12 weeks) for the most active authors.
    pub authors: Vec<(String, Vec<u32>)>,
}

/// A safety copy made before a destructive action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    pub refname: String,
    pub id: String,
    pub time: i64,
}
