//! Configuration — `.github/ship.yml`.
//!
//! Convention over configuration: the only required field is
//! `workflows.prepare`. Everything else has a sensible default, and a
//! repository with a two-line config is a fully working gh-ship setup.
//!
//! The raw source text is retained alongside the parsed model because
//! `serde_norway::Value` discards positions, and good diagnostics need
//! to point at the offending line.

use std::path::{Path, PathBuf};

use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::suggest;

/// The config schema version this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Default branch used to stage a release.
pub const DEFAULT_RELEASE_BRANCH: &str = "release/next";

/// Default Release PR title template.
pub const DEFAULT_PR_TITLE: &str = "Release {{ version }}";

/// Parsed configuration, plus the source it came from.
#[derive(Debug, Clone)]
pub struct Config {
    pub settings: Settings,
    pub source: Source,
}

/// The raw configuration text and its origin, kept for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub path: String,
    pub text: String,
}

impl Source {
    fn named(&self) -> NamedSource<String> {
        NamedSource::new(&self.path, self.text.clone()).with_language("yaml")
    }

    fn locate(&self, needle: &str) -> SourceSpan {
        suggest::locate(&self.text, needle)
    }
}

/// The configuration model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Config schema version. Must be [`CONFIG_VERSION`].
    pub version: u32,

    /// Branch on which the release is staged. Created by gh-ship if
    /// missing; the prepare workflow commits to it.
    #[serde(default = "default_release_branch")]
    pub release_branch: String,

    /// Branch the Release PR targets. Defaults to the repository's
    /// default branch, resolved at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,

    /// The workflows gh-ship dispatches.
    pub workflows: Workflows,

    /// Release PR rendering.
    #[serde(default)]
    pub pull_request: PullRequestConfig,

    /// GitHub Release behaviour.
    #[serde(default)]
    pub release: ReleaseConfig,
}

/// Workflows gh-ship dispatches.
///
/// These name workflows that **must** declare `on: workflow_dispatch`.
/// A `workflow_call`-only (so-called "reusable") workflow cannot be
/// dispatched by the GitHub API at all — `gh ship validate` checks this
/// so the failure surfaces at setup time, not mid-release.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workflows {
    /// Workflow that bumps the version, writes the changelog, commits,
    /// pushes, and uploads `ship.release.json`.
    pub prepare: String,

    /// Optional workflow run after the Release PR is merged and the
    /// draft release exists — typically builds and uploads assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<String>,
}

/// Release PR rendering.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PullRequestConfig {
    /// MiniJinja template for the PR title.
    #[serde(default = "default_pr_title")]
    pub title: String,

    /// Markdown prepended to the release notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,

    /// Markdown appended after the release notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,

    /// Labels applied to the Release PR.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// Reuse the existing Release PR rather than opening a new one each time.
    ///
    /// On by default, so a release under review keeps its number, its comments
    /// and its review state across repeated prepares. A closed-but-unmerged PR
    /// is reopened; a merged one is left alone and a new PR is opened, since
    /// that release has shipped.
    ///
    /// Set to `false` to close any open Release PR and open a fresh one on
    /// every prepare.
    #[serde(default = "default_true")]
    pub reuse: bool,
}

/// GitHub Release behaviour.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Create the release as a draft first, then undraft after the
    /// publish workflow succeeds.
    ///
    /// This is the default because it is the only ordering that lets a
    /// publish workflow upload assets to the release *before* anyone
    /// subscribed to the repository is notified about it.
    #[serde(default = "default_true")]
    pub draft: bool,
}

fn default_release_branch() -> String {
    DEFAULT_RELEASE_BRANCH.to_string()
}

fn default_pr_title() -> String {
    DEFAULT_PR_TITLE.to_string()
}

fn default_true() -> bool {
    true
}

impl Default for PullRequestConfig {
    fn default() -> Self {
        Self {
            title: default_pr_title(),
            header: None,
            footer: None,
            labels: Vec::new(),
            reuse: true,
        }
    }
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        Self { draft: true }
    }
}

