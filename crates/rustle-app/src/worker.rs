//! Background thread that performs all git calls for one repository tab. The UI talks to it only
//! through channels.

use crate::diffview::DiffView;
use crate::imagediff::{self, DecodedImage};
use rustle_core::{
    BlameLine, Branch, CancelToken, CliBackend, Commit, CommitFile, ConfigValue, ConflictVersions, DiffOpts,
    FileChange, GitError, GitRunner, Graph, GraphRow, GrepHit, LfsInfo, LogQuery, OpResult, PatchMode,
    PullMode, ReflogEntry, Remote, ResetMode, Side, StashEntry, Status, Submodule, TodoItem, Worktree,
    build_graph,
};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

const INITIAL_LIMIT: usize = 2_000;
const PAGE: usize = 3_000;

#[derive(Debug, Clone, Copy)]
pub struct ViewOpts {
    pub diff: DiffOpts,
    pub dark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Diff of a working-tree file (commit popup).
    Work,
    /// Diff of a file in a commit or a comparison (detail panel).
    Commit,
}

/// Where an image of an image diff comes from.
pub enum ImageSrc {
    /// A blob in git: `HEAD:path`, `<id>:path`, `:path` (the index).
    Blob(String),
    /// A file in the working tree.
    Worktree(String),
}

/// Keys shown in the configuration window.
pub const CONFIG_KEYS: [&str; 9] = [
    "user.name",
    "user.email",
    "pull.rebase",
    "fetch.prune",
    "push.autoSetupRemote",
    "core.autocrlf",
    "commit.gpgsign",
    "init.defaultBranch",
    "merge.conflictstyle",
];

pub struct ConfigRow {
    pub key: &'static str,
    pub value: ConfigValue,
}

/// Contents of a file for the file viewer.
pub struct FileData {
    /// `None` for binary files.
    pub text: Option<String>,
    pub size: usize,
}

pub enum Request {
    Open(PathBuf),
    Clone {
        url: String,
        dest: PathBuf,
    },
    InitRepo(PathBuf),
    /// Reload everything (log, graph, branches, status).
    Refresh,
    RefreshStatus,
    SetQuery(LogQuery),
    LoadMore,
    SetView(ViewOpts),
    SetPullMode(PullMode),
    SetGitPath(String),

    /// Empty list = everything.
    Stage(Vec<String>),
    /// Empty list = everything.
    Unstage(Vec<String>),
    Discard {
        change: FileChange,
        include_staged: bool,
    },
    ApplyPatch {
        patch: String,
        mode: PatchMode,
    },
    AddToGitignore(String),
    Commit {
        message: String,
        amend: bool,
        /// `Some(true)` = sign, `Some(false)` = do not sign, `None` = follow `commit.gpgsign`.
        sign: Option<bool>,
        no_verify: bool,
    },
    PushBranch(String),
    FastForward(String),

    LoadRemotes,
    AddRemote {
        name: String,
        url: String,
    },
    RenameRemote {
        old: String,
        new: String,
    },
    SetRemoteUrl {
        name: String,
        url: String,
    },
    RemoveRemote(String),
    PruneRemote(String),
    LoadConfig,
    SetConfig {
        key: String,
        value: String,
        global: bool,
    },
    UnsetConfig {
        key: String,
        global: bool,
    },

    CherryPickMany(Vec<String>),
    RevertMany(Vec<String>),

    Grep {
        pattern: String,
        rev: Option<String>,
        ignore_case: bool,
    },
    LoadFiles(Option<String>),
    ReadFile {
        rev: Option<String>,
        path: String,
    },

    ConflictMerge(String),
    WriteResolved {
        path: String,
        content: String,
    },

    ImageDiff {
        kind: DiffKind,
        old: Option<ImageSrc>,
        new: Option<ImageSrc>,
    },

    AddWorktree {
        path: String,
        branch: String,
        create: bool,
    },
    RemoveWorktree {
        path: String,
        force: bool,
    },
    PruneWorktrees,
    SubmoduleSync {
        path: Option<String>,
        remote: bool,
    },
    AddSubmodule {
        url: String,
        path: String,
    },
    LfsTrack(String),
    LfsPull,

    Fetch,
    Pull,
    Push {
        force: bool,
    },

