//! The command palette (Ctrl+K): a searchable list of everything Rustle can do.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpenRepository,
    CloneRepository,
    NewRepository,
    NewTab,
    CloseTab,
    Settings,
    About,
    CheckUpdates,
    ToggleTheme,
    Fetch,
    Pull,
    Push,
    ForcePush,
    Refresh,
    Commit,
    Stash,
    NewBranch,
    SearchCommits,
    Reflog,
    ApplyPatch,
    Dashboard,
    Hooks,
    FetchAll,
    PullAll,
    RepoSettings,
    UpdateSubmodules,
    AddWorktree,
    TrackLfs,
    OpenExplorer,
    OpenTerminal,
    OpenEditor,
    ContinueOperation,
    AbortOperation,
    ToggleSideBySide,
    ToggleIgnoreWhitespace,
    ShowFiles,
    ShowRefs,
    Checkout(String),
    OpenRecent(PathBuf),
    GoToTab(usize),
}

pub struct Entry {
    pub command: Command,
    pub title: String,
    /// Shortcut or other hint shown at the right.
    pub hint: String,
    /// Only available while a repository is open.
    pub needs_repo: bool,
}

fn e(command: Command, title: &str, hint: &str, needs_repo: bool) -> Entry {
    Entry {
        command,
        title: title.into(),
        hint: hint.into(),
        needs_repo,
    }
}

/// All commands, plus one entry per local branch, recent repository and open tab.
pub fn entries(branches: &[String], recent: &[PathBuf], tabs: &[String]) -> Vec<Entry> {
    use Command::*;
    let mut v = vec![
        e(OpenRepository, "Open repository…", "Ctrl+O", false),
        e(CloneRepository, "Clone repository…", "", false),
        e(NewRepository, "Create new repository…", "", false),
        e(NewTab, "New tab", "Ctrl+T", false),
        e(CloseTab, "Close tab", "Ctrl+W", false),
        e(Settings, "Settings", "Ctrl+,", false),
        e(About, "About Rustle", "", false),
        e(CheckUpdates, "Check for updates", "", false),
        e(ToggleTheme, "Toggle dark / light theme", "", false),
        e(Fetch, "Fetch", "", true),
        e(Pull, "Pull", "", true),
        e(Push, "Push", "", true),
        e(ForcePush, "Force push (with lease)…", "", true),
        e(Refresh, "Refresh", "F5", true),
        e(Commit, "Commit changes…", "", true),
        e(Stash, "Stash changes…", "", true),
        e(NewBranch, "New branch…", "", true),
        e(SearchCommits, "Search commits", "Ctrl+F", true),
        e(Reflog, "Show reflog", "", true),
        e(ApplyPatch, "Apply a patch file…", "", true),
        e(Dashboard, "Repository dashboard", "", true),
        e(Hooks, "Manage git hooks…", "", true),
        e(FetchAll, "Fetch all open repositories", "", false),
        e(PullAll, "Pull all open repositories", "", false),
        e(
            RepoSettings,
            "Repository settings (remotes, git config)…",
            "",
            true,
        ),
        e(UpdateSubmodules, "Update all submodules", "", true),
        e(AddWorktree, "Add worktree…", "", true),
        e(TrackLfs, "Track files with Git LFS…", "", true),
        e(OpenExplorer, "Open repository in Explorer", "", true),
        e(OpenTerminal, "Open repository in terminal", "", true),
        e(OpenEditor, "Open repository in editor", "", true),
        e(
            ContinueOperation,
            "Continue merge / rebase / cherry-pick",
            "",
            true,
        ),
        e(AbortOperation, "Abort merge / rebase / cherry-pick…", "", true),
        e(ToggleSideBySide, "Toggle side-by-side diffs", "", false),
        e(
            ToggleIgnoreWhitespace,
            "Toggle ignore whitespace in diffs",
            "",
            false,
        ),
        e(ShowFiles, "Show the file browser", "", true),
        e(ShowRefs, "Show branches and tags", "", true),
    ];
    v.extend(
        branches
            .iter()
            .map(|b| e(Checkout(b.clone()), &format!("Check out {b}"), "branch", true)),
    );
    v.extend(recent.iter().map(|p| {
        let name = p
            .file_name()
            .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
        e(OpenRecent(p.clone()), &format!("Open {name}"), "recent", false)
    }));
    v.extend(
        tabs.iter()
            .enumerate()
            .map(|(i, t)| e(GoToTab(i), &format!("Go to tab: {t}"), "tab", false)),
    );
    v
}

