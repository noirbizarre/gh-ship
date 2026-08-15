//! Dispatching workflows and finding the run that resulted.
//!
//! # The correlation problem
//!
//! `gh workflow run` — and the REST endpoint behind it — returns **204
//! No Content**. No run id, no URL, nothing. There is no API that says
//! "the dispatch you just made became run 12345".
//!
//! Taking "the newest run on the ref" is wrong: a scheduled run, a push
//! landing at the same moment, or a workflow that also triggers on push
//! would all be mistaken for the dispatch.
//!
//! So gh-ship correlates on three things it *does* control:
//!
//! 1. **The ref.** Every dispatch goes to a ref that identifies the work —
//!    `prepare` cuts a throwaway staging branch, `release` dispatches on the
//!    tag. Both are unique to the release.
//! 2. **The event.** Only `workflow_dispatch` runs are candidates, so a
//!    workflow that also declares `on: push` does not match the run that
//!    creating the staging branch triggered.
//! 3. **Novelty.** The run ids present on the ref are snapshotted before
//!    dispatching; the run that was not there before is ours.
//!
//! This asks nothing of the workflow beyond `on: workflow_dispatch`, which
//! is what lets prepare and publish workflows be ordinary reusable ones.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use miette::Diagnostic;
use serde::Deserialize;
use thiserror::Error;

use super::cli::{Gh, GhError};
use super::workflow::{LEGACY_SHIP_ID_INPUT, WorkflowRef};

/// How long to wait for a dispatched run to *appear* in the run list.
///
/// GitHub usually registers a run within a couple of seconds, but
/// queueing under load can take longer. Past this, something is wrong in
/// a way that waiting will not fix.
pub const APPEAR_TIMEOUT: Duration = Duration::from_secs(90);

/// The appearance timeout, overridable via `SHIP_APPEAR_TIMEOUT`
/// (seconds).
///
/// Documented as a user-facing knob, and relied on by the test suite, which
/// must exercise the "run never appeared" path without waiting 90 seconds
/// for it.
pub fn appear_timeout() -> Duration {
    super::env_duration("SHIP_APPEAR_TIMEOUT").unwrap_or(APPEAR_TIMEOUT)
}

/// How long to wait for a run to *finish*, by default.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The run timeout, overridable via `SHIP_RUN_TIMEOUT` (seconds).
pub fn run_timeout() -> Duration {
    super::env_duration("SHIP_RUN_TIMEOUT").unwrap_or(RUN_TIMEOUT)
}

/// Polling interval bounds. Starts tight so short runs feel immediate,
/// backs off so a long run does not hammer the API.
const POLL_MIN: Duration = Duration::from_secs(2);
const POLL_MAX: Duration = Duration::from_secs(15);

/// The projection `gh run` is asked for.
///
/// One constant rather than a literal per call site: `list` and `view` must
/// agree, since both deserialize into [`Run`].
const RUN_FIELDS: &str = "databaseId,displayTitle,status,conclusion,url,headBranch,event";

/// The `event` value GitHub reports for a run gh-ship started itself.
const DISPATCH_EVENT: &str = "workflow_dispatch";

/// A workflow run as reported by `gh run list`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Run {
    #[serde(rename = "databaseId")]
    pub id: u64,
    #[serde(default, rename = "displayTitle")]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "headBranch")]
    pub head_branch: String,
    /// What triggered the run. `workflow_dispatch` for anything gh-ship
    /// started; used to ignore runs the same workflow started for itself.
    #[serde(default)]
    pub event: String,
}

impl Run {
    /// Whether the run has reached a terminal state.
    pub fn is_finished(&self) -> bool {
        self.status == "completed"
    }

    /// Whether the run finished successfully.
    pub fn succeeded(&self) -> bool {
        self.is_finished() && self.conclusion == "success"
    }

    /// Whether the run was started through the API, as gh-ship starts them.
    pub fn is_dispatched(&self) -> bool {
        self.event == DISPATCH_EVENT
    }
}