    Checkout(Branch),
    CheckoutCommit(String),
    CreateBranch(String),
    CreateBranchAt {
        name: String,
        id: String,
        switch: bool,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    DeleteRemoteBranch(String),
    SetUpstream {
        branch: String,
        upstream: String,
    },
    CreateTag {
        name: String,
        target: Option<String>,
        message: Option<String>,
    },
    DeleteTag(String),
    PushTag(String),

    Merge(String),
    Rebase(String),
    InteractiveRebase {
        base: String,
        items: Vec<TodoItem>,
    },
    CherryPick(String),
    Revert(String),
    Reset {
        id: String,
        mode: ResetMode,
    },
    OpContinue,
    OpContinueMsg(String),
    OpAbort,
    RebaseSkip,
    Resolve {
        path: String,
        side: Side,
    },
    ConflictVersions(String),

    StashPush {
        message: String,
        include_untracked: bool,
    },
    StashFiles {
        paths: Vec<String>,
        message: String,
    },
    MergedBranches {
        remote: bool,
    },
    DeleteBranches {
        names: Vec<String>,
        remote: bool,
    },
    MergePreview(String),
    UndoLast,
    CheckoutStashed(rustle_core::Branch),
    PullRequestUrl(String),
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),

    FileDiff {
        change: FileChange,
        staged: bool,
    },
    CommitInfo(String),
    /// Periodic fetch that never prompts; failures are reported quietly.
    AutoFetch,
    CommitFileDiff {
        id: String,
        path: String,
    },
    CompareInfo {
        a: String,
        b: String,
    },
    CompareFileDiff {
        a: String,
        b: String,
        path: String,
    },
    Blame(String),
    Reflog,
    LoadIdentity,
    SetIdentity {
        name: String,
        email: String,
    },
}

pub struct RepoData {
    pub path: PathBuf,
    pub commits: Vec<Commit>,
    pub graph: Graph,
    pub branches: Vec<Branch>,
    pub tags: Vec<String>,
    pub stashes: Vec<StashEntry>,
    pub submodules: Vec<Submodule>,
    pub status: Status,
    pub query: LogQuery,
    /// More commits can be loaded.
    pub has_more: bool,
    /// Contents of `commit.template`, if configured.
    pub template: Option<String>,
    pub head_id: Option<String>,
    /// The graph including a leading row for the working-tree changes (a node on HEAD's lane).
    pub graph_wip: Option<Graph>,
    pub worktrees: Vec<Worktree>,
    pub git_dir: PathBuf,
    /// `commit.gpgsign` is on.
    pub sign_default: bool,
    pub lfs: LfsInfo,
}

pub enum Response {
    Repo(Box<RepoData>),
    Status(Status),
    Diff {
        kind: DiffKind,
        view: Box<DiffView>,
    },
    CommitBody {
        id: String,
        body: String,
    },
    CommitFiles {
        key: String,
        files: Vec<CommitFile>,
    },
    Conflict {
        path: String,
        versions: ConflictVersions,
    },
    Blame {
        path: String,
        lines: Vec<BlameLine>,
    },
    Reflog(Vec<ReflogEntry>),
    Identity {
        name: Option<String>,
        email: Option<String>,
    },
    Committed,
    MergedBranches {
        names: Vec<String>,
        remote: bool,
        base: String,
    },
    MergePreview {
        branch: String,
        conflicts: Vec<String>,
    },
    OpenUrl(String),
    Info(String),
    /// Result of a background fetch.
    AutoFetch(Result<(), String>),
    /// What commit hooks printed.
    HookOutput(String),
    Signature {
        id: String,
        status: char,
        signer: String,
    },
    Remotes(Vec<Remote>),
    Config(Vec<ConfigRow>),
    Grep {
        pattern: String,
        hits: Vec<GrepHit>,
    },
    Files {
        rev: Option<String>,
        paths: Vec<String>,
    },
    FileContent {
        rev: Option<String>,
        path: String,
        result: Result<FileData, String>,
    },
    ConflictMerge {
        path: String,
        result: Result<String, String>,
    },
    Images {
        kind: DiffKind,
        old: Option<DecodedImage>,
        new: Option<DecodedImage>,
    },
    Error(String),
    /// Every request ends with exactly one `Idle`.
    Idle,
}

pub struct Worker {
    pub tx: Sender<Request>,
    pub rx: Receiver<Response>,
    cancel: Arc<Mutex<CancelToken>>,
}

