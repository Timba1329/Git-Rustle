//! State of one repository tab: selection, filters, dialogs, and handling of worker responses.

use crate::diffview::{DiffAction, DiffView};
use crate::imagediff::{ImageDiffView, is_image_path};
use crate::settings::{Settings, ThemeMode};
use crate::watcher::{Change, RepoWatcher};
use crate::worker::{self, ConfigRow, DiffKind, ImageSrc, RepoData, Request, Response, ViewOpts, Worker};
use eframe::egui;
use rustle_core::conflict::{self, Choice, Segment};
use rustle_core::{
    BlameLine, Commit, CommitFile, ConflictVersions, DiffOpts, FileChange, GrepHit, LogQuery, PatchMode,
    ReflogEntry, Remote, ResetMode, TodoAction, TodoItem, build_patch,
};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub type LineSel = HashSet<(usize, usize, usize)>;

/// Commits that may be dragged to a new place even though nothing was pushed yet.
const MAX_UNPUSHED_WITHOUT_UPSTREAM: usize = 200;

pub struct RebaseRow {
    pub action: TodoAction,
    pub id: String,
    pub subject: String,
    pub message: String,
}

pub enum InputKind {
    NewBranch,
    BranchAt { id: String },
    RenameBranch { old: String },
    RenameRemote { old: String },
    Stash,
}

pub enum Dialog {
    None,
    Confirm {
        title: String,
        text: String,
        button: String,
        req: Option<Request>,
    },
    Input {
        title: String,
        label: String,
        value: String,
        kind: InputKind,
        /// Optional checkbox (label, state).
        check: Option<(String, bool)>,
    },
    Tag {
        target: String,
        name: String,
        message: String,
    },
    Reset {
        id: String,
        mode: ResetMode,
    },
    Rebase {
        base: String,
        rows: Vec<RebaseRow>,
    },
    /// Squash several commits into one: `plan` lists every replayed commit, oldest first, with
    /// whether it is one of the squashed ones.
    Squash {
        base: String,
        plan: Vec<(String, bool)>,
        message: String,
        count: usize,
        warning: Option<String>,
    },
    AddWorktree {
        path: String,
        branch: String,
        create: bool,
    },
    AddSubmodule {
        url: String,
        path: String,
    },
    TrackLfs {
        pattern: String,
    },
}

/// Things only the application (not a single tab) can do.
pub enum AppAct {
    OpenPath(PathBuf),
    NewTab,
    Settings,
    About,
    Clone,
    PickAndInit,
    Explorer(PathBuf),
    Terminal(PathBuf),
    Editor(PathBuf),
    RepoOpened(PathBuf),
}

pub enum Act {
    Send(Request),
    SelectCommit {
        idx: usize,
        ctrl: bool,
        shift: bool,
    },
    ClearMulti,
    CompareSelected,
    SquashSelected,
    CherryPickSelected,
    RevertSelected,
    DropCommit {
        from: String,
        to: String,
    },
    SelectCommitFile(String),
    TogglePin(String),
    GraphOnly(Option<String>),
    CommitPush,
    SelectFile(FileChange, bool),
    OpenCommit,
    Commit,
    NewBranch,
    Diff(DiffAction),
    Dialog(Dialog),
    ApplyQuery,
    ClearQuery,
    History(String),
    Blame(String),
    ShowReflog,
    ToggleAmend(bool),
    ReloadView,
    OpenRepoSettings,
    OpenFile {
        rev: Option<String>,
        path: String,
        line: Option<u32>,
    },
    LoadFiles,
    GrepFiles,
    BrowseAt(Option<String>),
    ShowDetails,
    ConflictWrite,
    App(AppAct),
}

/// The conflict editor state for one file.
pub struct ConflictEditor {
    pub path: String,
    pub segs: Vec<Segment>,
    pub choices: Vec<Option<Choice>>,
    /// Text of the custom edit of each block.
    pub custom: Vec<String>,
    /// Block that is currently focused.
    pub focus: usize,
    /// Block to scroll to on the next frame.
    pub jump: Option<usize>,
}

pub enum ViewerState {
    Loading,
    Failed(String),
    Binary(usize),
    Ready(Box<DiffView>),
}

pub struct FileViewer {
    pub rev: Option<String>,
    pub path: String,
    pub state: ViewerState,
    /// Row to scroll to once the content is there.
    pub jump: Option<usize>,
    pub find: String,
    pub find_pos: usize,
}

/// Folder structure of a list of paths.
#[derive(Default)]
pub struct FileTree {
    pub dirs: BTreeMap<String, FileTree>,
    /// (file name, full path)
    pub files: Vec<(String, String)>,
}

/// All files in display order (folders before files).
pub fn tree_paths(node: &FileTree, out: &mut Vec<String>) {
    for child in node.dirs.values() {
        tree_paths(child, out);
    }
    out.extend(node.files.iter().map(|(_, p)| p.clone()));
}

/// The first file in display order (folders before files).
pub fn first_in_tree(node: &FileTree) -> Option<String> {
    if let Some((_, c)) = node.dirs.iter().next() {
        return first_in_tree(c);
    }
    node.files.first().map(|(_, p)| p.clone())
}

impl FileTree {
    pub fn build(paths: &[String]) -> Self {
        let mut root = FileTree::default();
        for p in paths {
            let mut node = &mut root;
            let mut parts = p.split('/').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_some() {
                    node = node.dirs.entry(part.to_string()).or_default();
                } else {
                    node.files.push((part.to_string(), p.clone()));
                }
            }
        }
        root
    }
}

pub struct Tab {
    pub title: String,
    pub ctx: egui::Context,
    pub worker: Worker,
    pub repo: Option<RepoData>,
    pub pending: usize,
    pub error: Option<String>,
    pub info: Option<String>,
    /// Full text of a long error (shown in a window).
    pub details: Option<String>,

    // Log filters.
    pub search: String,
    pub author: String,
    pub branch_only: bool,
    pub path_filter: Option<String>,
    pub focus_search: bool,
    pub load_more_sent: bool,
    /// When a fetch (manual or automatic) was last started.
    pub last_fetch: Instant,
    /// Why the last automatic fetch failed, if it did.
    pub auto_fetch_error: Option<String>,

    // File watching.
    pub watcher: Option<RepoWatcher>,
    watch_tx: Sender<Change>,
    watch_rx: Receiver<Change>,
    watch_dirty: Option<Change>,
    quiet_until: Instant,

