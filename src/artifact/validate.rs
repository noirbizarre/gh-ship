//! Artifact validation with human-grade diagnostics.
//!
//! The pipeline is deliberately staged so each failure mode gets the
//! best possible message, rather than letting a later stage produce a
//! confusing one:
//!
//! 1. **Parse** — JSON syntax errors point at the offending byte.
//! 2. **Version pre-flight** — an unknown `schemaVersion` is reported as
//!    "upgrade gh-ship", not as a `const` violation buried in the schema.
//! 3. **Schema** — every violation becomes its own labelled snippet.
//! 4. **Deserialize** — cannot realistically fail after step 3, but is
//!    reported honestly if it does.

use std::path::Path;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use super::span::{self, Step};
use super::{Artifact, SCHEMA_VERSION};

/// A single schema violation, rendered as its own miette snippet.
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(ship::artifact::invalid))]
pub struct Issue {
    message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label("{label}")]
    span: SourceSpan,
    label: String,
    #[help]
    help: Option<String>,
}

/// Everything that can go wrong reading a `ship.release.json`.
#[derive(Debug, Error, Diagnostic)]
pub enum ArtifactError {
    #[error("failed to read `{path}`")]
    #[diagnostic(
        code(ship::artifact::io),
        help(
            "check that the workflow uploaded the `ship-release` artifact and that the path is correct"
        )
    )]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid JSON in `{path}`: {message}")]
    #[diagnostic(
        code(ship::artifact::json),
        help(
            "`ship.release.json` must be a single JSON object; check for trailing commas or unquoted keys"
        )
    )]
    Json {
        path: String,
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("unsupported artifact schema version: {found}")]
    #[diagnostic(code(ship::artifact::version))]
    UnsupportedVersion {
        found: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("this version is not supported")]
        span: SourceSpan,
        #[help]
        help: String,
    },

    #[error("`{path}` is not a valid release artifact ({n} problem{s})", n = issues.len(), s = if issues.len() == 1 { "" } else { "s" })]
    #[diagnostic(
        code(ship::artifact::invalid),
        help("see https://noirbizarre.github.io/gh-ship/specifications/release-artifact/")
    )]
    Invalid {
        path: String,
        #[related]
        issues: Vec<Issue>,
    },
}

impl ArtifactError {
    /// Render the error and all related issues as plain text.
    ///
    /// Used by unit tests and by `--json` output paths that need the
    /// message without miette's fancy renderer.
    pub fn render_plain(&self) -> String {
        let mut out = self.to_string();
        if let Self::Invalid { issues, .. } = self {
            for issue in issues {
                out.push_str("\n  - ");
                out.push_str(&issue.message);
                if let Some(h) = &issue.help {
                    out.push_str("\n    help: ");
                    out.push_str(h);
                }
            }
        }
        out
    }

    /// Number of distinct problems reported.
    pub fn problem_count(&self) -> usize {
        match self {
            Self::Invalid { issues, .. } => issues.len(),
            _ => 1,
        }
    }
}

/// Validate an artifact read from `path`.
pub fn validate_file(path: &Path) -> Result<Artifact, ArtifactError> {
    let display = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| ArtifactError::Io {
        path: display.clone(),
        source,
    })?;
    validate_str(&display, &text)
}

