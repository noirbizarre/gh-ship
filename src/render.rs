//! Pull Request rendering.
//!
//! The workflow supplies data; gh-ship renders the presentation. The
//! split matters: changing the wording of a Release PR should not
//! require editing a workflow, and a workflow should not have to know
//! anything about Markdown assembly.
//!
//! Templates are MiniJinja, and the **release artifact is the root
//! context**: `{{ version }}`, `{{ tag }}`, `{{ release.notes }}`. There
//! is no nesting under a `release.` prefix for the top-level fields,
//! because the artifact is the vocabulary users already learned from the
//! specification.

use miette::{Diagnostic, NamedSource, SourceSpan};
use minijinja::Environment;
use thiserror::Error;

use crate::artifact::Artifact;
use crate::config::PullRequestConfig;

/// Marker delimiting the embedded artifact in a PR body.
///
/// gh-ship keeps **zero local state**: everything it needs later is
/// reconstructed from GitHub. `gh ship release` needs the artifact that
/// `gh ship prepare` validated, possibly days later and on a different
/// machine, so the artifact rides along in the PR body inside an HTML
/// comment. Invisible when rendered, durable, and survives artifact
/// retention expiry.
pub const ARTIFACT_MARKER_START: &str = "<!-- ship:artifact";
pub const ARTIFACT_MARKER_END: &str = "-->";

/// Errors raised while rendering a template.
#[derive(Debug, Error, Diagnostic)]
#[error("failed to render `{field}`: {message}")]
#[diagnostic(code(ship::template), help("{help}"))]
pub struct TemplateError {
    pub field: String,
    pub message: String,
    #[source_code]
    pub src: NamedSource<String>,
    #[label("here")]
    pub span: SourceSpan,
    pub help: String,
}

