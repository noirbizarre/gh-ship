//! `gh ship init` — make a repository gh-ship enabled.
//!
//! The goal is under a minute, and the hard part is not writing YAML: it
//! is that the two most likely setup mistakes are invisible until a
//! release is already in flight.
//!
//! 1. Naming a `workflow_call`-only workflow, which the API cannot start.
//! 2. Omitting the `dry_run` input the prepare workflow is previewed with,
//!    which GitHub rejects the dispatch over.
//!
//! So `init` only ever offers workflows that satisfy the contract, and
//! explains — rather than silently hides — the ones that do not.

use std::path::Path;

use demand::{Confirm, DemandOption, Select};
use miette::{Diagnostic, IntoDiagnostic, Result};
use thiserror::Error;

use gh_ship::cli::{Cli, InitArgs};
use gh_ship::config::{CONFIG_VERSION, DEFAULT_PR_TITLE, DEFAULT_RELEASE_BRANCH};
use gh_ship::gh::workflow::{self, Workflow};
use gh_ship::logger;
use gh_ship::style::Theme;
use gh_ship::templates::{self, Role, TokenStrategy};

use super::repo_root;

/// Everything that can stop `init`.
///
/// Enumerated rather than raised ad hoc, so the whole `ship::init::*` code
/// namespace is visible in one place — the same reason [`super::release::
/// ReleaseError`] exists.
#[derive(Debug, Error, Diagnostic)]
pub enum InitError {
    #[error("{path} already exists")]
    #[diagnostic(
        code(ship::init::exists),
        help("pass `--force` to overwrite it, or edit it directly")
    )]
    Exists { path: String },

    /// `gh ship init` is the one interactive command. Every other command is
    /// scriptable, so this never blocks automation — and a CI job that
    /// reaches it has a configuration bug, not a terminal problem.
    #[error("`gh ship init` requires an interactive terminal")]
    #[diagnostic(
        code(ship::init::not_interactive),
        help(
            "`init` asks which workflows to use, so it needs a terminal. In automation, write \
             .github/ship.yml directly — see https://noirbizarre.github.io/gh-ship/configuration/"
        )
    )]
    NotInteractive,

    /// A filesystem failure, as a diagnostic rather than a bare `io::Error`.
    ///
    /// `init` is the command a newcomer meets first, so its failures carry
    /// the same code and help as everything else gh-ship reports.
    #[error("could not {verb} {path}: {source}")]
    #[diagnostic(
        code(ship::init::io),
        help("check the path exists and is writable, then re-run `gh ship init`")
    )]
    Io {
        verb: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl InitError {
    fn io<'a>(verb: &'static str, path: &'a Path) -> impl FnOnce(std::io::Error) -> Self + 'a {
        move |source| Self::Io {
            verb,
            path: path.display().to_string(),
            source,
        }
    }
}

pub fn run(cli: &Cli, args: &InitArgs, theme: Theme) -> Result<()> {
    let config_path = &cli.config;
    let root = repo_root(config_path);

    if config_path.exists() && !args.force {
        return Err(InitError::Exists {
            path: config_path.display().to_string(),
        }
        .into());
    }

    eprintln!("{}", logger::action(theme, "setting up", "gh ship"));

    // `init` is a conversation. Failing early with an explanation beats
    // letting the prompt library abort with `os error 6` when there is
    // no terminal to prompt on.
    require_interactive()?;

    let available = workflow::discover(&root);
    let (conforming, nonconforming): (Vec<_>, Vec<_>) = available
        .iter()
        .cloned()
        .partition(|w| w.contract_violations().is_empty());

    report_nonconforming(&nonconforming, theme);

    // --- the token ---------------------------------------------------------
    //
    // Asked first, because it changes what the generated workflows look
    // like, and because it is the decision with the least visible
    // consequences: the wrong answer produces a working release whose PR
    // is never tested.
    let strategy = choose_token_strategy()?;

    // --- prepare (required) ---------------------------------------------
    let prepare = choose_workflow(
        "Which workflow prepares the release?",
        "prepare-release",
        &conforming,
        theme,
    )?;
    let prepare_name = match prepare {
        Choice::Existing(w) => w.slug(),
        Choice::Generate => {
            write_template(
                &root,
                Role::Prepare,
                &templates::render(Role::Prepare, strategy),
                theme,
            )?;
            "prepare-release".to_string()
        }
        Choice::Skip => unreachable!("prepare is not skippable"),
    };

    // --- publish (optional) ---------------------------------------------
    //
    // The prepare workflow is excluded: using one workflow for both roles
    // is never what someone means, and offering it invites a misclick
    // that only shows up at release time.
    let publish_candidates: Vec<Workflow> = conforming
        .iter()
        .filter(|w| w.slug() != prepare_name)
        .cloned()
        .collect();

    let publish = choose_optional_workflow(
        "Which workflow publishes the release? (builds and uploads assets)",
        "publish-release",
        &publish_candidates,
        theme,
    )?;
    let publish_name = match publish {
        Choice::Existing(w) => Some(w.slug()),
        Choice::Generate => {
            write_template(
                &root,
                Role::Publish,
                &templates::render(Role::Publish, strategy),
                theme,
            )?;
            Some("publish-release".to_string())
        }
        Choice::Skip => None,
    };

    // --- write ------------------------------------------------------------
    let yaml = render_config(&prepare_name, publish_name.as_deref());

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(InitError::io("create", parent))?;
    }
    std::fs::write(config_path, &yaml).map_err(InitError::io("write", config_path))?;

    eprintln!();
    eprintln!(
        "{}",
        logger::ok(theme, &format!("wrote {}", config_path.display()))
    );
    eprintln!("{}", next_steps(theme, publish_name.is_some(), strategy));

    Ok(())
}