/// Best score of one query word as a subsequence of `t`, trying every possible starting letter.
fn token_score(q: &[char], t: &[char]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for start in 0..t.len() {
        if t[start] != q[0] {
            continue;
        }
        let (mut qi, mut score, mut last, mut run) = (0usize, 0i32, None::<usize>, 0i32);
        for ti in start..t.len() {
            if qi < q.len() && t[ti] == q[qi] {
                // Consecutive letters and word starts count most.
                run = if last == Some(ti.wrapping_sub(1)) {
                    run + 1
                } else {
                    0
                };
                score += 1 + run * 4;
                if ti == 0 || !t[ti - 1].is_alphanumeric() {
                    score += 8;
                }
                if let Some(l) = last {
                    score -= (ti - l - 1).min(6) as i32;
                }
                last = Some(ti);
                qi += 1;
            }
        }
        if qi == q.len() && best.is_none_or(|b| score > b) {
            best = Some(score);
        }
    }
    best
}

/// Case-insensitive match where every word of the query must occur, in order, as letters of `text`.
/// Higher is better; `None` when the query does not match.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut total = 0;
    let mut any = false;
    for word in query.split_whitespace() {
        any = true;
        let q: Vec<char> = word.to_lowercase().chars().collect();
        total += token_score(&q, &t)?;
    }
    if !any {
        return Some(0);
    }
    Some(total - (t.len() as i32) / 8)
}

/// Indices into `entries` matching `query`, best first. Without a repository, repository commands are hidden.
pub fn filter(entries: &[Entry], query: &str, has_repo: bool) -> Vec<usize> {
    let mut ranked: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .filter(|(_, en)| has_repo || !en.needs_repo)
        .filter_map(|(i, en)| fuzzy(query, &en.title).map(|s| (i, s)))
        .collect();
    if !query.trim().is_empty() {
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    ranked.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequences_only() {
        assert!(fuzzy("pll", "Pull").is_some());
        assert!(fuzzy("fp", "Force push").is_some());
        assert!(fuzzy("xyz", "Pull").is_none());
        assert!(fuzzy("llup", "Pull").is_none(), "order matters");
        assert_eq!(fuzzy("", "anything"), Some(0));
    }

    #[test]
    fn each_word_is_matched_on_its_own() {
        // "set" lines up with the start of "settings", not with the "s" in "repository".
        let all = entries(&[], &[], &[]);
        let top = filter(&all, "repo set", true)[0];
        assert_eq!(all[top].command, Command::RepoSettings);
        let top = filter(&all, "force", true)[0];
        assert_eq!(all[top].command, Command::ForcePush);
    }

    #[test]
    fn better_matches_rank_higher() {
        // A word-start, consecutive match beats a scattered one.
        let strong = fuzzy("push", "Push").unwrap();
        let weak = fuzzy("push", "Prune the unused shared hooks").unwrap();
        assert!(strong > weak, "{strong} vs {weak}");
    }

    #[test]
    fn filter_ranks_and_hides_repo_commands() {
        let all = entries(
            &["main".into(), "feature/x".into()],
            &[PathBuf::from("C:/code/app")],
            &["app".into()],
        );
        let with_repo = filter(&all, "push", true);
        assert_eq!(all[with_repo[0]].command, Command::Push);
        let without = filter(&all, "push", false);
        assert!(without.iter().all(|&i| !all[i].needs_repo));
        assert!(!without.iter().any(|&i| all[i].command == Command::Push));

        let co = filter(&all, "check out feature", true);
        assert_eq!(all[co[0]].command, Command::Checkout("feature/x".into()));
        let rec = filter(&all, "open app", false);
        assert!(
            rec.iter()
                .any(|&i| matches!(all[i].command, Command::OpenRecent(_)))
        );
        // Empty query lists everything in the original order.
        assert_eq!(filter(&all, "", true).len(), all.len());
    }
}
