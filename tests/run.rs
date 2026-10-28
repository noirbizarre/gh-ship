//! Drive automatic routing through the real CLI with isolated Actions events.
mod common;

use common::{CHANGED_ARTIFACT, GhStub, MINIMAL_CONFIG, Repo};
use serde_json::{Value, json};

fn event(repo: Repo, name: &str, payload: Value) -> Repo {
    let path = repo.path().join("event.json");
    std::fs::write(&path, payload.to_string()).unwrap();
    repo.with_env("GITHUB_ACTIONS", "true")
        .with_env("GITHUB_EVENT_NAME", name)
        .with_env("GITHUB_EVENT_PATH", path.to_str().unwrap())
}

fn identity(base: &str, head: &str, head_repo: Option<&str>, merged: bool) -> Value {
    json!({
        "merged": merged,
        "base": {"ref": base, "repo": {"full_name": "acme/widgets"}},
        "head": {"ref": head, "repo": head_repo.map(|name| json!({"full_name": name}))}
    })
}

fn release_stub(identity: Value) -> GhStub {
    GhStub::new()
        .pr_identity(&identity.to_string())
        .pr_body(&gh_ship::render::embed_artifact(
            "",
            &serde_json::from_str(CHANGED_ARTIFACT).unwrap(),
        ))
        .pr_state("OPEN")
        .viewed_pr_merged()
        .merge_commit("event-merge-sha")
}

fn assert_no_mutations(repo: &Repo) {
    let calls = repo.stub.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("workflow run")
            || c.starts_with("release create")
            || c.starts_with("release edit")
            || c.starts_with("pr create")
            || c.starts_with("pr merge")
            || (c.starts_with("api ") && c.contains("--method"))),
        "{calls:?}"
    );
}

#[test]
fn pushes_prepare_only_active_release_lines_using_payload_not_checkout() {
    for (config, branch) in [
        (MINIMAL_CONFIG.to_string(), "main"),
        (
            format!(
                "{MINIMAL_CONFIG}branches: [main, 'maint/*']\nrelease_branch: 'next/{{{{ match }}}}'\n"
            ),
            "maint/1.x",
        ),
    ] {
        let repo = event(
            Repo::new(&config, GhStub::new()).in_ci("wrong-checkout"),
            "push",
            json!({"ref": format!("refs/heads/{branch}"), "deleted": false}),
        );
        let out = repo.ship(&["run"]);
        assert_eq!(out.code, 0, "{}", out.diagnostics());
        assert!(
            repo.stub
                .called_with(&["workflow run", "--ref ship/prepare-"])
        );
        assert!(repo.stub.called_with(&[&format!("git/ref/heads/{branch}")]));
    }
}

#[test]
fn irrelevant_events_skip_without_mutation() {
    for (name, payload) in [
        ("push", json!({"ref": "refs/tags/v1", "deleted": false})),
        ("push", json!({"ref": "refs/heads/main", "deleted": true})),
        (
            "push",
            json!({"ref": "refs/heads/feature", "deleted": false}),
        ),
        (
            "pull_request",
            json!({"action": "closed", "pull_request": {"merged": false}}),
        ),
        ("pull_request", json!({"action": "opened"})),
        ("workflow_dispatch", json!({})),
        ("pull_request_target", json!({})),
    ] {
        let repo = event(Repo::new(MINIMAL_CONFIG, GhStub::new()), name, payload);
        let out = repo.ship(&["run"]);
        assert_eq!(out.code, 0, "{name}: {}", out.diagnostics());
        assert_no_mutations(&repo);
    }
}