/// Errors raised while loading configuration.
#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("no gh-ship configuration found at `{}`", path.display())]
    #[diagnostic(
        code(ship::config::missing),
        help("run `gh ship init` to create one — it takes less than a minute")
    )]
    Missing { path: PathBuf },

    #[error("failed to read `{}`", path.display())]
    #[diagnostic(code(ship::config::io), help("check the file is readable"))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid YAML in `{path}`: {message}")]
    #[diagnostic(code(ship::config::parse))]
    Parse {
        path: String,
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("here")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
    },

    #[error("unsupported config version: {found} (expected {CONFIG_VERSION})")]
    #[diagnostic(
        code(ship::config::version),
        help("set `version: {CONFIG_VERSION}` at the top of the file")
    )]
    UnsupportedVersion {
        found: u32,
        #[source_code]
        src: NamedSource<String>,
        #[label("unsupported")]
        span: SourceSpan,
    },

    #[error("`{field}` must not be empty")]
    #[diagnostic(code(ship::config::empty))]
    EmptyField {
        field: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("empty")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
    },
}

impl Config {
    /// Load configuration from `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::Missing {
                path: path.to_path_buf(),
            });
        }
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&path.display().to_string(), &text)
    }

    /// Parse configuration from text.
    pub fn parse(path: &str, text: &str) -> Result<Self, ConfigError> {
        let source = Source {
            path: path.to_string(),
            text: text.to_string(),
        };

        let settings: Settings = serde_norway::from_str(text).map_err(|e| {
            let message = e.to_string();
            ConfigError::Parse {
                path: path.to_string(),
                help: parse_help(&message),
                message,
                src: source.named(),
                span: e
                    .location()
                    .map(|l| SourceSpan::from((l.index(), 1)))
                    .unwrap_or_else(|| SourceSpan::from((0, 0))),
            }
        })?;

        let config = Self { settings, source };
        config.check()?;
        Ok(config)
    }

    /// Structural checks that serde cannot express.
    fn check(&self) -> Result<(), ConfigError> {
        let s = &self.settings;

        if s.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: s.version,
                src: self.source.named(),
                span: self.source.locate(&format!("version: {}", s.version)),
            });
        }

        if s.workflows.prepare.trim().is_empty() {
            return Err(ConfigError::EmptyField {
                field: "workflows.prepare".into(),
                src: self.source.named(),
                span: self.source.locate("prepare:"),
                help: Some(
                    "name the workflow that prepares the release, e.g. `prepare: prepare-release`"
                        .into(),
                ),
            });
        }

        if s.release_branch.trim().is_empty() {
            return Err(ConfigError::EmptyField {
                field: "release_branch".into(),
                src: self.source.named(),
                span: self.source.locate("release_branch:"),
                help: Some(format!(
                    "omit it to use the default, `{DEFAULT_RELEASE_BRANCH}`"
                )),
            });
        }

        Ok(())
    }

    /// Convenience accessors.
    pub fn release_branch(&self) -> &str {
        &self.settings.release_branch
    }

    pub fn prepare_workflow(&self) -> &str {
        &self.settings.workflows.prepare
    }

    pub fn publish_workflow(&self) -> Option<&str> {
        self.settings.workflows.publish.as_deref()
    }
}

