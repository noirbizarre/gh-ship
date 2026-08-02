//! Lifecycle tests: dispatch, correlation, artifact retrieval, rendering.
//!
//! These drive the real binary against a stubbed `gh`, so they exercise
//! the full path — argument construction, JSON decoding, polling, error
//! classification — without a network or credentials.
//!
//! The correlation tests matter most. `gh workflow run` returns no run
//! id, so gh-ship finds its run by a nonce it stamps into the dispatch
//! and the workflow echoes into its `run-name`. Everything downstream
//! depends on that being right.

mod common;

use common::{GhStub, Repo, with_redactions};

use common::MINIMAL_CONFIG as CONFIG;

const CHANGED_ARTIFACT: &str = r###"{"schemaVersion":1,"changed":true,"version":"1.4.0","tag":"v1.4.0","release":{"notes":"## Changes\n\n* Everything"}}"###;

// --- Happy path ----------------------------------------------------------

#[test]
fn preview_renders_the_pull_request_without_mutating_anything() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("## Changes"), "{}", out.stdout);
    assert!(out.stderr.contains("Release 1.4.0"), "{}", out.stderr);

    // The whole point of preview: nothing is created.
    assert!(
        !repo.stub.called_with(&["pr create"]),
        "preview must not open a PR: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["release create"]),
        "preview must not create a release"
    );
    assert!(
        !repo.stub.called_with(&["git/refs"]),
        "preview must not create a branch"
    );
}

#[test]
fn preview_dispatches_with_dry_run() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    repo.ship(&["preview"]);
    assert!(
        repo.stub.called_with(&["workflow run", "dry_run=true"]),
        "preview must tell the workflow not to commit: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn preview_passes_a_correlation_nonce() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    repo.ship(&["preview"]);
    assert!(
        repo.stub.called_with(&["workflow run", "ship_id="]),
        "every dispatch must carry a nonce: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn preview_reports_nothing_to_release_and_exits_zero() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().artifact(r#"{"schemaVersion":1,"changed":false}"#),
    );
    let out = repo.ship(&["preview"]);

    assert_eq!(
        out.code, 0,
        "nothing to release is a success: {}",
        out.stderr
    );
    assert!(out.stderr.contains("nothing to release"), "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["pr create"]),
        "an unchanged release must not open a PR"
    );
}

#[test]
fn preview_json_is_machine_readable() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["preview", "--json"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout must be valid JSON ({e}): {}", out.stdout));
    assert_eq!(v["artifact"]["version"], "1.4.0");
    assert_eq!(v["pull_request"]["title"], "Release 1.4.0");
    assert!(
        v["pull_request"]["body"]
            .as_str()
            .unwrap()
            .contains("## Changes")
    );
}

// --- Correlation failures ------------------------------------------------

/// The failure this whole design exists to prevent: a run that cannot be
/// found. The message must name the cause, not just report a timeout.
#[test]
fn a_run_that_never_appears_explains_the_run_name_contract() {
    let repo = Repo::new(CONFIG, GhStub::new().run_never_appears());
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("run-name"), "{}", out.stderr);
    assert!(
        out.stderr.contains("no run id"),
        "the help must explain why correlation needs run-name: {}",
        out.stderr
    );
}

#[test]
fn a_failed_run_reports_the_conclusion_and_a_link() {
    let repo = Repo::new(CONFIG, GhStub::new().run_status("completed", "failure"));
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("failure"), "{}", out.stderr);
    assert!(
        out.stderr.contains("actions/runs/42"),
        "a failure must link to the run: {}",
        out.stderr
    );
}

#[test]
fn a_cancelled_run_is_treated_as_a_failure() {
    let repo = Repo::new(CONFIG, GhStub::new().run_status("completed", "cancelled"));
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("cancelled"), "{}", out.stderr);
}

// --- Protocol failures ---------------------------------------------------