    // Selected commit(s) and detail panel.
    pub selected: Option<usize>,
    pub selected_id: Option<String>,
    pub compare_id: Option<String>,
    /// Commits picked with Ctrl/Shift+click (two or more means "several selected").
    pub multi: Vec<String>,
    /// Commits that can be dragged to a new place (unpushed, on the first-parent line).
    pub reorderable: HashSet<String>,
    pub detail_key: String,
    pub commit_body: String,
    pub signature: Option<(String, char, String)>,
    pub commit_files: Vec<CommitFile>,
    pub selected_cfile: Option<String>,
    pub pins: Vec<String>,
    pub graph_refs: Vec<String>,
    pub last_checkout: Option<rustle_core::Branch>,
    pub msg_prefix: String,
    pub prefix_done: bool,
    pub cfile_filter: String,
    pub cdir_epoch: u32,
    pub cdir_all: Option<bool>,
    pub branch_epoch: u32,
    pub branch_all: Option<bool>,
    pub branch_filter: String,
    pub msg_prefilled: bool,
    pub push_after: bool,
    pub pins_for: String,
    pub commit_diff: Option<DiffView>,
    pub commit_image: Option<ImageDiffView>,
    pub commit_sel: LineSel,

    // Commit popup.
    pub commit_open: bool,
    pub selected_file: Option<(String, bool)>,
    pub file_diff: Option<DiffView>,
    pub work_image: Option<ImageDiffView>,
    pub file_sel: LineSel,
    pub conflict: Option<(String, ConflictVersions)>,
    pub conflict_edit: Option<ConflictEditor>,
    pub message: String,
    pub amend: bool,
    pub sign: bool,
    pub no_verify: bool,
    pub amend_fetch: Option<String>,
    pub last_message: String,
    pub hook_output: Option<String>,
    pub reselect_pending: bool,

    // Diff view helpers.
    pub find: String,
    pub find_pos: usize,
    pub full_file: bool,

    pub dialog: Dialog,
    pub new_branch: String,
    pub blame: Option<(String, Vec<BlameLine>)>,
    pub reflog: Option<Vec<ReflogEntry>>,
    pub identity: Option<(Option<String>, Option<String>)>,
    pub view: ViewOpts,

    // Repository settings window.
    pub repo_settings_open: bool,
    pub remotes: Option<Vec<Remote>>,
    pub remote_edit: Vec<String>,
    pub new_remote: (String, String),
    pub config: Option<Vec<ConfigRow>>,
    pub config_edit: Vec<String>,

    // Sidebar: file browser.
    pub sidebar_files: bool,
    pub files_rev: Option<String>,
    pub files: Option<Vec<String>>,
    pub files_tree: Option<FileTree>,
    pub files_loading: bool,
    pub file_filter: String,
    pub grep_mode: bool,
    pub grep_text: String,
    pub grep: Option<(String, Vec<GrepHit>)>,
    pub viewer: Option<FileViewer>,
}

fn folder_name(p: &std::path::Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn view_opts(s: &Settings) -> ViewOpts {
    ViewOpts {
        diff: DiffOpts {
            ignore_whitespace: s.ignore_whitespace,
            context: s.context,
        },
        dark: s.theme == ThemeMode::Dark,
    }
}

/// The first-parent line from HEAD downwards (HEAD first).
pub fn first_parent_chain<'a>(commits: &'a [Commit], head: &str) -> Vec<&'a Commit> {
    let by_id: std::collections::HashMap<&str, &Commit> =
        commits.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut chain = Vec::new();
    let mut cur = by_id.get(head).copied();
    while let Some(c) = cur {
        chain.push(c);
        cur = c.parents.first().and_then(|p| by_id.get(p.as_str()).copied());
    }
    chain
}

