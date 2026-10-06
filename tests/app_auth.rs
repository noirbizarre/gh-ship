//! Built-in GitHub App authentication.
//!
//! The stubbed `gh` answers the two calls gh-ship makes as the App (find the
//! installation, mint a token) and records the `GH_TOKEN` every other call
//! ran with. That is what lets these tests say which token authenticated
//! what, without a network or a real App.

mod common;

use common::{CHANGED_ARTIFACT, GhStub, MINIMAL_CONFIG as CONFIG, Repo};

const KEY: &str = include_str!("fixtures/app-key.pem");

fn app_repo(stub: GhStub) -> Repo {
    Repo::new(CONFIG, stub.artifact(CHANGED_ARTIFACT))
        .with_env("SHIP_APP_CLIENT_ID", "Iv1.test")
        .with_env("SHIP_APP_PRIVATE_KEY", KEY)
        .with_env("GITHUB_REPOSITORY", "acme/widgets")
}

fn count(repo: &Repo, fragment: &str) -> usize {
    repo.stub
        .calls()
        .iter()
        .filter(|c| c.contains(fragment))
        .count()
}

#[test]
fn every_call_runs_with_the_minted_token() {
    let repo = app_repo(GhStub::new());
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());

    let tokens = repo.stub.installation_tokens();
    assert!(tokens.len() > 3, "expected several calls: {tokens:?}");
    assert!(
        tokens.iter().all(|t| t == "ghs_stub_1"),
        "one token should serve them all: {tokens:?}"
    );
}

#[test]
fn the_installation_is_looked_up_once_and_the_token_is_scoped_to_the_repository() {
    let repo = app_repo(GhStub::new());
    repo.ship(&["preview"]);

    assert_eq!(count(&repo, "repos/acme/widgets/installation"), 1);
    assert_eq!(count(&repo, "app/installations/4242/access_tokens"), 1);
    assert!(
        repo.stub
            .called_with(&["access_tokens", "-X POST", "repositories[]=widgets"]),
        "{:?}",
        repo.stub.calls()
    );
}

#[test]
fn the_jwt_is_sent_as_a_bearer_token() {
    let repo = app_repo(GhStub::new());
    repo.ship(&["preview"]);

    let jwt_calls: Vec<_> = repo
        .stub
        .calls()
        .into_iter()
        .filter(|c| c.contains("Authorization: Bearer"))
        .collect();
    assert_eq!(jwt_calls.len(), 2, "{jwt_calls:?}");
    // Three dot-separated base64url segments, starting with the RS256 header.
    assert!(jwt_calls[0].contains("Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9."));
}

#[test]
fn a_known_installation_id_skips_the_lookup() {
    let repo = app_repo(GhStub::new()).with_env("SHIP_APP_INSTALLATION_ID", "777");
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());

    assert_eq!(count(&repo, "/installation"), 1, "only the minting call");
    assert!(
        repo.stub
            .called_with(&["app/installations/777/access_tokens"])
    );
}

#[test]
fn without_credentials_nothing_changes() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT));
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());

    assert!(!repo.stub.called_with(&["Authorization: Bearer"]));
    assert!(
        repo.stub.tokens().iter().all(String::is_empty),
        "gh-ship must leave authentication to gh: {:?}",
        repo.stub.tokens()
    );
}

#[test]
fn an_inherited_token_is_replaced_by_the_apps() {
    let repo = app_repo(GhStub::new()).with_env("GH_TOKEN", "ghp_from_the_environment");
    repo.ship(&["preview"]);

    assert!(
        repo.stub
            .installation_tokens()
            .iter()
            .all(|t| t == "ghs_stub_1"),
        "{:?}",
        repo.stub.installation_tokens()
    );
}

#[test]
fn an_expiring_token_is_replaced_during_the_run() {
    // A zero lifetime means every call finds the token due, which is what a
    // long wait looks like to the cache once the hour is nearly up.
    let repo = app_repo(GhStub::new()).with_env("SHIP_APP_TOKEN_TTL", "0");
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());

    let tokens = repo.stub.installation_tokens();
    let mut distinct = tokens.clone();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        tokens.len(),
        "each call should have run on a fresh token: {tokens:?}"
    );
    assert!(tokens.len() > 3);
}

