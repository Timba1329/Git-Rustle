//! Background thread that performs all git calls for one repository tab. The UI talks to it only
//! through channels.

use crate::diffview::DiffView;
use rustle_core::{
    BlameLine, Branch, CancelToken, CliBackend, Commit, CommitFile, ConflictVersions, DiffOpts, FileChange,
    GitError, GitRunner, Graph, GraphRow, LogQuery, OpResult, PatchMode, PullMode, ReflogEntry, ResetMode,
    Side, StashEntry, Status, Submodule, TodoItem, build_graph,
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
    },

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
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    SubmoduleUpdate,

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
    Info(String),
    /// Result of a background fetch.
    AutoFetch(Result<(), String>),
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
        Ok(Response::Repo(Box::new(RepoData {
            path: b.path().to_path_buf(),
            graph,
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
            Request::Commit { message, amend } => {
                if amend {
                    self.be()?.commit_amend(&message)?;
                } else {
                    self.be()?.commit(&message)?;
                }
                vec![Response::Committed, snap(self)?]
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
            Request::SubmoduleUpdate => {
                self.be()?.submodule_update(Some(&c))?;
                vec![Response::Info("Submodules updated".into()), snap(self)?]
            }

            Request::FileDiff { change, staged } => {
                let text = self.be()?.file_diff(&change, staged, self.view.diff)?;
                let side = if staged { "staged" } else { "unstaged" };
                vec![self.diff(DiffKind::Work, format!("{} ({side})", change.path), &text)]
            }
            Request::CommitInfo(id) => {
                let b = self.be()?;
                vec![
                    Response::CommitBody {
                        body: b.commit_message(&id)?,
                        id: id.clone(),
                    },
                    Response::CommitFiles {
                        files: b.commit_files(&id)?,
                        key: id,
                    },
                ]
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