#[test]
fn a_run_without_an_artifact_names_the_protocol() {
    let repo = Repo::new(CONFIG, GhStub::new().no_artifact());
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("ship-release"), "{}", out.stderr);
    // miette hard-wraps, so match on a fragment that survives wrapping.
    assert!(
        out.stderr.contains("specifications/release-"),
        "the help must link to the specification: {}",
        out.stderr
    );
}

#[test]
fn an_artifact_with_the_wrong_filename_is_rejected() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .artifact_wrong_filename("release.json"),
    );
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("ship.release.json"), "{}", out.stderr);
    assert!(
        out.stderr.contains("part of the protocol"),
        "{}",
        out.stderr
    );
}

/// An invalid artifact must produce the same rich diagnostic as
/// `gh ship validate`, not a generic orchestration error.
#[test]
fn an_invalid_artifact_gets_the_full_diagnostic() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().artifact(r#"{"schemaVersion":1,"changed":true}"#),
    );
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("version"), "{}", out.stderr);
    assert!(out.stderr.contains("tag"), "{}", out.stderr);
    assert!(
        out.stderr.contains("changed:") && out.stderr.contains("false`"),
        "the contextual help must survive into lifecycle commands: {}",
        out.stderr
    );
}

// --- Environment failures ------------------------------------------------

#[test]
fn unauthenticated_gh_is_reported_actionably() {
    let repo = Repo::new(CONFIG, GhStub::new().unauthenticated());
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("gh auth login"), "{}", out.stderr);
}

// --- Transient API failures ----------------------------------------------

/// A real release run died on a single 504 from GitHub's GraphQL endpoint,
/// which was answering normally again seconds later. A blip on a read must
/// cost a retry, not the release.
#[test]
fn a_transient_failure_on_a_read_is_absorbed() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_exists(false)
            .artifact(CHANGED_ARTIFACT)
            .flaky("pr list", 2),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["pr create"]),
        "the release must proceed: {:?}",
        repo.stub.calls()
    );
    // The retry must be invisible: no half-failure leaking into the output.
    assert!(!out.stderr.contains("504"), "{}", out.stderr);
}

/// An outage is not a blip. Once the budget is spent the error surfaces,
/// and it has to say it was already retried — otherwise the reader's first
/// instinct is to do by hand what we just did four times.
#[test]
fn a_sustained_outage_gives_up_and_says_it_retried() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_exists(false)
            .artifact(CHANGED_ARTIFACT)
            .flaky("pr list", 99),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("504"), "{}", out.stderr);
    assert!(out.stderr.contains("retried"), "{}", out.stderr);

    let attempts = repo
        .stub
        .calls()
        .iter()
        .filter(|c| c.starts_with("pr list"))
        .count();
    assert_eq!(attempts, 4, "3 retries on top of the first attempt");
}

/// The reason retries are gated on reads: a 504 means GitHub stopped
/// answering, not that it stopped acting, so a retried `pr create` could
/// open two Release PRs.
#[test]
fn a_transient_failure_on_a_write_is_not_retried() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_exists(false)
            .artifact(CHANGED_ARTIFACT)
            .flaky("pr create", 1),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 1, "{}", out.stderr);

    let attempts = repo
        .stub
        .calls()
        .iter()
        .filter(|c| c.starts_with("pr create"))
        .count();
    assert_eq!(attempts, 1, "a write must be attempted exactly once");
}

#[test]
fn a_missing_config_suggests_init() {
    let repo = Repo::new(CONFIG, GhStub::new());
    std::fs::remove_file(repo.path().join(".github/ship.yml")).unwrap();
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("gh ship init"), "{}", out.stderr);
}

// --- Rendering -----------------------------------------------------------

#[test]
fn header_and_footer_wrap_the_workflow_notes() {
    let config = r#"
version: 1
workflows:
  prepare: prepare-release
pull_request:
  title: "Ship {{ tag }}"
  header: |
    Heads up.
  footer: |
    Bye.
"#;
    let repo = Repo::new(config, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["preview", "--json"]);

    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["pull_request"]["title"], "Ship v1.4.0");
    assert_eq!(
        v["pull_request"]["body"],
        "Heads up.\n\n## Changes\n\n* Everything\n\nBye."
    );
}

