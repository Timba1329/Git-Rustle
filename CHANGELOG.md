# Changelog

## Unreleased

- Live updates: file watcher for the working tree and refs; "Working changes" row in the graph.
- Per-branch push/publish and fast-forward; hook output and long error details; commit signing (GPG/SSH)
  and "skip hooks".
- Several commits: Ctrl/Shift+click, squash, cherry-pick, revert, compare, drag to reorder unpushed commits.
- Command palette (Ctrl+K); file browser with content search and file viewer; repository settings
  (remotes, git configuration).
- Image diffs, three-way conflict editor, find in diff, whole-file view, visible whitespace and
  whitespace-only change markers.
- Worktrees, submodule actions, Git LFS tracking; lossless handling of non-UTF-8 paths.
- Self-update: daily check against GitHub releases, "Update now" downloads, verifies the checksum,
  replaces the executable and restarts. Can be switched off in Settings.
- Per-branch push/pull badges (ahead/behind, `local`, `gone`) in the sidebar and branch menu.
- Automatic background fetch (default every 5 min, configurable in Settings); non-interactive, so
  it never opens a login prompt.

## 0.1.0

First release.

- Commit graph with search, filters, file history, blame, reflog and commit comparison.
- Staging by file, hunk and line; discard; amend; commit message history.
- Syntax-highlighted unified and side-by-side diffs with intra-line highlighting.
- Branch, tag, stash and submodule management; merge, rebase (also interactive), cherry-pick,
  revert, reset; conflict resolution.
- Fetch, pull (ff-only / merge / rebase), push, force push with lease; cancellable operations.
- Password / passphrase prompts for HTTPS and SSH.
- Tabs, recent repositories, clone and init; drop a folder on the window to open it.
- Dark and light theme, settings, keyboard shortcuts, remembered window geometry.
- Windows packaging: icon, version info, manifest, portable zip and Inno Setup installer.
