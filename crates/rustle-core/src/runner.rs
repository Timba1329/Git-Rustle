use crate::error::{GitError, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Lets another thread abort a running git process.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Extra options for a single git invocation.
#[derive(Default)]
pub struct RunOpts<'a> {
    /// Bytes written to the process' stdin.
    pub input: Option<&'a [u8]>,
    /// Kill the process when this token is cancelled.
    pub cancel: Option<&'a CancelToken>,
    /// Extra environment variables.
    pub env: &'a [(&'a str, &'a str)],
    /// Never ask the user for anything: drop the askpass helpers and tell credential managers
    /// not to show UI. Used for background work, which must fail quietly instead of prompting.
    pub non_interactive: bool,
    /// Treat exit codes `0..=max` as success (e.g. `git merge-file` returns the number of conflicts).
    pub max_ok_exit: Option<i32>,
}

/// Thin wrapper around `git` with safe defaults for GUI use.
#[derive(Debug, Clone)]
pub struct GitRunner {
    git: PathBuf,
    askpass: Option<PathBuf>,
}

impl GitRunner {
    /// Looks for git in PATH, then in standard install locations.
    pub fn discover() -> Result<Self> {
        Ok(Self {
            git: find_git().ok_or(GitError::NotFound)?,
            askpass: None,
        })
    }

    /// Use an explicit path (e.g. from the settings).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            git: path.into(),
            askpass: None,
        }
    }

    /// Program git and ssh call to ask for passwords and passphrases. It is started with the
    /// prompt as its first argument and `RUSTLE_ASKPASS=1` in its environment.
    pub fn with_askpass(mut self, program: impl Into<PathBuf>) -> Self {
        self.askpass = Some(program.into());
        self
    }

    pub fn git_path(&self) -> &Path {
        &self.git
    }

    fn command(&self, cwd: Option<&Path>) -> Command {
        let mut cmd = Command::new(&self.git);
        cmd.args(["-c", "core.longpaths=true", "-c", "color.ui=false"])
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_EDITOR", ":")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(a) = &self.askpass {
            cmd.env("GIT_ASKPASS", a)
                .env("SSH_ASKPASS", a)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("RUSTLE_ASKPASS", "1");
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Runs git and returns the raw bytes; a non-zero exit code is an error.
    pub fn run<S: AsRef<std::ffi::OsStr>>(&self, args: &[S], cwd: Option<&Path>) -> Result<Output> {
        self.run_opts(args, cwd, &RunOpts::default())
    }

    /// Like [`run`](Self::run) with stdin, cancellation and extra environment.
    pub fn run_opts<S: AsRef<std::ffi::OsStr>>(
        &self,
        args: &[S],
        cwd: Option<&Path>,
        opts: &RunOpts<'_>,
    ) -> Result<Output> {
        let mut cmd = self.command(cwd);
        cmd.args(args);
        for (k, v) in opts.env {
            cmd.env(k, v);
        }
        if opts.non_interactive {
            for k in [
                "GIT_ASKPASS",
                "SSH_ASKPASS",
                "SSH_ASKPASS_REQUIRE",
                "RUSTLE_ASKPASS",
            ] {
                cmd.env_remove(k);
            }
            cmd.env("GCM_INTERACTIVE", "never");
        }
        if opts.input.is_some() {
            cmd.stdin(Stdio::piped());
        }
        tracing::debug!(target: "git", "run {:?}", cmd);

        let spawn_err = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::NotFound
            } else {
                e.into()
            }
        };

        let out = if opts.input.is_none() && opts.cancel.is_none() {
            cmd.output().map_err(spawn_err)?
        } else {
            let mut child = cmd.spawn().map_err(spawn_err)?;
            if let (Some(data), Some(mut si)) = (opts.input, child.stdin.take()) {
                let data = data.to_vec();
                std::thread::spawn(move || {
                    let _ = si.write_all(&data);
                });
            }
            let mut so = child.stdout.take();
            let mut se = child.stderr.take();
            let t_out = std::thread::spawn(move || {
                let mut v = Vec::new();
                if let Some(s) = so.as_mut() {
                    let _ = s.read_to_end(&mut v);
                }
                v
            });
            let t_err = std::thread::spawn(move || {
                let mut v = Vec::new();
                if let Some(s) = se.as_mut() {
                    let _ = s.read_to_end(&mut v);
                }
                v
            });
            let status = loop {
                if let Some(s) = child.try_wait()? {
                    break s;
                }
                if opts.cancel.is_some_and(CancelToken::is_cancelled) {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    // Grandchildren may still hold the pipes open; let the readers finish on
                    // their own instead of waiting for them.
                    drop((t_out, t_err));
                    return Err(GitError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(15));
            };
            Output {
                status,
                stdout: t_out.join().unwrap_or_default(),
                stderr: t_err.join().unwrap_or_default(),
            }
        };

        let ok_code = opts
            .max_ok_exit
            .is_some_and(|m| out.status.code().is_some_and(|c| (0..=m).contains(&c)));
        if out.status.success() || ok_code {
            Ok(out)
        } else {
            Err(GitError::Failed {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }
}

/// Kills a child and everything it started (git spawns ssh, remote helpers, hooks, ...).
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn find_git() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Some(root) = std::env::var_os(var).map(PathBuf::from) {
                for sub in ["Git\\cmd\\git.exe", "Programs\\Git\\cmd\\git.exe"] {
                    let p = root.join(sub);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}