/// A rendered Release PR.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPullRequest {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// Render the Release PR for an artifact.
pub fn render(
    config: &PullRequestConfig,
    artifact: &Artifact,
) -> Result<RenderedPullRequest, TemplateError> {
    let context = context_for(artifact);

    // The artifact may override the title outright; otherwise the
    // configured template wins.
    let title = match artifact
        .pull_request
        .as_ref()
        .and_then(|p| p.title.as_deref())
    {
        Some(t) => t.to_string(),
        None => render_template("pull_request.title", &config.title, &context)?,
    };

    // A workflow that supplies a complete body has opinions we should
    // not override; header/footer assembly is skipped entirely.
    let body = match artifact
        .pull_request
        .as_ref()
        .and_then(|p| p.body.as_deref())
    {
        Some(b) => b.to_string(),
        None => {
            let header =
                render_optional("pull_request.header", config.header.as_deref(), &context)?;
            let footer =
                render_optional("pull_request.footer", config.footer.as_deref(), &context)?;
            assemble_body(header.as_deref(), artifact.notes(), footer.as_deref())
        }
    };

    let mut labels = config.labels.clone();
    if let Some(pr) = &artifact.pull_request {
        for label in &pr.labels {
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
    }

    Ok(RenderedPullRequest {
        title,
        body,
        labels,
    })
}

/// Join header, notes and footer with exactly one blank line between
/// the parts that are present.
///
/// Getting this wrong produces PR bodies with stacks of blank lines,
/// which is the kind of small ugliness people notice on every release.
fn assemble_body(header: Option<&str>, notes: &str, footer: Option<&str>) -> String {
    let parts: Vec<&str> = [header, Some(notes), footer]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    parts.join("\n\n")
}

/// Build the template context: the artifact, at the root.
fn context_for(artifact: &Artifact) -> minijinja::Value {
    // Serializing the artifact gives exactly the vocabulary documented
    // in the specification, with no translation layer to drift.
    minijinja::Value::from_serialize(artifact)
}

fn render_optional(
    field: &str,
    template: Option<&str>,
    context: &minijinja::Value,
) -> Result<Option<String>, TemplateError> {
    template
        .map(|t| render_template(field, t, context))
        .transpose()
}

fn render_template(
    field: &str,
    template: &str,
    context: &minijinja::Value,
) -> Result<String, TemplateError> {
    let env = Environment::new();
    env.render_str(template, context).map_err(|e| {
        let span = e
            .range()
            .map(|r| SourceSpan::from(r.start..r.end))
            .unwrap_or_else(|| SourceSpan::from((0, 0)));
        TemplateError {
            field: field.to_string(),
            message: e.to_string(),
            src: NamedSource::new(field, template.to_string()).with_language("jinja"),
            span,
            help: template_help(&e),
        }
    })
}

/// Turn MiniJinja's error kinds into guidance in gh-ship's vocabulary.
fn template_help(err: &minijinja::Error) -> String {
    use minijinja::ErrorKind;
    match err.kind() {
        ErrorKind::UndefinedError => {
            "the release artifact is the root context, so use `{{ version }}`, `{{ tag }}`, \
             `{{ release.notes }}` — not `{{ release.version }}`"
                .to_string()
        }
        ErrorKind::SyntaxError => {
            "check the template syntax; `{{ }}` interpolates and `{% %}` controls flow".to_string()
        }
        _ => "see https://noirbizarre.github.io/gh-ship/configuration/#templates".to_string(),
    }
}

/// Embed an artifact into a PR body, replacing any previous copy.
pub fn embed_artifact(body: &str, artifact: &Artifact) -> String {
    // HTML comments cannot nest, so a `-->` anywhere in the payload would close
    // the block early: the rest of the JSON would render as visible text, and
    // `extract_artifact` -- which searches for the first `-->` -- would recover
    // a truncated prefix that fails to parse. A changelog is exactly the kind
    // of payload that quotes commit subjects, and those can mention HTML
    // comments.
    //
    // `\u003e` is the JSON escape for `>`, so `serde_json::from_str` decodes it
    // back transparently and the read side needs no matching change. `-->` can
    // only ever occur inside a JSON string literal, never in the structure, so
    // replacing it unconditionally is safe.
    let json = serde_json::to_string(artifact)
        .unwrap_or_else(|_| "{}".to_string())
        .replace("-->", "--\\u003e");
    let block = format!("{ARTIFACT_MARKER_START}\n{json}\n{ARTIFACT_MARKER_END}");
    let stripped = strip_artifact(body);
    if stripped.trim().is_empty() {
        block
    } else {
        format!("{}\n\n{block}\n", stripped.trim_end())
    }
}

/// Recover an artifact previously embedded in a PR body.
pub fn extract_artifact(body: &str) -> Option<Artifact> {
    let start = body.find(ARTIFACT_MARKER_START)?;
    let after = start + ARTIFACT_MARKER_START.len();
    let end = body[after..].find(ARTIFACT_MARKER_END)? + after;
    serde_json::from_str(body[after..end].trim()).ok()
}

/// Remove the embedded artifact block from a body.
pub fn strip_artifact(body: &str) -> String {
    let Some(start) = body.find(ARTIFACT_MARKER_START) else {
        return body.to_string();
    };
    let after = start + ARTIFACT_MARKER_START.len();
    let Some(rel) = body[after..].find(ARTIFACT_MARKER_END) else {
        return body.to_string();
    };
    let end = after + rel + ARTIFACT_MARKER_END.len();
    format!("{}{}", &body[..start], &body[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{PullRequest, Release};

    fn artifact() -> Artifact {
        Artifact {
            schema_version: 1,
            changed: true,
            version: Some("1.4.0".into()),
            tag: Some("v1.4.0".into()),
            release: Some(Release {
                name: Some("Release v1.4.0".into()),
                notes: Some("## What's Changed\n\n* Everything".into()),
                prerelease: Some(false),
                make_latest: None,
            }),
            pull_request: None,
        }
    }

    #[test]
    fn renders_title_from_the_artifact_root_context() {
        let cfg = PullRequestConfig::default();
        let r = render(&cfg, &artifact()).unwrap();
        assert_eq!(r.title, "Release 1.4.0");
    }

    #[test]
    fn tag_is_available_too() {
        let cfg = PullRequestConfig {
            title: "Ship {{ tag }}".into(),
            ..Default::default()
        };
        assert_eq!(render(&cfg, &artifact()).unwrap().title, "Ship v1.4.0");
    }

    #[test]
    fn nested_release_fields_are_addressable() {
        let cfg = PullRequestConfig {
            title: "{{ release.name }}".into(),
            ..Default::default()
        };
        assert_eq!(render(&cfg, &artifact()).unwrap().title, "Release v1.4.0");
    }

    #[test]
    fn body_is_header_notes_footer() {
        let cfg = PullRequestConfig {
            header: Some("Heads up.\n".into()),
            footer: Some("Bye.\n".into()),
            ..Default::default()
        };
        let r = render(&cfg, &artifact()).unwrap();
        assert_eq!(
            r.body,
            "Heads up.\n\n## What's Changed\n\n* Everything\n\nBye."
        );
    }

    #[test]
    fn omitted_header_and_footer_leave_no_blank_lines() {
        let r = render(&PullRequestConfig::default(), &artifact()).unwrap();
        assert_eq!(r.body, "## What's Changed\n\n* Everything");
        assert!(!r.body.starts_with('\n'));
    }

    #[test]
    fn empty_notes_do_not_produce_stray_separators() {
        let mut a = artifact();
        a.release = None;
        let cfg = PullRequestConfig {
            header: Some("Only this.".into()),
            ..Default::default()
        };
        assert_eq!(render(&cfg, &a).unwrap().body, "Only this.");
    }

    #[test]
    fn artifact_can_override_title_and_body() {
        let mut a = artifact();
        a.pull_request = Some(PullRequest {
            title: Some("Custom title".into()),
            body: Some("Custom body".into()),
            labels: vec![],
        });
        let cfg = PullRequestConfig {
            header: Some("ignored".into()),
            ..Default::default()
        };
        let r = render(&cfg, &a).unwrap();
        assert_eq!(r.title, "Custom title");
        assert_eq!(
            r.body, "Custom body",
            "an explicit body must skip header/footer assembly entirely"
        );
    }

    #[test]
    fn labels_merge_without_duplicates() {
        let mut a = artifact();
        a.pull_request = Some(PullRequest {
            labels: vec!["release".into(), "automated".into()],
            ..Default::default()
        });
        let cfg = PullRequestConfig {
            labels: vec!["release".into()],
            ..Default::default()
        };
        assert_eq!(render(&cfg, &a).unwrap().labels, ["release", "automated"]);
    }

    #[test]
    fn undefined_variable_help_names_the_actual_mistake() {
        let cfg = PullRequestConfig {
            title: "{{ release.version }}".into(),
            ..Default::default()
        };
        // MiniJinja renders undefined as empty by default, so this
        // succeeds — but the help text exists for strict failures and
        // must point at the right vocabulary.
        let help = template_help(&minijinja::Error::new(
            minijinja::ErrorKind::UndefinedError,
            "x",
        ));
        assert!(help.contains("not `{{ release.version }}`"), "{help}");
        let _ = render(&cfg, &artifact());
    }

    #[test]
    fn syntax_errors_are_reported_with_a_span() {
        let cfg = PullRequestConfig {
            title: "Release {{ version".into(),
            ..Default::default()
        };
        let e = render(&cfg, &artifact()).unwrap_err();
        assert_eq!(e.field, "pull_request.title");
        assert!(e.help.contains("syntax"), "{}", e.help);
    }

    // --- artifact embedding ---------------------------------------------

    #[test]
    fn artifact_round_trips_through_a_pr_body() {
        let a = artifact();
        let body = embed_artifact("## Release\n\nSome notes.", &a);
        assert_eq!(extract_artifact(&body).unwrap(), a);
    }

    #[test]
    fn embedding_replaces_a_previous_artifact() {
        let mut a = artifact();
        let body = embed_artifact("Notes.", &a);

        a.version = Some("2.0.0".into());
        a.tag = Some("v2.0.0".into());
        let updated = embed_artifact(&body, &a);

        assert_eq!(extract_artifact(&updated).unwrap().version(), Some("2.0.0"));
        assert_eq!(
            updated.matches(ARTIFACT_MARKER_START).count(),
            1,
            "re-running prepare must not stack artifact blocks"
        );
    }

    #[test]
    fn embedded_artifact_is_invisible_in_rendered_markdown() {
        let body = embed_artifact("Visible.", &artifact());
        assert!(body.starts_with("Visible."));
        assert!(
            body.contains("<!--") && body.contains("-->"),
            "the artifact must live in an HTML comment: {body}"
        );
    }

    #[test]
    fn notes_containing_a_comment_terminator_survive_the_round_trip() {
        // A changelog quotes commit subjects, and a subject can mention an
        // HTML comment. Unescaped, that `-->` closes the block the artifact
        // lives in: the JSON spills into the rendered body and comes back
        // truncated, which surfaces as a bogus "does not carry a release
        // artifact".
        let mut a = artifact();
        a.release.as_mut().unwrap().notes = Some("- Extract <!-- more --> summary".into());

        let body = embed_artifact("Visible.", &a);

        let block = &body[body.find(ARTIFACT_MARKER_START).unwrap()..];
        assert_eq!(
            block.matches(ARTIFACT_MARKER_END).count(),
            1,
            "the payload must not be able to close the comment it lives in: {block}"
        );

        assert_eq!(extract_artifact(&body).unwrap(), a);
        assert_eq!(strip_artifact(&body).trim(), "Visible.");
    }

    #[test]
    fn stripping_recovers_the_human_body() {
        let original = "## Release\n\nSome notes.";
        let body = embed_artifact(original, &artifact());
        assert_eq!(strip_artifact(&body).trim(), original);
    }

    #[test]
    fn extracting_from_a_body_without_an_artifact_returns_none() {
        assert!(extract_artifact("just a normal PR body").is_none());
        assert!(extract_artifact("").is_none());
    }

    #[test]
    fn malformed_embedded_artifact_is_ignored_rather_than_fatal() {
        // A human editing the PR body should never make gh-ship panic.
        assert!(extract_artifact("<!-- ship:artifact\nnot json\n-->").is_none());
        assert_eq!(
            strip_artifact("<!-- ship:artifact\nunterminated"),
            "<!-- ship:artifact\nunterminated"
        );
    }

    #[test]
    fn embedding_into_an_empty_body_produces_only_the_block() {
        let body = embed_artifact("", &artifact());
        assert!(body.starts_with(ARTIFACT_MARKER_START));
        assert!(extract_artifact(&body).is_some());
    }
}
