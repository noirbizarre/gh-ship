//! Shared orchestration used by `preview`, `prepare` and `release`.
//!
//! The lifecycle commands all need the same few things: a resolved
//! repository, a dispatched workflow, a correlated run, and a validated
//! artifact. Keeping that sequence in one place means the three commands
//! cannot drift in how they wait, how they report, or how strictly they
//! validate.

use std::path::PathBuf;

use miette::{Diagnostic, Result};
use thiserror::Error;

use gh_ship::artifact::validate;
use gh_ship::artifact::{ARTIFACT_FILE, ARTIFACT_NAME, Artifact};
use gh_ship::cli::Cli;
use gh_ship::config::Config;
use gh_ship::gh::repo::{self, Repository};
use gh_ship::gh::run::{self, Run, ShipId};
use gh_ship::gh::workflow::WorkflowRef;
use gh_ship::gh::{Gh, workflow};
use gh_ship::logger;
use gh_ship::style::Theme;

use super::repo_root;

/// Everything that can go wrong between a run succeeding and gh-ship
/// holding its artifact.
///
/// These live here rather than in [`gh_ship::artifact`] because they are
/// about *transport* — tempdirs, `gh run download`, workflow-artifact
/// layout — and the artifact module is deliberately free of any GitHub,
/// network or filesystem coupling beyond reading a single file.
#[derive(Debug, Error, Diagnostic)]
pub enum ArtifactFetchError {
    #[error("could not create a temporary directory: {source}")]
    #[diagnostic(code(ship::artifact::tempdir))]
    Tempdir {
        #[source]
        source: std::io::Error,
    },

    #[error("could not download the `{ARTIFACT_NAME}` artifact: {message}")]
    #[diagnostic(code(ship::artifact::download), help("{help}"))]
    Download { message: String, help: String },

    #[error("`{ARTIFACT_FILE}` not found in the artifact")]
    #[diagnostic(code(ship::artifact::missing_file), help("{help}"))]
    MissingFile { help: String },

    #[error("could not read the downloaded `{ARTIFACT_FILE}`: {source}")]
    #[diagnostic(code(ship::artifact::read))]
    Read {
        #[source]
        source: std::io::Error,
    },
}

/// Everything the lifecycle commands resolve up front.
pub struct Context {
    pub gh: Gh,
    pub config: Config,
    pub repository: Repository,
    pub theme: Theme,
    /// Repository root, derived from the config path.
    ///
    /// Workflow discovery must not depend on the current directory: with
    /// `--repo`, or when run from a subdirectory, a CWD-relative lookup
    /// silently finds nothing and the raw config string ends up on the
    /// wire instead of a filename.
    pub root: PathBuf,
}

impl Context {
    /// Resolve configuration and repository.
    pub fn load(cli: &Cli, theme: Theme) -> Result<Self> {
        let config = Config::load(&cli.config)?;
        let gh = Gh::new(cli.repo.clone());
        let repository = repo::repository(&gh)?;
        Ok(Self {
            gh,
            config,
            repository,
            theme,
            root: repo_root(&cli.config),
        })
    }

    /// Resolve a configured workflow name to an id/slug pair.
    ///
    /// Falls back to the configured string when the workflow is not on
    /// disk — `gh` may still resolve it by display name, and failing here
    /// would be worse than letting GitHub answer.
    pub fn workflow(&self, configured: &str) -> WorkflowRef {
        let available = workflow::discover(&self.root);
        workflow::find(&available, configured)
            .map(|w| w.to_ref())
            .unwrap_or_else(|| WorkflowRef::unresolved(configured))
    }

    /// The branch the Release PR targets.
    ///
    /// Config wins; otherwise the repository's actual default branch,
    /// which is why this is resolved at runtime rather than defaulted to
    /// `main` in the config model.
    pub fn base_branch(&self) -> &str {
        self.config
            .base_branch()
            .unwrap_or(&self.repository.default_branch.name)
    }

    pub fn release_branch(&self) -> &str {
        self.config.release_branch()
    }

    pub fn repo_slug(&self) -> &str {
        &self.repository.name_with_owner
    }
}

/// Dispatch a workflow, wait for it, and return the validated artifact.
///
/// This is the heart of gh-ship, and the part with the most ways to go
/// wrong. Each step reports before it blocks, because a command that
/// silently waits ten minutes is indistinguishable from one that hung.
pub fn run_workflow(
    ctx: &Context,
    workflow_name: &str,
    branch: &str,
    inputs: &[(&str, String)],
) -> Result<Artifact> {
    run_workflow_as(ctx, workflow_name, branch, &ShipId::generate(), inputs)
}

/// As [`run_workflow`], with a caller-supplied nonce.
///
/// `prepare` names its staging branch after the nonce, so it has to choose the
/// value before dispatching.
pub fn run_workflow_as(
    ctx: &Context,
    workflow_name: &str,
    branch: &str,
    ship_id: &ShipId,
    inputs: &[(&str, String)],
) -> Result<Artifact> {
    let resolved = ctx.workflow(workflow_name);
    let finished = dispatch_and_wait(ctx, &resolved, branch, ship_id, inputs)?;
    fetch_artifact(ctx, &finished)
}

