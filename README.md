<p align="center"><img src="docs/icon.png" width="96" alt="Rustle"></p>

# Rustle

A fast, native Git client for Windows, written in Rust with [egui](https://github.com/emilk/egui).
It drives the real `git` command line, so your hooks, config, credential helpers, LFS and
submodules all behave exactly as they do in a terminal.

![Commit graph](docs/screenshot-main.png)

## Features

- **Commit graph** with coloured lanes, branch/tag labels and relative dates; loads incrementally, so
  large repositories stay fast. A **Working changes** row sits on top of HEAD when there are uncommitted changes.
- **Live:** the view reloads by itself when files or refs change on disk, and a background fetch keeps the
  push/pull badges per branch current (it never asks for a password).
- **Search and filter** by message, author, file history and "current branch only"; **search file contents**
  and browse the **file tree** of the working tree or of any commit, with a syntax-highlighted file viewer.
- **Several commits at once:** Ctrl/Shift+click to select; squash, cherry-pick, revert or compare them.
  **Drag** an unpushed commit onto another one to reorder.
- **Staging:** files, **hunks and single lines**; discard; amend; commit message history and
  `commit.template`; optional **signing** (GPG or SSH) and "skip hooks"; hook output is shown.
- **Diffs:** syntax highlighting, line numbers, side-by-side, intra-line highlighting, find in diff,
  visible whitespace, trailing-whitespace marks, whole-file view, ignore-whitespace and context options,
  and **image diffs** (side by side, swipe, onion skin).
- **Conflicts:** banner, a **three-way editor** (ours / base / theirs, per conflict: take either, both,
  the base or write your own), continue / abort / skip.
- **Branches, tags, stashes, submodules, worktrees and Git LFS** in the sidebar, with context menus.
  Per branch: ahead/behind badges, push or publish, fast-forward without checking out.
- **History editing:** merge, rebase, interactive rebase (reword/squash/fixup/drop/reorder),
  cherry-pick, revert, reset.
- **Remotes:** fetch, pull (fast-forward / merge / rebase), push, force push with lease, delete remote
  branches, push tags; running operations can be **cancelled**. A **repository settings** window edits
  remotes and common git configuration.
- **Command palette** (Ctrl+K), tabs, recent repositories, clone and init, drag a folder onto the window.
- Authentication prompts for HTTPS tokens and SSH passphrases (in addition to Git Credential Manager).
- Dark and light theme, adjustable text size, remembers window position, self-update from GitHub releases.
- Blame, reflog, open in Explorer / terminal / editor.

![Several commits selected](docs/screenshot-multi.png)

![Three-way conflict editor](docs/screenshot-details.png)

![Commit window](docs/screenshot-commit.png)

## Requirements

- Windows 10/11 (the code is portable, but only Windows is tested and packaged)
- [Git for Windows](https://git-scm.com/download/win) (Rustle looks in `PATH` and the default install
  locations; you can also set the path in Settings)

## Install

Download the installer or the portable zip from the GitHub releases page, or build it yourself
(below). The installer can add an *Open with Rustle* entry to the folder context menu.

```
rustle.exe [path-to-repository]
```

## Updates

Rustle checks the GitHub releases of this project at most once a day (switch it off, or point it
at another repository, in *Settings → Updates*). When a newer version exists, an **Update available**
button appears in the tab bar. *Update now* downloads `Rustle-<version>-portable.zip`, verifies it
against the published `.sha256` file, replaces `rustle.exe` and restarts. The old file is kept as
`rustle.exe.old` until the next start. If Rustle cannot write to its own folder (for example after a
machine-wide install in *Program Files*), it offers the releases page instead.

The checksum catches corrupt downloads; it is not a signature, so it cannot detect a tampered
release. Code signing (see `scripts/package.ps1`) would be needed for that.

## Releasing

1. Raise `version` in `Cargo.toml` and describe the changes in `CHANGELOG.md`.
2. Commit, then tag and push: `git tag v0.2.0` and `git push origin v0.2.0`.
3. The *Release* workflow builds, tests and publishes `Rustle-<version>-portable.zip`, its `.sha256`
   and the installer. The updater relies on exactly these asset names, and on the tag being `v<version>`.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+K` | Command palette |
| `Ctrl+O` | Open a repository |
| `Ctrl+T` / `Ctrl+W` | New / close tab |
| `Ctrl+Tab` | Next tab |
| `F5` | Refresh |
| `Ctrl+F` | Search commits |
| `Up` / `Down` | Move through the commit list |
| `Ctrl+Enter` | Commit (in the commit window) |
| `Ctrl+,` | Settings |
| `Esc` | Close the top-most dialog |

## Build from source

```
cargo run -p rustle-app          # debug build (dependencies are optimised, so it is quick)
cargo test --workspace
cargo build --release -p rustle-app
.\scripts\package.ps1            # portable zip (+ installer if Inno Setup 6 is installed)
```

`.\scripts\make-test-repo.ps1` creates a demo repository with a remote, branches and local changes.

## Project layout

- `crates/rustle-core` — `GitRunner` (git invocation with safe defaults), parsers, diff/patch
  builder, commit graph lanes, and all git operations. No UI dependencies.
- `crates/rustle-app` — the egui application. It talks to git only through a worker thread per
  tab (`worker.rs`), never directly from the UI.
- `installer/` — Inno Setup script. `scripts/` — packaging and demo scripts. `.github/` — CI and release workflows.

Design rules: git output is handled as bytes and only converted (lossily) to UTF-8 for display; the
UI thread never blocks on git; every destructive action asks for confirmation.

## Data and logs

Settings, the recent-repositories list and logs live in `%APPDATA%\Rustle` (`settings.ini`,
`recent.txt`, `rustle.log`, `crash.log`). Set `RUSTLE_LOG=debug` to log every git command.

## Known limitations

- Long lines in diffs scroll sideways instead of wrapping.
- File names that are not valid UTF-8 are preserved on Linux and macOS; on Windows git converts names to UTF-8
  itself, so the case hardly occurs there.
- Interactive rebase, squash and reordering work on the first-parent history of the current branch and refuse
  ranges that contain merge commits.
- Git LFS features need `git-lfs` to be installed.
- The updater has only been exercised against a local test server, not against a live GitHub release.
- Binaries are unsigned unless you provide a certificate (see `scripts/package.ps1`), so Windows
  SmartScreen may warn on first run.

## License

MIT — see [LICENSE](LICENSE).
