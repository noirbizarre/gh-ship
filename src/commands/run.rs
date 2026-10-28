//! Event-driven orchestration for a single GitHub Actions job.

use miette::{Diagnostic, Result};
use serde::Deserialize;
use thiserror::Error;

use gh_ship::cli::{BaseArgs, Cli, PrepareArgs, ReleaseArgs, RunArgs};
use gh_ship::gh::repo;
use gh_ship::{logger, style::Theme};

use super::context::{Context, resolve_repo};

#[derive(Debug, Error, Diagnostic)]
enum RunError {
    #[error("`gh ship run` requires GitHub Actions")]
    #[diagnostic(
        code(ship::run::outside_actions),
        help("use `gh ship prepare` or `gh ship release` locally")
    )]
    OutsideActions,

    #[error("{name} is required by `gh ship run`")]
    #[diagnostic(
        code(ship::run::missing_context),
        help("run this command in a GitHub Actions job with a push or pull_request event")
    )]
    MissingContext { name: &'static str },

    #[error("could not read GITHUB_EVENT_PATH: {source}")]
    #[diagnostic(code(ship::run::read_event))]
    ReadEvent {
        #[source]
        source: std::io::Error,
    },

    #[error("invalid {event} event payload: {source}")]
    #[diagnostic(code(ship::run::invalid_event))]
    InvalidEvent {
        event: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("closed pull_request event is missing its PR number")]
    #[diagnostic(code(ship::run::missing_pr))]
    MissingPr,

    #[error("--base / SHIP_BASE_BRANCH `{base}` conflicts with event branch `{branch}`")]
    #[diagnostic(
        code(ship::run::base_conflict),
        help("remove the base override or set it to the event branch")
    )]
    BaseConflict { base: String, branch: String },

    #[error("PR #{number} could not be confirmed as merged")]
    #[diagnostic(
        code(ship::run::unconfirmed_pr),
        help("check the triggering PR on GitHub before retrying")
    )]
    UnconfirmedPr { number: u64 },
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Prepare(String),
    Release(u64),
    Skip(&'static str),
}

#[derive(Deserialize)]
struct Push {
    #[serde(rename = "ref")]
    reference: String,
    deleted: bool,
}

#[derive(Deserialize)]
struct PullRequestEvent {
    action: String,
    number: Option<u64>,
    pull_request: Option<Merged>,
}

#[derive(Deserialize)]
struct Merged {
    merged: Option<bool>,
}

fn classify(name: &str, payload: &str) -> Result<Event> {
    match name {
        "push" => {
            let event: Push =
                serde_json::from_str(payload).map_err(|source| RunError::InvalidEvent {
                    event: name.to_string(),
                    source,
                })?;
            match event.reference.strip_prefix("refs/heads/") {
                Some(branch) if !event.deleted && !branch.is_empty() => {
                    Ok(Event::Prepare(branch.to_string()))
                }
                _ => Ok(Event::Skip("not an active branch push")),
            }
        }
        "pull_request" => {
            let event: PullRequestEvent =
                serde_json::from_str(payload).map_err(|source| RunError::InvalidEvent {
                    event: name.to_string(),
                    source,
                })?;
            if event.action != "closed" {
                return Ok(Event::Skip("not a closed pull request"));
            }
            if event
                .pull_request
                .is_some_and(|pr| pr.merged == Some(false))
            {
                return Ok(Event::Skip("pull request was not merged"));
            }
            let number = event.number.filter(|n| *n > 0).ok_or(RunError::MissingPr)?;
            Ok(Event::Release(number))
        }
        _ => Ok(Event::Skip(
            "unsupported event; use prepare or release explicitly",
        )),
    }
}

fn skip(theme: Theme, reason: &str) -> Result<()> {
    eprintln!("{}", logger::skip(theme, reason));
    Ok(())
}

fn check_base(args: &RunArgs, branch: &str) -> Result<()> {
    if let Some(base) = args
        .base
        .base
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && base != branch
    {
        return Err(RunError::BaseConflict {
            base: base.to_string(),
            branch: branch.to_string(),
        }
        .into());
    }
    Ok(())
}

pub fn run(cli: &Cli, args: &RunArgs, theme: Theme) -> Result<()> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Err(RunError::OutsideActions.into());
    }
    let name = std::env::var("GITHUB_EVENT_NAME").map_err(|_| RunError::MissingContext {
        name: "GITHUB_EVENT_NAME",
    })?;
    if !matches!(name.as_str(), "push" | "pull_request") {
        return skip(
            theme,
            "unsupported event; use prepare or release explicitly",
        );
    }
    let path = std::env::var_os("GITHUB_EVENT_PATH").ok_or(RunError::MissingContext {
        name: "GITHUB_EVENT_PATH",
    })?;
    let payload = std::fs::read_to_string(path).map_err(|source| RunError::ReadEvent { source })?;

    match classify(&name, &payload)? {
        Event::Skip(reason) => skip(theme, reason),
        Event::Prepare(base) => {
            check_base(args, &base)?;
            let (gh, repository) = resolve_repo(cli)?;
            let Some(ctx) = Context::for_event(cli, &base, theme, gh, repository)? else {
                return skip(theme, "push branch is not a configured release line");
            };
            super::prepare::run_with_context(
                &ctx,
                &PrepareArgs {
                    no_wait: args.no_wait,
                    base: BaseArgs { base: Some(base) },
                },
            )
        }
        Event::Release(number) => {
            let (gh, repository) = resolve_repo(cli)?;
            let identity = repo::pull_request_identity(&gh, number)?;
            if !identity.merged {
                return skip(theme, "pull request was not merged");
            }
            let same_repo = |reference: &repo::PullRequestRef| {
                reference.repo.as_ref().is_some_and(|r| {
                    r.full_name
                        .eq_ignore_ascii_case(&repository.name_with_owner)
                })
            };
            if !same_repo(&identity.head) || !same_repo(&identity.base) {
                return skip(theme, "pull request is not within the target repository");
            }
            let base = identity.base.branch;
            check_base(args, &base)?;
            let Some(ctx) = Context::for_event(cli, &base, theme, gh, repository)? else {
                return skip(theme, "PR base is not a configured release line");
            };
            if identity.head.branch != ctx.release_branch() {
                return skip(theme, "not the configured Release PR branch");
            }
            let pr = repo::view_pull_request(&ctx.gh, number)?;
            if pr.number != number || !pr.is_merged() {
                return Err(RunError::UnconfirmedPr { number }.into());
            }
            eprintln!("{}", logger::action(theme, "releasing", ctx.repo_slug()));
            super::release::run_with_pr(
                &ctx,
                &ReleaseArgs {
                    merge: false,
                    no_wait: args.no_wait,
                    base: BaseArgs { base: Some(base) },
                },
                pr,
            )
        }
    }
}