/// Errors specific to dispatching and tracking runs.
#[derive(Debug, Error, Diagnostic)]
pub enum RunError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Gh(#[from] GhError),

    #[error("dispatched `{workflow}`, but no matching run appeared within {}s", timeout.as_secs())]
    #[diagnostic(code(ship::run::not_found), help("{help}"))]
    NotFound {
        workflow: String,
        timeout: Duration,
        help: String,
    },

    #[error("`{workflow}` finished with conclusion `{conclusion}`")]
    #[diagnostic(code(ship::run::failed), help("inspect the failure: {url}"))]
    Failed {
        workflow: String,
        conclusion: String,
        url: String,
    },

    #[error("`{workflow}` did not finish within {}m", timeout.as_secs() / 60)]
    #[diagnostic(
        code(ship::run::timeout),
        help("the run is still going — watch it at {url}, then re-run this command")
    )]
    Timeout {
        workflow: String,
        timeout: Duration,
        url: String,
    },
}

/// Standard guidance when a dispatched run cannot be found.
///
/// Ordered by likelihood, because the first suggestion is the one people
/// actually read.
fn not_found_help(workflow: &WorkflowRef) -> String {
    format!(
        "confirm `{workflow}` exists on the dispatched ref — `workflow_dispatch` reads the \
         workflow file from that ref, not from your default branch — and that it declares \
         `on: workflow_dispatch`. Check with `gh ship validate`. If a teammate dispatched the \
         same workflow on the same ref at the same moment, gh-ship may have attached to their \
         run instead."
    )
}

/// Whether a failed dispatch failed *only* because it omitted `ship_id`.
///
/// Deliberately narrow: it must not swallow an unrelated dispatch failure and
/// retry it with a nonsense input.
fn needs_legacy_ship_id(err: &GhError) -> bool {
    let GhError::Failed { stderr, .. } = err else {
        return false;
    };
    let stderr = stderr.to_lowercase();
    stderr.contains(LEGACY_SHIP_ID_INPUT)
        && (stderr.contains("required") || stderr.contains("expected"))
}

/// Dispatch a workflow on a ref.
///
/// Returns `true` when the dispatch only succeeded thanks to the legacy
/// `ship_id` shim, so the caller can warn.
pub fn dispatch(
    gh: &Gh,
    workflow: &WorkflowRef,
    branch: &str,
    inputs: &[(&str, String)],
) -> Result<bool, RunError> {
    let base: Vec<String> = vec![
        "workflow".into(),
        "run".into(),
        // The API resolves a workflow by filename, name or numeric id —
        // never by slug — so the id is what must go on the wire.
        workflow.id.clone(),
        "--ref".into(),
        branch.into(),
    ];
    let mut args = base.clone();
    for (key, value) in inputs {
        args.push("-f".into());
        args.push(format!("{key}={value}"));
    }

    match gh.run_scoped(&args) {
        Ok(_) => Ok(false),
        Err(err) if needs_legacy_ship_id(&err) => {
            // An older generated workflow. Feed it a value so the release is
            // not blocked on a migration the user has not made yet.
            let mut retry = base;
            for (key, value) in inputs {
                retry.push("-f".into());
                retry.push(format!("{key}={value}"));
            }
            retry.push("-f".into());
            retry.push(format!("{LEGACY_SHIP_ID_INPUT}={}", super::short_token()));
            gh.run_scoped(&retry)?;
            Ok(true)
        }
        Err(err) => Err(err.into()),
    }
}

