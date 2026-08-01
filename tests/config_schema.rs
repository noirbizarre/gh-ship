//! The configuration schema must not drift from the Rust model.
//!
//! `schemas/config.v1.schema.json` exists for editor completion, not for
//! validation — `gh ship validate` keeps its hand-written diagnostics, which say
//! things a schema error cannot ("did you mean", why `release_branch` may not be
//! empty). That split is useful, but it means the schema has no natural pressure
//! keeping it truthful: nothing in the binary reads it, so a field added to
//! `src/config.rs` would silently go undocumented and editors would flag valid
//! configuration as invalid.
//!
//! These tests are that pressure.

use boon::{Compiler, Schemas};

const SCHEMA_PATH: &str = "schemas/config.v1.schema.json";
const SCHEMA_URL: &str = "https://noirbizarre.github.io/gh-ship/schema/config/v1.json";

fn schema_source() -> serde_json::Value {
    let text = std::fs::read_to_string(SCHEMA_PATH).expect("config schema exists");
    serde_json::from_str(&text).expect("config schema is valid JSON")
}

/// Validate a YAML document against the published config schema.
fn check(yaml: &str) -> Result<(), String> {
    let value: serde_json::Value = {
        let parsed: serde_norway::Value =
            serde_norway::from_str(yaml).map_err(|e| e.to_string())?;
        serde_json::to_value(parsed).map_err(|e| e.to_string())?
    };

    let mut schemas = Schemas::new();
    let mut compiler = Compiler::new();
    compiler
        .add_resource(SCHEMA_URL, schema_source())
        .expect("schema is a valid resource");
    let index = compiler
        .compile(SCHEMA_URL, &mut schemas)
        .expect("schema compiles");

    schemas.validate(&value, index).map_err(|e| e.to_string())
}

#[test]
fn schema_id_matches_the_published_url() {
    assert_eq!(
        schema_source()["$id"].as_str(),
        Some(SCHEMA_URL),
        "the $id must match where docs.yaml publishes it, or every editor \
         annotation in the wild breaks"
    );
}

/// The strongest guard available: gh-ship's own configuration, which is
/// exercised by every release, must satisfy the schema it ships.
#[test]
fn our_own_config_validates() {
    let yaml = std::fs::read_to_string(".github/ship.yml").expect("this repo is gh-ship enabled");
    check(&yaml).expect("gh-ship's own config must satisfy its own schema");
}

/// What `gh ship init` writes must validate, or every new user's editor
/// immediately reports their brand-new configuration as broken.
#[test]
fn the_generated_config_validates() {
    // Mirrors src/commands/init.rs::render_config for the fields that matter;
    // the round trip through Config::parse is covered by that module's tests.
    let yaml = "\
version: 1
release_branch: release/next
workflows:
  prepare: prepare-release
  publish: publish-release
pull_request:
  title: \"Release {{ version }}\"
";
    check(yaml).expect("the config init generates must validate");
}

#[test]
fn the_minimal_config_validates() {
    check("version: 1\nworkflows:\n  prepare: prepare-release\n")
        .expect("only version and workflows.prepare are required");
}

#[test]
fn every_documented_example_validates() {
    let schema = schema_source();
    let examples = schema["examples"].as_array().expect("schema has examples");
    assert!(!examples.is_empty());
    for (i, example) in examples.iter().enumerate() {
        let yaml = serde_norway::to_string(example).expect("example serialises");
        check(&yaml).unwrap_or_else(|e| panic!("schema example {i} does not validate: {e}"));
    }
}

// --- the schema must be as strict as serde ------------------------------

#[test]
fn unknown_keys_are_rejected() {
    // `deny_unknown_fields` in src/config.rs; the schema must agree, or editors
    // will happily autocomplete a key the binary refuses.
    assert!(check("version: 1\nworkflows:\n  prepare: p\nevents:\n  x: y\n").is_err());
    assert!(check("version: 1\nworkflows:\n  prepare: p\n  bogus: y\n").is_err());
    assert!(check("version: 1\nworkflows:\n  prepare: p\nrelease:\n  bogus: true\n").is_err());
    assert!(
        check("version: 1\nworkflows:\n  prepare: p\npull_request:\n  bogus: 1\n").is_err(),
        "pull_request must reject unknown keys too"
    );
}

#[test]
fn required_fields_are_enforced() {
    assert!(
        check("workflows:\n  prepare: p\n").is_err(),
        "version required"
    );
    assert!(check("version: 1\n").is_err(), "workflows required");
    assert!(
        check("version: 1\nworkflows: {}\n").is_err(),
        "workflows.prepare required"
    );
}

#[test]
fn the_version_is_pinned_to_one() {
    assert!(check("version: 2\nworkflows:\n  prepare: p\n").is_err());
    assert!(check("version: 0\nworkflows:\n  prepare: p\n").is_err());
}

#[test]
fn empty_strings_are_rejected_where_the_binary_rejects_them() {
    // Config::check() rejects both of these with a tailored diagnostic; the
    // schema should not call them valid.
    assert!(check("version: 1\nrelease_branch: \"\"\nworkflows:\n  prepare: p\n").is_err());
    assert!(check("version: 1\nworkflows:\n  prepare: \"\"\n").is_err());
}

#[test]
fn types_are_enforced() {
    assert!(
        check("version: 1\nworkflows:\n  prepare: p\nrelease:\n  draft: yes-please\n").is_err()
    );
    assert!(
        check("version: 1\nworkflows:\n  prepare: p\npull_request:\n  labels: nope\n").is_err(),
        "labels must be a list"
    );
}

/// Every field on the Rust model should be described. A missing one means an
/// editor reports valid configuration as invalid.
#[test]
fn every_config_field_is_described() {
    let schema = schema_source();
    let props = &schema["properties"];

    for field in [
        "version",
        "release_branch",
        "base_branch",
        "workflows",
        "pull_request",
        "release",
    ] {
        assert!(
            !props[field].is_null(),
            "root field `{field}` is undocumented"
        );
        assert!(
            props[field]["description"].is_string(),
            "root field `{field}` has no description"
        );
    }
    for field in ["prepare", "publish"] {
        assert!(
            !props["workflows"]["properties"][field].is_null(),
            "workflows.{field} is undocumented"
        );
    }
    for field in ["title", "header", "footer", "labels"] {
        assert!(
            !props["pull_request"]["properties"][field].is_null(),
            "pull_request.{field} is undocumented"
        );
    }
    assert!(!props["release"]["properties"]["draft"].is_null());
}
