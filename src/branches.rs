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

    #[error("`{pattern}` matches many branches, but its release branch does not vary")]
    #[diagnostic(code(ship::branches::constant_glob_release_branch), help("{help}"))]
    ConstantGlobReleaseBranch {
        pattern: String,
        help: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("every branch this matches stages on one branch")]
        span: miette::SourceSpan,
    },

    #[error("`{left}` and `{right}` would both stage on `{release}`")]
    #[diagnostic(
        code(ship::branches::colliding_release_branches),
        help(
            "two release lines sharing a branch share a Release PR, so each prepare would \
             overwrite the other. Give them release branches that differ — `{{ match }}` in the \
             template is usually enough"
        )
    )]
    CollidingReleaseBranches {
        left: String,
        right: String,
        release: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("collides with an earlier line")]
        span: miette::SourceSpan,
    },

    #[error("`{left}` and `{right}` can match branches that stage on the same release branch")]
    #[diagnostic(code(ship::branches::colliding_globs), help("{help}"))]
    CollidingGlobs {
        left: String,
        right: String,
        help: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("collides with an earlier line")]
        span: miette::SourceSpan,
    },

    #[error("`{release}` is both a release branch and a base branch")]
    #[diagnostic(code(ship::branches::release_branch_is_base_branch), help("{help}"))]
    ReleaseBranchIsBaseBranch {
        release: String,
        help: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("here")]
        span: miette::SourceSpan,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    Template(#[from] TemplateError),
}

/// Probe branch names used to decide whether a template varies.
///
/// They must share no prefix, no suffix and no length: `{{ match[0] }}`
/// and `{{ match | truncate(4) }}` are legitimate templates that would
/// render identically for two probes that happen to look alike, and be
/// wrongly condemned as constant.
const PROBE_A: &str = "aaaa";
const PROBE_B: &str = "zz";

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
        .find(|(_, rule)| !rule.is_pattern() && rule.branch == base)
        .map(|(i, _)| (i, base))
        .or_else(|| {
            entries
                .iter()
                .enumerate()
                .filter(|(_, rule)| rule.is_pattern())
                .find_map(|(i, rule)| glob_match(&rule.branch, base).map(|c| (i, c)))
        });

    let Some((index, capture)) = matched else {
        return Err(BranchError::NoMatch {
            base: base.to_string(),
            help: no_match_help(config, base),
        });
    };

    Ok(Line {
        base: base.to_string(),
        release: render_release_branch(
            config.release_branch_template_for(Some(index)),
            base,
            capture,
        )?,
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
        release: render_release_branch(config.release_branch_template(), base, base)?,
        entry: None,
    })
}

/// Config checks that need the template engine.
///
/// These live here rather than in [`crate::config::Config::check`] so
/// that the config module stays free of MiniJinja. `gh ship validate`
/// calls this, which is what turns a typo in `release_branch` into a
/// setup-time failure instead of a mid-release surprise.
///
/// Everything below guards one property: **two release lines must never
/// stage on the same branch**. They would share a head branch, and so a
/// Release PR, and each prepare would silently overwrite the other's.
pub fn check(config: &Config) -> Result<(), BranchError> {
    // Render the top-level template unconditionally, even when every
    // line overrides it. Otherwise a syntax error there lies dormant
    // until someone adds a line without an override, and then it
    // surfaces mid-release.
    render_release_branch(config.release_branch_template(), PROBE_A, PROBE_A)?;

    let mut exact: Vec<(&str, String)> = Vec::new();
    let selectors = config.selectors();

    for (i, rule) in config.branches().iter().enumerate() {
        let template = config.release_branch_template_for(Some(i));

        if rule.is_pattern() {
            // A glob matches many branches, so its release branch has to
            // tell them apart. Probe with two branches the glob really
            // matches: `branch` and `match` are not independent for a
            // glob — `release/*` matching `release/1.x` fixes the
            // capture at `1.x` — so varying one without the other would
            // condemn the perfectly sound `next/{{ branch }}`.
            let a = render_release_branch(template, &probe_branch(rule, PROBE_A), PROBE_A)?;
            let b = render_release_branch(template, &probe_branch(rule, PROBE_B), PROBE_B)?;
            if a == b {
                return Err(constant_glob(config, rule, template));
            }
            continue;
        }

        let name = render_release_branch(template, &rule.branch, &rule.branch)?;

        // A release branch that is also a base branch would have gh-ship
        // open a pull request from a branch into itself.
        if let Some(base) = selectors.iter().find(|s| **s == name) {
            return Err(release_branch_is_base(config, rule, &name, base));
        }

        if let Some((other, _)) = exact.iter().find(|(_, n)| *n == name) {
            return Err(collision(config, &rule.branch, other, &name));
        }
        exact.push((&rule.branch, name));
    }

    check_glob_pairs(config)
}

