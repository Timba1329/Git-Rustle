use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git was not found; install Git for Windows or set its path in the settings")]
    NotFound,
    #[error("could not start git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git failed (code {code:?}): {stderr}")]
    Failed { code: Option<i32>, stderr: String },
    #[error("unexpected git output: {0}")]
    Parse(String),
    #[error("no remote is configured")]
    NoRemote,
    #[error("{0} is not a git repository")]
    NotARepo(PathBuf),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, GitError>;

impl GitError {
    /// A short, human-friendly explanation for common failures, or the raw message.
    pub fn friendly(&self) -> String {
        if let GitError::Failed { stderr, .. } = self {
            let s = stderr.to_lowercase();
            if s.contains("index.lock") {
                return "Another git process is using this repository (index.lock exists). \
                        Close other git tools, or delete .git/index.lock if no git process is running."
                    .into();
            }
            if s.contains("authentication failed") || s.contains("could not read username") {
                return format!("Authentication failed. Check your credentials.\n{stderr}");
            }
            if s.contains("permission denied (publickey)") {
                return format!("SSH authentication failed (permission denied).\n{stderr}");
            }
            if s.contains("author identity unknown") || s.contains("please tell me who you are") {
                return "Git does not know who you are. Set your name and e-mail in Settings.".into();
            }
            if s.contains("not possible to fast-forward") || s.contains("diverging branches") {
                return "The branches have diverged. Pull with merge or rebase (see Settings), \
                        or resolve it manually."
                    .into();
            }
            if s.contains("non-fast-forward") || s.contains("[rejected]") {
                return format!("The push was rejected. Pull first (or force push with lease).\n{stderr}");
            }
            if s.contains("your local changes") && s.contains("would be overwritten") {
                return format!(
                    "You have local changes that would be overwritten. Commit or stash them first.\n{stderr}"
                );
            }
        }
        self.to_string()
    }
}