/// What the user picked for a given role.
enum Choice {
    Existing(Workflow),
    Generate,
    Skip,
}

/// Refuse to run without a terminal.
///
/// See [`InitError::NotInteractive`] for why this restriction exists.
fn require_interactive() -> Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Ok(());
    }
    Err(InitError::NotInteractive.into())
}

/// Ask which token the generated workflows should authenticate with.
///
/// The consequence, not the mechanism, is what each option leads with: a
/// user who has never hit the `GITHUB_TOKEN` restriction cannot weigh
/// "installation token" against "PAT", but can weigh "the Release PR runs
/// your CI" against "it does not".
fn choose_token_strategy() -> Result<TokenStrategy> {
    let picked = Select::new("How should the release workflows authenticate?")
        .description("GitHub's default token cannot trigger your CI on the Release PR")
        .option(
            DemandOption::new("app")
                .label("A GitHub App — a scoped token, minted per run, nothing stored"),
        )
        .option(DemandOption::new("pat").label("A personal access token — the SHIP_TOKEN secret"))
        .option(
            DemandOption::new("default")
                .label("The default GITHUB_TOKEN — the Release PR will not run CI"),
        )
        .run()
        .into_diagnostic()?;

    Ok(match picked {
        "app" => TokenStrategy::App,
        "default" => TokenStrategy::Default,
        _ => TokenStrategy::Pat,
    })
}

fn choose_workflow(
    prompt: &str,
    template_name: &str,
    conforming: &[Workflow],
    theme: Theme,
) -> Result<Choice> {
    if conforming.is_empty() {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                &format!("no conforming workflow found — generating {template_name}.yml")
            )
        );
        return Ok(Choice::Generate);
    }

    let mut select = Select::new(prompt).description("gh-ship will dispatch this workflow");
    for w in conforming {
        select = select.option(DemandOption::new(w.slug()).label(&w.describe()));
    }
    select = select.option(
        DemandOption::new("__generate__".to_string()).label(&format!(
            "Generate a new {template_name}.yml from a template"
        )),
    );

    let picked = select.run().into_diagnostic()?;
    Ok(resolve(picked, conforming))
}

fn choose_optional_workflow(
    prompt: &str,
    template_name: &str,
    conforming: &[Workflow],
    theme: Theme,
) -> Result<Choice> {
    if conforming.is_empty() {
        eprintln!(
            "{}",
            logger::skip(theme, "no conforming workflow found for this role")
        );
    }

    let mut select = Select::new(prompt)
        .description("optional — skip it if you have nothing to build or upload");

    // When there is nothing sensible to pick, "Skip" leads so the safe
    // choice is also the default one.
    if conforming.is_empty() {
        select = select
            .option(
                DemandOption::new("__skip__".to_string()).label("Skip — I don't publish assets"),
            )
            .option(
                DemandOption::new("__generate__".to_string()).label(&format!(
                    "Generate a new {template_name}.yml from a template"
                )),
            );
    } else {
        for w in conforming {
            select = select.option(DemandOption::new(w.slug()).label(&w.describe()));
        }
        select = select
            .option(
                DemandOption::new("__generate__".to_string()).label(&format!(
                    "Generate a new {template_name}.yml from a template"
                )),
            )
            .option(
                DemandOption::new("__skip__".to_string()).label("Skip — I don't publish assets"),
            );
    }

    let picked = select.run().into_diagnostic()?;
    Ok(resolve(picked, conforming))
}

