//! Which branch is gh-ship releasing from?
//!
//! With several release lines configured, the base branch is no longer a
//! setting but an input: it selects the line. gh-ship runs in exactly
//! two places, so it looks in exactly two places — the GitHub Actions
//! environment, and the local checkout — and gives up rather than guess.
//!
//! Nothing here spawns a process or touches the network. `gh` is the
//! only subprocess in the codebase, and reading `.git/HEAD` keeps it
//! that way: no `git` on `PATH` to depend on, no clone, no fetch, and a
//! detection story that unit tests can drive with two lines of
//! `fs::write`.

use std::collections::BTreeMap;
use std::path::Path;

/// Where a base branch came from, so `gh ship status` can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `--base`, or `SHIP_BASE_BRANCH`.
    Flag,
    /// The GitHub Actions environment.
    Ci,
    /// The local checkout's `.git/HEAD`.
    Git,
    /// Nothing said otherwise, so the repository's default branch.
    Default,
}

impl Origin {
    /// How the origin reads in a report.
    pub fn describe(self) -> &'static str {
        match self {
            Origin::Flag => "from --base",
            Origin::Ci => "detected from CI",
            Origin::Git => "detected from the checkout",
            Origin::Default => "the repository default",
        }
    }
}

/// A base branch and how it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub branch: String,
    pub origin: Origin,
}

/// Detect the branch gh-ship is releasing from.
///
/// `None` means "nothing to go on"; the caller decides what to fall back
/// to, because only it knows the repository's default branch.
pub fn base_branch(flag: Option<&str>, root: &Path) -> Option<Detected> {
    base_branch_in(flag, root, &environment())
}

/// As [`base_branch`], against a given environment.
///
/// The whole of the precedence lives here, taking the environment as an
/// argument, so that it can be tested without `std::env::set_var` —
/// process-global, while nextest runs tests in threads — and without the
/// tests silently reading the environment of the CI job running them,
/// where `GITHUB_REF` is very much set.
fn base_branch_in(
    flag: Option<&str>,
    root: &Path,
    env: &BTreeMap<String, String>,
) -> Option<Detected> {
    if let Some(branch) = flag.map(str::trim).filter(|b| !b.is_empty()) {
        return Some(Detected {
            branch: branch.to_string(),
            origin: Origin::Flag,
        });
    }
    if let Some(branch) = from_ci(env) {
        return Some(Detected {
            branch,
            origin: Origin::Ci,
        });
    }
    from_git(root).map(|branch| Detected {
        branch,
        origin: Origin::Git,
    })
}

/// The process environment, read once.
///
/// The single place this module touches `std::env`; everything below it
/// takes a map.
fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// The base branch according to the GitHub Actions environment.
fn from_ci(env: &BTreeMap<String, String>) -> Option<String> {
    let get = |key: &str| env.get(key).map(String::as_str).filter(|v| !v.is_empty());

    if get("GITHUB_ACTIONS") != Some("true") {
        return None;
    }

    // On a `pull_request` event this is the branch the PR targets —
    // exactly gh-ship's base branch. It must be checked first, because
    // `GITHUB_REF` on that event is `refs/pull/N/merge`, which names no
    // branch at all.
    //
    // `GITHUB_HEAD_REF` is deliberately *not* consulted: it is the PR's
    // *source* branch, so using it would resolve a release line from a
    // contributor's feature branch.
    if let Some(base) = get("GITHUB_BASE_REF") {
        return Some(base.to_string());
    }

    let reference = get("GITHUB_REF")?;
    let branch = reference.strip_prefix("refs/heads/")?;

    // `GITHUB_REF_NAME` is the runner's own already-stripped answer;
    // prefer it, but only once `GITHUB_REF` has confirmed this is a
    // branch and not a tag.
    Some(get("GITHUB_REF_NAME").unwrap_or(branch).to_string())
}

/// The checked-out branch, read from `.git/HEAD`.
///
/// A detached HEAD holds a bare SHA rather than a symbolic ref, so it
/// fails the prefix check and correctly yields nothing.
fn from_git(root: &Path) -> Option<String> {
    let git = resolve_git_dir(root)?;
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?;
    (!branch.is_empty()).then(|| branch.to_string())
}

