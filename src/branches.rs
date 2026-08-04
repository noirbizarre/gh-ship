//! Release lines — mapping a base branch to its release branch.
//!
//! A project maintaining `release/1.x` while `main` moves on has several
//! releases in flight at once. Rather than duplicating the whole
//! configuration per line, `branches` lists the base branches gh-ship
//! releases from and `release_branch` becomes a template rendered once
//! per line.
//!
//! This module resolves one base branch to one [`Line`]. It sits above
//! [`crate::config`] and beside [`crate::render`] because it needs both:
//! config alone cannot render, and render must not learn about branches.

use std::collections::BTreeMap;

use miette::Diagnostic;
use thiserror::Error;

use crate::config::Config;
use crate::render::{self, TemplateError};
use crate::suggest;

/// A resolved release line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The branch the Release PR targets.
    pub base: String,
    /// The branch the release is staged on.
    pub release: String,
    /// Index of the `branches` entry that matched, for diagnostics.
    /// `None` when no entries are configured.
    pub entry: Option<usize>,
}

/// Everything that can go wrong resolving a release line.
#[derive(Debug, Error, Diagnostic)]
pub enum BranchError {
    #[error("no branch rule matches `{base}`")]
    #[diagnostic(code(ship::branches::no_match), help("{help}"))]
    NoMatch { base: String, help: String },

    #[error(
        "`release_branch` is the same for every release line, so they would collide on one branch"
    )]
    #[diagnostic(code(ship::branches::ambiguous_release_branch), help("{help}"))]
    AmbiguousReleaseBranch {
        help: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("does not vary per line")]
        span: miette::SourceSpan,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Template(#[from] TemplateError),
}

/// Whether an entry is a glob rather than an exact branch name.
pub fn is_pattern(entry: &str) -> bool {
    entry.contains('*')
}

/// Match `input` against a glob containing exactly one `*`.
///
/// Returns what the `*` captured. `*` matches `/` — the globs here name
/// branches, and `release/*` should reach `release/1/x` as readily as
/// `release/1.x`. Making `*` stop at `/` would break `v*` against
/// `v1/x`, which is the worse surprise.
pub fn glob_match<'a>(pattern: &str, input: &'a str) -> Option<&'a str> {
    let (prefix, suffix) = pattern.split_once('*')?;
    // `*` must capture something. Without this, `release/*` matches a
    // bare `release/` with an empty capture, and `next/{{ match }}`
    // resolves to a release branch named after nothing. It also stops
    // the prefix and suffix from overlapping on a short input.
    if input.len() <= prefix.len() + suffix.len() {
        return None;
    }
    input.strip_prefix(prefix)?.strip_suffix(suffix)
}

/// Resolve `base` against the configured release lines.
///
/// Exact entries are tried before globs, whatever their order in the
/// file: writing `main` alongside `*` means "main is special", and
/// making that depend on line order would punish a reasonable config for
/// its layout. Globs are then tried in declaration order, first match
/// winning, so the file itself is the precedence table.
pub fn resolve(config: &Config, base: &str) -> Result<Line, BranchError> {
    let entries = config.branches();

    let matched = entries
        .iter()
        .enumerate()
        .find(|(_, e)| !is_pattern(e) && e.as_str() == base)
        .map(|(i, _)| (i, base))
        .or_else(|| {
            entries
                .iter()
                .enumerate()
                .filter(|(_, e)| is_pattern(e))
                .find_map(|(i, e)| glob_match(e, base).map(|c| (i, c)))
        });

    let Some((index, capture)) = matched else {
        return Err(BranchError::NoMatch {
            base: base.to_string(),
            help: no_match_help(entries, base),
        });
    };

    Ok(Line {
        base: base.to_string(),
        release: render_release_branch(config, base, capture)?,
        entry: Some(index),
    })
}

/// The single release line of a config without `branches`.
///
/// `match` is the branch itself here, so one template works whether or
/// not release lines are configured.
pub fn single(config: &Config, base: &str) -> Result<Line, BranchError> {
    Ok(Line {
        base: base.to_string(),
        release: render_release_branch(config, base, base)?,
        entry: None,
    })
}

/// Config checks that need the template engine.
///
/// These live here rather than in [`crate::config::Config::check`] so
/// that the config module stays free of MiniJinja. `gh ship validate`
/// calls this, which is what turns a typo in `release_branch` into a
/// setup-time failure instead of a mid-release surprise.
pub fn check(config: &Config) -> Result<(), BranchError> {
    // Compile-and-render against a probe: a template that cannot render
    // for any branch cannot render for a real one either.
    let probe = render_release_branch(config, "probe-a", "probe-a")?;

    // With several lines a constant `release_branch` puts every line on
    // the same head branch, where they would silently overwrite each
    // other's Release PR. Detect it by rendering twice rather than by
    // looking for `{{ branch }}` in the text, so `{{ branch | lower }}`
    // and friends are covered too.
    if config.branches().len() >= 2 {
        let other = render_release_branch(config, "probe-b", "probe-b")?;
        if probe == other {
            return Err(BranchError::AmbiguousReleaseBranch {
                help: format!(
                    "`{}` renders the same for every base branch. With several release lines it \
                     must vary — use `{{{{ match }}}}` (what a `*` captured, or the branch itself \
                     for an exact entry), e.g. `next/{{{{ match }}}}`",
                    config.release_branch_template()
                ),
                src: config.source.named(),
                span: config.source.locate("release_branch:"),
            });
        }
    }

    Ok(())
}