#[test]
fn preview_output_is_stable() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["preview"]);
    with_redactions(|| insta::assert_snapshot!("preview__changed", out.stderr));
}

#[test]
fn nothing_to_release_output_is_stable() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().artifact(r#"{"schemaVersion":1,"changed":false}"#),
    );
    let out = repo.ship(&["preview"]);
    with_redactions(|| insta::assert_snapshot!("preview__unchanged", out.stderr));
}

// --- prepare -------------------------------------------------------------

#[test]
fn prepare_creates_the_release_branch_when_missing() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .branch_exists(false)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["api", "git/refs", "POST"]),
        "the branch must exist before dispatch, because workflow_dispatch \
         reads the workflow from that ref: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn a_missing_release_branch_is_created_at_the_staged_commit() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .branch_exists(false)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub
            .called_with(&["git/refs", "POST", "refs/heads/release/next"]),
        "{:?}",
        repo.stub.calls()
    );
}

/// A merged-but-unpublished release must be protected before anything is
/// staged or promoted.
#[test]
fn the_in_flight_release_guard_short_circuits_before_staging() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234")
            .release_exists(false)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "the guard must short-circuit before the branch is touched: {:?}",
        repo.stub.calls()
    );
}

/// Preview must not dispatch on the release branch: `prepare` stages on a
/// throwaway branch cut from base, so base is what a real prepare runs
/// against. Using a stale release branch would make preview report history
/// that no longer matches reality.
#[test]
fn preview_dispatches_on_the_base_branch() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().branch_exists(true).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["workflow run", "--ref main"]),
        "preview should run against the base: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "preview must still mutate nothing: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn prepare_opens_a_pull_request() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(false).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["pr create"]),
        "{:?}",
        repo.stub.calls()
    );
    assert!(out.stderr.contains("Release PR opened"), "{}", out.stderr);
}

#[test]
fn prepare_updates_an_existing_pull_request() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(true).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["pr edit"]),
        "{:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["pr create"]),
        "re-running prepare must not open a second PR"
    );
}

/// The artifact must survive into the PR body: it is the only reason
/// `gh ship release` can work later with zero local state.
#[test]
fn prepare_embeds_the_artifact_in_the_pull_request_body() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(false).artifact(CHANGED_ARTIFACT),
    );
    repo.ship(&["prepare"]);

    let create = repo
        .stub
        .calls()
        .into_iter()
        .find(|c| c.starts_with("pr create"))
        .expect("pr create was called");

    assert!(create.contains("ship:artifact"), "{create}");
    assert!(create.contains("\"version\":\"1.4.0\""), "{create}");
}

#[test]
fn prepare_reports_nothing_to_release_without_opening_a_pr() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().artifact(r#"{"schemaVersion":1,"changed":false}"#),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("nothing to release"), "{}", out.stderr);
    assert!(!repo.stub.called_with(&["pr create"]));
    assert!(!repo.stub.called_with(&["pr edit"]));
}

#[test]
fn prepare_does_not_pass_dry_run() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    repo.ship(&["prepare"]);
    assert!(
        !repo.stub.called_with(&["workflow run", "dry_run=true"]),
        "a real prepare must let the workflow commit: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn prepare_no_wait_dispatches_and_stops() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["prepare", "--no-wait"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(repo.stub.called_with(&["workflow run"]));
    assert!(
        !repo.stub.called_with(&["run download"]),
        "--no-wait must not wait for the artifact"
    );
    assert!(!repo.stub.called_with(&["pr create"]));
}

#[test]
fn prepare_output_is_stable() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(false).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);
    with_redactions(|| insta::assert_snapshot!("prepare__opens_pr", out.stderr));
}

// --- status --------------------------------------------------------------

