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
      ship_id: { required: true, type: string }
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

/// Without the nonce in `run-name`, gh-ship cannot correlate a dispatch
/// to a run — so this must fail at setup time, not at release time.
#[test]
fn rejects_a_workflow_without_run_name_correlation() {
    let no_run_name = r#"name: prepare-release
on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: [{ run: echo hi }]
"#;
    let dir = repo(
        Some(MINIMAL_CONFIG),
        &[("prepare-release.yml", no_run_name)],
    );
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());

    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("run-name"), "{stderr}");
    assert!(
        stderr.contains("no run id"),
        "the help must explain *why* run-name is required: {stderr}"
    );
}

#[test]
fn rejects_a_workflow_missing_the_ship_id_input() {
    let no_input = r#"name: prepare-release
run-name: prepare-release (ship:${{ inputs.ship_id }})
on:
  workflow_dispatch:
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: [{ run: echo hi }]
"#;
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", no_input)]);
    let out = validate_in(dir.path());
    let (code, stderr) = (out.code, out.diagnostics());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("ship_id"), "{stderr}");
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

/// The templates `init` writes, and the workflows this repository
/// actually uses, must both stay conforming.
#[test]
fn shipped_templates_are_conforming_workflows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, role) in [
        ("prepare-release.yml", "prepare"),
        ("publish-release.yml", "publish"),
    ] {
        let body = std::fs::read_to_string(root.join("templates").join(name))
            .unwrap_or_else(|e| panic!("templates/{name} must exist: {e}"));

        // Each template is wired into the role it exists for. The publish
        // role additionally needs a prepare workflow to be a valid setup,
        // so the conforming fixture stands in for it.
        let dir = match role {
            "prepare" => repo(
                Some("version: 1\nworkflows:\n  prepare: prepare-release\n"),
                &[("prepare-release.yml", &body)],
            ),
            _ => repo(
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
        assert_eq!(code, 0, "templates/{name} is not conforming:\n{stderr}");
    }
}

// --- slug identification --------------------------------------------------

/// A workflow with an emoji display name must still be addressable from
/// the config by its filename slug. Requiring `🚢 Prepare Release` in
/// YAML would be miserable, and renaming a workflow would break releases.
const EMOJI_WORKFLOW: &str = r#"name: 🚢 Prepare Release
run-name: 🚢 Prepare Release (ship:${{ inputs.ship_id }})
on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
      dry_run: { required: false, type: boolean, default: false }
  workflow_call:
    inputs:
      ship_id: { required: true, type: string }
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