impl Worker {
    /// Aborts the git process that is currently running, if any.
    pub fn cancel(&self) {
        if let Ok(t) = self.cancel.lock() {
            t.cancel();
        }
    }
}

pub fn spawn(ctx: eframe::egui::Context, git_path: String, view: ViewOpts, pull: PullMode) -> Worker {
    let (req_tx, req_rx) = channel::<Request>();
    let (resp_tx, resp_rx) = channel::<Response>();
    let cancel = Arc::new(Mutex::new(CancelToken::new()));
    let cancel2 = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut state = State {
            backend: None,
            git_path,
            query: LogQuery::default(),
            limit: INITIAL_LIMIT,
            view,
            pull,
            cancel: CancelToken::new(),
            template: None,
            sign_default: false,
        };
        for req in req_rx {
            state.cancel = CancelToken::new();
            if let Ok(mut c) = cancel2.lock() {
                *c = state.cancel.clone();
            }
            let mut out = match state.handle(req) {
                Ok(r) => r,
                Err(GitError::Cancelled) => vec![Response::Info("Cancelled".into())],
                Err(e) => {
                    let mut v = vec![Response::Error(e.friendly())];
                    // Something may have changed despite the error (e.g. half staged).
                    if let Ok(s) = state.be().and_then(|b| b.status()) {
                        v.push(Response::Status(s));
                    }
                    v
                }
            };
            out.push(Response::Idle);
            for r in out {
                if resp_tx.send(r).is_err() {
                    return;
                }
            }
            ctx.request_repaint();
        }
    });
    Worker {
        tx: req_tx,
        rx: resp_rx,
        cancel,
    }
}

struct State {
    backend: Option<CliBackend>,
    git_path: String,
    query: LogQuery,
    limit: usize,
    view: ViewOpts,
    pull: PullMode,
    cancel: CancelToken,
    template: Option<String>,
    sign_default: bool,
}

fn msg(r: OpResult, done: &str) -> Response {
    Response::Info(match r {
        OpResult::Done => done.to_string(),
        OpResult::Conflicts => {
            "Stopped on conflicts. Resolve them in the Commit window, then continue.".to_string()
        }
    })
}

impl State {
    fn runner(&self) -> Result<GitRunner, GitError> {
        let r = if self.git_path.trim().is_empty() {
            GitRunner::discover()?
        } else {
            GitRunner::with_path(self.git_path.trim())
        };
        Ok(match std::env::current_exe() {
            Ok(exe) => r.with_askpass(exe),
            Err(_) => r,
        })
    }

    fn be(&self) -> Result<&CliBackend, GitError> {
        self.backend
            .as_ref()
            .ok_or_else(|| GitError::Parse("no repository open".into()))
    }

    fn open(&mut self, path: &std::path::Path) -> Result<(), GitError> {
        let b = CliBackend::open(self.runner()?, path)?;
        self.template = b.commit_template();
        self.sign_default = b.signing_default();
        self.backend = Some(b);
        self.query = LogQuery::default();
        self.limit = INITIAL_LIMIT;
        Ok(())
    }

    fn snapshot(&self) -> Result<Response, GitError> {
        let b = self.be()?;
        let mut q = self.query.clone();
        q.limit = self.limit;
        q.skip = 0;
        let commits = b.log_query(&q)?;
        let has_more = commits.len() >= self.limit;
        let graph = if q.is_filtered() {
            Graph {
                rows: vec![GraphRow::default(); commits.len()],
                width: 1,
            }
        } else {
            build_graph(&commits)
        };
        let head_id = commits
            .iter()
            .find(|c| c.refs.iter().any(|l| l.is_head))
            .map(|c| c.id.clone())
            .or_else(|| b.head_id());
        let graph_wip = (!q.is_filtered() && !commits.is_empty())
            .then(|| head_id.clone())
            .flatten()
            .map(|head| {
                let mut with_wip = Vec::with_capacity(commits.len() + 1);
                with_wip.push(Commit {
                    id: "WIP".into(),
                    parents: vec![head],
                    author_name: String::new(),
                    author_email: String::new(),
                    time: 0,
                    subject: String::new(),
                    refs: Vec::new(),
                });
                with_wip.extend(commits.iter().cloned());
                build_graph(&with_wip)
            });
        Ok(Response::Repo(Box::new(RepoData {
            path: b.path().to_path_buf(),
            graph,
            graph_wip,
            head_id,
            worktrees: b.worktrees().unwrap_or_default(),
            git_dir: b.git_dir().to_path_buf(),
            sign_default: self.sign_default,
            lfs: b.lfs_info(),
            commits,
            branches: b.branches()?,
            tags: b.tags()?,
            stashes: b.stash_list().unwrap_or_default(),
            submodules: b.submodules().unwrap_or_default(),
            status: b.status()?,
            query: self.query.clone(),
            has_more,
            template: self.template.clone(),
        })))
    }

