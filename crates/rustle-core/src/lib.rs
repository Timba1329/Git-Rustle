//! Rustle core: a clean layer around the `git` CLI. No UI dependencies.

pub mod backend;
pub mod conflict;
pub mod diffparse;
pub mod error;
pub mod ext;
pub mod graph;
pub mod model;
pub mod parse;
pub mod pathenc;
pub mod runner;

pub use backend::CliBackend;
pub use diffparse::{
    FileDiff, Hunk, LineKind, SbsCell, SbsRow, build_patch, intra_change, parse_diff, side_by_side,
};
pub use error::GitError;
pub use graph::{Graph, GraphRow, Seg, build_graph};
pub use model::{
    BlameLine, Branch, Commit, CommitFile, ConfigValue, ConflictVersions, DiffOpts, FileChange, GrepHit,
    LfsInfo, LogQuery, OpResult, OpState, PatchMode, PullMode, RefKind, RefLabel, ReflogEntry, Remote,
    ResetMode, Side, SignatureInfo, StashEntry, Status, Submodule, TodoAction, TodoItem, Worktree,
};
pub use runner::{CancelToken, GitRunner, RunOpts};
