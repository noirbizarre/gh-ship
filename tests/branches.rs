//! Release lines: detecting the base branch and resolving it to a line.
//!
//! These drive the real binary against a stubbed `gh`, so they cover the
//! whole path a user takes — the flag, the CI environment, the local
//! checkout — rather than the resolution function in isolation.
//!
//! The back-compat test at the bottom is the important one: a repository
//! without `branches` must behave exactly as it did before detection
//! existed, whatever the environment says.

mod common;

use common::{CHANGED_ARTIFACT, GhStub, Repo, with_redactions};

/// Two release lines: `main` and every `release/*` maintenance branch.
const LINES: &str = "version: 1\n\
                     branches: [main, \"release/*\"]\n\
                     release_branch: \"next/{{ branch }}\"\n\
                     workflows:\n  prepare: prepare-release\n";

fn stub() -> GhStub {
    GhStub::new().artifact(CHANGED_ARTIFACT)
}

// --- Detection -----------------------------------------------------------

#[test]
fn the_base_flag_selects_the_line() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["status", "--base", "release/1.x", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["base_branch"], "release/1.x");
    assert_eq!(status["release_branch"], "next/release/1.x");
    assert_eq!(status["branch_rule"], "release/*");
    assert_eq!(status["base_branch_origin"], "flag");
}

#[test]
fn the_ci_environment_selects_the_line() {
    let repo = Repo::new(LINES, stub()).in_ci("release/2.x");
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["base_branch"], "release/2.x");
    assert_eq!(status["release_branch"], "next/release/2.x");
    assert_eq!(status["base_branch_origin"], "ci");
}

#[test]
fn a_pull_request_run_uses_the_branch_it_targets() {
    let repo = Repo::new(LINES, stub()).in_pr("release/1.x", "feature/whatever");
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(
        status["base_branch"], "release/1.x",
        "never the contributor's branch"
    );
}

#[test]
fn the_local_checkout_selects_the_line() {
    let repo = Repo::new(LINES, stub()).with_git_head("release/3.x");
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["base_branch"], "release/3.x");
    assert_eq!(status["base_branch_origin"], "git");
}

#[test]
fn the_flag_beats_the_environment() {
    let repo = Repo::new(LINES, stub())
        .in_ci("release/9.x")
        .with_git_head("release/8.x");
    let out = repo.ship(&["status", "--base", "main", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["base_branch"], "main");
    assert_eq!(status["base_branch_origin"], "flag");
}

#[test]
fn nothing_detected_falls_back_to_the_repository_default() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(
        status["base_branch"], "main",
        "the default branch is a listed line, so this works"
    );
    assert_eq!(status["base_branch_origin"], "default");
}

// --- Matching ------------------------------------------------------------

#[test]
fn an_exact_entry_wins_over_a_pattern_declared_first() {
    let config = "version: 1\n\
                  branches: [\"release/*\", \"release/next\"]\n\
                  release_branch: \"staging/{{ branch }}\"\n\
                  workflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["status", "--base", "release/next", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["branch_rule"], "release/next");
    assert_eq!(
        status["release_branch"], "staging/release/next",
        "an exact entry captures the whole branch name"
    );
}

#[test]
fn patterns_are_tried_in_the_order_they_are_written() {
    let config = "version: 1\n\
                  branches: [\"release/1*\", \"release/*\"]\n\
                  release_branch: \"next/{{ branch }}\"\n\
                  workflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());

    let out = repo.ship(&["status", "--base", "release/1.x", "--json"]);
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["branch_rule"], "release/1*");

    let out = repo.ship(&["status", "--base", "release/2.x", "--json"]);
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["branch_rule"], "release/*");
}

// --- Refusals ------------------------------------------------------------

