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
use gh_ship::cli::{Cli, ValidateArgs};
use gh_ship::config::Config;
use gh_ship::gh::workflow::{self, Workflow};
use gh_ship::logger;
use gh_ship::style::Theme;
use gh_ship::suggest;

pub fn run(cli: &Cli, args: &ValidateArgs, theme: &Theme) -> Result<()> {
    match &args.artifact {
        Some(path) => validate_artifact(path, theme),
        None => validate_setup(cli, theme),
    }
}

// --- Artifact mode -------------------------------------------------------

fn validate_artifact(path: &Path, theme: &Theme) -> Result<()> {
    let artifact = validate::validate_file(path)?;
    let name = path.display().to_string();

    eprintln!(
        "{}",
        logger::ok(theme, &format!("{name} is a valid release artifact"))
    );

    if artifact.changed {
        if let Some(v) = artifact.version() {
            eprintln!("{}", logger::detail(theme, "version", v));
        }
        if let Some(t) = artifact.tag() {
            eprintln!("{}", logger::detail(theme, "tag", t));
        }
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
#[error("the gh-ship setup has {n} problem{s}", n = issues.len(), s = if issues.len() == 1 { "" } else { "s" })]
#[diagnostic(
    code(ship::validate::setup),
    help("see https://noirbizarre.github.io/gh-ship/workflows/ for the workflow contract")
)]
pub struct SetupInvalid {
    #[related]
    issues: Vec<WorkflowIssue>,
}

fn validate_setup(cli: &Cli, theme: &Theme) -> Result<()> {
    let config = Config::load(&cli.config)?;
    eprintln!(
        "{}",
        logger::ok(theme, &format!("{} is valid", cli.config.display()))
    );
    eprintln!(
        "{}",
        logger::detail(theme, "release branch", config.release_branch())
    );

    // Workflows are resolved relative to the repository root, which we
    // take to be the config file's grandparent (`.github/ship.yml`).
    let root = repo_root(&cli.config);
    let available = workflow::discover(&root);

    let mut issues = Vec::new();

    check_workflow(
        &config,
        &available,
        config.prepare_workflow(),
        "prepare",
        &mut issues,
        theme,
    );

    if let Some(publish) = config.publish_workflow() {
        check_workflow(&config, &available, publish, "publish", &mut issues, theme);
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

fn check_workflow(
    _config: &Config,
    available: &[Workflow],
    name: &str,
    role: &str,
    issues: &mut Vec<WorkflowIssue>,
    theme: &Theme,
) {
    let Some(found) = workflow::find(available, name) else {
        let names: Vec<&str> = available.iter().map(|w| w.name.as_str()).collect();
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
            problem: format!("(configured as `workflows.{role}`) was not found"),
            help,
        });
        return;
    };

    let violations = found.contract_violations();
    if violations.is_empty() {
        eprintln!(
            "{}",
            logger::detail(theme, role, &format!("{} ({})", found.name, found.id()))
        );
        return;
    }

    for v in violations {
        issues.push(WorkflowIssue {
            workflow: found.id(),
            problem: v.message().to_string(),
            help: Some(v.help().to_string()),
        });
    }
}

/// Infer the repository root from the config path.
///
/// `.github/ship.yml` → the directory containing `.github`. A config
/// passed with `--config` from elsewhere falls back to the current
/// directory, which is the best guess available.
fn repo_root(config: &Path) -> std::path::PathBuf {
    config
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == ".github"))
        .and_then(|p| p.parent())
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".").to_path_buf()
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_is_the_grandparent_of_a_dot_github_config() {
        assert_eq!(repo_root(Path::new(".github/ship.yml")), Path::new("."));
        assert_eq!(
            repo_root(Path::new("/src/proj/.github/ship.yml")),
            Path::new("/src/proj")
        );
    }

    #[test]
    fn repo_root_falls_back_to_cwd_for_unusual_paths() {
        assert_eq!(repo_root(Path::new("ship.yml")), Path::new("."));
        assert_eq!(repo_root(Path::new("/tmp/custom.yml")), Path::new("."));
    }
}