fn render_release_branch(
    config: &Config,
    branch: &str,
    capture: &str,
) -> Result<String, BranchError> {
    // A map rather than `minijinja::context!`: `match` is a Rust
    // keyword, and `r#match =>` reads like a mistake somebody would
    // helpfully "fix".
    let vars = BTreeMap::from([("branch", branch), ("match", capture)]);
    let value = minijinja::Value::from_serialize(&vars);
    Ok(render::render_template_with_help(
        "release_branch",
        config.release_branch_template(),
        &value,
        branch_template_help,
    )?)
}

/// Guidance for `release_branch`, whose context is the branch rather
/// than the release artifact.
fn branch_template_help(err: &minijinja::Error) -> String {
    use minijinja::ErrorKind;
    match err.kind() {
        ErrorKind::UndefinedError => {
            "the base branch is the context here: use `{{ branch }}` for the whole name, or \
             `{{ match }}` for what a `*` in the `branches` entry captured"
                .to_string()
        }
        ErrorKind::SyntaxError => {
            "check the template syntax; `{{ }}` interpolates and `{% %}` controls flow".to_string()
        }
        _ => "see https://noirbizarre.github.io/gh-ship/configuration/#release-lines".to_string(),
    }
}

fn no_match_help(entries: &[String], base: &str) -> String {
    let listed = entries
        .iter()
        .map(|e| format!("`{e}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut help = format!(
        "`branches` lists {listed}. Add `{base}` to it, or pass `--base` to release from a \
         branch that is listed"
    );
    if let Some(closest) = suggest::suggest(base, entries) {
        help = format!("{help}\ndid you mean `{closest}`?");
    }
    help
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(branches: &str, release_branch: &str) -> Config {
        let text = format!(
            "version: 1\nbranches: {branches}\nrelease_branch: \"{release_branch}\"\nworkflows:\n  prepare: x\n"
        );
        Config::parse(".github/ship.yml", &text).expect("config parses")
    }

    #[test]
    fn glob_captures_the_varying_part() {
        assert_eq!(glob_match("release/*", "release/1.x"), Some("1.x"));
        assert_eq!(glob_match("*-maint", "1.x-maint"), Some("1.x"));
        assert_eq!(glob_match("v*.x", "v1.x"), Some("1"));
    }

    #[test]
    fn glob_spans_slashes() {
        assert_eq!(glob_match("release/*", "release/1/x"), Some("1/x"));
    }

    #[test]
    fn glob_requires_something_to_capture() {
        assert_eq!(glob_match("release/*", "release/"), None);
        assert_eq!(glob_match("release/*", "main"), None);
        assert_eq!(glob_match("release/*", "hotfix/1.x"), None);
    }

    #[test]
    fn exact_entry_beats_a_later_pattern() {
        let c = config("[\"release/*\", \"release/next\"]", "next/{{ match }}");
        let line = resolve(&c, "release/next").unwrap();
        assert_eq!(line.entry, Some(1), "the exact entry wins on merit");
        assert_eq!(
            line.release, "next/release/next",
            "an exact entry captures the whole branch"
        );
    }

    #[test]
    fn patterns_are_tried_in_declaration_order() {
        let c = config("[\"release/*\", \"*\"]", "next/{{ match }}");
        assert_eq!(resolve(&c, "release/1.x").unwrap().entry, Some(0));
        assert_eq!(resolve(&c, "anything").unwrap().entry, Some(1));
    }

    #[test]
    fn both_template_variables_render() {
        let c = config("[\"release/*\"]", "{{ branch }}+{{ match }}");
        assert_eq!(
            resolve(&c, "release/1.x").unwrap().release,
            "release/1.x+1.x"
        );
    }

    #[test]
    fn a_single_line_gets_the_branch_as_its_match() {
        let c = Config::parse(
            ".github/ship.yml",
            "version: 1\nrelease_branch: \"next/{{ match }}\"\nworkflows:\n  prepare: x\n",
        )
        .unwrap();
        let line = single(&c, "main").unwrap();
        assert_eq!(line.release, "next/main");
        assert_eq!(line.entry, None);
    }

    #[test]
    fn no_match_lists_the_configured_lines() {
        let c = config("[main, \"release/*\"]", "next/{{ match }}");
        let e = resolve(&c, "feature/x").unwrap_err();
        let BranchError::NoMatch { help, .. } = &e else {
            panic!("expected NoMatch, got {e:?}")
        };
        assert!(help.contains("`main`"), "{help}");
        assert!(help.contains("`release/*`"), "{help}");
    }

    #[test]
    fn no_match_suggests_a_near_miss() {
        let c = config("[main, develop]", "next/{{ match }}");
        let e = resolve(&c, "mian").unwrap_err();
        let BranchError::NoMatch { help, .. } = &e else {
            panic!("expected NoMatch, got {e:?}")
        };
        assert!(help.contains("did you mean `main`?"), "{help}");
    }

    #[test]
    fn a_constant_release_branch_is_rejected_for_several_lines() {
        let c = config("[main, \"release/*\"]", "release/next");
        let e = check(&c).unwrap_err();
        assert!(
            matches!(e, BranchError::AmbiguousReleaseBranch { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn a_constant_release_branch_is_fine_for_one_line() {
        let c = config("[main]", "release/next");
        check(&c).expect("one line cannot collide with itself");
    }

    #[test]
    fn a_varying_release_branch_passes() {
        check(&config("[main, \"release/*\"]", "next/{{ match }}")).unwrap();
        check(&config("[main, \"release/*\"]", "next/{{ branch }}")).unwrap();
    }

    #[test]
    fn a_broken_template_is_reported_as_a_template_error() {
        let c = config("[main]", "next/{{ match ");
        let e = check(&c).unwrap_err();
        assert!(matches!(e, BranchError::Template(_)), "{e:?}");
    }
}
