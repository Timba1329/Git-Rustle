//! State of one repository tab: selection, filters, dialogs, and handling of worker responses.

use crate::diffview::{DiffAction, DiffView};
use crate::settings::{Settings, ThemeMode};
use crate::worker::{self, DiffKind, RepoData, Request, Response, ViewOpts, Worker};
use eframe::egui;
use rustle_core::{
    BlameLine, CommitFile, ConflictVersions, DiffOpts, FileChange, LogQuery, PatchMode, ReflogEntry,
    ResetMode, TodoAction, build_patch,
};
use std::collections::HashSet;
use std::path::PathBuf;

pub type LineSel = HashSet<(usize, usize, usize)>;

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
    SelectCommit { idx: usize, ctrl: bool },
    SelectCommitFile(String),
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
    App(AppAct),
}

pub struct Tab {
    pub title: String,
    pub worker: Worker,
    pub repo: Option<RepoData>,
    pub pending: usize,
    pub error: Option<String>,
    pub info: Option<String>,

    // Log filters.
    pub search: String,
    pub author: String,
    pub branch_only: bool,
    pub path_filter: Option<String>,
    pub focus_search: bool,
    pub load_more_sent: bool,
    /// When a fetch (manual or automatic) was last started.
    pub last_fetch: std::time::Instant,
    /// Why the last automatic fetch failed, if it did.
    pub auto_fetch_error: Option<String>,

    // Selected commit(s) and detail panel.
    pub selected: Option<usize>,
    pub selected_id: Option<String>,
    pub compare_id: Option<String>,
    pub detail_key: String,
    pub commit_body: String,
    pub commit_files: Vec<CommitFile>,
    pub selected_cfile: Option<String>,
    pub commit_diff: Option<DiffView>,
    pub commit_sel: LineSel,

    // Commit popup.
    pub commit_open: bool,
    pub selected_file: Option<(String, bool)>,
    pub file_diff: Option<DiffView>,
    pub file_sel: LineSel,
    pub conflict: Option<(String, ConflictVersions)>,
    pub message: String,
    pub amend: bool,
    pub amend_fetch: Option<String>,
    pub last_message: String,
    pub reselect_pending: bool,