#[test]
fn status_reports_a_fresh_repository() {
    let repo = Repo::new(CONFIG, GhStub::new().branch_exists(false).pr_exists(false));
    let out = repo.ship(&["status"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("does not exist"), "{}", out.stderr);
    assert!(out.stderr.contains("gh ship prepare"), "{}", out.stderr);
}

#[test]
fn status_mutates_nothing() {
    let repo = Repo::new(CONFIG, GhStub::new().pr_exists(true));
    repo.ship(&["status"]);

    for forbidden in [
        "workflow run",
        "pr create",
        "pr edit",
        "pr merge",
        "release create",
    ] {
        assert!(
            !repo.stub.called_with(&[forbidden]),
            "status is a pure query but called `{forbidden}`: {:?}",
            repo.stub.calls()
        );
    }
}

/// Status reads the artifact back out of the PR body — the round trip
/// that makes gh-ship stateless.
#[test]
fn status_recovers_the_artifact_from_the_pull_request() {
    let body = format!("Release notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(CONFIG, GhStub::new().pr_body(&body));
    let out = repo.ship(&["status"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("1.4.0"), "{}", out.stderr);
    assert!(out.stderr.contains("v1.4.0"), "{}", out.stderr);
}

#[test]
fn status_tells_a_merged_pr_to_release() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234def"),
    );
    let out = repo.ship(&["status"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("gh ship release"), "{}", out.stderr);
    assert!(
        out.stderr.contains("abc1234"),
        "the merge SHA should be shown: {}",
        out.stderr
    );
}

#[test]
fn status_json_is_machine_readable() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(CONFIG, GhStub::new().pr_body(&body));
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON ({e}): {}", out.stdout));
    assert_eq!(v["repository"], "acme/widgets");
    assert_eq!(v["release_branch"], "release/next");
    assert_eq!(v["artifact"]["version"], "1.4.0");
    assert_eq!(v["pull_request"]["number"], 7);
    assert!(v["next"].as_str().unwrap().contains("merge"));
}

// --- release -------------------------------------------------------------

/// A merged Release PR carrying the artifact gh-ship embedded earlier.
fn merged_repo(stub: GhStub) -> Repo {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    Repo::new(
        CONFIG,
        stub.pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234def5678"),
    )
}

#[test]
fn release_tags_the_merge_commit_not_the_branch_tip() {
    let repo = merged_repo(GhStub::new());
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub
            .called_with(&["release create", "v1.4.0", "--target abc1234def5678"]),
        "a squash merge creates a NEW commit, so only mergeCommit.oid is safe: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn release_creates_a_draft_first() {
    let repo = merged_repo(GhStub::new());
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["release create", "--draft"]),
        "the release must start as a draft so assets can be attached \
         before watchers are notified: {:?}",
        repo.stub.calls()
    );
    assert!(repo.stub.called_with(&["release edit", "--draft=false"]));
}

/// Ordering is the whole point of draft-first: upload, then publish.
#[test]
fn release_undrafts_only_after_the_publish_workflow() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\n  publish: prepare-release\n";
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        config,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234def5678"),
    );
    let out = repo.ship(&["release"]);
    assert_eq!(out.code, 0, "{}", out.stderr);

    let calls = repo.stub.calls();
    let dispatch = calls.iter().position(|c| c.starts_with("workflow run"));
    let undraft = calls.iter().position(|c| c.contains("--draft=false"));

    assert!(
        dispatch.is_some(),
        "the publish workflow must run: {calls:?}"
    );
    assert!(
        undraft.is_some(),
        "the release must be published: {calls:?}"
    );
    assert!(
        dispatch < undraft,
        "assets must be uploaded before the release becomes visible: {calls:?}"
    );
}

#[test]
fn release_refuses_an_open_pull_request() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(CONFIG, GhStub::new().pr_body(&body).pr_state("OPEN"));
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("still open"), "{}", out.stderr);
    assert!(out.stderr.contains("--merge"), "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["release create"]),
        "nothing may be released before the PR is merged"
    );
}

