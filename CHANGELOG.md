# Changelog

## Unreleased

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