    fn status(&self) -> Result<Vec<Response>, GitError> {
        Ok(vec![Response::Status(self.be()?.status()?)])
    }

    fn diff(&self, kind: DiffKind, title: String, text: &str) -> Response {
        Response::Diff {
            kind,
            view: Box::new(DiffView::build(title, text, self.view.dark)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle(&mut self, req: Request) -> Result<Vec<Response>, GitError> {
        let c = self.cancel.clone();
        let snap = |s: &State| s.snapshot();
        Ok(match req {
            Request::Open(path) => {
                self.open(&path)?;
                vec![snap(self)?]
            }
            Request::Clone { url, dest } => {
                CliBackend::clone_repo(&self.runner()?, &url, &dest, Some(&c))?;
                self.open(&dest)?;
                vec![Response::Info("Repository cloned".into()), snap(self)?]
            }
            Request::InitRepo(dir) => {
                CliBackend::init_repo(&self.runner()?, &dir)?;
                self.open(&dir)?;
                vec![Response::Info("Repository created".into()), snap(self)?]
            }
            Request::Refresh => vec![snap(self)?],
            Request::RefreshStatus => self.status()?,
            Request::SetQuery(q) => {
                self.query = q;
                self.limit = INITIAL_LIMIT;
                vec![snap(self)?]
            }
            Request::LoadMore => {
                self.limit += PAGE;
                vec![snap(self)?]
            }
            Request::SetView(v) => {
                self.view = v;
                vec![]
            }
            Request::SetPullMode(m) => {
                self.pull = m;
                vec![]
            }
            Request::SetGitPath(p) => {
                self.git_path = p;
                vec![]
            }

            Request::Stage(p) => {
                self.be()?.stage(&p)?;
                self.status()?
            }
            Request::Unstage(p) => {
                self.be()?.unstage(&p)?;
                self.status()?
            }
            Request::Discard {
                change,
                include_staged,
            } => {
                self.be()?.discard(&change, include_staged)?;
                self.status()?
            }
            Request::ApplyPatch { patch, mode } => {
                self.be()?.apply_patch(&patch, mode)?;
                self.status()?
            }
            Request::AddToGitignore(p) => {
                self.be()?.add_to_gitignore(&p)?;
                self.status()?
            }
            Request::Commit {
                message,
                amend,
                sign,
                no_verify,
            } => {
                let out = self.be()?.commit_with(&message, amend, sign, no_verify)?;
                let mut v = vec![Response::Committed];
                if !out.is_empty() {
                    v.push(Response::HookOutput(out));
                }
                v.push(snap(self)?);
                v
            }
            Request::PushBranch(name) => {
                self.be()?.push_branch(&name, Some(&c))?;
                vec![Response::Info(format!("Pushed {name}")), snap(self)?]
            }
            Request::FastForward(name) => {
                self.be()?.fast_forward_branch(&name, Some(&c))?;
                vec![Response::Info(format!("Fast-forwarded {name}")), snap(self)?]
            }

            Request::LoadRemotes => vec![Response::Remotes(self.be()?.remotes()?)],
            Request::AddRemote { name, url } => {
                self.be()?.add_remote(&name, &url)?;
                vec![Response::Remotes(self.be()?.remotes()?)]
            }
            Request::RenameRemote { old, new } => {
                self.be()?.rename_remote(&old, &new)?;
                vec![Response::Remotes(self.be()?.remotes()?), snap(self)?]
            }
            Request::SetRemoteUrl { name, url } => {
                self.be()?.set_remote_url(&name, &url)?;
                vec![Response::Remotes(self.be()?.remotes()?)]
            }
            Request::RemoveRemote(name) => {
                self.be()?.remove_remote(&name)?;
                vec![Response::Remotes(self.be()?.remotes()?), snap(self)?]
            }
            Request::PruneRemote(name) => {
                self.be()?.prune_remote(&name, Some(&c))?;
                vec![Response::Info(format!("Pruned {name}")), snap(self)?]
            }
            Request::LoadConfig => {
                let b = self.be()?;
                vec![Response::Config(
                    CONFIG_KEYS
                        .iter()
                        .map(|k| ConfigRow {
                            key: k,
                            value: b.get_config(k),
                        })
                        .collect(),
                )]
            }
            Request::SetConfig { key, value, global } => {
                let b = self.be()?;
                b.set_config(&key, &value, global)?;
                self.sign_default = b.signing_default();
                vec![Response::Info(format!("Set {key}"))]
            }
            Request::UnsetConfig { key, global } => {
                self.be()?.unset_config(&key, global)?;
                vec![Response::Info(format!("Unset {key}"))]
            }

            Request::CherryPickMany(ids) => {
                let r = self.be()?.cherry_pick_many(&ids)?;
                vec![msg(r, "Cherry-pick complete"), snap(self)?]
            }
            Request::RevertMany(ids) => {
                let r = self.be()?.revert_many(&ids)?;
                vec![msg(r, "Revert complete"), snap(self)?]
            }

            Request::Grep {
                pattern,
                rev,
                ignore_case,
            } => {
                let hits = self.be()?.grep(&pattern, rev.as_deref(), ignore_case, 1000)?;
                vec![Response::Grep { pattern, hits }]
            }
            Request::LoadFiles(rev) => {
                let b = self.be()?;
                let paths = match &rev {
                    Some(r) => b.ls_tree(r)?,
                    None => b.ls_files()?,
                };
                vec![Response::Files { rev, paths }]
            }
            Request::ReadFile { rev, path } => {
                let b = self.be()?;
                let bytes = match &rev {
                    Some(r) => b.blob(&format!("{r}:{path}")),
                    None => b.worktree_file(&path),
                };
                let result = bytes.map_err(|e| e.to_string()).map(|bytes| {
                    let size = bytes.len();
                    let text = (size <= 2_000_000 && !bytes.contains(&0))
                        .then(|| String::from_utf8(bytes).ok())
                        .flatten();
                    FileData { text, size }
                });
                vec![Response::FileContent { rev, path, result }]
            }

            Request::ConflictMerge(path) => {
                let result = self.be()?.conflict_merge(&path).map_err(|e| e.to_string());
                vec![Response::ConflictMerge { path, result }]
            }
            Request::WriteResolved { path, content } => {
                self.be()?.write_resolved(&path, &content)?;
                self.status()?
            }

            Request::ImageDiff { kind, old, new } => {
                let b = self.be()?;
                let load = |src: Option<ImageSrc>| -> Option<DecodedImage> {
                    let bytes = match src? {
                        ImageSrc::Blob(spec) => b.blob(&spec).ok()?,
                        ImageSrc::Worktree(p) => b.worktree_file(&p).ok()?,
                    };
                    if bytes.len() > 40_000_000 {
                        return None;
                    }
                    imagediff::decode(&bytes).ok()
                };
                vec![Response::Images {
                    kind,
                    old: load(old),
                    new: load(new),
                }]
            }

            Request::AddWorktree { path, branch, create } => {
                self.be()?.add_worktree(&path, &branch, create)?;
                vec![Response::Info("Worktree added".into()), snap(self)?]
            }
            Request::RemoveWorktree { path, force } => {
                self.be()?.remove_worktree(&path, force)?;
                vec![Response::Info("Worktree removed".into()), snap(self)?]
            }
            Request::PruneWorktrees => {
                self.be()?.prune_worktrees()?;
                vec![snap(self)?]
            }
            Request::SubmoduleSync { path, remote } => {
                self.be()?.submodule_sync(path.as_deref(), remote, Some(&c))?;
                vec![Response::Info("Submodules updated".into()), snap(self)?]
            }
            Request::AddSubmodule { url, path } => {
                self.be()?.add_submodule(&url, &path, Some(&c))?;
                vec![Response::Info("Submodule added".into()), snap(self)?]
            }
            Request::LfsTrack(pattern) => {
                self.be()?.lfs_track(&pattern)?;
                vec![
                    Response::Info(format!("Tracking {pattern} with Git LFS")),
                    snap(self)?,
                ]
            }
            Request::LfsPull => {
                self.be()?.lfs_pull(Some(&c))?;
                vec![Response::Info("LFS objects downloaded".into()), snap(self)?]
            }

            Request::AutoFetch => {
                let b = self.be()?;
                let before = b.branches()?;
                match b.fetch_background(Some(&c)) {
                    Ok(()) => {
                        // Only rebuild the view when the fetch actually changed something.
                        let mut v = vec![Response::AutoFetch(Ok(()))];
                        if b.branches()? != before {
                            v.push(snap(self)?);
                        }
                        v
                    }
                    Err(GitError::Cancelled) => vec![],
                    Err(e) => {
                        tracing::warn!("background fetch failed: {e}");
                        vec![Response::AutoFetch(Err(e.friendly()))]
                    }
                }
            }
            Request::Fetch => {
                self.be()?.fetch(Some(&c))?;
                vec![Response::Info("Fetch complete".into()), snap(self)?]
            }
            Request::Pull => {
                let r = self.be()?.pull(self.pull, Some(&c))?;
                vec![msg(r, "Pull complete"), snap(self)?]
            }
            Request::Push { force } => {
                self.be()?.push(force, Some(&c))?;
                vec![Response::Info("Push complete".into()), snap(self)?]
            }

            Request::Checkout(b) => {
                self.be()?.checkout(&b)?;
                vec![snap(self)?]
            }
            Request::CheckoutCommit(id) => {
                self.be()?.checkout_commit(&id)?;
                vec![Response::Info("Detached HEAD".into()), snap(self)?]
            }
            Request::CreateBranch(n) => {
                self.be()?.create_branch(&n)?;
                vec![snap(self)?]
            }
            Request::CreateBranchAt { name, id, switch } => {
                self.be()?.create_branch_at(&name, &id, switch)?;
                vec![snap(self)?]
            }
            Request::RenameBranch { old, new } => {
                self.be()?.rename_branch(&old, &new)?;
                vec![snap(self)?]
            }
            Request::DeleteBranch { name, force } => {
                self.be()?.delete_branch(&name, force)?;
                vec![snap(self)?]
            }
            Request::DeleteRemoteBranch(n) => {
                self.be()?.delete_remote_branch(&n)?;
                vec![Response::Info("Remote branch deleted".into()), snap(self)?]
            }
            Request::SetUpstream { branch, upstream } => {
                self.be()?.set_upstream(&branch, &upstream)?;
                vec![snap(self)?]
            }
            Request::CreateTag {
                name,
                target,
                message,
            } => {
                self.be()?
                    .create_tag(&name, target.as_deref(), message.as_deref())?;
                vec![snap(self)?]
            }
            Request::DeleteTag(n) => {
                self.be()?.delete_tag(&n)?;
                vec![snap(self)?]
            }
            Request::PushTag(n) => {
                self.be()?.push_tag(&n, Some(&c))?;
                vec![Response::Info(format!("Tag {n} pushed"))]
            }

            Request::Merge(b) => {
                let r = self.be()?.merge(&b)?;
                vec![msg(r, "Merge complete"), snap(self)?]
            }
            Request::Rebase(b) => {
                let r = self.be()?.rebase(&b)?;
                vec![msg(r, "Rebase complete"), snap(self)?]
            }
            Request::InteractiveRebase { base, items } => {
                let r = self.be()?.interactive_rebase(&base, &items)?;
                vec![msg(r, "Rebase complete"), snap(self)?]
            }
            Request::CherryPick(id) => {
                let r = self.be()?.cherry_pick(&id)?;
                vec![msg(r, "Cherry-pick complete"), snap(self)?]
            }
            Request::Revert(id) => {
                let r = self.be()?.revert(&id)?;
                vec![msg(r, "Revert complete"), snap(self)?]
            }
            Request::Reset { id, mode } => {
                self.be()?.reset(&id, mode)?;
                vec![snap(self)?]
            }
            Request::OpContinue => {
                let r = self.be()?.op_continue()?;
                vec![msg(r, "Operation complete"), snap(self)?]
            }
            Request::OpContinueMsg(m) => {
                let r = self.be()?.op_continue_msg(&m)?;
                vec![msg(r, "Operation complete"), snap(self)?]
            }
            Request::OpAbort => {
                self.be()?.op_abort()?;
                vec![Response::Info("Operation aborted".into()), snap(self)?]
            }
            Request::RebaseSkip => {
                let r = self.be()?.rebase_skip()?;
                vec![msg(r, "Rebase complete"), snap(self)?]
            }
            Request::Resolve { path, side } => {
                self.be()?.resolve(&path, side)?;
                self.status()?
            }
            Request::ConflictVersions(path) => {
                let versions = self.be()?.conflict_versions(&path)?;
                vec![Response::Conflict { path, versions }]
            }

            Request::StashPush {
                message,
                include_untracked,
            } => {
                self.be()?.stash_push(&message, include_untracked)?;
                vec![Response::Info("Changes stashed".into()), snap(self)?]
            }
            Request::StashFiles { paths, message } => {
                self.be()?.stash_files(&paths, &message)?;
                vec![Response::Info("Changes stashed".into()), snap(self)?]
            }
            Request::MergedBranches { remote } => {
                let b = self.be()?;
                let names = b.merged_branches(remote)?;
                let base = b.status()?.branch.unwrap_or_else(|| "HEAD".into());
                vec![Response::MergedBranches { names, remote, base }]
            }
            Request::DeleteBranches { names, remote } => {
                let (ok, failed) = self.be()?.delete_branches(&names, remote);
                let mut m = format!("Deleted {ok} branch(es)");
                if !failed.is_empty() {
                    m.push_str(&format!("; failed: {}", failed.join(", ")));
                }
                vec![Response::Info(m), snap(self)?]
            }
            Request::MergePreview(branch) => {
                let conflicts = self.be()?.merge_preview(&branch)?;
                vec![Response::MergePreview { branch, conflicts }]
            }
            Request::UndoLast => {
                let what = self.be()?.undo_last()?;
                vec![Response::Info(format!("Undone: {what}")), snap(self)?]
            }
            Request::CheckoutStashed(b) => {
                let r = self.be()?.checkout_stashed(&b)?;
                vec![msg(r, "Checked out; your changes were re-applied"), snap(self)?]
            }
            Request::PullRequestUrl(branch) => {
                vec![Response::OpenUrl(self.be()?.pull_request_url(&branch)?)]
            }
            Request::StashApply(i) => {
                let r = self.be()?.stash_apply(i)?;
                vec![msg(r, "Stash applied"), snap(self)?]
            }
            Request::StashPop(i) => {
                let r = self.be()?.stash_pop(i)?;
                vec![msg(r, "Stash applied and dropped"), snap(self)?]
            }
            Request::StashDrop(i) => {
                self.be()?.stash_drop(i)?;
                vec![snap(self)?]
            }

            Request::FileDiff { change, staged } => {
                let text = self.be()?.file_diff(&change, staged, self.view.diff)?;
                let side = if staged { "staged" } else { "unstaged" };
                vec![self.diff(DiffKind::Work, format!("{} ({side})", change.path), &text)]
            }
            Request::CommitInfo(id) => {
                let b = self.be()?;
                let mut v = vec![
                    Response::CommitBody {
                        body: b.commit_message(&id)?,
                        id: id.clone(),
                    },
                    Response::CommitFiles {
                        files: b.commit_files(&id)?,
                        key: id.clone(),
                    },
                ];
                if let Ok(sig) = b.signature(&id) {
                    v.push(Response::Signature {
                        id,
                        status: sig.status,
                        signer: sig.signer,
                    });
                }
                v
            }
            Request::CommitFileDiff { id, path } => {
                let text = self.be()?.commit_file_diff(&id, &path, self.view.diff)?;
                vec![self.diff(DiffKind::Commit, path, &text)]
            }
            Request::CompareInfo { a, b } => {
                let files = self.be()?.compare_files(&a, &b)?;
                vec![Response::CommitFiles {
                    key: format!("{a}..{b}"),
                    files,
                }]
            }
            Request::CompareFileDiff { a, b, path } => {
                let text = self.be()?.compare_file_diff(&a, &b, &path, self.view.diff)?;
                vec![self.diff(DiffKind::Commit, path, &text)]
            }
            Request::Blame(path) => {
                let lines = self.be()?.blame(&path)?;
                vec![Response::Blame { path, lines }]
            }
            Request::Reflog => vec![Response::Reflog(self.be()?.reflog(200)?)],
            Request::LoadIdentity => {
                let (name, email) = self.be()?.identity();
                vec![Response::Identity { name, email }]
            }
            Request::SetIdentity { name, email } => {
                self.be()?.set_identity(&name, &email)?;
                vec![Response::Info("Identity saved".into())]
            }
        })
    }
}
