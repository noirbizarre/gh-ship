//! The `ship.release.json` artifact — GH Ship's public protocol.
//!
//! This module is deliberately free of any GitHub, network or filesystem
//! coupling beyond reading a single file. `gh ship validate FILE` must
//! run inside any CI system with no `gh`, no auth, and no repository
//! checkout, so the schema is embedded in the binary and validation is
//! entirely local.

pub mod schema;
pub mod span;
pub mod validate;

use serde::{Deserialize, Serialize};

/// The protocol version this build of GH Ship speaks.
pub const SCHEMA_VERSION: u32 = 1;

/// Canonical URL of the v1 schema, used for `$schema` and in generated
/// workflow templates.
pub const SCHEMA_URL: &str = "https://noirbizarre.github.io/gh-ship/schema/release/v1.json";

/// The artifact name a workflow must upload.
pub const ARTIFACT_NAME: &str = "ship-release";

/// The filename inside that artifact.
pub const ARTIFACT_FILE: &str = "ship.release.json";

/// A validated release artifact.
///
/// Deserialization is deliberately permissive (`x-` extensions and
/// forward-compatible fields are tolerated by the model); *strictness*
/// lives in the JSON Schema, which runs first and produces far better
/// diagnostics than serde ever could.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,

    pub changed: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequest>,
}

/// GitHub Release content supplied by the workflow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Release {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_latest: Option<bool>,
}

/// Release PR overrides supplied by the workflow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

impl Artifact {
    /// The tag, which the schema guarantees is present when `changed`.
    ///
    /// Returns `None` for an unchanged artifact. Callers that have
    /// already short-circuited on `!changed` may unwrap.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The version, guaranteed present when `changed`.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Release notes, or an empty string when the workflow supplied none.
    pub fn notes(&self) -> &str {
        self.release
            .as_ref()
            .and_then(|r| r.notes.as_deref())
            .unwrap_or("")
    }

    /// Release title, falling back to the tag.
    pub fn release_name(&self) -> Option<&str> {
        self.release
            .as_ref()
            .and_then(|r| r.name.as_deref())
            .or_else(|| self.tag())
    }

    /// Whether the GitHub Release should be marked as a pre-release.
    pub fn is_prerelease(&self) -> bool {
        self.release
            .as_ref()
            .and_then(|r| r.prerelease)
            .unwrap_or(false)
    }

    /// Whether the GitHub Release should be marked as the repository's latest.
    ///
    /// `None` means the workflow did not say, in which case GitHub's own
    /// default applies — which is not the same as `Some(true)`, so the
    /// distinction is preserved rather than defaulted here.
    pub fn make_latest(&self) -> Option<bool> {
        self.release.as_ref().and_then(|r| r.make_latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_artifact_round_trips_minimally() {
        let json = r#"{"schemaVersion":1,"changed":false}"#;
        let a: Artifact = serde_json::from_str(json).unwrap();
        assert_eq!(a.schema_version, 1);
        assert!(!a.changed);
        assert_eq!(a.tag(), None);
        // Optional fields must not be re-emitted, so the artifact we
        // embed in a PR body stays as small as what the workflow sent.
        assert_eq!(serde_json::to_string(&a).unwrap(), json);
    }

    #[test]
    fn release_name_falls_back_to_tag() {
        let a: Artifact = serde_json::from_str(
            r#"{"schemaVersion":1,"changed":true,"version":"1.0.0","tag":"v1.0.0"}"#,
        )
        .unwrap();
        assert_eq!(a.release_name(), Some("v1.0.0"));
        assert_eq!(a.notes(), "");
        assert!(!a.is_prerelease());
        assert_eq!(
            a.make_latest(),
            None,
            "an unstated `make_latest` must stay unstated, so GitHub's own \
             default applies rather than gh-ship forcing `--latest=true`"
        );
    }

    #[test]
    fn explicit_release_fields_win() {
        let a: Artifact = serde_json::from_str(
            r#"{"schemaVersion":1,"changed":true,"version":"1.0.0","tag":"v1.0.0",
                "release":{"name":"Big One","notes":"stuff","prerelease":true,
                           "make_latest":false}}"#,
        )
        .unwrap();
        assert_eq!(a.release_name(), Some("Big One"));
        assert_eq!(a.notes(), "stuff");
        assert!(a.is_prerelease());
        assert_eq!(a.make_latest(), Some(false));
    }
}