/// Validate artifact text that came from somewhere other than a file
/// (a PR body, a downloaded artifact, stdin).
pub fn validate_str(name: &str, text: &str) -> Result<Artifact, ArtifactError> {
    let src = || NamedSource::new(name, text.to_string()).with_language("json");

    // --- 1. Parse --------------------------------------------------------
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| ArtifactError::Json {
        path: name.to_string(),
        message: e.to_string(),
        src: src(),
        span: line_col_to_span(text, e.line(), e.column()),
    })?;

    // --- 2. Version pre-flight ------------------------------------------
    //
    // Done before schema validation so a future artifact produces
    // "upgrade gh-ship" rather than an opaque `const` failure.
    if let Some(found) = value.get("schemaVersion") {
        let known = found.as_u64().is_some_and(|v| v == SCHEMA_VERSION as u64);
        if !known {
            let help = match found.as_u64() {
                Some(v) if v > SCHEMA_VERSION as u64 => format!(
                    "this artifact speaks protocol v{v}, but this gh-ship only understands v{SCHEMA_VERSION}. Upgrade gh-ship: `gh extension upgrade ship`"
                ),
                Some(v) => format!(
                    "protocol v{v} is no longer supported; regenerate the artifact with `schemaVersion: {SCHEMA_VERSION}`"
                ),
                None => format!(
                    "`schemaVersion` must be the integer {SCHEMA_VERSION}, not {}",
                    type_name(found)
                ),
            };
            return Err(ArtifactError::UnsupportedVersion {
                found: compact(found),
                src: src(),
                span: span::locate(text, &[Step::Prop("schemaVersion".into())])
                    .unwrap_or_else(|| SourceSpan::from((0, 0))),
                help,
            });
        }
    }

    // --- 3. Schema -------------------------------------------------------
    if let Err(err) = super::schema::validate(&value) {
        let mut issues = Vec::new();
        collect(&err, text, name, &mut issues);
        // A tree that yielded no leaves still means "invalid"; surface
        // the root rather than silently passing.
        if issues.is_empty() {
            issues.push(Issue {
                message: err.kind.to_string(),
                src: src(),
                span: SourceSpan::from((0, 0)),
                label: "here".into(),
                help: None,
            });
        }
        return Err(ArtifactError::Invalid {
            path: name.to_string(),
            issues,
        });
    }

    // --- 4. Deserialize --------------------------------------------------
    serde_json::from_value(value).map_err(|e| ArtifactError::Json {
        path: name.to_string(),
        message: e.to_string(),
        src: src(),
        span: SourceSpan::from((0, 0)),
    })
}

/// Walk boon's error tree and turn every leaf into an [`Issue`].
fn collect(err: &boon::ValidationError, text: &str, name: &str, out: &mut Vec<Issue>) {
    if !err.causes.is_empty() {
        for cause in &err.causes {
            collect(cause, text, name, out);
        }
        return;
    }

    let pointer = err.instance_location.to_string();
    let path = span::parse_pointer(&pointer);
    let (message, label, help, span) = describe(&err.kind, &pointer, &path, text);

    out.push(Issue {
        message,
        src: NamedSource::new(name, text.to_string()).with_language("json"),
        span: span.unwrap_or_else(|| {
            span::locate(text, &path).unwrap_or_else(|| SourceSpan::from((0, 0)))
        }),
        label,
        help,
    });
}

/// Translate a boon error kind into (message, label, help, span override).
///
/// The goal is that every message says *what* is wrong and every help
/// says *what to do about it*, in the vocabulary of the artifact spec
/// rather than of JSON Schema.
fn describe(
    kind: &boon::ErrorKind,
    pointer: &str,
    path: &[Step],
    text: &str,
) -> (String, String, Option<String>, Option<SourceSpan>) {
    let at = if pointer.is_empty() {
        "the artifact".to_string()
    } else {
        format!("`{pointer}`")
    };

    match kind {
        boon::ErrorKind::Required { want } => {
            let missing: Vec<String> = want.iter().map(|w| format!("`{w}`")).collect();
            let list = join(&missing);
            let help = required_help(want);
            (
                format!("{at} is missing {list}"),
                format!("missing {list}"),
                help,
                None,
            )
        }

        boon::ErrorKind::AdditionalProperties { got } => {
            let names: Vec<String> = got.iter().map(|g| g.to_string()).collect();
            let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
            let known = known_siblings(pointer);
            let help = names
                .first()
                .and_then(|n| crate::suggest::did_you_mean(n, &known))
                .or_else(|| {
                    Some(
                        "unknown fields are rejected so typos cannot silently do nothing; \
                         prefix a field with `x-` to carry your own metadata"
                            .to_string(),
                    )
                });
            // Point at the offending key, not the whole object.
            let span = names.first().and_then(|n| span::locate_key(text, path, n));
            (
                format!(
                    "{at} has unknown field{} {}",
                    plural(names.len()),
                    join(&quoted)
                ),
                "not allowed here".into(),
                help,
                span,
            )
        }

        boon::ErrorKind::Type { got, want } => {
            let want_list: Vec<String> = want.iter().map(|t| t.to_string()).collect();
            (
                format!("{at} must be {}, but is {got}", join_or(&want_list)),
                format!("expected {}", join_or(&want_list)),
                None,
                None,
            )
        }

        boon::ErrorKind::Const { want } => (
            format!("{at} must be {}", compact(want)),
            format!("expected {}", compact(want)),
            None,
            None,
        ),

        boon::ErrorKind::MinLength { want, .. } => (
            format!("{at} must not be empty"),
            "empty".into(),
            (*want == 1).then(|| "supply a value, or omit the field entirely".to_string()),
            None,
        ),

        boon::ErrorKind::Pattern { got, .. } => {
            let help = if pointer == "/tag" {
                Some("a git tag cannot contain whitespace".to_string())
            } else if pointer == "/version" {
                Some("`version` must not have leading or trailing whitespace".to_string())
            } else {
                None
            };
            (
                format!("{at} has an invalid value: {}", compact_str(got)),
                "invalid format".into(),
                help,
                None,
            )
        }

        boon::ErrorKind::UniqueItems { got } => (
            format!(
                "{at} contains duplicate entries (items {} and {})",
                got[0], got[1]
            ),
            "duplicated".into(),
            None,
            None,
        ),

        other => (format!("{at}: {other}"), "here".into(), None, None),
    }
}