/// Two globs collide when their release branches ignore what
/// distinguishes them: `release/1.x` and `v1.x` both capture `1.x`, so
/// `next/{{ match }}` sends both lines to `next/1.x`.
///
/// Probe each with the *same* capture on a branch of its own shape. A
/// template that uses `{{ branch }}` then comes out different, because
/// the two globs have different literal parts, and is left alone.
fn check_glob_pairs(config: &Config) -> Result<(), BranchError> {
    let globs: Vec<(usize, &crate::config::BranchRule)> = config
        .branches()
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.is_pattern())
        .collect();

    for (n, (i, left)) in globs.iter().enumerate() {
        let a = render_release_branch(
            config.release_branch_template_for(Some(*i)),
            &probe_branch(left, PROBE_A),
            PROBE_A,
        )?;
        for (j, right) in globs.iter().skip(n + 1) {
            let b = render_release_branch(
                config.release_branch_template_for(Some(*j)),
                &probe_branch(right, PROBE_A),
                PROBE_A,
            )?;
            if a == b {
                return Err(BranchError::CollidingGlobs {
                    left: left.branch.clone(),
                    right: right.branch.clone(),
                    help: format!(
                        "both render their release branch from `{{{{ match }}}}` alone, so any \
                         capture the two share — `{}` and `{}` both matching `1.x`, say — sends \
                         them to one branch. Include `{{{{ branch }}}}`, or give one line its own \
                         `release_branch`",
                        left.branch, right.branch
                    ),
                    src: config.source.named(),
                    span: config.source.locate(&right.branch),
                });
            }
        }
    }
    Ok(())
}

/// A branch the glob really matches, with `capture` in the `*` position.
fn probe_branch(rule: &crate::config::BranchRule, capture: &str) -> String {
    rule.branch.replacen('*', capture, 1)
}

fn constant_glob(config: &Config, rule: &crate::config::BranchRule, template: &str) -> BranchError {
    BranchError::ConstantGlobReleaseBranch {
        pattern: rule.branch.clone(),
        help: format!(
            "`{}` renders `{template}` for every branch `{}` matches, so they would all stage on \
             one branch. Use `{{{{ match }}}}` — what the `*` captured — as in \
             `next/{{{{ match }}}}`",
            template, rule.branch
        ),
        src: config.source.named(),
        span: config.source.locate(&rule.branch),
    }
}

fn release_branch_is_base(
    config: &Config,
    rule: &crate::config::BranchRule,
    name: &str,
    base: &str,
) -> BranchError {
    BranchError::ReleaseBranchIsBaseBranch {
        release: name.to_string(),
        help: if base == rule.branch {
            format!(
                "`{}` would stage on itself, so the Release PR would have the same branch on \
                 both sides. Give the line a distinct release branch, e.g. `next/{}`",
                rule.branch, rule.branch
            )
        } else {
            format!(
                "`{name}` is the base branch of another release line, so the two would fight \
                 over it. Give this line a distinct release branch"
            )
        },
        src: config.source.named(),
        span: config.source.locate(&rule.branch),
    }
}

fn collision(config: &Config, left: &str, right: &str, name: &str) -> BranchError {
    BranchError::CollidingReleaseBranches {
        left: left.to_string(),
        right: right.to_string(),
        release: name.to_string(),
        src: config.source.named(),
        span: config.source.locate(right),
    }
}