#[test]
fn release_can_merge_when_asked() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("OPEN")
            .merge_commit("abc1234def5678"),
    );
    let out = repo.ship(&["release", "--merge"]);

    assert!(
        repo.stub.called_with(&["pr merge"]),
        "{:?}",
        repo.stub.calls()
    );
    assert!(out.stderr.contains("merged"), "{}", out.stderr);

    // `--merge` is only useful if the release then proceeds, so assert the
    // outcome and not merely that `pr merge` was called.
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["release create"]),
        "{:?}",
        repo.stub.calls()
    );
}

#[test]
fn release_refuses_a_closed_pull_request() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(CONFIG, GhStub::new().pr_body(&body).pr_state("CLOSED"));
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("closed without merging"),
        "{}",
        out.stderr
    );
}

/// The zero-state design fails safe: no embedded artifact means gh-ship
/// says so, rather than guessing a version.
#[test]
fn release_without_an_embedded_artifact_explains_the_mechanism() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_body("Just some notes.").pr_state("MERGED"),
    );
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("does not carry a release artifact"),
        "{}",
        out.stderr
    );
    // miette hard-wraps help text, so match a fragment that survives it.
    assert!(out.stderr.contains("HTML"), "{}", out.stderr);
}

#[test]
fn release_without_a_pull_request_points_at_status() {
    let repo = Repo::new(CONFIG, GhStub::new().pr_exists(false));
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no Release PR found"), "{}", out.stderr);
    assert!(out.stderr.contains("gh ship prepare"), "{}", out.stderr);
}

#[test]
fn release_is_idempotent_when_the_release_already_exists() {
    let repo = merged_repo(GhStub::new().release_exists(true));
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("already exists"), "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["release create"]),
        "re-running release must not create a second release"
    );
}

/// `make_latest` is part of the published v1 artifact schema and exists so a
/// patch on an old branch, or a prerelease, need not become "Latest".
#[test]
fn release_honours_make_latest_from_the_artifact() {
    let artifact = r###"{"schemaVersion":1,"changed":true,"version":"1.4.0","tag":"v1.4.0","release":{"make_latest":false}}"###;
    let body = format!("Notes\n\n<!-- ship:artifact\n{artifact}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234def5678"),
    );
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["release create", "--latest=false"]),
        "an artifact asking not to be latest must say so on the wire: {:?}",
        repo.stub.calls()
    );
}

/// Silence must stay silence: without `make_latest`, GitHub applies its own
/// rule rather than gh-ship forcing one.
#[test]
fn release_omits_latest_when_the_artifact_is_silent() {
    let repo = merged_repo(GhStub::new());
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["release create", "--latest"]),
        "an unstated `make_latest` must not become a flag: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn release_carries_the_notes_from_the_artifact() {
    let repo = merged_repo(GhStub::new());
    repo.ship(&["release"]);

    let create = repo
        .stub
        .calls()
        .into_iter()
        .find(|c| c.starts_with("release create"))
        .expect("release create was called");
    assert!(create.contains("## Changes"), "{create}");
}

#[test]
fn release_output_is_stable() {
    let repo = merged_repo(GhStub::new());
    let out = repo.ship(&["release"]);
    with_redactions(|| insta::assert_snapshot!("release__ships", out.stderr));
}

// --- labels --------------------------------------------------------------

const LABELLED_CONFIG: &str = r#"
version: 1
workflows:
  prepare: prepare-release
pull_request:
  labels: [release]
"#;

/// The regression that cost a real Release PR: `gh pr create --label release`
/// fails outright when the label does not exist, taking the PR with it.
/// gh-ship must create the label first.
#[test]
fn a_missing_label_is_created_before_the_pr() {
    let repo = Repo::new(
        LABELLED_CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .labels(&[])
            .pr_create_rejects_unknown_labels(),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["label create", "release"]),
        "the missing label must be created: {:?}",
        repo.stub.calls()
    );
    assert!(repo.stub.called_with(&["pr create", "--label release"]));
}

#[test]
fn an_existing_label_is_not_recreated() {
    let repo = Repo::new(
        LABELLED_CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .labels(&["release"]),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !repo.stub.called_with(&["label create"]),
        "an existing label must not be recreated: {:?}",
        repo.stub.calls()
    );
}