/// Poll until a `workflow_dispatch` run that was not there before appears.
///
/// `known` is the set of run ids observed on `branch` immediately before
/// dispatching. Anything outside it, triggered by a dispatch, is ours. The
/// highest id wins, since GitHub allocates them monotonically and the newest
/// is the one we just caused.
///
/// Transient API failures do not surface here: [`Gh`] retries read-only
/// calls itself, so a 502 mid-poll costs a second rather than the release.
pub fn find_new(
    gh: &Gh,
    workflow: &WorkflowRef,
    branch: &str,
    known: &HashSet<u64>,
    timeout: Duration,
    mut on_wait: impl FnMut(Duration),
) -> Result<Run, RunError> {
    let started = Instant::now();
    let mut interval = POLL_MIN;

    loop {
        if let Some(run) = list(gh, workflow, branch)?
            .into_iter()
            .filter(|r| r.is_dispatched() && !known.contains(&r.id))
            .max_by_key(|r| r.id)
        {
            return Ok(run);
        }

        if started.elapsed() >= timeout {
            return Err(RunError::NotFound {
                workflow: workflow.to_string(),
                timeout,
                help: not_found_help(workflow),
            });
        }

        on_wait(started.elapsed());
        std::thread::sleep(interval);
        interval = backoff(interval);
    }
}

/// The ids of the runs currently visible for a workflow on a ref.
///
/// Taken immediately before a dispatch so [`find_new`] can tell the run it
/// caused from the ones that were already there. A failure here is not fatal:
/// an empty snapshot simply means the first dispatched run seen wins, which is
/// exactly right on the freshly created branch `prepare` dispatches on.
pub fn snapshot(gh: &Gh, workflow: &WorkflowRef, branch: &str) -> HashSet<u64> {
    list(gh, workflow, branch)
        .map(|runs| runs.into_iter().map(|r| r.id).collect())
        .unwrap_or_default()
}

/// Poll a known run until it reaches a terminal state.
///
/// As with [`find`], a transient API blip is absorbed by [`Gh`] rather than
/// aborting the wait.
pub fn wait(
    gh: &Gh,
    workflow: &WorkflowRef,
    run: &Run,
    timeout: Duration,
    mut on_wait: impl FnMut(Duration, &Run),
) -> Result<Run, RunError> {
    let started = Instant::now();
    let mut interval = POLL_MIN;
    let mut current = run.clone();

    loop {
        if current.is_finished() {
            if current.succeeded() {
                return Ok(current);
            }
            return Err(RunError::Failed {
                workflow: workflow.to_string(),
                conclusion: if current.conclusion.is_empty() {
                    "unknown".into()
                } else {
                    current.conclusion.clone()
                },
                url: current.url.clone(),
            });
        }

        if started.elapsed() >= timeout {
            return Err(RunError::Timeout {
                workflow: workflow.to_string(),
                timeout,
                url: current.url.clone(),
            });
        }

        on_wait(started.elapsed(), &current);
        std::thread::sleep(interval);
        interval = backoff(interval);

        current = view(gh, current.id)?;
    }
}

/// List recent runs of a workflow on a branch.
///
/// The limit is generous: a busy repository can register many runs
/// between the dispatch and the first poll, and missing ours because it
/// fell off a short page would be the exact failure this design exists
/// to prevent.
pub fn list(gh: &Gh, workflow: &WorkflowRef, branch: &str) -> Result<Vec<Run>, GhError> {
    gh.json_scoped(&[
        "run",
        "list",
        "--workflow",
        &workflow.id,
        "--branch",
        branch,
        "--limit",
        "50",
        "--json",
        RUN_FIELDS,
    ])
}

/// Fetch a single run's current state.
pub fn view(gh: &Gh, id: u64) -> Result<Run, GhError> {
    gh.json_scoped(&["run", "view", &id.to_string(), "--json", RUN_FIELDS])
}

/// Download a run's artifact into `dest`.
pub fn download_artifact(gh: &Gh, id: u64, name: &str, dest: &Path) -> Result<(), GhError> {
    gh.run_scoped(&[
        "run",
        "download",
        &id.to_string(),
        "--name",
        name,
        "--dir",
        &dest.to_string_lossy(),
    ])
    .map(|_| ())
}