#[test]
fn a_revoked_token_is_replaced_and_the_call_repeated() {
    let repo = app_repo(GhStub::new().rejects_token("ghs_stub_1"));
    let out = repo.ship(&["preview"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());

    assert_eq!(count(&repo, "access_tokens"), 2, "{:?}", repo.stub.calls());
    let tokens = repo.stub.installation_tokens();
    assert_eq!(tokens.first().map(String::as_str), Some("ghs_stub_1"));
    assert_eq!(tokens.last().map(String::as_str), Some("ghs_stub_2"));
}

#[test]
fn the_token_is_masked_in_github_actions() {
    let repo = app_repo(GhStub::new()).with_env("GITHUB_ACTIONS", "true");
    let out = repo.ship(&["preview"]);

    assert!(
        out.diagnostics().contains("::add-mask::ghs_stub_1"),
        "{}",
        out.diagnostics()
    );
    assert!(!out.stdout.contains("ghs_stub_1"), "{}", out.stdout);
}

#[test]
fn the_token_is_not_announced_outside_github_actions() {
    let repo = app_repo(GhStub::new());
    let out = repo.ship(&["preview"]);
    assert!(!out.diagnostics().contains("ghs_stub_1"));
}

#[test]
fn the_token_lifetime_warning_is_moot_once_gh_ship_refreshes() {
    let long = ["SHIP_RUN_TIMEOUT", "3600"];

    let external = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT))
        .in_ci("main")
        .with_env(long[0], long[1]);
    assert!(
        external
            .ship(&["preview"])
            .diagnostics()
            .contains("lifetime of a GitHub App"),
        "an externally minted token still has the ceiling"
    );

    let builtin = app_repo(GhStub::new())
        .in_ci("main")
        .with_env(long[0], long[1]);
    assert!(
        !builtin
            .ship(&["preview"])
            .diagnostics()
            .contains("lifetime of a GitHub App"),
        "a refreshed token has no ceiling"
    );
}

// --- Failures --------------------------------------------------------------

#[test]
fn half_configured_credentials_are_an_error_not_a_fallback() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT))
        .with_env("SHIP_APP_CLIENT_ID", "Iv1.test");
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1, "{}", out.diagnostics());
    assert!(
        out.diagnostics().contains("SHIP_APP_PRIVATE_KEY"),
        "{}",
        out.diagnostics()
    );
    assert!(repo.stub.calls().is_empty(), "nothing should reach gh");
}

#[test]
fn an_unreadable_key_is_reported() {
    let repo = app_repo(GhStub::new()).with_env("SHIP_APP_PRIVATE_KEY", "not a key");
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1, "{}", out.diagnostics());
    assert!(
        // miette wraps long messages, so match a fragment that stays on one line.
        out.diagnostics().contains("ship::gh::app_auth")
            && out.diagnostics().contains("SHIP_APP_PRIVATE_KEY"),
        "{}",
        out.diagnostics()
    );
}

#[test]
fn an_unknown_repository_asks_for_one() {
    let repo = Repo::new(CONFIG, GhStub::new().artifact(CHANGED_ARTIFACT))
        .with_env("SHIP_APP_CLIENT_ID", "Iv1.test")
        .with_env("SHIP_APP_PRIVATE_KEY", KEY);
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1, "{}", out.diagnostics());
    assert!(
        out.diagnostics().contains("SHIP_APP_INSTALLATION_ID"),
        "{}",
        out.diagnostics()
    );
}

#[test]
fn a_rejected_jwt_points_at_the_credentials() {
    let repo = app_repo(GhStub::new().app_rejects_jwt());
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1, "{}", out.diagnostics());
    assert!(
        out.diagnostics().contains("SHIP_APP_CLIENT_ID"),
        "{}",
        out.diagnostics()
    );
}

#[test]
fn an_app_that_is_not_installed_says_so() {
    let repo = app_repo(GhStub::new().app_not_installed());
    let out = repo.ship(&["preview"]);

    assert_eq!(out.code, 1, "{}", out.diagnostics());
    assert!(
        out.diagnostics().contains("not installed"),
        "{}",
        out.diagnostics()
    );
}

#[test]
fn validate_needs_no_authentication_even_with_credentials_set() {
    let repo = app_repo(GhStub::new());
    let out = repo.ship(&["validate", "--help"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(repo.stub.calls().is_empty());
}