fn render_release_branch(
    template: &str,
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
        template,
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

fn no_match_help(config: &Config, base: &str) -> String {
    let selectors = config.selectors();
    let listed = selectors
        .iter()
        .map(|e| format!("`{e}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut help = format!(
        "`branches` lists {listed}. Add `{base}` to it, or pass `--base` to release from a \
         branch that is listed"
    );
    if let Some(closest) = suggest::suggest(base, &selectors) {
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

    /// A config whose `branches` block is written out in full, for the
    /// mapping form.
    fn config_yaml(branches: &str, release_branch: &str) -> Config {
        let text = format!(
            "version: 1\nrelease_branch: \"{release_branch}\"\nbranches:\n{branches}workflows:\n  prepare: x\n"
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
    fn an_override_wins_over_the_top_level_template() {
        let c = config_yaml(
            "  - branch: main\n    release_branch: next/release\n  - \"release/*\"\n",
            "next/{{ match }}",
        );
        assert_eq!(resolve(&c, "main").unwrap().release, "next/release");
        assert_eq!(
            resolve(&c, "release/1.x").unwrap().release,
            "next/1.x",
            "a line without an override still falls back"
        );
    }

    #[test]
    fn an_override_is_a_template_too() {
        let c = config_yaml(
            "  - branch: \"release/*\"\n    release_branch: \"maint/{{ match }}\"\n",
            "next/{{ match }}",
        );
        assert_eq!(resolve(&c, "release/1.x").unwrap().release, "maint/1.x");
    }

    #[test]
    fn a_glob_whose_release_branch_cannot_vary_is_rejected() {
        // Every maintenance branch would stage on `release/next`. This
        // holds even as the only entry, which is why it is checked per
        // entry rather than only when several are configured.
        let e = check(&config("[\"release/*\"]", "release/next")).unwrap_err();
        assert!(
            matches!(e, BranchError::ConstantGlobReleaseBranch { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn a_glob_may_index_its_capture() {
        // The probes must share no prefix, suffix or length, or this
        // legitimate template looks constant and is condemned.
        check(&config("[\"release/*\"]", "next/{{ match[0] }}"))
            .expect("`{{ match[0] }}` varies with the capture");
        check(&config("[\"release/*\"]", "next/{{ match[:2] }}"))
            .expect("so does a sliced capture");
    }

    #[test]
    fn two_globs_that_can_produce_one_name_are_rejected() {
        // `release/1.x` and `v1.x` both capture `1.x`.
        let e = check(&config("[\"release/*\", \"v*\"]", "next/{{ match }}")).unwrap_err();
        assert!(matches!(e, BranchError::CollidingGlobs { .. }), "{e:?}");
    }

    #[test]
    fn two_globs_distinguished_by_their_templates_pass() {
        check(&config_yaml(
            "  - branch: \"release/*\"\n  - branch: \"v*\"\n    release_branch: \"tags/{{ match }}\"\n",
            "next/{{ match }}",
        ))
        .expect("distinct templates cannot collide");
    }

    #[test]
    fn two_exact_lines_with_one_release_branch_are_rejected() {
        let e = check(&config_yaml(
            "  - branch: main\n    release_branch: next/release\n  - branch: develop\n    release_branch: next/release\n",
            "release/next",
        ))
        .unwrap_err();
        assert!(
            matches!(e, BranchError::CollidingReleaseBranches { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn a_release_branch_that_is_its_own_base_is_rejected() {
        // Would open a pull request from `main` into `main`.
        let e = check(&config_yaml(
            "  - branch: main\n    release_branch: main\n",
            "release/next",
        ))
        .unwrap_err();
        assert!(
            matches!(e, BranchError::ReleaseBranchIsBaseBranch { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn a_release_branch_that_is_another_lines_base_is_rejected() {
        let e = check(&config_yaml(
            "  - branch: main\n    release_branch: develop\n  - branch: develop\n",
            "next/{{ match }}",
        ))
        .unwrap_err();
        assert!(
            matches!(e, BranchError::ReleaseBranchIsBaseBranch { .. }),
            "{e:?}"
        );
    }

    #[test]
    fn a_constant_release_branch_is_fine_for_one_exact_line() {
        let c = config("[main]", "release/next");
        check(&c).expect("one line cannot collide with itself");
    }

    #[test]
    fn every_line_overriding_leaves_the_top_level_template_unused_but_checked() {
        // The top-level template is rendered even when nothing falls
        // back to it, so a syntax error there cannot lie dormant until
        // someone adds a line without an override.
        let e = check(&config_yaml(
            "  - branch: main\n    release_branch: next/main\n",
            "next/{{ match ",
        ))
        .unwrap_err();
        assert!(matches!(e, BranchError::Template(_)), "{e:?}");
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