fn resolve(picked: String, conforming: &[Workflow]) -> Choice {
    match picked.as_str() {
        "__generate__" => Choice::Generate,
        "__skip__" => Choice::Skip,
        slug => conforming
            .iter()
            .find(|w| w.slug() == slug)
            .cloned()
            .map(Choice::Existing)
            .unwrap_or(Choice::Skip),
    }
}

/// Explain workflows that were found but cannot be used.
///
/// Hiding them would be worse than useless: the user knows the workflow
/// exists, and would conclude gh-ship is broken.
fn report_nonconforming(workflows: &[Workflow], theme: Theme) {
    let relevant: Vec<&Workflow> = workflows
        .iter()
        .filter(|w| w.dispatchable || w.callable)
        .collect();
    if relevant.is_empty() {
        return;
    }

    eprintln!();
    eprintln!(
        "{}",
        logger::warn(
            theme,
            &format!(
                "{} workflow{} cannot be used by gh-ship yet:",
                relevant.len(),
                logger::plural(relevant.len())
            )
        )
    );
    for w in &relevant {
        for v in w.contract_violations() {
            eprintln!("{}", logger::detail(theme, &w.slug(), v.message()));
        }
    }
    eprintln!(
        "{}",
        logger::skip(
            theme,
            "run `gh ship validate` after setup for the full explanation"
        )
    );
    eprintln!();
}

fn write_template(root: &Path, role: Role, body: &str, theme: Theme) -> Result<()> {
    let dir = root.join(workflow::WORKFLOW_DIR);
    std::fs::create_dir_all(&dir).map_err(InitError::io("create", &dir))?;

    let path = dir.join(role.filename());
    if path.exists() {
        let overwrite = Confirm::new(format!("{} exists. Overwrite it?", path.display()))
            .affirmative("Overwrite")
            .negative("Keep")
            .run()
            .into_diagnostic()?;
        if !overwrite {
            eprintln!(
                "{}",
                logger::skip(theme, &format!("kept {}", path.display()))
            );
            return Ok(());
        }
    }

    std::fs::write(&path, body).map_err(InitError::io("write", &path))?;
    eprintln!(
        "{}",
        logger::ok(theme, &format!("wrote {}", path.display()))
    );
    Ok(())
}