/// A label is decoration; the Release PR is the valuable artifact. If the
/// label cannot be created, the PR must still be opened.
#[test]
fn the_pr_is_still_created_when_a_label_cannot_be() {
    let repo = Repo::new(
        LABELLED_CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .labels(&[])
            .label_create_fails()
            .pr_create_rejects_unknown_labels(),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(
        out.code, 0,
        "a label must never cost the Release PR:\n{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("could not be created"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("issues: write"), "{}", out.stderr);
    assert!(repo.stub.called_with(&["pr create"]));
    assert!(
        !repo.stub.called_with(&["pr create", "--label"]),
        "the unusable label must be dropped, not sent: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn no_configured_labels_means_no_label_calls() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    repo.ship(&["prepare"]);
    assert!(
        !repo.stub.called_with(&["label list"]),
        "nothing to do, so nothing should be asked of GitHub"
    );
}

// --- slug identification -------------------------------------------------

/// `gh` resolves a workflow by filename, name or id — never by stem — so
/// the filename must reach the API even though output shows the slug.
#[test]
fn gh_receives_the_filename_while_output_shows_the_slug() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["prepare"]);

    assert!(
        repo.stub
            .called_with(&["workflow run", "prepare-release.yml"]),
        "the API argument must keep the extension: {:?}",
        repo.stub.calls()
    );
    assert!(
        out.stderr.contains("dispatching prepare-release on"),
        "output should show the slug: {}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("prepare-release.yml"),
        "output should not leak the filename: {}",
        out.stderr
    );
}

// --- pending-release guard ------------------------------------------------

/// Between merging the Release PR and running `gh ship release` the tag does
/// not exist, so a changelog tool still reports the version as unreleased.
/// Preparing again there would start a second release for a version already
/// merged. This is the window a push-triggered prepare lands in on the very
/// push that merges the PR.
#[test]
fn prepare_refuses_while_a_merged_release_is_unpublished() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234")
            .release_exists(false)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(
        out.code, 0,
        "a pending release is an expected state, not a failure — an \
         orchestrator must not go red on every push:\n{}",
        out.stderr
    );
    assert!(out.stderr.contains("not yet published"), "{}", out.stderr);
    assert!(out.stderr.contains("gh ship release"), "{}", out.stderr);

    assert!(
        !repo.stub.called_with(&["workflow run"]),
        "nothing should be dispatched: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["pr create"]) && !repo.stub.called_with(&["pr edit"]),
        "no second Release PR: {:?}",
        repo.stub.calls()
    );
}

/// Once the release exists, that cycle is done and the next may start.
#[test]
fn prepare_proceeds_once_the_merged_release_is_published() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234")
            .release_exists(true)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["workflow run"]),
        "a completed release must not block the next one: {:?}",
        repo.stub.calls()
    );
}

/// Merging the Release PR is also a push to base, so automation prepares on it.
/// Once the release is published and its merge is still the tip, there is
/// provably nothing to release: dispatching the prepare workflow only to be
/// told `changed: false` is noise, not information.
///
/// The comparison is on the merge sha, not the commit message, so it holds for
/// merge commits, squashes and rebases alike.
#[test]
fn prepare_skips_when_the_published_merge_is_still_the_base_tip() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            // What the stub reports as the tip of the base branch.
            .merge_commit("a1b2c3d4e5f6")
            .release_exists(true)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(
        out.code, 0,
        "having just released is an expected state, not a failure:\n{}",
        out.stderr
    );
    assert!(out.stderr.contains("already published"), "{}", out.stderr);

    assert!(
        !repo.stub.called_with(&["workflow run"]),
        "nothing should be dispatched: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["pr create"]) && !repo.stub.called_with(&["pr edit"]),
        "no Release PR churn: {:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["PATCH"]),
        "the guard must short-circuit before any branch is touched: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn prepare_proceeds_while_the_release_pr_is_open() {
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body(&body)
            .pr_state("OPEN")
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(repo.stub.called_with(&["workflow run"]));
    assert!(
        repo.stub.called_with(&["pr edit"]),
        "an open Release PR should be refreshed: {:?}",
        repo.stub.calls()
    );
}

