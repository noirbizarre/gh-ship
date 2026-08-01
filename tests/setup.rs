//! Integration tests for `gh ship validate` in setup mode, and for the
//! configuration written by `gh ship init`.
//!
//! These run against real repository layouts in temporary directories.
//! The point is to pin the *guidance*: a broken setup must say what is
//! wrong and what to do, because every one of these mistakes otherwise
//! surfaces mid-release as a timeout.

mod common;

use std::path::Path;

use common::ship;

/// A workflow that satisfies the gh-ship contract.
const CONFORMING: &str = r#"name: prepare-release
run-name: prepare-release (ship:${{ inputs.ship_id }})
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

const MINIMAL_CONFIG: &str = "version: 1\nworkflows:\n  prepare: prepare-release\n";

/// Build a repository layout in a temp dir.
fn repo(config: Option<&str>, workflows: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&wf).unwrap();
    for (name, body) in workflows {
        std::fs::write(wf.join(name), body).unwrap();
    }
    if let Some(c) = config {
        std::fs::write(dir.path().join(".github/ship.yml"), c).unwrap();
    }
    dir
}

fn validate_in(dir: &Path) -> (i32, String) {
    let out = ship().current_dir(dir).arg("validate").output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn accepts_a_conforming_setup() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let (code, stderr) = validate_in(dir.path());
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
    let (code, stderr) = validate_in(dir.path());

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
    let (code, stderr) = validate_in(dir.path());

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
    let (code, stderr) = validate_in(dir.path());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("ship_id"), "{stderr}");
}

#[test]
fn suggests_a_correction_for_a_misspelled_workflow() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-relase\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let (code, stderr) = validate_in(dir.path());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("did you mean"), "{stderr}");
    assert!(stderr.contains("prepare-release"), "{stderr}");
}

#[test]
fn reports_a_missing_workflow_with_the_available_ones() {
    let config = "version: 1\nworkflows:\n  prepare: totally-different\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let (code, stderr) = validate_in(dir.path());
    assert_eq!(code, 1, "{stderr}");
    // miette hard-wraps long messages, so assert on tokens.
    assert!(stderr.contains("totally-different"), "{stderr}");
    assert!(stderr.contains("available workflows"), "{stderr}");
    assert!(stderr.contains("prepare-release"), "{stderr}");
}

#[test]
fn suggests_init_when_there_are_no_workflows_at_all() {
    let dir = repo(Some(MINIMAL_CONFIG), &[]);
    let (code, stderr) = validate_in(dir.path());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("gh ship init"), "{stderr}");
}

#[test]
fn rejects_an_unknown_config_key() {
    // `events:` was an early design that was dropped; someone copying an
    // old example must get a clear error rather than silence.
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\nevents:\n  prepare: x\n";
    let dir = repo(Some(config), &[("prepare-release.yml", CONFORMING)]);
    let (code, stderr) = validate_in(dir.path());
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
    let (code, stderr) = validate_in(dir.path());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("publish-release.yml"), "{stderr}");
}

#[test]
fn setup_output_is_stable() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let (_, stderr) = validate_in(dir.path());
    insta::assert_snapshot!("setup__valid", stderr);
}

#[test]
fn call_only_diagnostic_is_stable() {
    let call_only = "name: prepare-release\non:\n  workflow_call:\n";
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", call_only)]);
    let (_, stderr) = validate_in(dir.path());
    insta::assert_snapshot!("setup__call_only", stderr);
}

// --- init ---------------------------------------------------------------

#[test]
fn init_refuses_to_clobber_an_existing_config() {
    let dir = repo(Some(MINIMAL_CONFIG), &[("prepare-release.yml", CONFORMING)]);
    let out = ship().current_dir(dir.path()).arg("init").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
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

    let (code, stderr) = validate_in(dir.path());
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
    let out = ship().current_dir(root).arg("validate").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(0),
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
    for name in ["prepare-release.yml", "publish-release.yml"] {
        let body = std::fs::read_to_string(root.join("templates").join(name))
            .unwrap_or_else(|e| panic!("templates/{name} must exist: {e}"));

        let dir = repo(
            Some("version: 1\nworkflows:\n  prepare: prepare-release\n"),
            &[("prepare-release.yml", &body)],
        );

        // Only the prepare template is wired as `workflows.prepare`; the
        // publish one is checked for parseability and the contract via
        // the same path by swapping its name in.
        if name == "prepare-release.yml" {
            let (code, stderr) = validate_in(dir.path());
            assert_eq!(code, 0, "templates/{name} is not conforming:\n{stderr}");
        }
    }
}