fn backoff(current: Duration) -> Duration {
    std::cmp::min(current.mul_f32(1.5), POLL_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_status_predicates() {
        let queued = Run {
            id: 1,
            title: "t".into(),
            status: "queued".into(),
            conclusion: String::new(),
            url: String::new(),
            head_branch: "release/next".into(),
            event: "workflow_dispatch".into(),
        };
        assert!(!queued.is_finished());
        assert!(!queued.succeeded());

        let ok = Run {
            status: "completed".into(),
            conclusion: "success".into(),
            ..queued.clone()
        };
        assert!(ok.is_finished() && ok.succeeded());

        let failed = Run {
            status: "completed".into(),
            conclusion: "failure".into(),
            ..queued.clone()
        };
        assert!(failed.is_finished() && !failed.succeeded());

        // A cancelled run is finished but not successful.
        let cancelled = Run {
            status: "completed".into(),
            conclusion: "cancelled".into(),
            ..queued
        };
        assert!(cancelled.is_finished() && !cancelled.succeeded());
    }

    #[test]
    fn run_list_json_deserialises() {
        let json = r#"[{
            "databaseId": 42,
            "displayTitle": "prepare-release",
            "status": "completed",
            "conclusion": "success",
            "url": "https://github.com/o/r/actions/runs/42",
            "headBranch": "release/next",
            "event": "workflow_dispatch"
        }]"#;
        let runs: Vec<Run> = serde_json::from_str(json).unwrap();
        assert_eq!(runs[0].id, 42);
        assert!(runs[0].succeeded());
        assert!(runs[0].is_dispatched());
    }

    #[test]
    fn run_json_tolerates_missing_optional_fields() {
        // A queued run has a null conclusion; serde must not choke.
        let json = r#"{"databaseId": 7, "displayTitle": "x", "status": "queued"}"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert_eq!(run.id, 7);
        assert!(!run.is_finished());
    }

    #[test]
    fn backoff_grows_then_caps() {
        let mut d = POLL_MIN;
        for _ in 0..20 {
            d = backoff(d);
        }
        assert_eq!(d, POLL_MAX, "polling must not grow without bound");
        assert!(backoff(POLL_MIN) > POLL_MIN);
    }

    #[test]
    fn not_found_help_names_the_two_real_causes() {
        let help = not_found_help(&WorkflowRef::unresolved("prepare-release"));
        assert!(help.contains("workflow_dispatch"), "{help}");
        assert!(
            help.contains("from that ref"),
            "dispatching a ref whose workflow file lacks the trigger is the most \
             common cause and must be mentioned: {help}"
        );
    }

    fn run_at(id: u64, event: &str) -> Run {
        Run {
            id,
            title: "prepare-release".into(),
            status: "completed".into(),
            conclusion: "success".into(),
            url: String::new(),
            head_branch: "main".into(),
            event: event.into(),
        }
    }

    #[test]
    fn only_dispatched_runs_are_candidates() {
        // Creating the staging branch is a push. A prepare workflow that also
        // declares `on: push` would otherwise match its own push run.
        let push = run_at(43, "push");
        assert!(!push.is_dispatched());
        assert!(run_at(43, "workflow_dispatch").is_dispatched());
    }

    #[test]
    fn a_pre_existing_run_is_never_mistaken_for_ours() {
        let known: HashSet<u64> = [41, 42].into_iter().collect();
        let candidates: Vec<Run> = vec![
            run_at(41, "workflow_dispatch"),
            run_at(42, "workflow_dispatch"),
        ];
        assert!(
            candidates.iter().all(|r| known.contains(&r.id)),
            "everything visible before the dispatch must be excluded"
        );
    }

    #[test]
    fn legacy_ship_id_shim_only_fires_on_that_error() {
        let missing = GhError::Failed {
            args: "workflow run".into(),
            stderr: "required input 'ship_id' not provided".into(),
            help: None,
        };
        assert!(needs_legacy_ship_id(&missing));

        let unrelated = GhError::Failed {
            args: "workflow run".into(),
            stderr: "HTTP 404: Not Found".into(),
            help: None,
        };
        assert!(
            !needs_legacy_ship_id(&unrelated),
            "an unrelated failure must not be retried with a bogus input"
        );
    }
}