/// A merged PR carrying no artifact cannot be released from, so preparing
/// afresh is the way out rather than a dead end.
#[test]
fn prepare_proceeds_when_a_merged_pr_has_no_artifact() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_body("Someone edited this body away.")
            .pr_state("MERGED")
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["workflow run"]),
        "{:?}",
        repo.stub.calls()
    );
}

#[test]
fn prepare_proceeds_when_there_is_no_release_pr() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(false).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(repo.stub.called_with(&["workflow run"]));
}

// --- tagging --------------------------------------------------------------

/// A draft release does not create its git ref, and the publish workflow is
/// dispatched on that tag and checks it out. Without an explicit tag the very
/// first release fails *after* the release object already exists.
#[test]
fn release_tags_the_merge_commit_before_creating_the_release() {
    let repo = merged_repo(GhStub::new());
    let out = repo.ship(&["release"]);

    assert_eq!(out.code, 0, "{}", out.stderr);

    let calls = repo.stub.calls();
    let tagged = calls
        .iter()
        .position(|c| c.contains("refs/tags/v1.4.0") || c.contains("ref=refs/tags/v1.4.0"));
    let created = calls.iter().position(|c| c.starts_with("release create"));

    assert!(tagged.is_some(), "the tag must be created: {calls:?}");
    assert!(created.is_some(), "the release must be created: {calls:?}");
    assert!(
        tagged < created,
        "the tag must exist before the release, not as a side effect of it: {calls:?}"
    );
}

/// The tag must land on the merge commit. A squash merge creates a new commit,
/// so a SHA remembered from prepare would tag something not on the base branch.
#[test]
fn the_tag_points_at_the_merge_commit() {
    let repo = merged_repo(GhStub::new());
    repo.ship(&["release"]);

    let tag_call = repo
        .stub
        .calls()
        .into_iter()
        .find(|c| c.contains("refs/tags/"))
        .expect("tag was created");
    assert!(
        tag_call.contains("sha=abc1234def5678"),
        "the tag must point at the merge commit: {tag_call}"
    );
}

/// Re-running after a partly completed release must not fail on the tag.
#[test]
fn an_existing_tag_is_not_an_error() {
    let repo = merged_repo(GhStub::new().tag_exists());
    let out = repo.ship(&["release"]);

    assert_eq!(
        out.code, 0,
        "re-running after a partial failure must work:\n{}",
        out.stderr
    );
    assert!(repo.stub.called_with(&["release create"]));
}

/// The publish workflow is dispatched on the tag, which is why the tag has to
/// exist first.
#[test]
fn the_publish_workflow_is_dispatched_on_the_tag() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\n  publish: prepare-release\n";
    let body = format!("Notes\n\n<!-- ship:artifact\n{CHANGED_ARTIFACT}\n-->");
    let repo = Repo::new(
        config,
        GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("abc1234def5678"),
    );
    let out = repo.ship(&["release"]);
    assert_eq!(out.code, 0, "{}", out.stderr);

    let calls = repo.stub.calls();
    let tagged = calls.iter().position(|c| c.contains("refs/tags/"));
    let dispatched = calls.iter().position(|c| c.starts_with("workflow run"));

    assert!(
        repo.stub.called_with(&["workflow run", "--ref v1.4.0"]),
        "publish must be dispatched on the tag: {calls:?}"
    );
    assert!(
        tagged < dispatched,
        "the ref must exist before it is dispatched on: {calls:?}"
    );
}

// --- pull_request.reuse ---------------------------------------------------

const REUSE_OFF: &str = r#"
version: 1
workflows:
  prepare: prepare-release
pull_request:
  reuse: false
"#;

#[test]
fn reuse_updates_an_open_release_pr_in_place() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_state("OPEN").artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["pr edit"]),
        "{:?}",
        repo.stub.calls()
    );
    assert!(!repo.stub.called_with(&["pr create"]));
    assert!(
        !repo.stub.called_with(&["pr reopen"]),
        "it was never closed"
    );
}