/// Context-aware guidance for missing required properties.
///
/// The `changed`/`version`/`tag` interaction is the single most common
/// authoring mistake, so it gets a bespoke explanation.
fn required_help(want: &[&str]) -> Option<String> {
    let needs_release = want.contains(&"version") || want.contains(&"tag");
    if needs_release {
        return Some(
            "`changed: true` promises there is something to release, so `version` and `tag` \
             are required. If there is nothing to release, set `changed: false` and omit them."
                .to_string(),
        );
    }
    if want.contains(&"schemaVersion") {
        return Some(format!(
            "every artifact must declare `\"schemaVersion\": {SCHEMA_VERSION}`"
        ));
    }
    if want.contains(&"changed") {
        return Some(
            "`changed` tells gh-ship whether to proceed; it is required even when true".to_string(),
        );
    }
    None
}

/// Field names valid at a given pointer, used for "did you mean?".
fn known_siblings(pointer: &str) -> Vec<&'static str> {
    match pointer {
        "" => vec![
            "$schema",
            "schemaVersion",
            "changed",
            "version",
            "tag",
            "release",
            "pull_request",
        ],
        "/release" => vec!["name", "notes", "prerelease", "make_latest"],
        "/pull_request" => vec!["title", "body", "labels"],
        _ => vec![],
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn join(items: &[String]) -> String {
    join_with(items, "and")
}

fn join_or(items: &[String]) -> String {
    join_with(items, "or")
}

fn join_with(items: &[String], conj: &str) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} {conj} {b}"),
        [rest @ .., last] => format!("{}, {conj} {last}", rest.join(", ")),
    }
}

fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Render a value compactly, truncating long strings so a 40 KB
/// changelog never floods the terminal.
fn compact(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => compact_str(s),
        other => other.to_string(),
    }
}

fn compact_str(s: &str) -> String {
    const MAX: usize = 40;
    if s.chars().count() <= MAX {
        format!("{s:?}")
    } else {
        let head: String = s.chars().take(MAX).collect();
        format!("{head:?}…")
    }
}