/// Render `.github/ship.yml`, documented inline.
///
/// The comments are the point: a config a newcomer can read and modify
/// without opening the docs is worth more than a terse one.
pub fn render_config(prepare: &str, publish: Option<&str>) -> String {
    let mut out = String::new();

    // The modeline gives editors completion and inline validation for this
    // file. It is a comment, so it costs nothing at parse time, and the
    // `$schema:` form is understood by both yaml-language-server (VS Code,
    // Neovim) and JetBrains IDEs.
    out.push_str("# $schema: https://noirbizarre.github.io/gh-ship/schema/config/v1.json\n");

    out.push_str(&format!(
        "# gh-ship configuration\n\
         # Docs: https://noirbizarre.github.io/gh-ship/configuration/\n\
         #\n\
         # gh-ship orchestrates your release. It never bumps versions,\n\
         # writes changelogs, or runs your release logic — your workflows do.\n\
         \n\
         version: {CONFIG_VERSION}\n\
         \n"
    ));

    out.push_str(&format!(
        "# Branch on which the release is staged. gh-ship stages each release\n\
         # on a throwaway branch and moves this one onto the result, so do not\n\
         # push to it yourself.\n\
         release_branch: {DEFAULT_RELEASE_BRANCH}\n\
         \n\
         # Base branches gh-ship releases from — one release line each.\n\
         # When omitted, the repository's default branch is the only line.\n\
         # branches: [main]\n\
         \n"
    ));

    out.push_str(
        "# Workflows gh-ship dispatches.\n\
         #\n\
         # These must declare `on: workflow_dispatch` — a `workflow_call`-only\n\
         # workflow cannot be started through the API. The prepare workflow must\n\
         # also accept a `dry_run` input, which `gh ship preview` dispatches with.\n\
         # `gh ship validate` checks both.\n\
         workflows:\n",
    );
    out.push_str(&format!("  prepare: {prepare}\n"));
    match publish {
        Some(p) => out.push_str(&format!("  publish: {p}\n")),
        None => out.push_str("  # publish: publish-release\n"),
    }
    out.push('\n');

    out.push_str(
        "# Release PR rendering. Templates are MiniJinja, and the release\n\
         # artifact is the root context: {{ version }}, {{ tag }},\n\
         # {{ release.notes }}.\n\
         #\n\
         # The PR body is: header + the notes your workflow produced + footer.\n\
         #\n\
         # The title is also the commit message: GitHub pre-fills the\n\
         # \"Squash and merge\" dialog from it and appends `(#42)`, so keep it a\n\
         # Conventional Commit.\n\
         #\n\
         # Because it is Conventional, your changelog tool will parse it — make\n\
         # sure it also skips it, or every release will look like an unreleased\n\
         # change and propose the next one out of nothing.\n\
         pull_request:\n",
    );
    // The default lives in one place; echoing it as a literal here would
    // silently drift the day it changes.
    out.push_str(&format!("  title: \"{DEFAULT_PR_TITLE}\"\n"));
    out.push_str(
        "\x20 # header: |\n\
        \x20 #   This PR prepares the next release.\n\
        \x20 # footer: |\n\
        \x20 #   Generated automatically by gh-ship.\n\
        \x20 # labels: [release]\n\
         \n",
    );

    out.push_str(
        "# GitHub Release behaviour.\n\
         # release:\n\
        \x20 # Create the release as a draft, then undraft it once the publish\n\
        \x20 # workflow succeeds. This is the only ordering that lets the publish\n\
        \x20 # workflow attach assets before watchers are notified.\n\
        \x20 # draft: true\n",
    );

    out
}

