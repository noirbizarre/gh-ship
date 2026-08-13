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
use gh_ship::gh::cli::Gh;
use gh_ship::gh::repo::{self, MergeSettings, SQUASH_MESSAGE_BLANK, SQUASH_TITLE_PR};
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
        check_squash_settings(cli, theme);
        Ok(())
    } else {
        Err(SetupInvalid { issues }.into())
    }
}

// --- Squash-merge settings ----------------------------------------------

/// What is wrong with the repository's squash settings.
///
/// `None` means gh-ship has no opinion, which is not the same as an
/// empty `Vec`: a repository that does not offer squash merging composes
/// no squash commit, and telling it how to compose one better is noise.
///
/// A pure function over the three values, so the decision is testable
/// without a `gh` stub — the reporting around it is not where bugs hide.
fn squash_problems(settings: &MergeSettings) -> Option<Vec<&'static str>> {
    if !settings.allow_squash_merge {
        return None;
    }

    let mut problems = Vec::new();

    if settings.squash_merge_commit_title != SQUASH_TITLE_PR {
        // `COMMIT_OR_PR_TITLE` prefers the single commit's own subject
        // when the PR has exactly one commit — which a Release PR
        // usually does. The configured `pull_request.title` is then
        // silently bypassed in favour of whatever the bump tool wrote.
        problems.push(
            "the squash commit subject can come from the bump commit instead of the PR title, \
             bypassing `pull_request.title`",
        );
    }

    if settings.squash_merge_commit_message != SQUASH_MESSAGE_BLANK {
        // `PR_BODY` copies the whole Release PR body — release notes
        // *and* the embedded `<!-- ship:artifact -->` JSON — into git
        // history. `COMMIT_MESSAGES` copies the staging-branch commits.
        problems.push(
            "the squash commit body copies the release notes and the embedded artifact into git \
             history",
        );
    }

    Some(problems)
}

/// Report squash settings that would spoil the release commit.
///
/// Best-effort and never fatal. `gh ship validate` is otherwise entirely
/// offline — it is documented as safe to run as the first step of any CI
/// job — so a missing `gh`, missing auth, or a repository this token
/// cannot read must not turn a config check into a failure. Any error
/// means "no opinion", and no opinion is reported silently.
fn check_squash_settings(cli: &Cli, theme: Theme) {
    // `gh api` needs a resolved `OWNER/REPO` in the path, so the slug is
    // looked up first. Both calls are allowed to fail into silence.
    let gh = Gh::new(cli.repo.clone());
    let Ok(repository) = repo::repository(&gh) else {
        return;
    };
    let gh = gh.scoped_to(repository.name_with_owner.clone());

    let Ok(settings) = repo::merge_settings(&gh) else {
        return;
    };

    let Some(problems) = squash_problems(&settings) else {
        return;
    };
    if problems.is_empty() {
        eprintln!(
            "{}",
            logger::ok(theme, "squash merges produce a clean release commit")
        );
        return;
    }

    eprintln!(
        "{}",
        logger::warn(theme, "this repository squashes the Release PR badly")
    );
    for problem in &problems {
        eprintln!("{}", logger::note(theme, &[problem]));
    }

    let slug = &repository.name_with_owner;
    eprintln!(
        "{}",
        logger::note(
            theme,
            &[
                "Fix it once, with admin rights:",
                &format!("  gh api -X PATCH repos/{slug} \\"),
                "    -f squash_merge_commit_title=PR_TITLE \\",
                "    -f squash_merge_commit_message=BLANK",
            ]
        )
    );
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

    // Reported whether or not the workflow is otherwise valid: it is not a
    // failure, just dead weight the user can delete.
    if found.declares_legacy_ship_id() {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                &format!(
                    "{} still declares a `ship_id` input — gh-ship no longer sends it. \
                     Remove it, along with the `ship:` marker in `run-name`; the \
                     compatibility shim that fills it in goes away next release.",
                    found.slug()
                )
            )
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(allow: bool, title: &str, message: &str) -> MergeSettings {
        MergeSettings {
            allow_squash_merge: allow,
            squash_merge_commit_title: title.into(),
            squash_merge_commit_message: message.into(),
        }
    }

    #[test]
    fn the_recommended_settings_have_nothing_to_report() {
        let s = settings(true, SQUASH_TITLE_PR, SQUASH_MESSAGE_BLANK);
        assert_eq!(squash_problems(&s), Some(Vec::new()));
    }

    /// GitHub's own defaults, which is what most repositories will hit.
    #[test]
    fn githubs_defaults_are_flagged_on_both_counts() {
        let s = settings(true, "COMMIT_OR_PR_TITLE", "COMMIT_MESSAGES");
        assert_eq!(squash_problems(&s).unwrap().len(), 2);
    }

    #[test]
    fn the_body_source_is_flagged_on_its_own() {
        let s = settings(true, SQUASH_TITLE_PR, "PR_BODY");
        let problems = squash_problems(&s).unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("artifact"), "{problems:?}");
    }

    #[test]
    fn the_title_source_is_flagged_on_its_own() {
        let s = settings(true, "COMMIT_OR_PR_TITLE", SQUASH_MESSAGE_BLANK);
        let problems = squash_problems(&s).unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("pull_request.title"), "{problems:?}");
    }

    /// Nothing to compose means nothing to complain about — telling a
    /// merge-commit-only repository how to configure squashing is noise.
    #[test]
    fn a_repository_without_squash_merging_is_left_alone() {
        let s = settings(false, "COMMIT_OR_PR_TITLE", "PR_BODY");
        assert_eq!(squash_problems(&s), None);
    }

    /// An unreadable response deserialises to empty strings; that must
    /// not be read as "correctly configured".
    #[test]
    fn unknown_values_are_not_mistaken_for_the_recommended_ones() {
        let s = settings(true, "", "");
        assert_eq!(squash_problems(&s).unwrap().len(), 2);
    }
}