/// Convert serde_json's 1-based line/column into a byte offset span.
fn line_col_to_span(text: &str, line: usize, column: usize) -> SourceSpan {
    if line == 0 {
        return SourceSpan::from((0, 0));
    }
    let mut offset = 0usize;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            let col = column.saturating_sub(1).min(l.len());
            return SourceSpan::from((offset + col, 1));
        }
        offset += l.len();
    }
    SourceSpan::from((text.len().saturating_sub(1), 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> ArtifactError {
        validate_str("ship.release.json", text).unwrap_err()
    }

    #[test]
    fn accepts_minimal_unchanged_artifact() {
        let a = validate_str("t.json", r#"{"schemaVersion":1,"changed":false}"#).unwrap();
        assert!(!a.changed);
    }

    #[test]
    fn accepts_full_changed_artifact() {
        let a = validate_str(
            "t.json",
            r###"{
                "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
                "schemaVersion": 1,
                "changed": true,
                "version": "1.4.0",
                "tag": "v1.4.0",
                "release": {"name": "Release v1.4.0", "notes": "## Notes", "prerelease": false},
                "pull_request": {"title": "Release v1.4.0", "labels": ["release"]}
            }"###,
        )
        .unwrap();
        assert_eq!(a.version(), Some("1.4.0"));
        assert_eq!(a.tag(), Some("v1.4.0"));
    }

    #[test]
    fn allows_x_extensions_everywhere() {
        validate_str(
            "t.json",
            r#"{"schemaVersion":1,"changed":true,"version":"1","tag":"v1",
                "x-tool":"my-tool","release":{"x-internal":42}}"#,
        )
        .expect("x- prefixed fields are reserved for third-party tools");
    }

    #[test]
    fn rejects_syntax_error_with_position() {
        let e = err("{\"schemaVersion\": 1,\n \"changed\": tru}");
        assert!(matches!(e, ArtifactError::Json { .. }), "got {e:?}");
    }

    #[test]
    fn rejects_future_schema_version_with_upgrade_hint() {
        let e = err(r#"{"schemaVersion":2,"changed":false}"#);
        let ArtifactError::UnsupportedVersion { help, found, .. } = &e else {
            panic!("expected UnsupportedVersion, got {e:?}");
        };
        assert_eq!(found, "2");
        assert!(help.contains("Upgrade gh-ship"), "{help}");
    }

    #[test]
    fn rejects_non_integer_schema_version() {
        let e = err(r#"{"schemaVersion":"1","changed":false}"#);
        assert!(
            matches!(e, ArtifactError::UnsupportedVersion { .. }),
            "got {e:?}"
        );
    }

    #[test]
    fn requires_version_and_tag_when_changed() {
        let e = err(r#"{"schemaVersion":1,"changed":true}"#);
        let rendered = e.render_plain();
        assert!(rendered.contains("`version`"), "{rendered}");
        assert!(rendered.contains("`tag`"), "{rendered}");
        assert!(
            rendered.contains("set `changed: false`"),
            "help must explain the changed/version relationship: {rendered}"
        );
    }

    #[test]
    fn does_not_require_version_when_unchanged() {
        validate_str("t.json", r#"{"schemaVersion":1,"changed":false}"#).unwrap();
    }

    #[test]
    fn reports_missing_required_root_fields() {
        let e = err("{}");
        let r = e.render_plain();
        assert!(r.contains("schemaVersion"), "{r}");
        assert!(r.contains("changed"), "{r}");
    }

    #[test]
    fn rejects_unknown_field_and_suggests_correction() {
        let e = err(r#"{"schemaVersion":1,"changed":false,"tags":"v1"}"#);
        let r = e.render_plain();
        assert!(r.contains("unknown field"), "{r}");
        assert!(
            r.contains("did you mean `tag`?"),
            "typo suggestion missing: {r}"
        );
    }

    #[test]
    fn unknown_field_without_close_match_explains_x_prefix() {
        let e = err(r#"{"schemaVersion":1,"changed":false,"wibble":1}"#);
        let r = e.render_plain();
        assert!(r.contains("x-"), "{r}");
    }

    #[test]
    fn rejects_wrong_types_with_readable_message() {
        let e = err(r#"{"schemaVersion":1,"changed":"yes"}"#);
        let r = e.render_plain();
        assert!(r.contains("must be boolean"), "{r}");
        assert!(r.contains("but is string"), "{r}");
    }

    #[test]
    fn rejects_tag_with_whitespace() {
        let e = err(r#"{"schemaVersion":1,"changed":true,"version":"1.0","tag":"v1 0"}"#);
        let r = e.render_plain();
        assert!(r.contains("cannot contain whitespace"), "{r}");
    }

    #[test]
    fn rejects_empty_version() {
        let e = err(r#"{"schemaVersion":1,"changed":true,"version":"","tag":"v1"}"#);
        let r = e.render_plain();
        assert!(r.contains("must not be empty"), "{r}");
    }

    #[test]
    fn rejects_duplicate_labels() {
        let e = err(r#"{"schemaVersion":1,"changed":false,"pull_request":{"labels":["a","a"]}}"#);
        let r = e.render_plain();
        assert!(r.contains("duplicate"), "{r}");
    }

    #[test]
    fn collects_multiple_problems_at_once() {
        let e = err(r#"{"schemaVersion":1,"changed":true,"bogus":1}"#);
        assert!(
            e.problem_count() >= 2,
            "validation must not stop at the first error: {}",
            e.render_plain()
        );
    }

    #[test]
    fn long_strings_are_truncated_in_messages() {
        let long = "x".repeat(500);
        let e = err(&format!(
            r#"{{"schemaVersion":1,"changed":true,"version":"1.0","tag":"{long} y"}}"#
        ));
        let r = e.render_plain();
        assert!(
            r.len() < 400,
            "a huge value must not flood the message: {}",
            r.len()
        );
    }

    #[test]
    fn line_col_maps_to_offset() {
        let text = "abc\ndefg\nhi";
        let s = line_col_to_span(text, 2, 3);
        assert_eq!(&text[s.offset()..s.offset() + s.len()], "f");
    }
}
