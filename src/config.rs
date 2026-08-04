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
///
/// `name` is what the diagnostic shows, not necessarily a filesystem
/// path: configuration is also parsed from strings in tests and from
/// embedded sources.
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub name: String,
    pub text: String,
}

impl Source {
    pub(crate) fn named(&self) -> NamedSource<String> {
        NamedSource::new(&self.name, self.text.clone()).with_language("yaml")
    }

    pub(crate) fn locate(&self, needle: &str) -> SourceSpan {
        suggest::span_of_substring(&self.text, needle)
    }
}

/// The configuration model.
///
/// Every struct below denies unknown fields, mirroring
/// `additionalProperties: false` in `schemas/config.v1.schema.json` — a
/// typo in a config someone hand-wrote must be an error, not a silent
/// no-op. The artifact model deliberately does the opposite; see
/// [`crate::artifact::Artifact`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Config schema version. Must be [`CONFIG_VERSION`].
    pub version: u32,

    /// Branch on which the release is staged.
    ///
    /// A MiniJinja template rendered per release line, with `branch`
    /// (the full base branch) and `match` (what a `*` in the matching
    /// [`Settings::branches`] entry captured) in context. A config with
    /// a single release line can leave it a plain string.
    #[serde(default = "default_release_branch")]
    pub release_branch: String,

    /// The base branches gh-ship releases from, one release line each.
    ///
    /// An entry containing `*` is a glob; anything else is an exact
    /// branch name. Empty — the default — means the repository's own
    /// default branch, resolved at runtime, which is why there is no
    /// literal `main` anywhere in this model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,

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
///
/// `#[serde(default)]` sits on the container rather than on each field so
/// that [`PullRequestConfig::default`] is the single source of truth: an
/// omitted key and an explicitly-defaulted struct cannot drift apart.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PullRequestConfig {
    /// MiniJinja template for the PR title.
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
    pub reuse: bool,
}

/// GitHub Release behaviour.
///
/// See [`PullRequestConfig`] for why the default lives on the container.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Create the release as a draft first, then undraft after the
    /// publish workflow succeeds.
    ///
    /// This is the default because it is the only ordering that lets a
    /// publish workflow upload assets to the release *before* anyone
    /// subscribed to the repository is notified about it.
    pub draft: bool,
}

fn default_release_branch() -> String {
    DEFAULT_RELEASE_BRANCH.to_string()
}

