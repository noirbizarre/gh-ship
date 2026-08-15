//! The workflow templates `gh ship init` writes.
//!
//! Two templates, three ways to authenticate, six outputs. The variation
//! is expressed in the templates themselves rather than in Rust, because
//! the interesting part of these files is the hundred-odd lines that have
//! nothing to do with tokens, and keeping three copies of those would
//! drift within a release.
//!
//! They live in the library rather than next to [`crate::cli`]'s `init`
//! command so the integration tests can render every combination and put
//! it through the real `gh ship validate`. A template that parses but
//! violates the workflow contract is exactly the kind of thing `init`
//! must never write.

use crate::gh::workflow::Role;
use minijinja::syntax::SyntaxConfig;
use minijinja::{Environment, UndefinedBehavior, context};

/// The prepare template, in its unrendered form.
const PREPARE: &str = include_str!("../templates/prepare-release.yml.jinja");
/// The publish template, in its unrendered form.
const PUBLISH: &str = include_str!("../templates/publish-release.yml.jinja");

/// Which token the generated workflows authenticate with.
///
/// This is the one decision `init` cannot make on the user's behalf, and
/// the one whose consequences are least visible: pick
/// [`Default`](TokenStrategy::Default) and everything works except the
/// thing you wanted, which is CI running on the Release PR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenStrategy {
    /// Mint an installation token in the job with
    /// `actions/create-github-app-token`.
    App,
    /// A personal access token, stored as the `SHIP_TOKEN` secret.
    Pat,
    /// `GITHUB_TOKEN`, which cannot trigger other workflows.
    Default,
}

impl TokenStrategy {
    /// The value the templates branch on.
    fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Pat => "pat",
            Self::Default => "default",
        }
    }
}

/// The file name `init` writes a role's template to.
///
/// `.yml` is a choice, not a requirement: workflow discovery keys off the
/// file stem, so either extension yields the same slug.
pub fn filename(role: Role) -> &'static str {
    match role {
        Role::Prepare => "prepare-release.yml",
        Role::Publish => "publish-release.yml",
    }
}

/// The unrendered template backing a role.
fn source(role: Role) -> &'static str {
    match role {
        Role::Prepare => PREPARE,
        Role::Publish => PUBLISH,
    }
}

/// The environment the workflow templates are rendered in.
///
/// Three settings are load-bearing, and one of them is the whole reason
/// this is not a stock `Environment`.
fn environment() -> Environment<'static> {
    let mut env = Environment::new();

    // `{{` belongs to GitHub Actions here, not to us. A stock MiniJinja
    // would read `${{ vars.APP_CLIENT_ID }}` as `$` followed by an
    // expression and — undefined being lenient by default — render it as
    // a bare `$`, quietly producing a workflow that authenticates with
    // nothing.
    //
    // The templates interpolate nothing at all: every difference between
    // the strategies is a block selected by `{% if %}`. So these
    // delimiters are never typed by anyone. They exist to be unreachable,
    // which is what lets the templates carry authentic, copy-pasteable
    // Actions syntax.
    env.set_syntax(
        SyntaxConfig::builder()
            .variable_delimiters("{@", "@}")
            .build()
            .expect("the delimiters are a literal, valid pair"),
    );

    // A condition on a name that does not exist is a bug in a template we
    // ship. Fail the test suite rather than silently take the else branch.
    env.set_undefined_behavior(UndefinedBehavior::Strict);

    // Without these, every `{% if %}` leaves behind the newline and the
    // indentation of the line it occupied.
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    // MiniJinja drops one trailing newline by default; a workflow file
    // ends with one, and `end-of-file-fixer` would put it back on every
    // commit.
    env.set_keep_trailing_newline(true);

    env
}