#[test]
fn release_is_bound_to_event_pr_even_with_newer_open_pr() {
    let repo = event(
        Repo::new(
            MINIMAL_CONFIG,
            release_stub(identity("main", "release/next", Some("acme/widgets"), true)),
        ),
        "pull_request",
        json!({"action": "closed", "number": 42, "pull_request": {"merged": true}}),
    );
    let out = repo.ship(&["run"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(repo.stub.called_with(&["pr view 42"]));
    assert!(!repo.stub.called_with(&["pr list"]));
    assert!(
        repo.stub
            .called_with(&["refs/tags/v1.4.0", "sha=event-merge-sha"])
    );
    assert!(repo.stub.called_with(&["release create", "--draft"]));
    assert!(repo.stub.called_with(&["release edit", "--draft=false"]));
    assert!(!repo.stub.called_with(&["pr merge"]));
}

#[test]
fn missing_pr_object_is_recovered_and_custom_release_branch_matches() {
    let config =
        format!("{MINIMAL_CONFIG}branches: ['maint/*']\nrelease_branch: 'next/{{{{ match }}}}'\n");
    for payload in [
        json!({"action": "closed", "number": 42}),
        json!({"action": "closed", "number": 42, "pull_request": null}),
        json!({"action": "closed", "number": 42, "pull_request": {}}),
    ] {
        let repo = event(
            Repo::new(
                &config,
                release_stub(identity(
                    "maint/1.x",
                    "next/1.x",
                    Some("acme/widgets"),
                    true,
                )),
            ),
            "pull_request",
            payload,
        );
        let out = repo.ship(&["run"]);
        assert_eq!(out.code, 0, "{}", out.diagnostics());
        assert!(repo.stub.called_with(&["release create"]));
    }
}

#[test]
fn other_prs_are_not_releases() {
    for routing in [
        identity("main", "release/next", Some("fork/widgets"), true),
        identity("main", "release/next", None, true),
        identity("main", "feature", Some("acme/widgets"), true),
        identity("feature", "release/next", Some("acme/widgets"), true),
        identity("main", "release/next", Some("acme/widgets"), false),
    ] {
        let repo = event(
            Repo::new(MINIMAL_CONFIG, release_stub(routing)),
            "pull_request",
            json!({"action": "closed", "number": 42}),
        );
        let out = repo.ship(&["run"]);
        assert_eq!(out.code, 0, "{}", out.diagnostics());
        assert_no_mutations(&repo);
    }
}

#[test]
fn missing_context_payload_and_conflicting_base_fail_without_mutation() {
    let local = Repo::new(MINIMAL_CONFIG, GhStub::new());
    let out = local.ship(&["run"]);
    assert_eq!(out.code, 1);
    assert!(out.diagnostics().contains("use `gh ship prepare`"));
    assert!(local.stub.calls().is_empty());

    let repo = event(
        Repo::new(MINIMAL_CONFIG, GhStub::new()),
        "push",
        json!({"ref": "refs/heads/main", "deleted": false}),
    );
    let out = repo.ship(&["run", "--base", "elsewhere"]);
    assert_eq!(out.code, 1);
    assert!(out.diagnostics().contains("conflicts"));
    assert_no_mutations(&repo);

    std::fs::write(repo.path().join("event.json"), "not json").unwrap();
    assert_eq!(repo.ship(&["run"]).code, 1);
    let repo = repo.with_env("GITHUB_EVENT_PATH", "/nonexistent/event.json");
    assert_eq!(repo.ship(&["run"]).code, 1);
    assert_no_mutations(&repo);
}

#[test]
fn no_wait_is_forwarded_to_prepare() {
    let repo = event(
        Repo::new(MINIMAL_CONFIG, GhStub::new()),
        "push",
        json!({"ref": "refs/heads/main", "deleted": false}),
    );
    let out = repo.ship(&["run", "--no-wait"]);
    assert_eq!(out.code, 0, "{}", out.diagnostics());
    assert!(repo.stub.called_with(&["workflow run"]));
    assert!(!repo.stub.called_with(&["run download"]));
}

#[test]
fn release_no_wait_leaves_a_draft_and_completed_publisher_is_reused() {
    let config = "version: 1\nworkflows:\n  prepare: prepare-release\n  publish: publish-release\n";
    for complete in [false, true] {
        let mut stub = release_stub(identity("main", "release/next", Some("acme/widgets"), true));
        if complete {
            stub = stub
                .existing_run("completed", "success")
                .release_exists(true);
        }
        let repo = event(
            Repo::new(config, stub),
            "pull_request",
            json!({"action": "closed", "number": 42}),
        );
        let out = repo.ship(&["run", "--no-wait"]);
        assert_eq!(out.code, 0, "{}", out.diagnostics());
        if complete {
            assert!(!repo.stub.called_with(&["workflow run"]));
            assert!(!repo.stub.called_with(&["release create"]));
            assert!(repo.stub.called_with(&["release edit", "--draft=false"]));
        } else {
            assert!(repo.stub.called_with(&["workflow run", "--ref v1.4.0"]));
            assert!(!repo.stub.called_with(&["release edit"]));
        }
    }
}

#[test]
fn merge_push_guard_is_preserved_before_and_after_release() {
    for published in [false, true] {
        let body =
            gh_ship::render::embed_artifact("", &serde_json::from_str(CHANGED_ARTIFACT).unwrap());
        let stub = GhStub::new()
            .pr_body(&body)
            .pr_state("MERGED")
            .merge_commit("a1b2c3d4e5f6")
            .release_exists(published);
        let repo = event(
            Repo::new(MINIMAL_CONFIG, stub),
            "push",
            json!({"ref": "refs/heads/main", "deleted": false}),
        );
        let out = repo.ship(&["run"]);
        assert_eq!(out.code, 0, "{}", out.diagnostics());
        assert_no_mutations(&repo);
        assert!(out.diagnostics().contains(if published {
            "nothing new"
        } else {
            "not yet published"
        }));
    }
}

#[test]
fn supported_events_require_context_and_routing_identity() {
    for payload in [
        json!({}),
        json!({"action": "closed"}),
        json!({"action": "closed", "number": 0}),
    ] {
        let repo = event(
            Repo::new(MINIMAL_CONFIG, GhStub::new()),
            "pull_request",
            payload,
        );
        assert_eq!(repo.ship(&["run"]).code, 1);
        assert_no_mutations(&repo);
    }
    let repo = Repo::new(MINIMAL_CONFIG, GhStub::new()).with_env("GITHUB_ACTIONS", "true");
    assert_eq!(repo.ship(&["run"]).code, 1);
    let repo = repo.with_env("GITHUB_EVENT_NAME", "push");
    assert_eq!(repo.ship(&["run"]).code, 1);
    assert!(repo.stub.calls().is_empty());

    let repo = event(
        Repo::new(
            MINIMAL_CONFIG,
            release_stub(identity("main", "release/next", Some("acme/widgets"), true)),
        ),
        "pull_request",
        json!({"action": "closed", "number": 42}),
    )
    .base("other");
    assert_eq!(repo.ship(&["run"]).code, 1);
    assert_no_mutations(&repo);
}
