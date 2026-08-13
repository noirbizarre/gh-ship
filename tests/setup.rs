//! Integration tests for `gh ship validate` in setup mode, and for the
//! configuration written by `gh ship init`.
//!
//! These run against real repository layouts in temporary directories.
//! The point is to pin the *guidance*: a broken setup must say what is
//! wrong and what to do, because every one of these mistakes otherwise
//! surfaces mid-release as a timeout.

mod common;

use std::path::Path;

use common::{
    CONFORMING_WORKFLOW as CONFORMING, MINIMAL_CONFIG, Outcome, layout as repo, ship,
    with_redactions,
};
use gh_ship::templates::{self, Role, TokenStrategy};

fn validate_in(dir: &Path) -> Outcome {
    Outcome::run(ship().current_dir(dir).arg("validate"))
}

#[test]
fn accepts_a_conforming_setup() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("workflows satisfy the gh-ship contract"),
        "{stderr}"
    );
}

/// The single most likely setup mistake: naming a "reusable workflow",
/// which the API cannot start at all.
#[test]
fn rejects_a_workflow_call_only_workflow() {
    let call_only = r#"name: prepare-release
on:
  workflow_call:
    inputs:
      dry_run: { required: false, type: boolean, default: false }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: [{ run: echo hi }]
"#;
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", call_only)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());

    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("workflow_dispatch"), "{stderr}");
    assert!(
        stderr.contains("reusable"),
        "the message must connect `workflow_call` to the word users know: {stderr}"
    );
}

/// A plain dispatchable workflow with the one input `preview` needs is all
/// gh-ship asks for: no `run-name`, no correlation input.
#[test]
fn accepts_a_workflow_without_run_name_or_ship_id() {
    let plain = r#"name: prepare-release
on:
  workflow_dispatch:
    inputs:
      dry_run: { required: false, type: boolean, default: false }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: [{ run: echo hi }]
"#;
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", plain)]);
    let out = validate_in(dir.path());
    assert_eq!(out.code, 0, "{}", out.diagnostics());
}

/// A leftover `ship_id` input is dead weight, not a failure: the release
/// must still go out while the user gets around to deleting it.
#[test]
fn reports_but_tolerates_a_leftover_ship_id_input() {
    let legacy = r#"name: prepare-release
run-name: prepare-release (ship:${{ inputs.ship_id }})
on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
      dry_run: { required: false, type: boolean, default: false }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: [{ run: echo hi }]
"#;
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", legacy)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());

    assert_eq!(
        code, 0,
        "a retired input must not fail validation: {stderr}"
    );
    assert!(stderr.contains("ship_id"), "{stderr}");
    assert!(
        stderr.contains("no longer sends it"),
        "the user must be told it is safe to delete: {stderr}"
    );
}

#[test]
fn suggests_a_correction_for_a_misspelled_workflow() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-relase\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("did you mean"), "{stderr}");
    assert!(stderr.contains("prepare-release"), "{stderr}");
}

#[test]
fn reports_a_missing_workflow_with_the_available_ones() {
    let config = "version: 1\nworkflows:\n  prepare: totally-different\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    // miette hard-wraps long messages, so assert on tokens.
    assert!(stderr.contains("totally-different"), "{stderr}");
    assert!(stderr.contains("available workflows"), "{stderr}");
    assert!(stderr.contains("prepare-release"), "{stderr}");
}

#[test]
fn suggests_init_when_there_are_no_workflows_at_all() {
    let dir = repo(Some(MINIMAL_CONFIG), &[]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("gh ship init"), "{stderr}");
}

#[test]
fn rejects_an_unknown_config_key() {
    // `events:` was an early design that was dropped; someone copying an
    // old example must get a clear error rather than silence.
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\nevents:\n  prepare: x\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("unknown field"), "{stderr}");
}

#[test]
fn validates_the_publish_workflow_too() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\n  publish: publish-release\n";
    let broken_publish = "name: publish-release\non:\n  workflow_call:\n";
    let dir = repo(
        Some(config),
        &[
            ("prepare-release.yml", CONFORMING),
            ("publish-release.yml", broken_publish),
        ],
    );
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    // Reported by slug, not filename: that is what the config refers to.
    assert!(stderr.contains("publish-release"), "{stderr}");
    assert!(
        !stderr.contains("publish-release.yml"),
        "output should use the slug, not the filename: {stderr}"
    );
}

