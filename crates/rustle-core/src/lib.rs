//! Rustle core: a clean layer around the `git` CLI. No UI dependencies.

pub mod backend;
pub mod diffparse;
pub mod error;
pub mod graph;
pub mod model;
pub mod parse;
pub mod runner;

pub use backend::CliBackend;
pub use diffparse::{
    FileDiff, Hunk, LineKind, SbsCell, SbsRow, build_patch, intra_change, parse_diff, side_by_side,
};
pub use error::GitError;
pub use graph::{Graph, GraphRow, Seg, build_graph};
pub use model::{
    BlameLine, Branch, Commit, CommitFile, ConflictVersions, DiffOpts, FileChange, LogQuery, OpResult,
    OpState, PatchMode, PullMode, RefKind, RefLabel, ReflogEntry, ResetMode, Side, StashEntry, Status,
    Submodule, TodoAction, TodoItem,
};
pub use runner::{CancelToken, GitRunner, RunOpts};