/// Turn serde_norway's terse messages into something actionable.
fn parse_help(message: &str) -> Option<String> {
    if message.contains("unknown field") {
        // serde already lists the expected fields; point at the docs
        // for the shape rather than repeating them.
        return Some(
            "see https://noirbizarre.github.io/gh-ship/configuration/ for the full schema".into(),
        );
    }
    if message.contains("missing field `workflows`") {
        return Some(
            "add a `workflows:` block naming the workflow that prepares your release".into(),
        );
    }
    if message.contains("missing field `prepare`") {
        return Some("`workflows.prepare` is the only required workflow".into());
    }
    if message.contains("missing field `version`") {
        return Some(format!("start the file with `version: {CONFIG_VERSION}`"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "version: 1\nworkflows:\n  prepare: prepare-release\n";

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(".github/ship.yml", text)
    }

    #[test]
    fn minimal_config_is_enough() {
        let c = parse(MINIMAL).unwrap();
        assert_eq!(c.prepare_workflow(), "prepare-release");
        assert_eq!(c.release_branch(), DEFAULT_RELEASE_BRANCH);
        assert_eq!(c.publish_workflow(), None);
        assert_eq!(c.settings.pull_request.title, DEFAULT_PR_TITLE);
        assert!(c.settings.release.draft, "draft-first is the default");
        assert_eq!(
            c.settings.base_branch, None,
            "base defaults to the repo default"
        );
    }

    #[test]
    fn full_config_round_trips() {
        let text = r#"
version: 1
release_branch: release/staging
base_branch: develop
workflows:
  prepare: prepare-release
  publish: publish-release
pull_request:
  title: "Ship {{ version }}"
  header: |
    Heads up.
  footer: |
    Bye.
  labels: [release, automated]
release:
  draft: false
"#;
        let c = parse(text).unwrap();
        assert_eq!(c.release_branch(), "release/staging");
        assert_eq!(c.settings.base_branch.as_deref(), Some("develop"));
        assert_eq!(c.publish_workflow(), Some("publish-release"));
        assert_eq!(c.settings.pull_request.title, "Ship {{ version }}");
        assert_eq!(
            c.settings.pull_request.header.as_deref(),
            Some("Heads up.\n")
        );
        assert_eq!(c.settings.pull_request.labels, ["release", "automated"]);
        assert!(c.settings.pull_request.reuse, "reuse defaults to true");
        assert!(!c.settings.release.draft);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let e = parse("version: 1\nworkflows:\n  prepare: x\nevents:\n  foo: bar\n").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("unknown field"), "{msg}");
    }

    #[test]
    fn rejects_unsupported_version() {
        let e = parse("version: 99\nworkflows:\n  prepare: x\n").unwrap_err();
        assert!(matches!(
            e,
            ConfigError::UnsupportedVersion { found: 99, .. }
        ));
    }

    #[test]
    fn requires_a_prepare_workflow() {
        let e = parse("version: 1\nworkflows: {}\n").unwrap_err();
        assert!(e.to_string().contains("prepare"), "{e}");
    }

    #[test]
    fn rejects_empty_prepare_workflow() {
        let e = parse("version: 1\nworkflows:\n  prepare: \"  \"\n").unwrap_err();
        assert!(matches!(e, ConfigError::EmptyField { .. }));
    }

    #[test]
    fn rejects_empty_release_branch() {
        let e = parse("version: 1\nrelease_branch: \"\"\nworkflows:\n  prepare: x\n").unwrap_err();
        let ConfigError::EmptyField { field, help, .. } = &e else {
            panic!("expected EmptyField, got {e:?}")
        };
        assert_eq!(field, "release_branch");
        assert!(help.as_ref().unwrap().contains(DEFAULT_RELEASE_BRANCH));
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let e = parse("version: 1\nworkflows:\n\tprepare: x\n").unwrap_err();
        assert!(matches!(e, ConfigError::Parse { .. }), "{e:?}");
    }

    #[test]
    fn missing_file_suggests_init() {
        let e = Config::load(Path::new("/nonexistent/ship.yml")).unwrap_err();
        let help = miette::Diagnostic::help(&e).unwrap().to_string();
        assert!(help.contains("gh ship init"), "{help}");
    }

    #[test]
    fn missing_workflows_block_is_explained() {
        let e = parse("version: 1\n").unwrap_err();
        let ConfigError::Parse { help, .. } = &e else {
            panic!("expected Parse, got {e:?}")
        };
        assert!(help.as_ref().unwrap().contains("workflows:"), "{help:?}");
    }
}