#[test]
fn setup_output_is_stable() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let out = validate_in(dir.path());
    with_redactions(|| insta::assert_snapshot!("setup__valid", out.diagnostics()));
}

#[test]
fn call_only_diagnostic_is_stable() {
    let call_only = "name: prepare-release\non:\n  workflow_call:\n";
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", call_only)]);
    let out = validate_in(dir.path());
    with_redactions(|| insta::assert_snapshot!("setup__call_only", out.diagnostics()));
}

// --- init ---------------------------------------------------------------

#[test]
fn init_refuses_to_clobber_an_existing_config() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let out = Outcome::run(ship().current_dir(dir.path()).arg("init"));
    let stderr = out.diagnostics();
    assert_eq!(out.code, 1);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert!(stderr.contains("--force"), "{stderr}");
}

/// `init` writes a config, and `validate` must accept it. If these two
/// ever disagree, setup is broken for every new user.
#[test]
fn init_output_round_trips_through_validate() {
    let dir = repo(None, &[("prepare-release.yml", CONFORMING)]);

    // Write the config `init` would produce, without driving the TUI.
    let config =
        "version: 1\nrelease_branch: release/next\nworkflows:\n  prepare: prepare-release\n";
    std::fs::write(dir.path().join(".github/ship.yml"), config).unwrap();

    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 0, "{stderr}");
}

// --- dogfooding ----------------------------------------------------------

/// gh-ship releases gh-ship. Its own configuration and workflows must
/// satisfy the contract it enforces on everyone else — otherwise the
/// project cannot ship itself, and the docs describe something the
/// authors do not do.
#[test]
fn our_own_setup_satisfies_the_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = validate_in(root);
    let stderr = out.diagnostics();

    assert_eq!(
        out.code, 0,
        "gh-ship must be able to release itself:\n{stderr}"
    );
    assert!(
        stderr.contains("workflows satisfy the gh-ship contract"),
        "{stderr}"
    );
}

/// Every workflow `init` can write must be conforming.
///
/// `init` writes a *rendering*, not the template on disk, and there are
/// six of them: two roles times three token strategies. Checking only one
/// left the other five to be discovered by a user mid-release, which is
/// the failure this whole test file exists to prevent.
#[test]
fn every_generated_workflow_is_conforming() {
    for strategy in [
        TokenStrategy::App,
        TokenStrategy::Pat,
        TokenStrategy::Default,
    ] {
        for role in [Role::Prepare, Role::Publish] {
            let body = templates::render(role, strategy);

            // Each template is wired into the role it exists for. The
            // publish role additionally needs a prepare workflow to be a
            // valid setup, so the conforming fixture stands in for it.
            let dir = match role {
                Role::Prepare => repo(
                    Some("version: 1\nworkflows:\n  prepare: prepare-release\n"),
                    &[("prepare-release.yml", &body)],
                ),
                Role::Publish => repo(
                    Some(
                        "version: 1\nworkflows:\n  prepare: prepare-release\n  publish: publish-release\n",
                    ),
                    &[
                        ("prepare-release.yml", CONFORMING),
                        ("publish-release.yml", &body),
                    ],
                ),
            };

            let out = validate_in(dir.path());
            let (code, stderr) = (out.code, out.diagnostics());
            assert_eq!(
                code,
                0,
                "the {strategy:?} rendering of {} is not conforming:\n{stderr}",
                role.filename()
            );
        }
    }
}

// --- slug identification --------------------------------------------------

/// A workflow with an emoji display name must still be addressable from
/// the config by its filename slug. Requiring `🚢 Prepare Release` in
/// YAML would be miserable, and renaming a workflow would break releases.
const EMOJI_WORKFLOW: &str = r#"name: 🚢 Prepare Release
on:
  workflow_dispatch:
    inputs:
      dry_run: { required: false, type: boolean, default: false }
  workflow_call:
    inputs:
      dry_run: { required: false, type: boolean, default: false }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#;