impl Default for PullRequestConfig {
    fn default() -> Self {
        Self {
            title: DEFAULT_PR_TITLE.to_string(),
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

    #[error("invalid YAML in `{name}`: {message}")]
    #[diagnostic(code(ship::config::parse))]
    Parse {
        name: String,
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

    #[error("`{field}` is no longer a configuration key")]
    #[diagnostic(code(ship::config::removed_field), help("{help}"))]
    RemovedField {
        field: String,
        help: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("remove this")]
        span: SourceSpan,
    },

    #[error("invalid branch pattern `{pattern}`: {reason}")]
    #[diagnostic(code(ship::config::branch_pattern), help("{help}"))]
    BranchPattern {
        pattern: String,
        reason: String,
        help: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("`branches` lists `{entry}` twice")]
    #[diagnostic(
        code(ship::config::duplicate_branch),
        help("the first match wins, so the second entry is dead — remove one")
    )]
    DuplicateBranch {
        entry: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("duplicate")]
        span: SourceSpan,
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
    pub fn parse(name: &str, text: &str) -> Result<Self, ConfigError> {
        let source = Source {
            name: name.to_string(),
            text: text.to_string(),
        };

        let settings: Settings = serde_norway::from_str(text).map_err(|e| {
            let message = e.to_string();
            // `base_branch` was folded into `branches`, and
            // `deny_unknown_fields` would report it as an anonymous typo.
            // A removed key deserves its migration instructions.
            if message.contains("unknown field `base_branch`") {
                return ConfigError::RemovedField {
                    field: "base_branch".into(),
                    help: "`base_branch` was replaced by `branches`, which lists every base \
                           branch gh-ship releases from. Write `branches: [develop]`."
                        .into(),
                    src: source.named(),
                    span: source.locate("base_branch:"),
                };
            }
            ConfigError::Parse {
                name: name.to_string(),
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

        self.check_branches()?;

        Ok(())
    }

    /// Shape checks for `branches`.
    ///
    /// Only the checks that need no templating live here; whether the
    /// `release_branch` template compiles, and whether it actually
    /// varies per line, belong to [`crate::branches::check`] so that
    /// this module stays free of MiniJinja.
    fn check_branches(&self) -> Result<(), ConfigError> {
        let mut seen: Vec<&str> = Vec::new();

        for (i, entry) in self.settings.branches.iter().enumerate() {
            let entry = entry.as_str();

            if entry.trim().is_empty() {
                return Err(ConfigError::EmptyField {
                    field: format!("branches[{i}]"),
                    src: self.source.named(),
                    span: self.source.locate("branches:"),
                    help: Some(
                        "each entry is a base branch name, or a glob such as `release/*`".into(),
                    ),
                });
            }

            if entry.matches('*').count() > 1 {
                return Err(ConfigError::BranchPattern {
                    pattern: entry.to_string(),
                    reason: "a pattern may contain at most one `*`".into(),
                    help: "`*` captures the part of the branch name that varies, and \
                           `{{ match }}` is a single value — split this into several entries"
                        .into(),
                    src: self.source.named(),
                    span: self.source.locate(entry),
                });
            }

            if seen.contains(&entry) {
                return Err(ConfigError::DuplicateBranch {
                    entry: entry.to_string(),
                    src: self.source.named(),
                    span: self.source.locate(entry),
                });
            }
            seen.push(entry);
        }

        Ok(())
    }

    /// Convenience accessors.
    ///
    /// This is the `release_branch` *template*, not a branch name: with
    /// several release lines it renders differently per line. The
    /// resolved name comes from the release line — see
    /// [`crate::branches::Line`].
    pub fn release_branch_template(&self) -> &str {
        &self.settings.release_branch
    }

    /// The configured base branches, empty when the repository default
    /// branch is the only release line.
    pub fn branches(&self) -> &[String] {
        &self.settings.branches
    }

    /// Whether explicit release lines are configured.
    ///
    /// This is the switch that turns on base-branch detection: without
    /// it there is nothing to select, so the repository default branch
    /// is as good an answer as it ever was.
    pub fn has_branches(&self) -> bool {
        !self.settings.branches.is_empty()
    }

    pub fn prepare_workflow(&self) -> &str {
        &self.settings.workflows.prepare
    }

    pub fn publish_workflow(&self) -> Option<&str> {
        self.settings.workflows.publish.as_deref()
    }

    pub fn pull_request(&self) -> &PullRequestConfig {
        &self.settings.pull_request
    }

    pub fn release(&self) -> &ReleaseConfig {
        &self.settings.release
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
        assert_eq!(c.release_branch_template(), DEFAULT_RELEASE_BRANCH);
        assert_eq!(c.publish_workflow(), None);
        assert_eq!(c.settings.pull_request.title, DEFAULT_PR_TITLE);
        assert!(c.settings.release.draft, "draft-first is the default");
        assert!(
            !c.has_branches(),
            "no branches means the repo default branch"
        );
    }

    #[test]
    fn full_config_round_trips() {
        let text = r#"
version: 1
release_branch: release/staging
branches: [develop]
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
        assert_eq!(c.release_branch_template(), "release/staging");
        assert_eq!(c.branches(), ["develop"]);
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
    fn parses_several_release_lines() {
        let text = "version: 1\nbranches: [main, \"release/*\"]\nrelease_branch: \"next/{{ match }}\"\nworkflows:\n  prepare: x\n";
        let c = parse(text).unwrap();
        assert_eq!(c.branches(), ["main", "release/*"]);
        assert!(c.has_branches());
    }

    #[test]
    fn base_branch_explains_its_replacement() {
        let e = parse("version: 1\nbase_branch: develop\nworkflows:\n  prepare: x\n").unwrap_err();
        let ConfigError::RemovedField { field, help, .. } = &e else {
            panic!("expected RemovedField, got {e:?}")
        };
        assert_eq!(field, "base_branch");
        assert!(help.contains("branches: [develop]"), "{help}");
    }

    #[test]
    fn rejects_a_pattern_with_several_wildcards() {
        let e = parse("version: 1\nbranches: [\"a*b*c\"]\nworkflows:\n  prepare: x\n").unwrap_err();
        let ConfigError::BranchPattern { pattern, .. } = &e else {
            panic!("expected BranchPattern, got {e:?}")
        };
        assert_eq!(pattern, "a*b*c");
    }

    #[test]
    fn rejects_a_duplicated_branch() {
        let e =
            parse("version: 1\nbranches: [main, main]\nworkflows:\n  prepare: x\n").unwrap_err();
        assert!(matches!(e, ConfigError::DuplicateBranch { .. }), "{e:?}");
    }

    #[test]
    fn rejects_an_empty_branch_entry() {
        let e = parse("version: 1\nbranches: [\"  \"]\nworkflows:\n  prepare: x\n").unwrap_err();
        let ConfigError::EmptyField { field, .. } = &e else {
            panic!("expected EmptyField, got {e:?}")
        };
        assert_eq!(field, "branches[0]");
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