/// Three Release PRs were closed by the reset bug. The next prepare must
/// recover the last one rather than open a fourth.
#[test]
fn reuse_reopens_a_closed_release_pr() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_state("CLOSED").artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub.called_with(&["pr reopen"]),
        "a closed Release PR must be reopened, not replaced: {:?}",
        repo.stub.calls()
    );
    assert!(repo.stub.called_with(&["pr edit"]));
    assert!(
        !repo.stub.called_with(&["pr create"]),
        "no second PR: {:?}",
        repo.stub.calls()
    );
    assert!(out.stderr.contains("reopening"), "{}", out.stderr);
}

/// A merged PR belongs to a release that already shipped; reopening it would
/// resurrect it.
#[test]
fn reuse_never_touches_a_merged_release_pr() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .pr_state("MERGED")
            .merge_commit("abc1234")
            .release_exists(true)
            .artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!repo.stub.called_with(&["pr reopen"]));
    assert!(!repo.stub.called_with(&["pr edit"]));
    assert!(
        repo.stub.called_with(&["pr create"]),
        "a shipped release starts a new PR: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn reuse_false_closes_the_open_pr_and_opens_a_new_one() {
    let repo = Repo::new(
        REUSE_OFF,
        GhStub::new().pr_state("OPEN").artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    let calls = repo.stub.calls();
    let closed = calls.iter().position(|c| c.starts_with("pr close"));
    let created = calls.iter().position(|c| c.starts_with("pr create"));

    assert!(closed.is_some(), "the old PR must be retired: {calls:?}");
    assert!(created.is_some(), "{calls:?}");
    assert!(
        closed < created,
        "close before opening its successor: {calls:?}"
    );
    assert!(!repo.stub.called_with(&["pr edit"]));
}

#[test]
fn no_existing_pr_simply_opens_one() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new().pr_exists(false).artifact(CHANGED_ARTIFACT),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(repo.stub.called_with(&["pr create"]));
    assert!(!repo.stub.called_with(&["pr reopen"]));
    assert!(!repo.stub.called_with(&["pr close"]));
}

// --- Staging-branch sweep ------------------------------------------------

/// Abandoned staging branches accumulate forever if the sweep does not run:
/// every failure part-way through a prepare, and every `--no-wait`, leaves
/// one behind.
#[test]
fn prepare_sweeps_abandoned_staging_branches() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .stale_staging_branch("ship/prepare-deadbeef1234"),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/ship/prepare-deadbeef1234", "DELETE"]),
        "the abandoned staging branch must be deleted: {:?}",
        repo.stub.calls()
    );
}

/// The sweep is housekeeping. A branch that cannot be deleted — a protected
/// ref, a missing scope — must never be the reason a release fails.
#[test]
fn a_staging_branch_that_cannot_be_deleted_does_not_fail_the_release() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .stale_staging_branch("ship/prepare-undeletable")
            .branch_delete_fails(),
    );
    let out = repo.ship(&["prepare"]);

    assert_eq!(
        out.code, 0,
        "housekeeping must not fail the release: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("could not delete the staging branch"),
        "the failure must still be reported: {}",
        out.stderr
    );
    assert!(repo.stub.called_with(&["pr create"]));
}

/// `gh api` has no `--repo` flag, so routing an API call through the scoped
/// helper makes it fail — and the sweep swallows its errors, so it fails
/// silently and staging branches pile up unnoticed.
#[test]
fn prepare_works_against_an_explicit_repository() {
    let repo = Repo::new(
        CONFIG,
        GhStub::new()
            .artifact(CHANGED_ARTIFACT)
            .stale_staging_branch("ship/prepare-deadbeef1234"),
    );
    let out = repo.ship(&["--repo", "acme/widgets", "prepare"]);

    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        repo.stub
            .called_with(&["api", "git/refs/heads/ship/prepare-deadbeef1234", "DELETE"]),
        "the sweep must still work under --repo: {:?}",
        repo.stub.calls()
    );
}