#[test]
fn a_workflow_with_an_emoji_name_resolves_by_slug() {
    let dir = repo(
        Some(MINIMAL_CONFIG),
        &[("prepare-release.yml", EMOJI_WORKFLOW)],
    );
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());

    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("prepare-release"),
        "the slug should be shown: {stderr}"
    );
    assert!(
        stderr.contains("🚢 Prepare Release"),
        "the display name should still be surfaced alongside it: {stderr}"
    );
}

/// The emoji name must never leak into a suggestion, because a config
/// cannot use it.
#[test]
fn suggestions_use_slugs_not_display_names() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-relase\n";
    let dir = repo(Some(config), &[("prepare-release.yml", EMOJI_WORKFLOW)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());

    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("did you mean"), "{stderr}");
    assert!(stderr.contains("prepare-release"), "{stderr}");
    assert!(
        !stderr.contains("🚢"),
        "a suggestion must be something you can paste into YAML: {stderr}"
    );
}

/// The two release workflows in this repository carry emoji names. Proving
/// they still validate is the dogfooding check for the whole slug change.
#[test]
fn our_own_emoji_named_workflows_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = validate_in(root);
    let stderr = out.diagnostics();

    assert_eq!(out.code, 0, "{stderr}");
    assert!(stderr.contains("🚀 Prepare Release"), "{stderr}");
    assert!(stderr.contains("📦 Publish Release"), "{stderr}");
}

// --- Squash-merge settings ----------------------------------------------

/// The Release PR title is the release commit message, but only if the
/// repository is told to use it. This check is the difference between a
/// Conventional release commit and one carrying the whole changelog plus
/// the embedded artifact JSON.
mod squash {
    use super::*;
    use common::{GhStub, Repo};

    fn validate_with(stub: GhStub) -> Outcome {
        Repo::new(MINIMAL_CONFIG, stub).ship(&["validate"])
    }

    #[test]
    fn approves_the_recommended_settings() {
        let out = validate_with(GhStub::new().squash(true, "PR_TITLE", "BLANK"));
        let stderr = out.diagnostics();
        assert_eq!(out.code, 0, "{stderr}");
        assert!(
            stderr.contains("squash merges produce a clean release commit"),
            "{stderr}"
        );
    }

    /// GitHub's own defaults. The warning must name the fix, not just
    /// the problem — nobody remembers these API field names.
    #[test]
    fn warns_about_githubs_defaults_and_gives_the_fix() {
        let out =
            validate_with(GhStub::new().squash(true, "COMMIT_OR_PR_TITLE", "COMMIT_MESSAGES"));
        let stderr = out.diagnostics();

        assert_eq!(
            out.code, 0,
            "a bad setting is advice, not a failure: {stderr}"
        );
        assert!(stderr.contains("squashes the Release PR badly"), "{stderr}");
        assert!(stderr.contains("pull_request.title"), "{stderr}");
        assert!(
            stderr.contains("gh api -X PATCH repos/acme/widgets"),
            "the fix must be pasteable: {stderr}"
        );
        assert!(
            stderr.contains("squash_merge_commit_title=PR_TITLE"),
            "{stderr}"
        );
        assert!(
            stderr.contains("squash_merge_commit_message=BLANK"),
            "{stderr}"
        );
    }

    /// `PR_BODY` is the one that puts the `<!-- ship:artifact -->` blob
    /// into git history.
    #[test]
    fn warns_when_the_commit_body_would_copy_the_pr_body() {
        let out = validate_with(GhStub::new().squash(true, "PR_TITLE", "PR_BODY"));
        let stderr = out.diagnostics();
        assert!(stderr.contains("artifact into git history"), "{stderr}");
    }

    #[test]
    fn says_nothing_when_the_repository_does_not_squash() {
        let out = validate_with(GhStub::new().squash(false, "COMMIT_OR_PR_TITLE", "PR_BODY"));
        let stderr = out.diagnostics();
        assert_eq!(out.code, 0, "{stderr}");
        assert!(!stderr.contains("squash"), "{stderr}");
    }
}