/// The real `.git` directory, following the `gitdir:` indirection.
///
/// In a linked worktree — which is exactly how someone maintaining two
/// release lines locally works — `.git` is a file pointing elsewhere.
fn resolve_git_dir(root: &Path) -> Option<std::path::PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return Some(git);
    }
    let pointer = std::fs::read_to_string(&git).ok()?;
    let target = pointer.trim().strip_prefix("gitdir:")?.trim();
    let target = Path::new(target);
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        root.join(target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ignores_the_environment_outside_actions() {
        let e = env(&[("GITHUB_REF", "refs/heads/main")]);
        assert_eq!(from_ci(&e), None, "GITHUB_ACTIONS gates the whole lookup");
    }

    #[test]
    fn reads_the_branch_of_a_push() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/heads/release/1.x"),
            ("GITHUB_REF_NAME", "release/1.x"),
        ]);
        assert_eq!(from_ci(&e), Some("release/1.x".into()));
    }

    #[test]
    fn falls_back_to_stripping_the_ref() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/heads/main"),
        ]);
        assert_eq!(from_ci(&e), Some("main".into()));
    }

    #[test]
    fn a_pull_request_uses_the_branch_it_targets() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/pull/7/merge"),
            ("GITHUB_BASE_REF", "release/1.x"),
            ("GITHUB_HEAD_REF", "feature/whatever"),
        ]);
        assert_eq!(
            from_ci(&e),
            Some("release/1.x".into()),
            "never the contributor's branch"
        );
    }

    #[test]
    fn base_ref_wins_over_ref_name() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/heads/main"),
            ("GITHUB_REF_NAME", "main"),
            ("GITHUB_BASE_REF", "release/1.x"),
        ]);
        assert_eq!(from_ci(&e), Some("release/1.x".into()));
    }

    #[test]
    fn a_tag_names_no_branch() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/tags/v1.0.0"),
            ("GITHUB_REF_NAME", "v1.0.0"),
        ]);
        assert_eq!(from_ci(&e), None, "a tag must not be mistaken for a branch");
    }

    #[test]
    fn a_merge_ref_without_a_base_names_no_branch() {
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/pull/7/merge"),
        ]);
        assert_eq!(from_ci(&e), None);
    }

    #[test]
    fn reads_the_checked_out_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "ref: refs/heads/release/1.x\n",
        )
        .unwrap();
        assert_eq!(from_git(dir.path()), Some("release/1.x".into()));
    }

    #[test]
    fn a_detached_head_names_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "9fceb02c0b1f7e3f4a2d5c6b8a1e0d3f5c7b9a2e\n",
        )
        .unwrap();
        assert_eq!(from_git(dir.path()), None);
    }

    #[test]
    fn follows_a_worktree_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("elsewhere");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            dir.path().join(".git"),
            format!("gitdir: {}\n", real.display()),
        )
        .unwrap();
        assert_eq!(from_git(dir.path()), Some("main".into()));
    }

    #[test]
    fn nothing_to_go_on_is_not_an_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(from_git(dir.path()), None);
    }

    #[test]
    fn the_flag_wins() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/heads/main"),
        ]);
        let d = base_branch_in(Some("release/2.x"), dir.path(), &e).unwrap();
        assert_eq!(d.branch, "release/2.x");
        assert_eq!(d.origin, Origin::Flag);
    }

    #[test]
    fn ci_beats_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/local\n").unwrap();
        let e = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REF", "refs/heads/from-ci"),
        ]);
        let d = base_branch_in(None, dir.path(), &e).unwrap();
        assert_eq!(d.branch, "from-ci");
        assert_eq!(d.origin, Origin::Ci);
    }

    #[test]
    fn the_checkout_is_the_last_resort() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/local\n").unwrap();
        let d = base_branch_in(None, dir.path(), &env(&[])).unwrap();
        assert_eq!(d.branch, "local");
        assert_eq!(d.origin, Origin::Git);
    }

    #[test]
    fn a_blank_flag_is_not_an_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(base_branch_in(Some("  "), dir.path(), &env(&[])), None);
    }

    #[test]
    fn nothing_anywhere_is_not_an_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(base_branch_in(None, dir.path(), &env(&[])), None);
    }
}