#[test]
fn an_unlisted_branch_is_refused() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["prepare", "--base", "feature/x"]);

    assert_ne!(
        out.code, 0,
        "releasing from an unlisted branch is a mistake"
    );
    let diagnostics = out.diagnostics();
    assert!(
        diagnostics.contains("no branch rule matches"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("`release/*`"), "{diagnostics}");
    assert!(
        !repo.stub.called_with(&["workflow run"]),
        "nothing should be dispatched: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn preview_refuses_an_unlisted_branch_too() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["preview", "--base", "feature/x"]);

    assert_ne!(
        out.code, 0,
        "preview and prepare must agree on what is releasable"
    );
}

#[test]
fn a_constant_release_branch_across_lines_is_refused() {
    let config = "version: 1\n\
                  branches: [main, \"release/*\"]\n\
                  release_branch: next/release\n\
                  workflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["validate"]);

    assert_ne!(out.code, 0, "two lines would collide on one branch");
    assert!(
        out.diagnostics()
            .contains("ship::branches::constant_glob_release_branch"),
        "{}",
        out.diagnostics()
    );
}

// --- Per-entry overrides -------------------------------------------------

#[test]
fn lifecycle_commands_refuse_self_targeting_before_mutations() {
    for branches in ["", "branches: [main]\n"] {
        for command in ["prepare", "preview", "release", "status"] {
            let config = format!(
                "version: 1\n{branches}release_branch: main\nworkflows:\n  prepare: prepare-release\n"
            );
            let repo = Repo::new(&config, stub());
            let out = repo.ship(&[command, "--base", "main"]);
            assert_ne!(out.code, 0, "{}", out.diagnostics());
            assert!(out.diagnostics().contains("release_branch_is_base_branch"));
            assert!(
                !repo.stub.called_with(&["--method"]),
                "{:?}",
                repo.stub.calls()
            );
            assert!(!repo.stub.called_with(&["workflow run"]));
        }
    }
}

/// One line deviates; the rest fall back to the top-level template.
const OVERRIDE: &str = "version: 1\n\
                        release_branch: \"next/{{ match }}\"\n\
                        branches:\n\
                        \x20 - branch: main\n\
                        \x20   release_branch: next/release\n\
                        \x20 - \"release/*\"\n\
                        workflows:\n  prepare: prepare-release\n";

#[test]
fn an_overridden_line_stages_where_it_says() {
    let repo = Repo::new(OVERRIDE, stub());
    let out = repo.ship(&["status", "--base", "main", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["release_branch"], "next/release");
    assert_eq!(status["branch_rule"], "main");
}

#[test]
fn a_line_without_an_override_still_falls_back() {
    let repo = Repo::new(OVERRIDE, stub());
    let out = repo.ship(&["status", "--base", "release/1.x", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["release_branch"], "next/1.x");
}

#[test]
fn prepare_opens_the_pull_request_from_the_overridden_branch() {
    let repo = Repo::new(OVERRIDE, stub());
    let out = repo.ship(&["prepare", "--base", "main"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(
        repo.stub.called_with(&["pr create", "next/release"]),
        "the override has to reach the PR, not just the report: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn validate_says_which_lines_override_the_template() {
    let repo = Repo::new(OVERRIDE, stub());
    let out = repo.ship(&["validate"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    with_redactions(|| insta::assert_snapshot!("validate_overrides", out.diagnostics()));
}

#[test]
fn the_selector_key_is_branch_not_match() {
    let config = "version: 1\nbranches:\n  - match: main\nworkflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["validate"]);

    assert_ne!(out.code, 0);
    with_redactions(|| insta::assert_snapshot!("match_is_not_the_key", out.diagnostics()));
}

#[test]
fn two_globs_that_can_produce_one_branch_are_refused() {
    // `release/1.x` and `v1.x` both capture `1.x`.
    let config = "version: 1\n\
                  branches: [\"release/*\", \"v*\"]\n\
                  release_branch: \"next/{{ match }}\"\n\
                  workflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["validate"]);

    assert_ne!(out.code, 0);
    assert!(
        // The code, not the prose: miette wraps the message, so any
        // long-enough phrase is at the mercy of the terminal width.
        out.diagnostics()
            .contains("ship::branches::colliding_globs"),
        "{}",
        out.diagnostics()
    );
}

#[test]
fn a_release_branch_that_is_a_base_branch_is_refused() {
    let config = "version: 1\n\
                  branches:\n\
                  \x20 - branch: main\n\
                  \x20   release_branch: main\n\
                  workflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["validate"]);

    assert_ne!(out.code, 0, "a PR from main into main is not a release");
    assert!(
        out.diagnostics()
            .contains("ship::branches::release_branch_is_base_branch"),
        "{}",
        out.diagnostics()
    );
}

// --- Staging is scoped per line -----------------------------------------

#[test]
fn staging_branches_are_scoped_to_their_release_line() {
    use sha2::{Digest, Sha256};
    let prefix = format!("ship/prepare-{:x}-", Sha256::digest(b"release/1.x"));
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["prepare", "--base", "release/1.x"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());

    // The sweep asks GitHub for this line's staging branches only, so a
    // prepare running on another line cannot have its branch deleted
    // out from under its workflow.
    assert!(
        repo.stub
            .called_with(&[&format!("matching-refs/heads/{prefix}")]),
        "the sweep must be scoped to the line: {:?}",
        repo.stub.calls()
    );
    assert!(
        repo.stub.called_with(&[&prefix]),
        "the staging branch carries the line: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn each_line_gets_its_own_release_branch_and_pull_request() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["prepare", "--base", "release/1.x"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(
        repo.stub.called_with(&["pr create", "next/release/1.x"]),
        "the PR is opened from this line's release branch: {:?}",
        repo.stub.calls()
    );
    assert!(
        repo.stub.called_with(&["pr create", "release/1.x"]),
        "and targets this line's base: {:?}",
        repo.stub.calls()
    );
}

// --- Back-compatibility --------------------------------------------------

#[test]
fn staging_cleanup_ignores_other_lines_and_malformed_tokens() {
    use sha2::{Digest, Sha256};
    for (base, other) in [("release/1.x", "release-1.x"), ("main", "main-next")] {
        let own = format!("ship/prepare-{:x}-", Sha256::digest(base.as_bytes()));
        let foreign = format!(
            "ship/prepare-{:x}-deadbeef1234",
            Sha256::digest(other.as_bytes())
        );
        let config = format!(
            "version: 1\nbranches: [{base}, {other}]\nrelease_branch: 'next/{{{{ branch }}}}'\nworkflows:\n  prepare: prepare-release\n"
        );
        for stale in [
            foreign,
            format!("{own}deadbeef1234-extra"),
            format!("{own}deadbeef1234"),
        ] {
            let repo = Repo::new(&config, stub().stale_staging_branch(&stale));
            let out = repo.ship(&["prepare", "--base", base]);
            assert_eq!(out.code, 0, "{}", out.diagnostics());
            assert_eq!(
                repo.stub.called_with(&[&stale, "DELETE"]),
                stale == format!("{own}deadbeef1234")
            );
        }
    }
}

#[test]
fn glob_self_targets_and_mixed_collisions_are_refused() {
    for (branches, template) in [
        ("[\"release/*\"]", "release/{{ match }}"),
        ("[main, \"release/*\"]", "next/{{ match }}"),
    ] {
        let config = format!(
            "version: 1\nbranches: {branches}\nrelease_branch: \"{template}\"\nworkflows:\n  prepare: prepare-release\n"
        );
        let repo = Repo::new(&config, stub());
        for command in ["validate", "prepare"] {
            let out = repo.ship(&[command]);
            assert_ne!(out.code, 0, "{}", out.diagnostics());
            assert!(!repo.stub.called_with(&["workflow run"]));
            assert!(!repo.stub.called_with(&["--method"]));
        }
    }
}

#[test]
fn without_release_lines_the_environment_is_ignored() {
    // The regression guard for every existing user: detection must not
    // retarget the Release PR just because CI happens to run on a
    // feature branch.
    let repo = Repo::new(common::MINIMAL_CONFIG, stub())
        .in_ci("feature/x")
        .with_git_head("feature/x");
    let out = repo.ship(&["status", "--json"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    let status: serde_json::Value = serde_json::from_str(&out.stdout).expect("status is JSON");
    assert_eq!(status["base_branch"], "main", "the repository default wins");
    assert_eq!(status["release_branch"], "release/next");
    assert_eq!(status["base_branch_origin"], "default");
    assert!(
        status["branch_rule"].is_null(),
        "there is no line to name when none are configured"
    );
}

#[test]
fn without_release_lines_staging_branches_keep_their_old_names() {
    // Scoping the sweep only when lines exist is what stops a branch
    // staged by an earlier version from being orphaned forever.
    let repo = Repo::new(common::MINIMAL_CONFIG, stub());
    let out = repo.ship(&["prepare"]);

    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(
        repo.stub
            .called_with(&["matching-refs/heads/ship/prepare-"]),
        "{:?}",
        repo.stub.calls()
    );
    assert!(
        !repo.stub.called_with(&["ship/prepare-main-"]),
        "the single-line name must not gain a line component: {:?}",
        repo.stub.calls()
    );
}

#[test]
fn base_branch_explains_where_it_went() {
    let config = "version: 1\nbase_branch: develop\nworkflows:\n  prepare: prepare-release\n";
    let repo = Repo::new(config, stub());
    let out = repo.ship(&["validate"]);

    assert_ne!(out.code, 0);
    with_redactions(|| insta::assert_snapshot!("base_branch_removed", out.diagnostics()));
}

#[test]
fn an_unmatched_branch_diagnostic_is_stable() {
    let repo = Repo::new(LINES, stub());
    let out = repo.ship(&["status", "--base", "mian"]);

    assert_ne!(out.code, 0);
    with_redactions(|| insta::assert_snapshot!("no_matching_line", out.diagnostics()));
}