impl Tab {
    pub fn new(ctx: &egui::Context, s: &Settings) -> Self {
        let view = view_opts(s);
        let (watch_tx, watch_rx) = channel();
        Self {
            title: "New tab".into(),
            ctx: ctx.clone(),
            worker: worker::spawn(ctx.clone(), s.git_path.clone(), view, s.pull_mode),
            repo: None,
            pending: 0,
            error: None,
            info: None,
            details: None,
            search: String::new(),
            author: String::new(),
            branch_only: false,
            path_filter: None,
            focus_search: false,
            load_more_sent: false,
            last_fetch: Instant::now(),
            auto_fetch_error: None,
            watcher: None,
            watch_tx,
            watch_rx,
            watch_dirty: None,
            quiet_until: Instant::now(),
            selected: None,
            selected_id: None,
            compare_id: None,
            multi: Vec::new(),
            reorderable: HashSet::new(),
            detail_key: String::new(),
            commit_body: String::new(),
            signature: None,
            commit_files: Vec::new(),
            selected_cfile: None,
            pins: Vec::new(),
            graph_refs: Vec::new(),
            last_checkout: None,
            msg_prefix: String::new(),
            prefix_done: false,
            cfile_filter: String::new(),
            cdir_epoch: 0,
            cdir_all: None,
            branch_epoch: 0,
            branch_all: None,
            branch_filter: String::new(),
            msg_prefilled: false,
            push_after: false,
            pins_for: String::new(),
            commit_diff: None,
            commit_image: None,
            commit_sel: LineSel::new(),
            commit_open: false,
            selected_file: None,
            file_diff: None,
            work_image: None,
            file_sel: LineSel::new(),
            conflict: None,
            conflict_edit: None,
            message: String::new(),
            amend: false,
            sign: false,
            no_verify: false,
            amend_fetch: None,
            last_message: String::new(),
            hook_output: None,
            reselect_pending: false,
            find: String::new(),
            find_pos: 0,
            full_file: false,
            dialog: Dialog::None,
            new_branch: String::new(),
            blame: None,
            reflog: None,
            identity: None,
            view,
            repo_settings_open: false,
            remotes: None,
            remote_edit: Vec::new(),
            new_remote: (String::new(), String::new()),
            config: None,
            config_edit: Vec::new(),
            sidebar_files: false,
            files_rev: None,
            files: None,
            files_tree: None,
            files_loading: false,
            file_filter: String::new(),
            grep_mode: false,
            grep_text: String::new(),
            grep: None,
            viewer: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    pub fn send(&mut self, req: Request) {
        if matches!(req, Request::Fetch | Request::AutoFetch) {
            self.last_fetch = Instant::now();
        }
        self.pending += 1;
        self.error = None;
        let _ = self.worker.tx.send(req);
    }

    pub fn open_path(&mut self, p: PathBuf) {
        self.title = folder_name(&p);
        self.reset_selection();
        self.send(Request::Open(p));
    }

    fn reset_selection(&mut self) {
        self.reset_detail();
        self.commit_sel.clear();
        self.selected_file = None;
        self.file_diff = None;
        self.work_image = None;
        self.file_sel.clear();
        self.conflict = None;
        self.conflict_edit = None;
        self.files = None;
        self.files_tree = None;
        self.files_rev = None;
        self.grep = None;
        self.viewer = None;
    }

    pub fn query(&self) -> LogQuery {
        let opt = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        LogQuery {
            text: opt(&self.search),
            author: opt(&self.author),
            path: self.path_filter.clone(),
            current_branch_only: self.branch_only,
            revs: self.graph_refs.clone(),
            ..Default::default()
        }
    }

    fn apply_query(&mut self) {
        self.reset_detail();
        let q = self.query();
        self.send(Request::SetQuery(q));
    }

    fn reset_detail(&mut self) {
        self.selected = None;
        self.selected_id = None;
        self.compare_id = None;
        self.multi.clear();
        self.detail_key.clear();
        self.commit_files.clear();
        self.selected_cfile = None;
        self.commit_diff = None;
        self.commit_image = None;
        self.commit_body.clear();
        self.signature = None;
    }

    /// The view options with the "whole file" override applied.
    fn effective_view(&self, s: &Settings) -> ViewOpts {
        let mut v = view_opts(s);
        if self.full_file {
            v.diff.context = 100_000;
        }
        v
    }

    /// Re-requests the diffs that are on screen (after a theme or diff-option change).
    pub fn reload_diffs(&mut self, s: &Settings) {
        self.view = self.effective_view(s);
        let v = self.view;
        let _ = self.worker.tx.send(Request::SetView(v));
        self.pending += 1;
        // `SetView` answers with only `Idle`.
        if self.commit_open {
            self.reselect_pending = true;
        }
        if self.selected_cfile.is_some() {
            self.request_commit_file_diff();
        }
    }

    fn request_commit_file_diff(&mut self) {
        let (Some(path), Some(id)) = (self.selected_cfile.clone(), self.selected_id.clone()) else {
            return;
        };
        match self.compare_id.clone() {
            Some(a) => self.send(Request::CompareFileDiff { a, b: id, path }),
            None => self.send(Request::CommitFileDiff { id, path }),
        }
    }

    /// Asks for before/after images of the selected commit file.
    fn request_commit_image(&mut self) {
        let (Some(path), Some(id)) = (self.selected_cfile.clone(), self.selected_id.clone()) else {
            return;
        };
        let (old, new) = match &self.compare_id {
            Some(a) => (format!("{a}:{path}"), format!("{id}:{path}")),
            None => (format!("{id}^:{path}"), format!("{id}:{path}")),
        };
        self.send(Request::ImageDiff {
            kind: DiffKind::Commit,
            old: Some(ImageSrc::Blob(old)),
            new: Some(ImageSrc::Blob(new)),
        });
    }

    /// Asks for before/after images of the selected working-tree file.
    fn request_work_image(&mut self) {
        let Some((path, staged)) = self.selected_file.clone() else {
            return;
        };
        let Some(f) = self
            .repo
            .as_ref()
            .and_then(|r| r.status.files.iter().find(|f| f.path == path))
            .cloned()
        else {
            return;
        };
        let (old, new) = if staged {
            (
                (f.index != 'A').then(|| ImageSrc::Blob(format!("HEAD:{path}"))),
                (f.index != 'D').then(|| ImageSrc::Blob(format!(":{path}"))),
            )
        } else {
            (
                (!f.is_untracked()).then(|| ImageSrc::Blob(format!(":{path}"))),
                (f.worktree != 'D').then(|| ImageSrc::Worktree(path.clone())),
            )
        };
        self.send(Request::ImageDiff {
            kind: DiffKind::Work,
            old,
            new,
        });
    }

    /// Keeps the file selection in the commit popup valid after a status change and loads
    /// the diff (or conflict editor) of the selected file.
    pub fn reselect(&mut self) {
        let Some(r) = &self.repo else { return };
        let files = &r.status.files;
        let kept = self.selected_file.as_ref().and_then(|(path, staged)| {
            files
                .iter()
                .find(|f| &f.path == path)
                .map(|f| {
                    let still = if *staged { f.is_staged() } else { f.is_unstaged() };
                    (f.clone(), if still { *staged } else { f.is_staged() })
                })
                .filter(|(_, now_staged)| *staged == *now_staged || *staged)
        });
        let unstaged_paths: Vec<String> = files
            .iter()
            .filter(|f| f.is_unstaged() && !f.conflicted)
            .map(|f| f.path.clone())
            .collect();
        let top_unstaged =
            first_in_tree(&FileTree::build(&unstaged_paths)).and_then(|p| files.iter().find(|f| f.path == p));
        let pick = kept
            .or_else(|| files.iter().find(|f| f.conflicted).map(|f| (f.clone(), false)))
            .or_else(|| top_unstaged.map(|f| (f.clone(), false)))
            .or_else(|| files.iter().find(|f| f.is_staged()).map(|f| (f.clone(), true)));
        match pick {
            Some((change, staged)) => self.show_file(change, staged),
            None => {
                self.selected_file = None;
                self.file_diff = None;
                self.work_image = None;
                self.conflict = None;
                self.conflict_edit = None;
            }
        }
    }

    fn show_file(&mut self, change: FileChange, staged: bool) {
        self.selected_file = Some((change.path.clone(), staged));
        if change.conflicted {
            self.file_diff = None;
            self.work_image = None;
            let keep = self.conflict_edit.as_ref().is_some_and(|e| e.path == change.path);
            if !keep {
                self.conflict_edit = None;
                self.conflict = None;
            }
            self.send(Request::ConflictMerge(change.path));
        } else {
            self.conflict = None;
            self.conflict_edit = None;
            self.send(Request::FileDiff { change, staged });
        }
    }

    // ------------------------------------------------------------------ file watching

    fn start_watcher(&mut self, settings: &Settings) {
        self.watcher = None;
        if !settings.watch_files {
            return;
        }
        let Some(r) = &self.repo else { return };
        match RepoWatcher::new(&r.path, &r.git_dir, self.watch_tx.clone(), self.ctx.clone()) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => tracing::warn!("cannot watch {}: {e}", r.path.display()),
        }
    }

    /// Switches file watching on or off (after a settings change).
    pub fn set_watching(&mut self, settings: &Settings) {
        self.start_watcher(settings);
    }

    fn handle_watch_events(&mut self) {
        let mut change: Option<Change> = None;
        while let Ok(c) = self.watch_rx.try_recv() {
            change = Some(if change == Some(Change::Refs) || c == Change::Refs {
                Change::Refs
            } else {
                Change::Worktree
            });
        }
        // Events right after our own operations are the echo of what we just did.
        if let Some(c) = change
            && Instant::now() >= self.quiet_until
        {
            self.watch_dirty = Some(if self.watch_dirty == Some(Change::Refs) || c == Change::Refs {
                Change::Refs
            } else {
                Change::Worktree
            });
        }
        if let Some(c) = self.watch_dirty
            && !self.busy()
            && self.repo.is_some()
        {
            self.watch_dirty = None;
            self.send(match c {
                Change::Refs => Request::Refresh,
                Change::Worktree => Request::RefreshStatus,
            });
        }
    }

    // ------------------------------------------------------------------ responses

    #[allow(clippy::too_many_lines)]
    pub fn poll(&mut self, settings: &mut Settings) -> Vec<AppAct> {
        let mut app = Vec::new();
        while let Ok(resp) = self.worker.rx.try_recv() {
            match resp {
                Response::Repo(data) => {
                    let new_path = self.repo.as_ref().map(|r| &r.path) != Some(&data.path);
                    if self.title == "New tab" || new_path {
                        self.title = folder_name(&data.path);
                    }
                    if new_path {
                        app.push(AppAct::RepoOpened(data.path.clone()));
                        // First automatic fetch shortly after opening a repository.
                        let iv = Duration::from_secs(u64::from(settings.auto_fetch_minutes) * 60);
                        self.last_fetch = Instant::now()
                            .checked_sub(iv.saturating_sub(Duration::from_secs(15)))
                            .unwrap_or_else(Instant::now);
                        self.auto_fetch_error = None;
                    }
                    self.selected = self
                        .selected_id
                        .as_ref()
                        .and_then(|id| data.commits.iter().position(|c| &c.id == id));
                    if self.selected.is_none() && self.compare_id.is_none() {
                        self.selected_id = None;
                    }
                    self.multi.retain(|id| data.commits.iter().any(|c| &c.id == id));
                    if self.message.is_empty()
                        && !self.amend
                        && let Some(t) = &data.template
                    {
                        self.message = t.clone();
                    }
                    self.repo = Some(*data);
                    self.compute_reorderable();
                    if new_path {
                        self.start_watcher(settings);
                    }
                    // Lists that depend on the commit or the files are loaded again on demand.
                    self.files = None;
                    self.files_tree = None;
                    self.grep = None;
                    self.load_more_sent = false;
                    self.reselect_pending = true;
                }
                Response::Status(s) => {
                    if let Some(r) = &mut self.repo {
                        r.status = s;
                    }
                    self.reselect_pending = true;
                }
                Response::Diff { kind, view } => {
                    let binary = view.is_binary();
                    match kind {
                        DiffKind::Work => {
                            self.file_diff = Some(*view);
                            self.file_sel.clear();
                            self.conflict = None;
                            self.work_image = None;
                            if binary && self.selected_file.as_ref().is_some_and(|(p, _)| is_image_path(p)) {
                                self.request_work_image();
                            }
                        }
                        DiffKind::Commit => {
                            self.commit_diff = Some(*view);
                            self.commit_sel.clear();
                            self.commit_image = None;
                            if binary && self.selected_cfile.as_deref().is_some_and(is_image_path) {
                                self.request_commit_image();
                            }
                        }
                    }
                }
                Response::Images { kind, old, new, .. } => {
                    let view = ImageDiffView::new(&self.ctx, old, new);
                    match kind {
                        DiffKind::Work => self.work_image = Some(view),
                        DiffKind::Commit => self.commit_image = Some(view),
                    }
                }
                Response::CommitBody { id, body } => {
                    if self.amend_fetch.as_deref() == Some(id.as_str()) {
                        self.amend_fetch = None;
                        self.message = body.clone();
                    }
                    if self.selected_id.as_deref() == Some(id.as_str()) {
                        // First line is the subject, shown separately.
                        self.commit_body = body
                            .split_once('\n')
                            .map_or(String::new(), |(_, r)| r.trim().to_string());
                    }
                }
                Response::Signature { id, status, signer } => {
                    if self.selected_id.as_deref() == Some(id.as_str()) {
                        self.signature = Some((id, status, signer));
                    }
                }
                Response::CommitFiles { key, files } => {
                    if key == self.detail_key {
                        let keep = self
                            .selected_cfile
                            .as_ref()
                            .is_some_and(|p| p.is_empty() || files.iter().any(|f| &f.path == p));
                        if !keep {
                            self.selected_cfile = if files.len() > 1 {
                                Some(String::new())
                            } else {
                                files.first().map(|f| f.path.clone())
                            };
                        }
                        self.commit_files = files;
                        if self.selected_cfile.is_some() {
                            self.request_commit_file_diff();
                        } else {
                            self.commit_diff = None;
                        }
                    }
                }
                Response::Conflict { path, versions } => {
                    self.file_diff = None;
                    self.conflict = Some((path, versions));
                }
                Response::ConflictMerge { path, result } => {
                    if self.selected_file.as_ref().is_none_or(|(p, _)| *p != path) {
                        continue;
                    }
                    let segs = result.as_ref().ok().map(|t| conflict::parse_conflicts(t));
                    match segs {
                        Some(segs) if conflict::conflict_count(&segs) > 0 => {
                            let n = conflict::conflict_count(&segs);
                            // Keep the choices when the same file is loaded again after a status change.
                            let same = self
                                .conflict_edit
                                .as_ref()
                                .is_some_and(|e| e.path == path && e.choices.len() == n);
                            if !same {
                                self.conflict_edit = Some(ConflictEditor {
                                    path,
                                    segs,
                                    choices: vec![None; n],
                                    custom: vec![String::new(); n],
                                    focus: 0,
                                    jump: Some(0),
                                });
                            }
                            self.conflict = None;
                        }
                        _ => {
                            // Not text, or nothing left to merge by hand: fall back to the stage view.
                            self.conflict_edit = None;
                            self.send(Request::ConflictVersions(path));
                        }
                    }
                }
                Response::Blame { path, lines } => self.blame = Some((path, lines)),
                Response::Reflog(l) => self.reflog = Some(l),
                Response::Identity { name, email } => self.identity = Some((name, email)),
                Response::MergedBranches { names, remote, base } => {
                    if names.is_empty() {
                        self.info = Some(format!(
                            "No {}branches are merged into {base}",
                            if remote { "remote " } else { "" }
                        ));
                    } else {
                        let shown: Vec<&str> = names.iter().take(25).map(String::as_str).collect();
                        let more = names.len().saturating_sub(25);
                        let mut text = format!(
                            "These {} branches are fully merged into {base}:\n\n{}",
                            names.len(),
                            shown.join("\n")
                        );
                        if more > 0 {
                            text.push_str(&format!("\n… and {more} more"));
                        }
                        if remote {
                            text.push_str("\n\nThey are deleted on the remote for everyone.");
                        }
                        let n = names.len();
                        self.dialog = Dialog::Confirm {
                            title: "Delete merged branches".into(),
                            text,
                            button: format!("Delete {n}"),
                            req: Some(Request::DeleteBranches { names, remote }),
                        };
                    }
                }
                Response::MergePreview { branch, conflicts } => {
                    let into = self
                        .repo
                        .as_ref()
                        .and_then(|r| r.status.branch.clone())
                        .unwrap_or_default();
                    let text = if conflicts.is_empty() {
                        format!("{branch} merges cleanly into {into}: no conflicts.")
                    } else {
                        format!(
                            "Merging {branch} into {into} conflicts in {} file(s):\n\n{}",
                            conflicts.len(),
                            conflicts.join("\n")
                        )
                    };
                    self.dialog = Dialog::Confirm {
                        title: "Merge preview".into(),
                        text,
                        button: "Merge".into(),
                        req: Some(Request::Merge(branch)),
                    };
                }
                Response::OpenUrl(url) => self.ctx.open_url(egui::OpenUrl::new_tab(url)),
                Response::Remotes(r) => {
                    self.remote_edit = r.iter().map(|x| x.fetch_url.clone()).collect();
                    self.remotes = Some(r);
                }
                Response::Config(rows) => {
                    self.config_edit = rows
                        .iter()
                        .map(|r| r.value.value.clone().unwrap_or_default())
                        .collect();
                    self.config = Some(rows);
                }
                Response::Grep { pattern, hits } => {
                    self.grep = Some((pattern, hits));
                }
                Response::Files { rev, paths } => {
                    self.files_loading = false;
                    if rev == self.files_rev {
                        self.files_tree = Some(FileTree::build(&paths));
                        self.files = Some(paths);
                    }
                }
                Response::FileContent { rev, path, result } => {
                    if let Some(v) = &mut self.viewer
                        && v.rev == rev
                        && v.path == path
                    {
                        v.state = match result {
                            Err(e) => ViewerState::Failed(e),
                            Ok(d) => match d.text {
                                None => ViewerState::Binary(d.size),
                                Some(t) => ViewerState::Ready(Box::new(DiffView::build_file(
                                    &path,
                                    &t,
                                    settings.theme == ThemeMode::Dark,
                                ))),
                            },
                        };
                    }
                }
                Response::HookOutput(t) => self.hook_output = Some(t),
                Response::Committed => {
                    self.prefix_done = false;
                    if self.push_after {
                        self.push_after = false;
                        let branch = self.repo.as_ref().and_then(|r| r.status.branch.clone());
                        match branch {
                            Some(b) => self.send(Request::PushBranch(b)),
                            None => self.send(Request::Push { force: false }),
                        }
                    }
                    settings.remember_message(&self.last_message);
                    settings.save();
                    self.message.clear();
                    self.amend = false;
                    self.file_diff = None;
                    self.work_image = None;
                    self.selected_file = None;
                    self.commit_open = false;
                }
                Response::AutoFetch(r) => self.auto_fetch_error = r.err(),
                Response::Info(m) => {
                    self.info = Some(m);
                    self.error = None;
                }
                Response::Error(e) => {
                    self.push_after = false;
                    if let Some(b) = self.last_checkout.take()
                        && (e.contains("would be overwritten") || e.contains("commit your changes or stash"))
                    {
                        self.dialog = Dialog::Confirm {
                            title: "Local changes in the way".into(),
                            text: format!(
                                "Checking out {} would overwrite your local changes.\n\nStash them, switch branch and re-apply them?",
                                b.name
                            ),
                            button: "Stash and check out".into(),
                            req: Some(Request::CheckoutStashed(b)),
                        };
                    }
                    self.error = Some(e);
                    self.info = None;
                }
                Response::Idle => {
                    self.pending = self.pending.saturating_sub(1);
                    self.quiet_until = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        if std::mem::take(&mut self.reselect_pending) && self.commit_open {
            self.reselect();
        }
        self.handle_watch_events();
        app
    }

    // ------------------------------------------------------------------ commit selection

    /// (position in the log, commit) of the multi-selection, newest first.
    pub fn multi_sorted(&self) -> Vec<(usize, &Commit)> {
        let Some(r) = &self.repo else { return Vec::new() };
        let mut v: Vec<(usize, &Commit)> = r
            .commits
            .iter()
            .enumerate()
            .filter(|(_, c)| self.multi.contains(&c.id))
            .collect();
        v.sort_by_key(|(i, _)| *i);
        v
    }

    fn select_commit(&mut self, idx: usize, ctrl: bool, shift: bool) {
        let Some(id) = self
            .repo
            .as_ref()
            .and_then(|r| r.commits.get(idx))
            .map(|c| c.id.clone())
        else {
            return;
        };

        if shift && let Some(anchor) = self.selected {
            let (lo, hi) = (anchor.min(idx), anchor.max(idx));
            self.multi = self.repo.as_ref().map_or_else(Vec::new, |r| {
                r.commits[lo..=hi].iter().map(|c| c.id.clone()).collect()
            });
            return;
        }
        if ctrl {
            if self.multi.is_empty()
                && let Some(cur) = &self.selected_id
            {
                self.multi.push(cur.clone());
            }
            if let Some(pos) = self.multi.iter().position(|m| *m == id) {
                self.multi.remove(pos);
            } else {
                self.multi.push(id.clone());
            }
            // The last clicked commit stays the anchor for Shift+click.
            self.selected = Some(idx);
            if self.multi.len() <= 1 {
                // Back to a plain selection of the remaining commit (or none).
                let keep = self.multi.pop();
                self.selected_id = None;
                if let Some(k) = keep {
                    let i = self
                        .repo
                        .as_ref()
                        .and_then(|r| r.commits.iter().position(|c| c.id == k));
                    if let Some(i) = i {
                        self.select_plain(i, false);
                    }
                } else {
                    self.reset_detail();
                }
            } else {
                self.selected_id = Some(id);
            }
            return;
        }
        self.multi.clear();
        self.select_plain(idx, true);
    }

    /// Plain click: show one commit (or hide it again when it was already shown).
    fn select_plain(&mut self, idx: usize, toggle: bool) {
        let Some(id) = self
            .repo
            .as_ref()
            .and_then(|r| r.commits.get(idx))
            .map(|c| c.id.clone())
        else {
            return;
        };
        self.commit_files.clear();
        self.selected_cfile = None;
        self.commit_diff = None;
        self.commit_image = None;
        self.commit_body.clear();
        self.signature = None;
        if toggle && self.selected_id.as_deref() == Some(id.as_str()) && self.compare_id.is_none() {
            self.selected = None;
            self.selected_id = None;
            self.detail_key.clear();
        } else {
            self.compare_id = None;
            self.detail_key = id.clone();
            self.selected = Some(idx);
            self.selected_id = Some(id.clone());
            self.send(Request::CommitInfo(id));
        }
    }

    fn compare_selected(&mut self) {
        let sorted = self.multi_sorted();
        if sorted.len() != 2 {
            return;
        }
        // The older commit (further down the list) is "before".
        let (newer, older) = (sorted[0].1.id.clone(), sorted[1].1.id.clone());
        let newer_idx = sorted[0].0;
        self.multi.clear();
        self.commit_files.clear();
        self.selected_cfile = None;
        self.commit_diff = None;
        self.commit_image = None;
        self.signature = None;
        self.detail_key = format!("{older}..{newer}");
        self.compare_id = Some(older.clone());
        self.selected = Some(newer_idx);
        self.selected_id = Some(newer.clone());
        self.send(Request::CompareInfo { a: older, b: newer });
    }

    // ------------------------------------------------------------------ history editing helpers

    /// Commits that may be moved or squashed: the unpushed part of the first-parent line from HEAD.
    fn compute_reorderable(&mut self) {
        self.reorderable.clear();
        let Some(r) = &self.repo else { return };
        let Some(head) = r.head_id.as_deref() else { return };
        let chain = first_parent_chain(&r.commits, head);
        let limit = match r.branches.iter().find(|b| b.is_head) {
            Some(b) if b.upstream.is_some() && !b.gone => b.ahead as usize,
            _ => MAX_UNPUSHED_WITHOUT_UPSTREAM,
        };
        for c in chain.iter().take(limit) {
            if c.parents.len() != 1 {
                break;
            }
            self.reorderable.insert(c.id.clone());
        }
    }

    /// How many commits from HEAD down are not on the upstream yet (`None` without an upstream).
    fn unpushed_count(&self) -> Option<usize> {
        self.repo
            .as_ref()?
            .branches
            .iter()
            .find(|b| b.is_head)
            .filter(|b| b.upstream.is_some() && !b.gone)
            .map(|b| b.ahead as usize)
    }

    /// Builds the squash dialog for the selected commits, or says why it cannot be done.
    pub fn squash_dialog(&self) -> Result<Dialog, String> {
        let r = self.repo.as_ref().ok_or("No repository open")?;
        let head = r.head_id.as_deref().ok_or("The repository has no commits")?;
        let chain = first_parent_chain(&r.commits, head);
        let mut pos: Vec<usize> = Vec::new();
        for id in &self.multi {
            pos.push(
                chain
                    .iter()
                    .position(|c| &c.id == id)
                    .ok_or("Select commits on the current branch (its first-parent history)")?,
            );
        }
        pos.sort_unstable();
        let (min, max) = (pos[0], *pos.last().ok_or("Select at least two commits")?);
        if pos.len() < 2 {
            return Err("Select at least two commits".into());
        }
        if max - min + 1 != pos.len() {
            return Err("The commits must follow each other directly".into());
        }
        if chain[..=max].iter().any(|c| c.parents.len() > 1) {
            return Err("A merge commit is in the way; rebasing would flatten it".into());
        }
        let base = chain[max]
            .parents
            .first()
            .cloned()
            .ok_or("The initial commit cannot be squashed into")?;
        let warning = self.unpushed_count().is_some_and(|ahead| max >= ahead).then(|| {
            "Some of these commits were already pushed. You will have to force push afterwards.".to_string()
        });
        let selected: HashSet<usize> = pos.iter().copied().collect();
        let plan: Vec<(String, bool)> = (0..=max)
            .rev()
            .map(|p| (chain[p].id.clone(), selected.contains(&p)))
            .collect();
        let mut subjects: Vec<&str> = pos.iter().rev().map(|&p| chain[p].subject.as_str()).collect();
        let first = subjects.remove(0).to_string();
        let message = if subjects.is_empty() {
            first
        } else {
            format!(
                "{first}\n\n{}",
                subjects
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        Ok(Dialog::Squash {
            base,
            plan,
            message,
            count: pos.len(),
            warning,
        })
    }

    /// Builds the confirmation for dragging commit `from` above commit `to`.
    pub fn reorder_dialog(&self, from: &str, to: &str) -> Result<Dialog, String> {
        let r = self.repo.as_ref().ok_or("No repository open")?;
        let head = r.head_id.as_deref().ok_or("The repository has no commits")?;
        if !self.reorderable.contains(from) || !self.reorderable.contains(to) {
            return Err("Only commits that have not been pushed yet can be reordered".into());
        }
        let chain = first_parent_chain(&r.commits, head);
        let fp = chain
            .iter()
            .position(|c| c.id == from)
            .ok_or("Commit not found")?;
        let tp = chain.iter().position(|c| c.id == to).ok_or("Commit not found")?;
        if fp == tp {
            return Err("Nothing to move".into());
        }
        let max = fp.max(tp);
        let base = chain[max]
            .parents
            .first()
            .cloned()
            .ok_or("The initial commit cannot be moved")?;
        let original: Vec<usize> = (0..=max).collect();
        let mut order = original.clone();
        let moved = order.remove(fp);
        let at = order.iter().position(|&p| p == tp).ok_or("Commit not found")?;
        order.insert(at, moved);
        if order == original {
            return Err("The commit is already there".into());
        }
        let items: Vec<TodoItem> = order
            .iter()
            .rev()
            .map(|&p| TodoItem {
                action: TodoAction::Pick,
                id: chain[p].id.clone(),
                message: None,
            })
            .collect();
        Ok(Dialog::Confirm {
            title: "Reorder commits".into(),
            text: format!(
                "Move \"{}\" above \"{}\"?\n\nThis rewrites {} commit(s). Conflicts are possible; you can abort afterwards.",
                chain[fp].subject,
                chain[tp].subject,
                max + 1
            ),
            button: "Reorder".into(),
            req: Some(Request::InteractiveRebase { base, items }),
        })
    }

    fn show_error_dialog(&mut self, text: String) {
        self.dialog = Dialog::Confirm {
            title: "Cannot do that".into(),
            text,
            button: "OK".into(),
            req: None,
        };
    }

    // ------------------------------------------------------------------ acts

    #[allow(clippy::too_many_lines)]
    pub fn apply(&mut self, acts: Vec<Act>) -> Vec<AppAct> {
        let mut app = Vec::new();
        for act in acts {
            match act {
                Act::Send(r) => {
                    self.last_checkout = match &r {
                        Request::Checkout(b) => Some(b.clone()),
                        _ => None,
                    };
                    self.send(r);
                }
                Act::GraphOnly(name) => {
                    self.graph_refs = name.into_iter().collect();
                    self.apply_query();
                }
                Act::App(a) => app.push(a),
                Act::SelectCommit { idx, ctrl, shift } => self.select_commit(idx, ctrl, shift),
                Act::ClearMulti => self.multi.clear(),
                Act::CompareSelected => self.compare_selected(),
                Act::SquashSelected => match self.squash_dialog() {
                    Ok(d) => self.dialog = d,
                    Err(e) => self.show_error_dialog(e),
                },
                Act::CherryPickSelected => {
                    // Oldest first, so the commits apply in their original order.
                    let ids: Vec<String> = self
                        .multi_sorted()
                        .iter()
                        .rev()
                        .map(|(_, c)| c.id.clone())
                        .collect();
                    if !ids.is_empty() {
                        self.send(Request::CherryPickMany(ids));
                        self.multi.clear();
                    }
                }
                Act::RevertSelected => {
                    // Newest first.
                    let ids: Vec<String> = self.multi_sorted().iter().map(|(_, c)| c.id.clone()).collect();
                    if !ids.is_empty() {
                        self.send(Request::RevertMany(ids));
                        self.multi.clear();
                    }
                }
                Act::DropCommit { from, to } => match self.reorder_dialog(&from, &to) {
                    Ok(d) => self.dialog = d,
                    Err(e) => self.info = Some(e),
                },
                Act::TogglePin(n) => {
                    if let Some(i) = self.pins.iter().position(|x| *x == n) {
                        self.pins.remove(i);
                    } else {
                        self.pins.push(n);
                    }
                    if let Some(r) = &self.repo {
                        crate::branches::save_pins(&r.path, &self.pins);
                    }
                }
                Act::SelectCommitFile(p) => {
                    self.selected_cfile = Some(p);
                    self.request_commit_file_diff();
                }
                Act::SelectFile(change, staged) => self.show_file(change, staged),
                Act::OpenCommit => {
                    self.commit_open = true;
                    if let Some(r) = &self.repo {
                        self.sign = r.sign_default;
                    }
                    self.reselect();
                }
                Act::CommitPush => {
                    let msg = self.message.trim().to_string();
                    if !msg.is_empty() {
                        self.last_message = msg.clone();
                        self.push_after = true;
                        let (amend, sign, no_verify) = (self.amend, self.sign, self.no_verify);
                        self.send(Request::Commit {
                            message: msg,
                            amend,
                            sign: Some(sign),
                            no_verify,
                        });
                    }
                }
                Act::Commit => {
                    let msg = self.message.trim().to_string();
                    if !msg.is_empty() {
                        self.last_message = msg.clone();
                        let (amend, sign, no_verify) = (self.amend, self.sign, self.no_verify);
                        self.send(Request::Commit {
                            message: msg,
                            amend,
                            sign: Some(sign),
                            no_verify,
                        });
                    }
                }
                Act::NewBranch => {
                    let name = self.new_branch.trim().to_string();
                    if !name.is_empty() {
                        self.new_branch.clear();
                        self.send(Request::CreateBranch(name));
                    }
                }
                Act::Diff(a) => self.diff_action(a),
                Act::Dialog(d) => self.dialog = d,
                Act::ApplyQuery => self.apply_query(),
                Act::ClearQuery => {
                    self.search.clear();
                    self.author.clear();
                    self.path_filter = None;
                    self.branch_only = false;
                    self.apply_query();
                }
                Act::History(p) => {
                    self.path_filter = Some(p);
                    self.apply_query();
                }
                Act::Blame(p) => {
                    self.blame = None;
                    self.send(Request::Blame(p));
                }
                Act::ShowReflog => {
                    self.reflog = None;
                    self.send(Request::Reflog);
                }
                Act::ToggleAmend(on) => {
                    self.amend = on;
                    if on && let Some(c) = self.repo.as_ref().and_then(|r| r.commits.first()) {
                        let id = c.id.clone();
                        self.amend_fetch = Some(id.clone());
                        self.send(Request::CommitInfo(id));
                    }
                }
                Act::ReloadView => {}
                Act::OpenRepoSettings => {
                    self.repo_settings_open = true;
                    self.send(Request::LoadRemotes);
                    self.send(Request::LoadConfig);
                }
                Act::OpenFile { rev, path, line } => {
                    // Row of the line: file header, hunk header, then one row per line.
                    let jump = line.map(DiffView::file_line_row);
                    self.viewer = Some(FileViewer {
                        rev: rev.clone(),
                        path: path.clone(),
                        state: ViewerState::Loading,
                        jump,
                        find: String::new(),
                        find_pos: 0,
                    });
                    self.send(Request::ReadFile { rev, path });
                }
                Act::LoadFiles => {
                    if !self.files_loading {
                        self.files_loading = true;
                        let rev = self.files_rev.clone();
                        self.send(Request::LoadFiles(rev));
                    }
                }
                Act::GrepFiles => {
                    let pattern = self.grep_text.trim().to_string();
                    if !pattern.is_empty() {
                        let rev = self.files_rev.clone();
                        self.send(Request::Grep {
                            pattern,
                            rev,
                            ignore_case: true,
                        });
                    }
                }
                Act::BrowseAt(rev) => {
                    self.files_rev = rev;
                    self.files = None;
                    self.files_tree = None;
                    self.grep = None;
                    self.sidebar_files = true;
                    self.files_loading = true;
                    let rev = self.files_rev.clone();
                    self.send(Request::LoadFiles(rev));
                }
                Act::ShowDetails => self.details = self.error.clone(),
                Act::ConflictWrite => {
                    if let Some(e) = &self.conflict_edit
                        && e.choices.iter().all(Option::is_some)
                    {
                        let choices: Vec<Option<Choice>> = e
                            .choices
                            .iter()
                            .enumerate()
                            .map(|(i, c)| match c {
                                Some(Choice::Custom(_)) => Some(Choice::Custom(e.custom[i].clone())),
                                other => other.clone(),
                            })
                            .collect();
                        let content = conflict::resolve(&e.segs, &choices);
                        let path = e.path.clone();
                        self.conflict_edit = None;
                        self.send(Request::WriteResolved { path, content });
                    }
                }
            }
        }
        app
    }

    fn diff_action(&mut self, a: DiffAction) {
        let DiffAction::Patch {
            file,
            hunk,
            lines,
            mode,
        } = a;
        let Some(view) = &self.file_diff else { return };
        let Some(f) = view.files.get(file) else { return };
        let reverse = mode != PatchMode::Stage;
        let Some(patch) = build_patch(f, hunk, lines.as_ref(), reverse) else {
            self.error = Some("Nothing selected to apply.".into());
            return;
        };
        if mode == PatchMode::Discard {
            let what = if lines.is_some() {
                "the selected lines"
            } else {
                "this hunk"
            };
            self.dialog = Dialog::Confirm {
                title: "Discard changes".into(),
                text: format!("Discard {what}? This cannot be undone."),
                button: "Discard".into(),
                req: Some(Request::ApplyPatch { patch, mode }),
            };
        } else {
            self.send(Request::ApplyPatch { patch, mode });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|s| (*s).to_string()).collect(),
            author_name: String::new(),
            author_email: String::new(),
            time: 0,
            subject: format!("subject {id}"),
            refs: Vec::new(),
        }
    }

    #[test]
    fn chain_follows_first_parents_only() {
        // m merges side into main; the chain from m goes m -> a -> root.
        let commits = vec![
            commit("m", &["a", "side"]),
            commit("side", &["root"]),
            commit("a", &["root"]),
            commit("root", &[]),
        ];
        let ids: Vec<&str> = first_parent_chain(&commits, "m")
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["m", "a", "root"]);
        assert!(first_parent_chain(&commits, "unknown").is_empty());
    }

    #[test]
    fn file_tree_groups_paths() {
        let t = FileTree::build(&[
            "README.md".into(),
            "src/main.rs".into(),
            "src/util/mod.rs".into(),
            "src/util/io.rs".into(),
        ]);
        assert_eq!(t.files.len(), 1);
        assert_eq!(t.dirs.len(), 1);
        let src = &t.dirs["src"];
        assert_eq!(
            src.files,
            vec![("main.rs".to_string(), "src/main.rs".to_string())]
        );
        assert_eq!(src.dirs["util"].files.len(), 2);
    }
    fn repo_with(commits: Vec<Commit>, ahead: Option<u32>) -> RepoData {
        use rustle_core::{Branch, Graph, LfsInfo, Status};
        RepoData {
            path: PathBuf::from("x"),
            graph: Graph::default(),
            graph_wip: None,
            head_id: commits.first().map(|c| c.id.clone()),
            commits,
            branches: vec![Branch {
                name: "main".into(),
                is_head: true,
                is_remote: false,
                upstream: ahead.map(|_| "origin/main".into()),
                ahead: ahead.unwrap_or(0),
                behind: 0,
                gone: false,
            }],
            tags: vec![],
            stashes: vec![],
            submodules: vec![],
            status: Status::default(),
            query: LogQuery::default(),
            has_more: false,
            template: None,
            worktrees: vec![],
            git_dir: PathBuf::from("x/.git"),
            sign_default: false,
            lfs: LfsInfo::default(),
        }
    }

    /// A tab holding a linear history c4 (HEAD) -> c3 -> c2 -> c1.
    fn tab_with_history(ahead: Option<u32>) -> Tab {
        let mut tab = Tab::new(&egui::Context::default(), &Settings::default());
        let commits = vec![
            commit("c4", &["c3"]),
            commit("c3", &["c2"]),
            commit("c2", &["c1"]),
            commit("c1", &[]),
        ];
        tab.repo = Some(repo_with(commits, ahead));
        tab.compute_reorderable();
        tab
    }

    #[test]
    fn squash_plan_for_adjacent_commits() {
        let mut tab = tab_with_history(None);
        tab.multi = vec!["c3".into(), "c2".into()];
        let Ok(Dialog::Squash {
            base,
            plan,
            message,
            count,
            warning,
        }) = tab.squash_dialog()
        else {
            panic!("expected a squash dialog");
        };
        assert_eq!(base, "c1");
        assert_eq!(count, 2);
        // Oldest first; c4 is replayed on top and is not part of the squash.
        assert_eq!(
            plan,
            vec![
                ("c2".to_string(), true),
                ("c3".to_string(), true),
                ("c4".to_string(), false)
            ]
        );
        assert!(message.starts_with("subject c2") && message.contains("- subject c3"));
        assert!(warning.is_none(), "nothing has an upstream, so nothing is pushed");
    }

    #[test]
    fn squash_is_refused_for_bad_selections() {
        let mut tab = tab_with_history(None);
        tab.multi = vec!["c4".into(), "c2".into()];
        assert!(tab.squash_dialog().is_err(), "gap between the commits");
        tab.multi = vec!["c2".into()];
        assert!(tab.squash_dialog().is_err(), "needs at least two");
        tab.multi = vec!["c1".into(), "c2".into()];
        assert!(
            tab.squash_dialog().is_err(),
            "the initial commit has no parent to rebase onto"
        );
        tab.multi = vec!["c3".into(), "nope".into()];
        assert!(tab.squash_dialog().is_err(), "unknown commit");
    }

    #[test]
    fn squash_warns_about_pushed_commits() {
        // Only c4 is unpushed (ahead 1); c3 and c2 were pushed already.
        let mut tab = tab_with_history(Some(1));
        tab.multi = vec!["c3".into(), "c2".into()];
        let Ok(Dialog::Squash { warning, .. }) = tab.squash_dialog() else {
            panic!()
        };
        assert!(warning.unwrap().contains("force push"));
    }

    #[test]
    fn merge_commits_block_history_rewrites() {
        let mut tab = Tab::new(&egui::Context::default(), &Settings::default());
        tab.repo = Some(repo_with(
            vec![
                commit("m", &["b", "side"]),
                commit("side", &["a"]),
                commit("b", &["a"]),
                commit("a", &["root"]),
                commit("root", &[]),
            ],
            None,
        ));
        tab.compute_reorderable();
        // The merge sits above b and a, so rewriting them would flatten it.
        tab.multi = vec!["b".into(), "a".into()];
        assert!(tab.squash_dialog().is_err());
        assert!(
            tab.reorderable.is_empty(),
            "the first commit is a merge, so nothing may be moved"
        );
    }

    #[test]
    fn only_unpushed_commits_can_be_dragged() {
        let tab = tab_with_history(Some(2));
        assert!(tab.reorderable.contains("c4") && tab.reorderable.contains("c3"));
        assert!(!tab.reorderable.contains("c2") && !tab.reorderable.contains("c1"));
        assert!(tab.reorder_dialog("c2", "c4").is_err());

        let all = tab_with_history(None);
        assert!(all.reorderable.contains("c2"));
        assert!(!all.reorderable.contains("c1"), "the initial commit cannot move");
    }

    #[test]
    fn dragging_a_commit_above_another_builds_the_rebase() {
        let tab = tab_with_history(None);
        // c3 dropped above c4: the new order (newest first) is c3, c4.
        let Ok(Dialog::Confirm {
            req: Some(Request::InteractiveRebase { base, items }),
            ..
        }) = tab.reorder_dialog("c3", "c4")
        else {
            panic!("expected a reorder confirmation");
        };
        assert_eq!(base, "c2");
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["c4", "c3"], "oldest first: c4 is now below c3");
        assert!(items.iter().all(|i| i.action == TodoAction::Pick));
        // Dropping on itself or in the same place does nothing.
        assert!(tab.reorder_dialog("c3", "c3").is_err());
        assert!(
            tab.reorder_dialog("c4", "c3").is_err(),
            "c4 is already directly above c3"
        );
    }

    #[test]
    fn ctrl_and_shift_click_build_a_multi_selection() {
        let mut tab = tab_with_history(None);
        tab.select_commit(1, false, false);
        assert_eq!(tab.selected_id.as_deref(), Some("c3"));
        assert!(tab.multi.is_empty());

        tab.select_commit(3, true, false);
        assert_eq!(tab.multi.len(), 2);
        assert!(tab.multi.contains(&"c3".to_string()) && tab.multi.contains(&"c1".to_string()));

        // Clicking a selected commit again removes it; one left means a plain selection.
        tab.select_commit(3, true, false);
        assert!(tab.multi.is_empty());
        assert_eq!(tab.selected_id.as_deref(), Some("c3"));

        // Shift selects a range from the anchor.
        tab.select_commit(0, false, true);
        let ids: Vec<&str> = tab.multi.iter().map(String::as_str).collect();
        assert_eq!(ids, ["c4", "c3"]);
        assert_eq!(tab.multi_sorted().len(), 2);
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn file_tree_lists_folders_before_files() {
        let tree = FileTree::build(&paths(&["z.txt", "app/b.rs", "app/a.rs", "lang/nl/x.php"]));
        let mut out = Vec::new();
        tree_paths(&tree, &mut out);
        assert_eq!(out, ["app/b.rs", "app/a.rs", "lang/nl/x.php", "z.txt"]);
        assert_eq!(first_in_tree(&tree).as_deref(), Some("app/b.rs"));
    }

    #[test]
    fn first_in_tree_of_nothing_is_none() {
        assert_eq!(first_in_tree(&FileTree::build(&[])), None);
    }
}
