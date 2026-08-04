//! `gh ship validate` — check a release artifact, or the setup itself.
//!
//! Two modes, one command:
//!
//! - **`gh ship validate FILE`** checks a release artifact. No network,
//!   no repository, no GitHub authentication — safe as the first step of
//!   any CI job, on any CI system.
//! - **`gh ship validate`** checks `.github/ship.yml` and the workflows
//!   it names, against the contract in [`gh_ship::gh::workflow`].
//!
//! The second mode exists because every one of those contract
//! violations otherwise surfaces mid-release as a confusing timeout.
//! Catching them at setup time is the difference between a five-second
//! fix and a twenty-minute debugging session.

use std::path::Path;

use miette::{Diagnostic, Result};
use thiserror::Error;

use gh_ship::artifact::validate;
use gh_ship::branches;
use gh_ship::cli::{Cli, ValidateArgs};
use gh_ship::config::Config;
use gh_ship::gh::workflow::{self, Role, Workflow};
use gh_ship::logger;
use gh_ship::style::Theme;
use gh_ship::suggest;

use super::repo_root;

pub fn run(cli: &Cli, args: &ValidateArgs, theme: Theme) -> Result<()> {
    match &args.artifact {
        Some(path) => validate_artifact(path, theme),
        None => validate_setup(cli, theme),
    }
}

// --- Artifact mode -------------------------------------------------------

fn validate_artifact(path: &Path, theme: Theme) -> Result<()> {
    let artifact = validate::validate_file(path)?;
    let name = path.display().to_string();

    eprintln!(
        "{}",
        logger::ok(theme, &format!("{name} is a valid release artifact"))
    );

    if artifact.changed {
        eprintln!(
            "{}",
            logger::release_identity(theme, artifact.version(), artifact.tag())
        );
        if artifact.is_prerelease() {
            eprintln!("{}", logger::detail(theme, "prerelease", "yes"));
        }
    } else {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                "changed: false — gh-ship would report nothing to release"
            )
        );
    }

    Ok(())
}

// --- Setup mode ----------------------------------------------------------

/// A problem with the configured workflows.
#[derive(Debug, Error, Diagnostic)]
#[error("workflow `{workflow}` {problem}")]
#[diagnostic(code(ship::validate::workflow))]
pub struct WorkflowIssue {
    workflow: String,
    problem: String,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("the gh-ship setup has {n} problem{s}", n = issues.len(), s = logger::plural(issues.len()))]
#[diagnostic(
    code(ship::validate::setup),
    help("see https://noirbizarre.github.io/gh-ship/workflows/ for the workflow contract")
)]
pub struct SetupInvalid {
    #[related]
    issues: Vec<WorkflowIssue>,
}

fn validate_setup(cli: &Cli, theme: Theme) -> Result<()> {
    let config = Config::load(&cli.config)?;

    // Whether `release_branch` compiles, and whether it actually varies
    // per line, needs the template engine — so it is checked here rather
    // than at parse time. Catching it now is the whole point: a typo in
    // a branch template must fail at setup, not mid-release. It runs
    // before the "is valid" line, because announcing validity and then
    // failing reads as a contradiction.
    branches::check(&config)?;

    eprintln!(
        "{}",
        logger::ok(theme, &format!("{} is valid", cli.config.display()))
    );

    report_release_lines(&config, theme)?;

    // Workflows are resolved relative to the repository root, which we
    // take to be the config file's grandparent (`.github/ship.yml`).
    let root = repo_root(&cli.config);
    let available = workflow::discover(&root);

    let mut issues = Vec::new();

    check_workflow(
        &available,
        config.prepare_workflow(),
        Role::Prepare,
        &mut issues,
        theme,
    );

    if let Some(publish) = config.publish_workflow() {
        check_workflow(&available, publish, Role::Publish, &mut issues, theme);
    }

    if issues.is_empty() {
        eprintln!(
            "{}",
            logger::ok(theme, "workflows satisfy the gh-ship contract")
        );
        Ok(())
    } else {
        Err(SetupInvalid { issues }.into())
    }
}

/// Show what each release line resolves to.
///
/// Only exact entries can be resolved without knowing a real branch, so
/// globs are reported as the template they will render — which is still
/// enough to catch "I meant `{{ match }}`, I wrote `{{ matched }}`".
///
/// The template shown is the one actually in force for that line, and an
/// overridden line says so: when a release lands on an unexpected
/// branch, this is the line that explains why.
fn report_release_lines(config: &Config, theme: Theme) -> Result<()> {
    if !config.has_branches() {
        eprintln!(
            "{}",
            logger::detail(theme, "release branch", config.release_branch_template())
        );
        return Ok(());
    }

    for (i, rule) in config.branches().iter().enumerate() {
        let template = config.release_branch_template_for(Some(i));
        let resolved = if rule.is_pattern() {
            format!("{template} (per match)")
        } else {
            branches::resolve(config, &rule.branch)?.release
        };
        let origin = if rule.release_branch.is_some() {
            " (override)"
        } else {
            ""
        };
        eprintln!(
            "{}",
            logger::detail(
                theme,
                "release line",
                &format!("{} -> {resolved}{origin}", rule.branch)
            )
        );
    }
    Ok(())
}

fn check_workflow(
    available: &[Workflow],
    name: &str,
    role: Role,
    issues: &mut Vec<WorkflowIssue>,
    theme: Theme,
) {
    let Some(found) = workflow::find(available, name) else {
        let names: Vec<String> = available.iter().map(|w| w.slug()).collect();
        let help = if available.is_empty() {
            Some(format!(
                "no workflows found under {} — run `gh ship init` to generate one",
                workflow::WORKFLOW_DIR
            ))
        } else {
            Some(
                suggest::did_you_mean(name, &names)
                    .unwrap_or_else(|| format!("available workflows: {}", names.join(", "))),
            )
        };
        issues.push(WorkflowIssue {
            workflow: name.to_string(),
            problem: format!("(configured as `workflows.{}`) was not found", role.key()),
            help,
        });
        return;
    };

    let violations = found.contract_violations_as(role);
    if violations.is_empty() {
        eprintln!("{}", logger::detail(theme, role.key(), &found.describe()));
        return;
    }

    for v in violations {
        issues.push(WorkflowIssue {
            workflow: found.slug(),
            problem: v.message().to_string(),
            help: Some(v.help().to_string()),
        });
    }
}