/// Render a template for the chosen role and token strategy.
///
/// Panics if the template is malformed, which is a bug in gh-ship rather
/// than anything a user did: the sources are compiled in, and every
/// combination is rendered by the test suite, so a broken template cannot
/// reach a release.
pub fn render(role: Role, strategy: TokenStrategy) -> String {
    environment()
        .render_str(source(role), context! { token => strategy.as_str() })
        .expect("a shipped template must render")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::workflow;
    use std::path::Path;

    const STRATEGIES: [TokenStrategy; 3] = [
        TokenStrategy::App,
        TokenStrategy::Pat,
        TokenStrategy::Default,
    ];
    const ROLES: [Role; 2] = [Role::Prepare, Role::Publish];

    /// Every rendering must satisfy the contract the templates exist to
    /// teach. What `init` writes is the rendered form, so rendering is
    /// what has to be checked — if this fails, `init` generates workflows
    /// that `gh ship validate` immediately rejects.
    #[test]
    fn every_rendering_satisfies_the_contract() {
        for strategy in STRATEGIES {
            for role in ROLES {
                let rendered = render(role, strategy);
                let name = filename(role);
                let w = workflow::parse(Path::new(name), &rendered).unwrap_or_else(|| {
                    panic!("{name} must be parseable YAML for {strategy:?}:\n{rendered}")
                });
                assert_eq!(
                    w.contract_violations(),
                    vec![],
                    "{name} violates the gh-ship workflow contract for {strategy:?}"
                );
                assert!(w.callable, "{name} should also be reusable");
            }

            assert!(
                workflow::parse(
                    Path::new("prepare-release.yml"),
                    &render(Role::Prepare, strategy)
                )
                .unwrap()
                .accepts_dry_run(),
                "`gh ship preview` needs the prepare workflow to accept dry_run"
            );
        }
    }

    /// No rendering may leak the scaffolding that produced it.
    ///
    /// A surviving `{%` is the signature of a mistyped tag, and MiniJinja
    /// will not complain about one that is merely never closed the way
    /// the author intended.
    #[test]
    fn no_rendering_leaks_template_syntax() {
        for strategy in STRATEGIES {
            for role in ROLES {
                let rendered = render(role, strategy);
                for tag in ["{%", "%}", "{@", "@}", "{#", "#}"] {
                    assert!(
                        !rendered.contains(tag),
                        "{:?}/{strategy:?} leaked `{tag}`:\n{rendered}",
                        role
                    );
                }
            }
        }
    }

    /// The App variant is the one with a trap: an App token is a step
    /// output, so a job-level `env:` would expand to the empty string and
    /// fall back to `GITHUB_TOKEN` without failing.
    #[test]
    fn the_app_variant_authenticates_every_step() {
        for role in ROLES {
            let rendered = render(role, TokenStrategy::App);
            let name = filename(role);

            assert!(
                rendered.contains("actions/create-github-app-token@v3"),
                "{name} must mint a token:\n{rendered}"
            );
            assert!(
                rendered.contains("client-id: ${{ vars.APP_CLIENT_ID }}")
                    && rendered.contains("private-key: ${{ secrets.APP_PRIVATE_KEY }}"),
                "{name} must mint it from the credentials the docs tell users to \
                 create:\n{rendered}"
            );
            assert!(
                !rendered.contains("SHIP_TOKEN"),
                "{name} must not mention the PAT secret:\n{rendered}"
            );
            assert!(
                !rendered.contains("    env:\n      GH_TOKEN:"),
                "{name} must not set GH_TOKEN at the job level, where a step \
                 output does not resolve:\n{rendered}"
            );
            assert!(
                rendered.contains("GH_TOKEN: ${{ steps.app-token.outputs.token }}"),
                "{name} runs gh without the App token:\n{rendered}"
            );
        }

        // The commit identity is the App's own bot user, not a literal
        // copied from an example.
        let prepare = render(Role::Prepare, TokenStrategy::App);
        assert!(
            !prepare.contains("41898282+github-actions[bot]"),
            "the App variant must not commit as github-actions[bot]:\n{prepare}"
        );
        assert!(
            prepare.contains("steps.app-token.outputs.app-slug")
                && prepare.contains("steps.bot.outputs.id"),
            "the App variant must commit as its own bot user:\n{prepare}"
        );
    }

    /// The PAT variant is the only one that should name `SHIP_TOKEN`, and
    /// it has to explain why the secret is worth creating.
    #[test]
    fn the_pat_variant_explains_the_token_trap() {
        let prepare = render(Role::Prepare, TokenStrategy::Pat);
        assert!(prepare.contains("SHIP_TOKEN"), "{prepare}");
        assert!(
            prepare.contains("cannot trigger other workflows"),
            "{prepare}"
        );
    }

    /// The default variant must not advertise a secret the user declined
    /// to create.
    #[test]
    fn the_default_variant_mentions_no_secret() {
        for role in ROLES {
            let rendered = render(role, TokenStrategy::Default);
            assert!(!rendered.contains("SHIP_TOKEN"), "{rendered}");
            assert!(!rendered.contains("app-token"), "{rendered}");
            assert!(
                rendered.contains("GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}"),
                "{rendered}"
            );
        }
    }

    /// The protocol is what gh-ship actually reads, and it is the same
    /// under every strategy.
    #[test]
    fn every_prepare_rendering_uploads_the_protocol_artifact() {
        for strategy in STRATEGIES {
            let rendered = render(Role::Prepare, strategy);
            assert!(rendered.contains("name: ship-release"), "{rendered}");
            assert!(rendered.contains("ship.release.json"), "{rendered}");
            assert!(
                rendered.contains("gh ship validate ship.release.json"),
                "the template should validate before uploading:\n{rendered}"
            );
        }
    }

    /// gh-ship has already created the tag by the time publish runs, so
    /// the checkout must pin it rather than follow a branch that may have
    /// moved.
    #[test]
    fn every_publish_rendering_checks_out_the_tag() {
        for strategy in STRATEGIES {
            let rendered = render(Role::Publish, strategy);
            assert!(rendered.contains("ref: ${{ inputs.tag }}"), "{rendered}");
            assert!(rendered.contains("--clobber"), "{rendered}");
        }
    }
}
