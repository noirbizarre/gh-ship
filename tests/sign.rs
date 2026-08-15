//! Integration tests for `gh ship sign`.
//!
//! The stub cannot prove a signature — only GitHub can, and only for a
//! bot. What it *can* pin down is the decision the command makes around
//! that answer, which is where the damage would be: never rewriting a
//! commit that is already signed, and never moving a branch onto a
//! rewrite that came back unsigned.

mod common;

use common::{MINIMAL_CONFIG, Repo, stub::GhStub};

/// The happy path: an unsigned tip is re-created and the branch moved.
#[test]
fn signs_the_tip_and_moves_the_branch() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).in_ci("ship/prepare-abc123");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 0, "sign should succeed: {}", out.diagnostics());
    assert!(
        repo.stub.called_with(&["api", "git/commits", "POST"]),
        "the commit must be re-created through the API: {:?}",
        repo.stub.calls()
    );
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/ship/prepare-abc123", "PATCH"]),
        "the branch must be moved onto the signed commit: {:?}",
        repo.stub.calls()
    );
}

/// Re-creating a commit somebody signed with their own key would replace
/// a signature they chose with one they did not.
#[test]
fn an_already_signed_commit_is_left_alone() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new().head_already_signed()).in_ci("release/next");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 0, "a no-op must succeed: {}", out.diagnostics());
    assert!(
        !repo.stub.called_with(&["api", "git/commits", "POST"]),
        "an already-signed commit must not be re-created: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "an already-signed commit must not move its branch: {:?}",
        repo.stub.calls()
    );
    assert!(
        out.diagnostics().contains("already signed"),
        "the no-op must say why: {}",
        out.diagnostics()
    );
}

/// GitHub signs only for a bot. Under any other token the re-created
/// commit comes back unsigned, and moving the branch onto it would
/// change the author for no benefit at all.
#[test]
fn an_unsigned_result_aborts_without_moving_the_branch() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new().cannot_sign()).in_ci("release/next");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 1, "sign must fail: {}", out.diagnostics());
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "the branch must be left untouched: {:?}",
        repo.stub.calls()
    );
    let diagnostics = out.diagnostics();
    assert!(
        diagnostics.contains("GITHUB_TOKEN") && diagnostics.contains("App"),
        "the diagnostic must name the token requirement: {diagnostics}"
    );
}

/// In a workflow the ref comes from the environment, so the common case
/// needs no argument at all.
#[test]
fn the_branch_defaults_to_the_dispatched_ref() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).in_ci("ship/prepare-deadbeef");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 0, "sign should succeed: {}", out.diagnostics());
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/ship/prepare-deadbeef", "PATCH"]),
        "GITHUB_REF_NAME must select the branch: {:?}",
        repo.stub.calls()
    );
}

/// An explicit branch wins over the environment, for the workflow that
/// pushes somewhere other than the ref it was dispatched on.
#[test]
fn an_explicit_branch_overrides_the_environment() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).in_ci("ship/prepare-abc123");

    let out = repo.ship(&["sign", "release/next"]);

    assert_eq!(out.code, 0, "sign should succeed: {}", out.diagnostics());
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/release/next", "PATCH"]),
        "the argument must win: {:?}",
        repo.stub.calls()
    );
}

/// A `pull_request` event's `GITHUB_REF` is `refs/pull/N/merge`, which
/// names no branch. Guessing the PR's target would sign the wrong thing.
#[test]
fn a_ref_that_names_no_branch_is_refused() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).in_pr("main", "feature");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 1, "sign must fail: {}", out.diagnostics());
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "nothing may be moved when the branch is unknown: {:?}",
        repo.stub.calls()
    );
    assert!(
        out.diagnostics().contains("gh ship sign <branch>"),
        "the diagnostic must say how to recover: {}",
        out.diagnostics()
    );
}

/// Outside CI the checkout is the only thing left to read.
#[test]
fn the_branch_falls_back_to_the_checkout() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).with_git_head("release/next");

    let out = repo.ship(&["sign"]);

    assert_eq!(out.code, 0, "sign should succeed: {}", out.diagnostics());
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/release/next", "PATCH"]),
        ".git/HEAD must select the branch: {:?}",
        repo.stub.calls()
    );
}

/// `gh api` takes no `--repo`, so the repository must be spelled into
/// the URL. The stub exits non-zero if the flag ever appears.
#[test]
fn signing_works_under_an_explicit_repo() {
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).in_ci("release/next");

    let out = repo.ship(&["--repo", "acme/widgets", "sign"]);

    assert_eq!(
        out.code,
        0,
        "sign should succeed under --repo: {}",
        out.diagnostics()
    );
    assert!(
        repo.stub.called_with(&["api", "git/commits", "POST"]),
        "the commit must still be re-created: {:?}",
        repo.stub.calls()
    );
}
