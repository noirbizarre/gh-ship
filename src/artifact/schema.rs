//! Compilation of the embedded release artifact schema.
//!
//! The schema is baked into the binary with `include_str!` so that
//! `gh ship validate` never touches the network. The published copy at
//! [`super::SCHEMA_URL`] and this file are the same bytes — a test
//! asserts the `$id` matches so they cannot drift.

use std::sync::OnceLock;

use boon::{Compiler, SchemaIndex, Schemas};

/// The v1 schema source, embedded at build time.
pub const RELEASE_V1: &str = include_str!("../../schemas/release.v1.schema.json");

struct Compiled {
    schemas: Schemas,
    index: SchemaIndex,
}

static COMPILED: OnceLock<Compiled> = OnceLock::new();

/// Compile (once) and return the v1 schema.
///
/// Panics only if the embedded schema is itself invalid, which is a
/// build-time bug caught by this module's tests.
fn compiled() -> &'static Compiled {
    COMPILED.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(RELEASE_V1).expect("embedded schema is valid JSON");
        let mut schemas = Schemas::new();
        let mut compiler = Compiler::new();
        compiler
            .add_resource(super::SCHEMA_URL, value)
            .expect("embedded schema is a valid resource");
        let index = compiler
            .compile(super::SCHEMA_URL, &mut schemas)
            .expect("embedded schema compiles");
        Compiled { schemas, index }
    })
}

/// Validate a parsed JSON value against the v1 schema.
pub fn validate(value: &serde_json::Value) -> Result<(), boon::ValidationError<'static, '_>> {
    let c = compiled();
    c.schemas.validate(value, c.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_schema_compiles() {
        // Forces the OnceLock; any schema authoring error panics here
        // rather than in a user's terminal.
        let _ = compiled();
    }

    #[test]
    fn schema_id_matches_published_url() {
        let v: serde_json::Value = serde_json::from_str(RELEASE_V1).unwrap();
        assert_eq!(
            v["$id"].as_str(),
            Some(crate::artifact::SCHEMA_URL),
            "the embedded schema's $id must match the URL we tell users to reference"
        );
    }

    #[test]
    fn schema_declares_2020_12() {
        let v: serde_json::Value = serde_json::from_str(RELEASE_V1).unwrap();
        assert_eq!(
            v["$schema"].as_str(),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
    }

    #[test]
    fn documented_examples_validate() {
        let v: serde_json::Value = serde_json::from_str(RELEASE_V1).unwrap();
        let examples = v["examples"].as_array().expect("schema has examples");
        assert!(!examples.is_empty());
        for (i, ex) in examples.iter().enumerate() {
            assert!(
                validate(ex).is_ok(),
                "schema example {i} does not validate against its own schema"
            );
        }
    }
}
