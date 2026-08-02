//! Repository, branch, pull request and release operations.
//!
//! All of these go through the GitHub CLI. gh-ship deliberately holds no
//! REST client and no tokens.

use serde::Deserialize;

use super::cli::{Gh, GhError};

// --- Repository ----------------------------------------------------------

/// The subset of repository metadata gh-ship needs.
#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
    #[serde(rename = "defaultBranchRef")]
    pub default_branch: BranchRef,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BranchRef {
    pub name: String,
}

/// Resolve the current (or `--repo`) repository.
pub fn repository(gh: &Gh) -> Result<Repository, GhError> {
    gh.json_scoped(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner,defaultBranchRef,url",
    ])
}

// --- Branches ------------------------------------------------------------

/// Whether a branch exists on the remote.
///
/// Uses the API rather than the local clone, because gh-ship must work
/// with `--repo` against a repository that was never cloned.
pub fn branch_exists(gh: &Gh, repo: &str, branch: &str) -> Result<bool, GhError> {
    match gh.run(&[
        "api",
        &format!("repos/{repo}/branches/{branch}"),
        "--silent",
    ]) {
        Ok(_) => Ok(true),
        // A missing branch is a 404, which is an expected answer here
        // rather than an error worth surfacing.
        Err(GhError::Failed { stderr, .. }) if stderr.contains("404") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Create `branch` pointing at `sha`.
pub fn create_branch_at(gh: &Gh, repo: &str, branch: &str, sha: &str) -> Result<(), GhError> {
    gh.run(&[
        "api",
        &format!("repos/{repo}/git/refs"),
        "--method",
        "POST",
        "-f",
        &format!("ref=refs/heads/{branch}"),
        "-f",
        &format!("sha={sha}"),
        "--silent",
    ])
    .map(|_| ())
}

/// Create `refs/tags/{tag}` at `sha`.
///
/// gh-ship tags explicitly rather than letting `gh release create` do it as a
/// side effect, because a **draft** release does not create the git ref — the
/// tag only appears when the release is published. The publish workflow is
/// dispatched on that tag and checks it out, so without this the very first
/// release fails after the release object already exists.
///
/// Idempotent: a tag that already exists is not an error, so re-running after a
/// partial failure works.
pub fn create_tag(gh: &Gh, repo: &str, tag: &str, sha: &str) -> Result<(), GhError> {
    match gh.run(&[
        "api",
        &format!("repos/{repo}/git/refs"),
        "--method",
        "POST",
        "-f",
        &format!("ref=refs/tags/{tag}"),
        "-f",
        &format!("sha={sha}"),
        "--silent",
    ]) {
        Ok(_) => Ok(()),
        Err(GhError::Failed { stderr, .. })
            if stderr.contains("already exists") || stderr.contains("422") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Force `branch` to point at `sha`, discarding whatever was there.
///
/// Used to bring the release branch back in line with its base. The reset is
/// not a fast-forward — the release branch carries a version bump the base does
/// not — so `force` is required.
pub fn reset_branch(gh: &Gh, repo: &str, branch: &str, sha: &str) -> Result<(), GhError> {
    gh.run(&[
        "api",
        &format!("repos/{repo}/git/refs/heads/{branch}"),
        "--method",
        "PATCH",
        "-f",
        &format!("sha={sha}"),
        "-F",
        "force=true",
        "--silent",
    ])
    .map(|_| ())
}

/// Delete a branch, ignoring one that is already gone.
pub fn delete_branch(gh: &Gh, repo: &str, branch: &str) -> Result<(), GhError> {
    match gh.run(&[
        "api",
        &format!("repos/{repo}/git/refs/heads/{branch}"),
        "--method",
        "DELETE",
        "--silent",
    ]) {
        Ok(_) => Ok(()),
        // Already deleted is the desired state, not a failure.
        Err(GhError::Failed { stderr, .. })
            if stderr.contains("404") || stderr.contains("Reference does not exist") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Branch names starting with `prefix`.
///
/// Used to sweep abandoned staging branches. A failure to list is not fatal:
/// housekeeping should never be the reason a release cannot proceed.
pub fn matching_branches(gh: &Gh, repo: &str, prefix: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Ref {
        #[serde(rename = "ref")]
        name: String,
    }

    // Unscoped on purpose: `gh api` has no `--repo` flag, and the repository
    // is already in the URL path. Using the scoped variant here made the sweep
    // fail silently whenever `-R`/`SHIP_REPO` was set.
    gh.json::<Vec<Ref>, _>(&[
        "api",
        &format!("repos/{repo}/git/matching-refs/heads/{prefix}"),
    ])
    .map(|refs| {
        refs.into_iter()
            .filter_map(|r| r.name.strip_prefix("refs/heads/").map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

/// The commit SHA at the tip of a branch.
pub fn branch_sha(gh: &Gh, repo: &str, branch: &str) -> Result<String, GhError> {
    let out = gh.run(&[
        "api",
        &format!("repos/{repo}/git/ref/heads/{branch}"),
        "--jq",
        ".object.sha",
    ])?;
    Ok(out.trim().to_string())
}

// --- Labels --------------------------------------------------------------

/// Default colour for labels gh-ship creates.
///
/// Fixed rather than random so the result is reproducible and testable.
/// A pale blue reads as informational without competing with whatever
/// palette a project already uses.
const LABEL_COLOR: &str = "BFD4F2";

#[derive(Debug, Deserialize)]
struct Label {
    name: String,
}

/// Ensure every requested label exists, returning those safe to apply.
///
/// `gh pr create --label x` fails outright when `x` does not exist, which
/// means a missing label takes the whole Release PR down with it. That
/// trade is plainly wrong: the PR is the valuable artifact and the label
/// is decoration.
///
/// So gh-ship creates what is missing, and if it cannot — no `issues:
/// write`, a protected repository — it reports the label as dropped so
/// the caller can warn and carry on.
///
/// Returns `(usable, dropped)`.
pub fn ensure_labels(gh: &Gh, labels: &[String]) -> (Vec<String>, Vec<String>) {
    if labels.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // A failure to list is not fatal either: assume nothing exists and
    // let the create attempts sort it out.
    let existing: Vec<String> = gh
        .json_scoped::<Vec<Label>, _>(&["label", "list", "--limit", "200", "--json", "name"])
        .map(|labels| labels.into_iter().map(|l| l.name).collect())
        .unwrap_or_default();

    let mut usable = Vec::new();
    let mut dropped = Vec::new();

    for label in labels {
        // GitHub label names are case-insensitive for uniqueness.
        if existing.iter().any(|e| e.eq_ignore_ascii_case(label)) {
            usable.push(label.clone());
            continue;
        }
        match create_label(gh, label) {
            Ok(()) => usable.push(label.clone()),
            Err(_) => dropped.push(label.clone()),
        }
    }

    (usable, dropped)
}

fn create_label(gh: &Gh, name: &str) -> Result<(), GhError> {
    gh.run_scoped(&[
        "label",
        "create",
        name,
        "--color",
        LABEL_COLOR,
        "--description",
        "Created by gh-ship",
    ])
    .map(|_| ())
}

// --- Pull requests -------------------------------------------------------

/// The subset of PR fields gh-ship needs.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default, rename = "mergeCommit")]
    pub merge_commit: Option<Commit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    pub oid: String,
}

/// Interpret a pull request `state` string.
///
/// GitHub returns it uppercase (`MERGED`), `gh` echoes it in mixed case,
/// and the `status` command re-exposes it on its own struct — so the
/// comparison lives here once instead of being re-derived per call site.
pub mod state {
    pub fn is_merged(state: &str) -> bool {
        state.eq_ignore_ascii_case("merged")
    }

    pub fn is_open(state: &str) -> bool {
        state.eq_ignore_ascii_case("open")
    }

    pub fn is_closed(state: &str) -> bool {
        state.eq_ignore_ascii_case("closed")
    }
}

impl PullRequest {
    pub fn is_merged(&self) -> bool {
        state::is_merged(&self.state)
    }

    pub fn is_open(&self) -> bool {
        state::is_open(&self.state)
    }

    /// The commit the PR landed as.
    ///
    /// Never cache a SHA taken before the merge: a squash or rebase
    /// merge creates a *new* commit, so the branch tip gh-ship saw
    /// during `prepare` is not what ends up on the base branch. This is
    /// the only trustworthy source.
    pub fn merged_sha(&self) -> Option<&str> {
        self.merge_commit.as_ref().map(|c| c.oid.as_str())
    }
}

const PR_FIELDS: &str = "number,url,title,body,state,mergeCommit";

/// Find the Release PR for a branch, if any.
///
/// Looks at open PRs first, then falls back to any state, so
/// `gh ship release` can still find a PR that has just been merged.
pub fn find_pull_request(gh: &Gh, head: &str, base: &str) -> Result<Option<PullRequest>, GhError> {
    for state in ["open", "all"] {
        let prs: Vec<PullRequest> = gh.json_scoped(&[
            "pr", "list", "--head", head, "--base", base, "--state", state, "--limit", "1",
            "--json", PR_FIELDS,
        ])?;
        if let Some(pr) = prs.into_iter().next() {
            return Ok(Some(pr));
        }
    }
    Ok(None)
}

/// Fetch a PR by number.
pub fn view_pull_request(gh: &Gh, number: u64) -> Result<PullRequest, GhError> {
    gh.json_scoped(&["pr", "view", &number.to_string(), "--json", PR_FIELDS])
}

/// Open a Release PR.
pub fn create_pull_request(
    gh: &Gh,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<String, GhError> {
    let mut args: Vec<String> = vec![
        "pr".into(),
        "create".into(),
        "--head".into(),
        head.into(),
        "--base".into(),
        base.into(),
        "--title".into(),
        title.into(),
        "--body".into(),
        body.into(),
    ];
    for label in labels {
        args.push("--label".into());
        args.push(label.clone());
    }
    let out = gh.run_scoped(&args)?;
    Ok(out.trim().to_string())
}

/// Update an existing Release PR.
pub fn update_pull_request(
    gh: &Gh,
    number: u64,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<(), GhError> {
    let mut args: Vec<String> = vec![
        "pr".into(),
        "edit".into(),
        number.to_string(),
        "--title".into(),
        title.into(),
        "--body".into(),
        body.into(),
    ];
    for label in labels {
        args.push("--add-label".into());
        args.push(label.clone());
    }
    gh.run_scoped(&args).map(|_| ())
}

/// Reopen a closed Release PR.
///
/// Reopening keeps the PR number, its comments and its review state, which is
/// the whole point of reusing one: a release under review should not lose that
/// review because the branch moved.
pub fn reopen_pull_request(gh: &Gh, number: u64) -> Result<(), GhError> {
    gh.run_scoped(&["pr", "reopen", &number.to_string()])
        .map(|_| ())
}

/// Close a Release PR without merging it.
pub fn close_pull_request(gh: &Gh, number: u64) -> Result<(), GhError> {
    gh.run_scoped(&["pr", "close", &number.to_string()])
        .map(|_| ())
}

/// Merge a Release PR.
pub fn merge_pull_request(gh: &Gh, number: u64) -> Result<(), GhError> {
    gh.run_scoped(&["pr", "merge", &number.to_string(), "--merge"])
        .map(|_| ())
}

// --- Releases ------------------------------------------------------------

/// Everything `gh release create` needs.
///
/// A struct rather than a parameter list: these are five values of three types,
/// and at the call site `create_release(gh, tag, target, name, notes, true,
/// false, None)` says nothing about which flag is which.
pub struct NewRelease<'a> {
    pub tag: &'a str,
    pub target: &'a str,
    pub name: &'a str,
    pub notes: &'a str,
    pub draft: bool,
    pub prerelease: bool,
    /// `None` when the artifact did not say, in which case the flag is
    /// omitted and GitHub applies its own rule — which is not the same as
    /// asking for `--latest=true`.
    pub make_latest: Option<bool>,
}

/// Create a GitHub Release.
pub fn create_release(gh: &Gh, release: &NewRelease<'_>) -> Result<String, GhError> {
    let mut args: Vec<String> = vec![
        "release".into(),
        "create".into(),
        release.tag.into(),
        "--target".into(),
        release.target.into(),
        "--title".into(),
        release.name.into(),
        "--notes".into(),
        release.notes.into(),
    ];
    if release.draft {
        args.push("--draft".into());
    }
    if release.prerelease {
        args.push("--prerelease".into());
    }
    if let Some(latest) = release.make_latest {
        args.push(format!("--latest={latest}"));
    }
    let out = gh.run_scoped(&args)?;
    Ok(out.trim().to_string())
}

/// Publish a draft release.
pub fn publish_release(gh: &Gh, tag: &str) -> Result<(), GhError> {
    gh.run_scoped(&["release", "edit", tag, "--draft=false"])
        .map(|_| ())
}

/// Whether a release already exists for a tag.
pub fn release_exists(gh: &Gh, tag: &str) -> Result<bool, GhError> {
    match gh.run_scoped(&["release", "view", tag, "--json", "tagName"]) {
        Ok(_) => Ok(true),
        Err(GhError::Failed { stderr, .. })
            if stderr.contains("release not found") || stderr.contains("404") =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub returns `MERGED`, `gh` echoes `Merged`, and `status`
    /// re-exposes whatever it was handed — so the comparison must not
    /// care.
    #[test]
    fn pull_request_state_is_read_case_insensitively() {
        for s in ["MERGED", "merged", "Merged"] {
            assert!(state::is_merged(s), "{s}");
            assert!(!state::is_open(s), "{s}");
            assert!(!state::is_closed(s), "{s}");
        }
        assert!(state::is_open("OPEN"));
        assert!(state::is_closed("CLOSED"));
        assert!(!state::is_merged("CLOSED"));
    }

    #[test]
    fn repository_json_deserialises() {
        let json = r#"{
            "nameWithOwner": "noirbizarre/gh-ship",
            "defaultBranchRef": {"name": "main"},
            "url": "https://github.com/noirbizarre/gh-ship"
        }"#;
        let r: Repository = serde_json::from_str(json).unwrap();
        assert_eq!(r.name_with_owner, "noirbizarre/gh-ship");
        assert_eq!(r.default_branch.name, "main");
    }

    #[test]
    fn pull_request_state_predicates() {
        let open: PullRequest =
            serde_json::from_str(r#"{"number":1,"state":"OPEN","url":"u","title":"t","body":"b"}"#)
                .unwrap();
        assert!(open.is_open());
        assert!(!open.is_merged());
        assert_eq!(open.merged_sha(), None);

        let merged: PullRequest = serde_json::from_str(
            r#"{"number":1,"state":"MERGED","url":"u","title":"t","body":"b",
                "mergeCommit":{"oid":"abc123"}}"#,
        )
        .unwrap();
        assert!(merged.is_merged());
        assert_eq!(
            merged.merged_sha(),
            Some("abc123"),
            "the merge commit is the only trustworthy SHA after a squash merge"
        );
    }

    #[test]
    fn pull_request_json_tolerates_missing_fields() {
        // `mergeCommit` is null on an open PR.
        let pr: PullRequest =
            serde_json::from_str(r#"{"number":7,"state":"OPEN","mergeCommit":null}"#).unwrap();
        assert_eq!(pr.number, 7);
        assert!(pr.merged_sha().is_none());
        assert_eq!(pr.body, "", "a PR with no body must not fail to parse");
    }

    #[test]
    fn state_matching_is_case_insensitive() {
        // `gh` returns uppercase states; a future version returning
        // lowercase must not silently break merge detection.
        for s in ["MERGED", "merged", "Merged"] {
            let pr: PullRequest =
                serde_json::from_str(&format!(r#"{{"number":1,"state":"{s}"}}"#)).unwrap();
            assert!(pr.is_merged(), "state `{s}` should count as merged");
        }
    }
}