    pub dialog: Dialog,
    pub new_branch: String,
    pub blame: Option<(String, Vec<BlameLine>)>,
    pub reflog: Option<Vec<ReflogEntry>>,
    pub identity: Option<(Option<String>, Option<String>)>,
    pub view: ViewOpts,
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

impl Tab {
    pub fn new(ctx: &egui::Context, s: &Settings) -> Self {
        let view = view_opts(s);
        Self {
            title: "New tab".into(),
            worker: worker::spawn(ctx.clone(), s.git_path.clone(), view, s.pull_mode),
            repo: None,
            pending: 0,
            error: None,
            info: None,
            search: String::new(),
            author: String::new(),
            branch_only: false,
            path_filter: None,
            focus_search: false,
            load_more_sent: false,
            last_fetch: std::time::Instant::now(),
            auto_fetch_error: None,
            selected: None,
            selected_id: None,
            compare_id: None,
            detail_key: String::new(),
            commit_body: String::new(),
            commit_files: Vec::new(),
            selected_cfile: None,
            commit_diff: None,
            commit_sel: LineSel::new(),
            commit_open: false,
            selected_file: None,
            file_diff: None,
            file_sel: LineSel::new(),
            conflict: None,
            message: String::new(),
            amend: false,
            amend_fetch: None,
            last_message: String::new(),
            reselect_pending: false,
            dialog: Dialog::None,
            new_branch: String::new(),
            blame: None,
            reflog: None,
            identity: None,
            view,
        }
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    pub fn send(&mut self, req: Request) {
        if matches!(req, Request::Fetch | Request::AutoFetch) {
            self.last_fetch = std::time::Instant::now();
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
        self.selected = None;
        self.selected_id = None;
        self.compare_id = None;
        self.detail_key.clear();
        self.commit_body.clear();
        self.commit_files.clear();
        self.selected_cfile = None;
        self.commit_diff = None;
        self.commit_sel.clear();
        self.selected_file = None;
        self.file_diff = None;
        self.file_sel.clear();
        self.conflict = None;
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
        self.detail_key.clear();
        self.commit_files.clear();
        self.selected_cfile = None;
        self.commit_diff = None;
        self.commit_body.clear();
    }

    /// Re-requests the diffs that are on screen (after a theme or diff-option change).
    pub fn reload_diffs(&mut self, s: &Settings) {
        self.view = view_opts(s);
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

    /// Keeps the file selection in the commit popup valid after a status change and loads
    /// the diff (or conflict versions) of the selected file.
    pub fn reselect(&mut self) {
        let Some(r) = &self.repo else { return };
        let files = &r.status.files;
        let kept = self.selected_file.as_ref().and_then(|(path, staged)| {
            files.iter().find(|f| &f.path == path).map(|f| {
                let still = if *staged { f.is_staged() } else { f.is_unstaged() };
                (f.clone(), if still { *staged } else { f.is_staged() })
            })
        });
        let pick = kept
            .or_else(|| files.iter().find(|f| f.conflicted).map(|f| (f.clone(), false)))
            .or_else(|| files.iter().find(|f| f.is_unstaged()).map(|f| (f.clone(), false)))
            .or_else(|| files.iter().find(|f| f.is_staged()).map(|f| (f.clone(), true)));
        match pick {
            Some((change, staged)) => {
                self.selected_file = Some((change.path.clone(), staged));
                if change.conflicted {
                    self.file_diff = None;
                    self.send(Request::ConflictVersions(change.path));
                } else {
                    self.conflict = None;
                    self.send(Request::FileDiff { change, staged });
                }
            }
            None => {
                self.selected_file = None;
                self.file_diff = None;
                self.conflict = None;
            }
        }
    }

    pub fn poll(&mut self, settings: &mut Settings) -> Vec<AppAct> {
        let mut app = Vec::new();
        while let Ok(resp) = self.worker.rx.try_recv() {
            match resp {
                Response::Repo(data) => {
                    if self.title == "New tab" || self.repo.as_ref().map(|r| &r.path) != Some(&data.path) {
                        self.title = folder_name(&data.path);
                    }
                    if self.repo.as_ref().map(|r| &r.path) != Some(&data.path) {
                        app.push(AppAct::RepoOpened(data.path.clone()));
                        // First automatic fetch shortly after opening a repository.
                        let iv = std::time::Duration::from_secs(u64::from(settings.auto_fetch_minutes) * 60);
                        self.last_fetch = std::time::Instant::now()
                            .checked_sub(iv.saturating_sub(std::time::Duration::from_secs(15)))
                            .unwrap_or_else(std::time::Instant::now);
                        self.auto_fetch_error = None;
                    }
                    self.selected = self
                        .selected_id
                        .as_ref()
                        .and_then(|id| data.commits.iter().position(|c| &c.id == id));
                    if self.selected.is_none() && self.compare_id.is_none() {
                        self.selected_id = None;
                    }
                    if self.message.is_empty()
                        && !self.amend
                        && let Some(t) = &data.template
                    {
                        self.message = t.clone();
                    }
                    self.repo = Some(*data);
                    self.load_more_sent = false;
                    self.reselect_pending = true;
                }
                Response::Status(s) => {
                    if let Some(r) = &mut self.repo {
                        r.status = s;
                    }
                    self.reselect_pending = true;
                }
                Response::Diff { kind, view } => match kind {
                    DiffKind::Work => {
                        self.file_diff = Some(*view);
                        self.file_sel.clear();
                        self.conflict = None;
                    }
                    DiffKind::Commit => {
                        self.commit_diff = Some(*view);
                        self.commit_sel.clear();
                    }
                },
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
                Response::CommitFiles { key, files } => {
                    if key == self.detail_key {
                        let keep = self
                            .selected_cfile
                            .as_ref()
                            .is_some_and(|p| files.iter().any(|f| &f.path == p));
                        if !keep {
                            self.selected_cfile = files.first().map(|f| f.path.clone());
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
                Response::Blame { path, lines } => self.blame = Some((path, lines)),
                Response::Reflog(l) => self.reflog = Some(l),
                Response::Identity { name, email } => self.identity = Some((name, email)),
                Response::Committed => {
                    settings.remember_message(&self.last_message);
                    settings.save();
                    self.message.clear();
                    self.amend = false;
                    self.file_diff = None;
                    self.selected_file = None;
                    self.commit_open = false;
                }
                Response::AutoFetch(r) => self.auto_fetch_error = r.err(),
                Response::Info(m) => {
                    self.info = Some(m);
                    self.error = None;
                }
                Response::Error(e) => {
                    self.error = Some(e);
                    self.info = None;
                }
                Response::Idle => self.pending = self.pending.saturating_sub(1),
            }
        }
        if std::mem::take(&mut self.reselect_pending) && self.commit_open {
            self.reselect();
        }
        app
    }

    pub fn apply(&mut self, acts: Vec<Act>) -> Vec<AppAct> {
        let mut app = Vec::new();
        for act in acts {
            match act {
                Act::Send(r) => self.send(r),
                Act::App(a) => app.push(a),
                Act::SelectCommit { idx, ctrl } => self.select_commit(idx, ctrl),
                Act::SelectCommitFile(p) => {
                    self.selected_cfile = Some(p);
                    self.request_commit_file_diff();
                }
                Act::SelectFile(change, staged) => {
                    self.selected_file = Some((change.path.clone(), staged));
                    if change.conflicted {
                        self.file_diff = None;
                        self.send(Request::ConflictVersions(change.path));
                    } else {
                        self.conflict = None;
                        self.send(Request::FileDiff { change, staged });
                    }
                }
                Act::OpenCommit => {
                    self.commit_open = true;
                    self.reselect();
                }
                Act::Commit => {
                    let msg = self.message.trim().to_string();
                    if !msg.is_empty() {
                        self.last_message = msg.clone();
                        let amend = self.amend;
                        self.send(Request::Commit { message: msg, amend });
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
            }
        }
        app
    }

    fn select_commit(&mut self, idx: usize, ctrl: bool) {
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
        self.commit_body.clear();
        if ctrl && self.selected_id.as_ref().is_some_and(|s| *s != id) {
            let a = self.selected_id.take().unwrap_or_default();
            self.detail_key = format!("{a}..{id}");
            self.compare_id = Some(a.clone());
            self.selected = Some(idx);
            self.selected_id = Some(id.clone());
            self.send(Request::CompareInfo { a, b: id });
        } else if self.selected_id.as_deref() == Some(id.as_str()) && self.compare_id.is_none() {
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