/// Dispatch a workflow on a ref, find the run it created, and wait for it.
///
/// Every dispatch gh-ship makes goes through here, so a publish that
/// cross-compiles for an hour reports itself exactly like a prepare that
/// takes twenty seconds.
pub(crate) fn dispatch_and_wait(
    ctx: &Context,
    workflow: &WorkflowRef,
    branch: &str,
    ship_id: &ShipId,
    inputs: &[(&str, String)],
) -> Result<Run> {
    let theme = ctx.theme;

    eprintln!(
        "{}",
        logger::action(theme, "dispatching", &format!("{workflow} on {branch}"))
    );

    let ship_id = run::dispatch_as(&ctx.gh, workflow, branch, ship_id, inputs)?;
    eprintln!("{}", logger::detail(theme, "ship id", ship_id.as_str()));

    let found = find_run(ctx, workflow, branch, &ship_id)?;
    eprintln!("{}", logger::detail_url(theme, "run", &found.url));

    wait_for_run(ctx, workflow, &found)
}

pub(crate) fn find_run(
    ctx: &Context,
    workflow: &WorkflowRef,
    branch: &str,
    ship_id: &ShipId,
) -> Result<Run> {
    let theme = ctx.theme;
    let mut announced = false;
    let found = run::find(
        &ctx.gh,
        workflow,
        branch,
        ship_id,
        run::appear_timeout(),
        |elapsed| {
            if !announced {
                eprintln!("{}", logger::skip(theme, "waiting for the run to appear"));
                announced = true;
            } else if elapsed.as_secs().is_multiple_of(15) {
                eprintln!(
                    "{}",
                    logger::skip(
                        theme,
                        &format!("still waiting ({})", logger::duration(elapsed))
                    )
                );
            }
        },
    )?;
    Ok(found)
}

pub(crate) fn wait_for_run(ctx: &Context, workflow: &WorkflowRef, found: &Run) -> Result<Run> {
    let theme = ctx.theme;
    eprintln!(
        "{}",
        logger::action(theme, "waiting for", &workflow.to_string())
    );

    let mut last_status = String::new();
    let finished = run::wait(
        &ctx.gh,
        workflow,
        found,
        run::run_timeout(),
        |elapsed, current| {
            // Report on transitions rather than on every poll: a status
            // line per two seconds is noise, a line per state change is
            // information.
            if current.status != last_status {
                last_status = current.status.clone();
                eprintln!("{}", logger::detail(theme, "status", &current.status));
            } else if elapsed.as_secs().is_multiple_of(30) {
                eprintln!(
                    "{}",
                    logger::skip(
                        theme,
                        &format!("still running ({})", logger::duration(elapsed))
                    )
                );
            }
        },
    )?;

    eprintln!("{}", logger::ok(theme, &format!("{workflow} succeeded")));
    Ok(finished)
}

/// Download and validate the release artifact produced by a run.
fn fetch_artifact(ctx: &Context, finished: &Run) -> Result<Artifact> {
    let theme = ctx.theme;
    let dir = tempfile::tempdir().map_err(|source| ArtifactFetchError::Tempdir { source })?;

    eprintln!("{}", logger::action(theme, "downloading", ARTIFACT_NAME));

    run::download_artifact(&ctx.gh, finished.id, ARTIFACT_NAME, dir.path()).map_err(|e| {
        ArtifactFetchError::Download {
            message: e.to_string(),
            help: format!(
                "the run succeeded but produced no `{ARTIFACT_NAME}` artifact. A conforming \
                 workflow must upload `{ARTIFACT_FILE}` under that name — see {}. Run: {}",
                "https://noirbizarre.github.io/gh-ship/specifications/release-artifact/",
                finished.url
            ),
        }
    })?;

    let path = dir.path().join(ARTIFACT_FILE);
    if !path.exists() {
        return Err(ArtifactFetchError::MissingFile {
            help: format!(
                "the `{ARTIFACT_NAME}` artifact does not contain `{ARTIFACT_FILE}`. \
                 The filename is part of the protocol. Run: {}",
                finished.url
            ),
        }
        .into());
    }

    // Report the artifact under its protocol name rather than the
    // temporary path it happens to occupy: the user never chose that
    // path and it means nothing to them.
    let text = std::fs::read_to_string(&path).map_err(|source| ArtifactFetchError::Read { source })?;
    let artifact = validate::validate_str(ARTIFACT_FILE, &text)?;
    eprintln!("{}", logger::ok(theme, "artifact is valid"));
    Ok(artifact)
}

/// Report the `changed: false` outcome.
///
/// A workflow finding nothing to release is the system working. It gets
/// a success marker and exit 0, never a warning.
pub fn report_nothing_to_release(theme: Theme) {
    eprintln!();
    eprintln!("{}", logger::nothing_to_release(theme));
}

/// Print a rendered Release PR for human review.
pub fn print_rendered(theme: Theme, title: &str, body: &str, labels: &[String]) {
    eprintln!();
    eprintln!("{}", logger::rule(theme, "Pull Request"));
    eprintln!();
    eprintln!("{}", logger::detail(theme, "title", title));
    if !labels.is_empty() {
        eprintln!("{}", logger::detail(theme, "labels", &labels.join(", ")));
    }
    eprintln!();
    println!("{body}");
    eprintln!();
    eprintln!("{}", logger::rule(theme, "end"));
}
