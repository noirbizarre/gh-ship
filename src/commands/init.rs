//! `gh ship init` — make a repository gh-ship enabled.
//!
//! The goal is under a minute, and the hard part is not writing YAML: it
//! is that the two most likely setup mistakes are invisible until a
//! release is already in flight.
//!
//! 1. Naming a `workflow_call`-only workflow, which the API cannot start.
//! 2. Omitting the `run-name` nonce, without which gh-ship cannot find
//!    the run it dispatched.
//!
//! So `init` only ever offers workflows that satisfy the contract, and
//! explains — rather than silently hides — the ones that do not.

use std::path::Path;

use demand::{Confirm, DemandOption, Select};
use miette::{IntoDiagnostic, Result};

use gh_ship::cli::{Cli, InitArgs};
use gh_ship::config::{CONFIG_VERSION, DEFAULT_RELEASE_BRANCH};
use gh_ship::gh::workflow::{self, Workflow};
use gh_ship::logger;
use gh_ship::style::Theme;

use super::repo_root;

/// Templates shipped in the binary, offered when a workflow is missing.
const PREPARE_TEMPLATE: &str = include_str!("../../templates/prepare-release.yml");
const PUBLISH_TEMPLATE: &str = include_str!("../../templates/publish-release.yml");

pub fn run(cli: &Cli, args: &InitArgs, theme: Theme) -> Result<()> {
    let config_path = &cli.config;
    let root = repo_root(config_path);

    if config_path.exists() && !args.force {
        return Err(miette::miette!(
            code = "ship::init::exists",
            help = "pass `--force` to overwrite it, or edit it directly",
            "{} already exists",
            config_path.display()
        ));
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
            write_template(&root, "prepare-release.yml", PREPARE_TEMPLATE, theme)?;
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
    )?;
    let publish_name = match publish {
        Choice::Existing(w) => Some(w.slug()),
        Choice::Generate => {
            write_template(&root, "publish-release.yml", PUBLISH_TEMPLATE, theme)?;
            Some("publish-release".to_string())
        }
        Choice::Skip => None,
    };

    // --- write ------------------------------------------------------------
    let yaml = render_config(&prepare_name, publish_name.as_deref());

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| write_error("create", parent, &e))?;
    }
    std::fs::write(config_path, &yaml).map_err(|e| write_error("write", config_path, &e))?;

    eprintln!();
    eprintln!(
        "{}",
        logger::ok(theme, &format!("wrote {}", config_path.display()))
    );
    print_next_steps(theme, publish_name.is_some());

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
/// `gh ship init` is the one interactive command. Every other command is
/// scriptable, so this restriction never blocks automation — and a
/// CI job that reaches this has a configuration bug, not a terminal
/// problem, so it should be told so.
fn require_interactive() -> Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Ok(());
    }
    Err(miette::miette!(
        code = "ship::init::not_interactive",
        help = "`init` asks which workflows to use, so it needs a terminal. \
                In automation, write .github/ship.yml directly — see \
                https://noirbizarre.github.io/gh-ship/configuration/",
        "`gh ship init` requires an interactive terminal"
    ))
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
        select = select.option(DemandOption::new(w.slug()).label(&option_label(w)));
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
) -> Result<Choice> {
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
            select = select.option(DemandOption::new(w.slug()).label(&option_label(w)));
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

/// Label a workflow for the prompt.
///
/// The slug leads because that is the identifier written to the config;
/// the display name follows only when it adds information.
fn option_label(w: &Workflow) -> String {
    if w.has_distinct_name() {
        format!("{}  —  {}", w.slug(), w.name)
    } else {
        w.slug()
    }
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
                if relevant.len() == 1 { "" } else { "s" }
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

fn write_template(root: &Path, filename: &str, body: &str, theme: Theme) -> Result<()> {
    let dir = root.join(workflow::WORKFLOW_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| write_error("create", &dir, &e))?;

    let path = dir.join(filename);
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

    std::fs::write(&path, body).map_err(|e| write_error("write", &path, &e))?;
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
    // file. It is a comment, so it costs nothing at parse time.
    out.push_str(
        "# yaml-language-server: $schema=https://noirbizarre.github.io/gh-ship/schema/config/v1.json\n",
    );

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
         # Branch the Release PR targets.\n\
         # Defaults to the repository's default branch.\n\
         # base_branch: main\n\
         \n"
    ));

    out.push_str(
        "# Workflows gh-ship dispatches.\n\
         #\n\
         # These must declare `on: workflow_dispatch` — a `workflow_call`-only\n\
         # workflow cannot be started through the API. They must also stamp\n\
         # `ship_id` into their `run-name`, which is how gh-ship finds the run\n\
         # it started, and the prepare workflow must accept a `dry_run` input.\n\
         # `gh ship validate` checks all three.\n\
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
         pull_request:\n\
        \x20 title: \"Release {{ version }}\"\n\
        \x20 # header: |\n\
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

fn print_next_steps(theme: Theme, has_publish: bool) {
    eprintln!();
    eprintln!("{}", logger::rule(theme, "Next steps"));
    eprintln!();
    eprintln!("  1. Edit the generated workflow: replace the placeholder steps");
    eprintln!("     with your own versioning and changelog tooling.");
    eprintln!();
    eprintln!("  2. Run `gh ship validate` to check the setup.");
    eprintln!();
    eprintln!("  3. Run `gh ship preview` to see the Release PR without");
    eprintln!("     changing anything.");
    eprintln!();
    eprintln!("{}", logger::warn(theme, "One thing to decide: the token."));
    eprintln!("     GitHub's default GITHUB_TOKEN cannot trigger other workflows,");
    eprintln!("     so a Release PR it authors will NOT run your CI. If the Release");
    eprintln!("     PR must be tested before merging, add a PAT or GitHub App token");
    eprintln!("     as the `SHIP_TOKEN` secret. The generated workflow already");
    eprintln!("     prefers it when present.");
    eprintln!("     https://noirbizarre.github.io/gh-ship/workflows/#tokens");
    if has_publish {
        eprintln!();
        eprintln!("     Your publish workflow uploads assets to the DRAFT release;");
        eprintln!("     gh-ship undrafts it only after that workflow succeeds.");
    }
    eprintln!();
}

/// A filesystem failure, as a diagnostic rather than a bare `io::Error`.
///
/// `init` is the command a newcomer meets first, so its failures carry the
/// same code and help as everything else gh-ship reports.
fn write_error(verb: &str, path: &Path, source: &std::io::Error) -> miette::Report {
    miette::miette!(
        code = "ship::init::io",
        help = "check the path exists and is writable, then re-run `gh ship init`",
        "could not {verb} {}: {source}",
        path.display()
    )
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
        assert_eq!(c.release_branch(), DEFAULT_RELEASE_BRANCH);
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
            yaml.starts_with("# yaml-language-server: $schema="),
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
        assert!(yaml.contains("run-name"), "{yaml}");
        assert!(yaml.contains("never bumps versions"), "{yaml}");
    }

    /// The shipped templates must satisfy the contract they exist to
    /// teach. If this fails, `init` generates workflows that `validate`
    /// immediately rejects.
    #[test]
    fn shipped_templates_satisfy_the_contract() {
        for (name, body) in [
            ("prepare-release.yml", PREPARE_TEMPLATE),
            ("publish-release.yml", PUBLISH_TEMPLATE),
        ] {
            let w = workflow::parse(Path::new(name), body)
                .unwrap_or_else(|| panic!("{name} must be parseable YAML"));
            assert_eq!(
                w.contract_violations(),
                vec![],
                "{name} violates the gh-ship workflow contract"
            );
            assert!(w.callable, "{name} should also be reusable");
        }
    }

    #[test]
    fn prepare_template_supports_dry_run() {
        let w = workflow::parse(Path::new("prepare-release.yml"), PREPARE_TEMPLATE).unwrap();
        assert!(
            w.accepts_dry_run(),
            "`gh ship preview` needs the prepare workflow to accept dry_run"
        );
    }

    #[test]
    fn prepare_template_uploads_the_protocol_artifact() {
        assert!(PREPARE_TEMPLATE.contains("name: ship-release"));
        assert!(PREPARE_TEMPLATE.contains("ship.release.json"));
        assert!(
            PREPARE_TEMPLATE.contains("gh ship validate ship.release.json"),
            "the template should validate before uploading"
        );
    }

    #[test]
    fn prepare_template_explains_the_token_trap() {
        assert!(PREPARE_TEMPLATE.contains("SHIP_TOKEN"));
        assert!(PREPARE_TEMPLATE.contains("cannot trigger other workflows"));
    }

    #[test]
    fn publish_template_checks_out_the_tag_not_a_branch() {
        assert!(PUBLISH_TEMPLATE.contains("ref: ${{ inputs.tag }}"));
        assert!(PUBLISH_TEMPLATE.contains("--clobber"));
    }

    /// `init` must write the slug, never the display name: an emoji name
    /// in `.github/ship.yml` would be unusable.
    #[test]
    fn option_label_leads_with_the_slug() {
        let emoji = workflow::parse(
            Path::new(".github/workflows/prepare-release.yml"),
            "name: 🚢 Prepare Release\non: workflow_dispatch\n",
        )
        .unwrap();
        let label = option_label(&emoji);
        assert!(label.starts_with("prepare-release"), "{label}");
        assert!(label.contains("🚢 Prepare Release"), "{label}");

        // No decoration when the name adds nothing.
        let plain = workflow::parse(
            Path::new(".github/workflows/ci.yml"),
            "name: ci\non: push\n",
        )
        .unwrap();
        assert_eq!(option_label(&plain), "ci");
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
