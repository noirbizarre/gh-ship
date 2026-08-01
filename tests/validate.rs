//! Integration tests for `gh ship validate`.
//!
//! The corpus under `tests/fixtures/artifacts` is the executable
//! specification of the protocol: every file in `valid/` must be
//! accepted, every file in `invalid/` must be rejected, and each
//! rejection's diagnostic is snapshotted so a regression in message
//! quality shows up as a diff.

mod common;

use common::{fixtures_in, ship, validate_fixture};

/// Fixtures live in a directory; the snapshot name is the filename so
/// snapshots stay stable when fixtures are added or reordered.
fn stem(path: &std::path::Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}

#[test]
fn every_valid_fixture_is_accepted() {
    for path in fixtures_in("artifacts/valid") {
        let name = stem(&path);
        let out = validate_fixture(&format!("artifacts/valid/{name}.json"));
        assert!(
            out.succeeded(),
            "valid fixture `{name}` was rejected (exit {}):\n{}",
            out.code,
            out.diagnostics()
        );
    }
}

#[test]
fn every_invalid_fixture_is_rejected() {
    for path in fixtures_in("artifacts/invalid") {
        let name = stem(&path);
        let out = validate_fixture(&format!("artifacts/invalid/{name}.json"));
        assert_eq!(
            out.code,
            1,
            "invalid fixture `{name}` was accepted:\n{}",
            out.diagnostics()
        );
    }
}

#[test]
fn valid_artifact_output_is_stable() {
    for path in fixtures_in("artifacts/valid") {
        let name = stem(&path);
        let out = validate_fixture(&format!("artifacts/valid/{name}.json"));
        insta::assert_snapshot!(format!("valid__{name}"), out.diagnostics());
    }
}

#[test]
fn invalid_artifact_diagnostics_are_stable() {
    for path in fixtures_in("artifacts/invalid") {
        let name = stem(&path);
        let out = validate_fixture(&format!("artifacts/invalid/{name}.json"));
        insta::assert_snapshot!(format!("invalid__{name}"), out.diagnostics());
    }
}

#[test]
fn missing_file_reports_a_readable_error() {
    let out = ship()
        .arg("validate")
        .arg("does-not-exist.json")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr.contains("failed to read"), "{stderr}");
    assert!(
        stderr.contains("ship-release"),
        "the help should point at the artifact upload step: {stderr}"
    );
}

/// With no artifact, `validate` checks the setup — so outside a
/// configured repository it must point at `init` rather than fail
/// obscurely.
#[test]
fn validate_without_a_target_checks_the_setup() {
    let dir = tempfile::tempdir().unwrap();
    let out = ship()
        .current_dir(dir.path())
        .arg("validate")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr.contains("no gh-ship configuration"), "{stderr}");
    assert!(stderr.contains("gh ship init"), "{stderr}");
}

/// `validate` must not need `gh`, network, auth, or a repository.
///
/// This is what lets workflows validate before uploading, and lets
/// non-GitHub CI check artifacts at all. We enforce it by emptying
/// `PATH` (so `gh` and `git` are unreachable) and clearing every token
/// variable, then running in a directory that is not a git repository.
#[test]
fn validate_works_with_no_gh_no_auth_and_no_repo() {
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("ship.release.json");
    std::fs::write(
        &artifact,
        r#"{"schemaVersion":1,"changed":true,"version":"1.0.0","tag":"v1.0.0"}"#,
    )
    .unwrap();

    let out = ship()
        .current_dir(dir.path())
        .env("PATH", "")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_CONFIG_DIR")
        .arg("validate")
        .arg("ship.release.json")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "validate must be fully self-contained:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The unchanged case is the one CI branches on, so pin its contract:
/// exit 0, and a message that says so plainly.
#[test]
fn unchanged_artifact_exits_zero() {
    let out = validate_fixture("artifacts/valid/minimal-unchanged.json");
    assert!(out.succeeded());
    assert!(
        out.diagnostics().contains("nothing to release"),
        "{}",
        out.diagnostics()
    );
}