/// Render the closing guidance.
///
/// Pure, like every other renderer: `init` is the one command nobody can
/// drive from a test, so the part worth pinning is kept out of the
/// prompting.
fn next_steps(theme: Theme, has_publish: bool, strategy: TokenStrategy) -> String {
    let mut out = vec![
        String::new(),
        logger::rule(theme, "Next steps"),
        String::new(),
        logger::step(
            theme,
            1,
            &[
                "Edit the generated workflow: replace the placeholder steps",
                "with your own versioning and changelog tooling.",
            ],
        ),
        String::new(),
        logger::step(theme, 2, &["Run `gh ship validate` to check the setup."]),
        String::new(),
        logger::step(
            theme,
            3,
            &[
                "Run `gh ship preview` to see the Release PR without",
                "changing anything.",
            ],
        ),
        String::new(),
    ];

    // The token is the one part `init` cannot finish on the user's
    // behalf — the workflow is written, but it authenticates with
    // something that does not exist yet.
    match strategy {
        TokenStrategy::App => {
            out.push(logger::warn(theme, "one thing left: the GitHub App"));
            out.push(logger::note(
                theme,
                &[
                    "The generated workflows mint their token with",
                    "actions/create-github-app-token, so they need an App to mint",
                    "it from. Register one, grant it contents, actions,",
                    "pull_requests and issues write, install it on this",
                    "repository, then set APP_CLIENT_ID as a variable and",
                    "APP_PRIVATE_KEY as a secret, both in the `release`",
                    "environment — an environment variable resolves only in a",
                    "job that declares that environment.",
                ],
            ));
            out.push(logger::note_url(
                theme,
                "https://noirbizarre.github.io/gh-ship/workflows/#using-a-github-app",
            ));
        }
        TokenStrategy::Pat => {
            out.push(logger::warn(theme, "one thing left: the token"));
            out.push(logger::note(
                theme,
                &[
                    "The generated workflows prefer the `SHIP_TOKEN` secret and",
                    "fall back to GITHUB_TOKEN, which cannot trigger other",
                    "workflows — so until you add SHIP_TOKEN, the Release PR",
                    "will NOT run your CI.",
                ],
            ));
            out.push(logger::note_url(
                theme,
                "https://noirbizarre.github.io/gh-ship/workflows/#tokens",
            ));
        }
        TokenStrategy::Default => {
            out.push(logger::warn(theme, "the Release PR will not run your CI"));
            out.push(logger::note(
                theme,
                &[
                    "You chose the default GITHUB_TOKEN, which GitHub deliberately",
                    "prevents from triggering other workflow runs. Nothing else is",
                    "affected. Re-run `gh ship init --force` and pick a GitHub App",
                    "or a PAT to change it.",
                ],
            ));
            out.push(logger::note_url(
                theme,
                "https://noirbizarre.github.io/gh-ship/workflows/#tokens",
            ));
        }
    }

    if has_publish {
        out.push(String::new());
        out.push(logger::note(
            theme,
            &[
                "Your publish workflow uploads assets to the DRAFT release;",
                "gh-ship undrafts it only after that workflow succeeds.",
            ],
        ));
    }
    out.push(String::new());

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gh_ship::config::Config;

    #[test]
    fn generated_config_is_valid() {
        let yaml = render_config("prepare-release", Some("publish-release"));
        let c = Config::parse(".github/ship.yml", &yaml).expect("init must emit a valid config");
        assert_eq!(c.prepare_workflow(), "prepare-release");
        assert_eq!(c.publish_workflow(), Some("publish-release"));
        assert_eq!(c.release_branch_template(), DEFAULT_RELEASE_BRANCH);
    }

    #[test]
    fn generated_config_without_publish_is_valid() {
        let yaml = render_config("prepare-release", None);
        let c = Config::parse(".github/ship.yml", &yaml).unwrap();
        assert_eq!(c.publish_workflow(), None);
        assert!(
            yaml.contains("# publish: publish-release"),
            "the optional key should be shown as a commented example"
        );
    }

    /// The generated config must carry the editor modeline, and must still be
    /// the schema-valid shape asserted by tests/config_schema.rs.
    #[test]
    fn generated_config_carries_the_schema_modeline() {
        let yaml = render_config("prepare-release", None);
        assert!(
            yaml.starts_with("# $schema: "),
            "the modeline must be the first line to be picked up: {yaml}"
        );
        assert!(
            yaml.contains("schema/config/v1.json"),
            "it must point at the config schema, not the release one: {yaml}"
        );
        // Still parses: it is only a comment.
        Config::parse(".github/ship.yml", &yaml).expect("modeline must not break parsing");
    }

    #[test]
    fn generated_config_is_documented() {
        let yaml = render_config("prepare-release", None);
        // The two traps every user hits must be explained in the file
        // itself, not only in the docs.
        assert!(yaml.contains("workflow_dispatch"), "{yaml}");
        assert!(yaml.contains("dry_run"), "{yaml}");
        assert!(yaml.contains("never bumps versions"), "{yaml}");
    }

    /// Every commented-out key in the generated config must be a key the
    /// parser still accepts.
    ///
    /// The generated file is documentation that happens to be executable:
    /// a user uncomments a line and expects it to work. A commented hint
    /// for a removed key is worse than no hint at all, and it cannot be
    /// caught by parsing the output — the line is a comment, so it parses
    /// either way. This compares the hints against the published schema,
    /// which `tests/config_schema.rs` in turn keeps aligned with the Rust
    /// model.
    #[test]
    fn commented_keys_are_live_schema_keys() {
        let yaml = render_config("prepare-release", None);

        let schema: serde_json::Value = {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/config.v1.schema.json");
            let text = std::fs::read_to_string(&path).expect("config schema exists");
            serde_json::from_str(&text).expect("config schema is valid JSON")
        };

        // Only top-level hints are checked: a nested one (`  # publish:`)
        // would have to be resolved against its parent's subschema, and
        // the keys that get removed are the top-level ones.
        let hints = yaml.lines().filter_map(|line| {
            let key = line.strip_prefix("# ")?.split_once(':')?.0;
            // A prose comment is not a hint: keys are bare identifiers.
            key.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_')
                .then_some(key)
        });

        let properties = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("the schema declares top-level properties");

        for key in hints {
            assert!(
                properties.contains_key(key),
                "the generated config suggests `{key}`, which is not a \
                 configuration key — see schemas/config.v1.schema.json"
            );
        }
    }

    /// Uncommenting a hint must produce a config that still parses.
    ///
    /// This is what a user actually does with a commented example, and
    /// it is the step the schema comparison above cannot make on its
    /// own: a key can exist and still be suggested with a value the
    /// parser rejects.
    #[test]
    fn the_hints_work_once_uncommented() {
        let yaml = render_config("prepare-release", Some("publish-release"));
        let uncommented: String = yaml
            .lines()
            .map(|line| match line.split_once("# ") {
                // Only hints: a prose comment has no `key: value` shape.
                Some((indent, rest))
                    if indent.trim().is_empty()
                        && rest.split_once(':').is_some_and(|(k, _)| {
                            k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                        }) =>
                {
                    format!("{indent}{rest}")
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let c = Config::parse(".github/ship.yml", &uncommented)
            .expect("every commented hint must be uncommentable");
        assert!(c.has_branches(), "the branches hint must take effect");
    }

    /// The closing guidance is the only place the token is explained to
    /// someone who has just run `init`. It must name what *they* chose —
    /// telling an App user about `SHIP_TOKEN` sends them to create the
    /// wrong thing — and the publish note must appear only when there is a
    /// publish workflow to note.
    #[test]
    fn next_steps_explain_the_token_and_the_draft() {
        let t = Theme::plain();

        let without = next_steps(t, false, TokenStrategy::Pat);
        assert!(without.contains("gh ship validate"), "{without}");
        assert!(without.contains("gh ship preview"), "{without}");
        assert!(without.contains("SHIP_TOKEN"), "{without}");
        assert!(
            without.contains("https://noirbizarre.github.io/gh-ship/workflows/#tokens"),
            "{without}"
        );
        assert!(
            !without.contains("DRAFT release"),
            "there is no publish workflow to talk about:\n{without}"
        );

        let with = next_steps(t, true, TokenStrategy::Pat);
        assert!(with.contains("DRAFT release"), "{with}");

        // Each strategy sends the user somewhere different.
        let app = next_steps(t, false, TokenStrategy::App);
        assert!(
            app.contains("APP_CLIENT_ID") && app.contains("APP_PRIVATE_KEY"),
            "{app}"
        );
        assert!(!app.contains("SHIP_TOKEN"), "{app}");
        assert!(
            app.contains("https://noirbizarre.github.io/gh-ship/workflows/#using-a-github-app"),
            "{app}"
        );

        let default = next_steps(t, false, TokenStrategy::Default);
        assert!(default.contains("will not run your CI"), "{default}");
        assert!(!default.contains("SHIP_TOKEN"), "{default}");
    }

    /// `init` must write the slug, never the display name: an emoji name
    /// in `.github/ship.yml` would be unusable.
    #[test]
    fn describe_leads_with_the_slug() {
        let emoji = workflow::parse(
            Path::new(".github/workflows/prepare-release.yml"),
            "name: 🚢 Prepare Release\non: workflow_dispatch\n",
        )
        .unwrap();
        let label = emoji.describe();
        assert!(label.starts_with("prepare-release"), "{label}");
        assert!(label.contains("🚢 Prepare Release"), "{label}");

        // No decoration when the name adds nothing.
        let plain = workflow::parse(
            Path::new(".github/workflows/ci.yml"),
            "name: ci\non: push\n",
        )
        .unwrap();
        assert_eq!(plain.describe(), "ci");
    }

    /// Both prompts key their options by slug, so `resolve` must find the
    /// picked workflow. Keying by filename instead made every pick fall
    /// through to `Choice::Skip` — and `Skip` is a panic for `prepare`.
    #[test]
    fn resolve_matches_the_value_the_prompt_offers() {
        let w = workflow::parse(
            Path::new(".github/workflows/prepare-release.yaml"),
            "name: 🚀 Prepare Release\non: workflow_dispatch\n",
        )
        .unwrap();
        let conforming = vec![w.clone()];

        // What `DemandOption::new(...)` is built from must round-trip.
        match resolve(w.slug(), &conforming) {
            Choice::Existing(found) => assert_eq!(found.slug(), w.slug()),
            _ => panic!("the offered value must resolve to the workflow it labels"),
        }
        assert!(matches!(
            resolve("__generate__".into(), &conforming),
            Choice::Generate
        ));
        assert!(matches!(
            resolve("__skip__".into(), &conforming),
            Choice::Skip
        ));
    }
}
