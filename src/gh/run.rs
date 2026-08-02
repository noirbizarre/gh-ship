//! Dispatching workflows and finding the run that resulted.
//!
//! # The correlation problem
//!
//! `gh workflow run` — and the REST endpoint behind it — returns **204
//! No Content**. No run id, no URL, nothing. There is no API that says
//! "the dispatch you just made became run 12345".
//!
//! The obvious workaround is to list recent runs and take the newest one
//! created after the dispatch. That is wrong in every interesting case:
//! a teammate dispatching concurrently, a scheduled run, a push landing
//! at the same moment, or GitHub queueing the run seconds later. It
//! fails rarely enough to pass testing and often enough to corrupt a
//! release.
//!
//! So gh-ship makes correlation explicit and part of the protocol:
//!
//! 1. It generates a nonce and passes it as the `ship_id` input.
//! 2. The workflow is required to stamp it into its own `run-name`.
//! 3. gh-ship polls the run list and matches on that nonce.
//!
//! This is why `run-name` is mandatory and why `gh ship validate`
//! refuses a workflow without it: the alternative is a guess.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::cli::{Gh, GhError};
use super::workflow::WorkflowRef;

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
    env_duration("SHIP_APPEAR_TIMEOUT").unwrap_or(APPEAR_TIMEOUT)
}

/// The run timeout, overridable via `SHIP_RUN_TIMEOUT` (seconds).
pub fn run_timeout() -> Duration {
    env_duration("SHIP_RUN_TIMEOUT").unwrap_or(RUN_TIMEOUT)
}

fn env_duration(key: &str) -> Option<Duration> {
    std::env::var(key)
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// How long to wait for a run to *finish*, by default.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Polling interval bounds. Starts tight so short runs feel immediate,
/// backs off so a long run does not hammer the API.
const POLL_MIN: Duration = Duration::from_secs(2);
const POLL_MAX: Duration = Duration::from_secs(15);

/// The projection `gh run` is asked for.
///
/// One constant rather than a literal per call site: `list` and `view` must
/// agree, since both deserialize into [`Run`].
const RUN_FIELDS: &str = "databaseId,displayTitle,status,conclusion,url,headBranch";

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
}

/// A correlation nonce.
///
/// Short enough to read in a run title, long enough not to collide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipId(String);

impl ShipId {
    /// Generate a fresh nonce.
    pub fn generate() -> Self {
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        Self(uuid[..12].to_string())
    }

    /// Wrap an existing value, for tests and for resuming.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The marker a conforming workflow puts in its `run-name`.
    pub fn marker(&self) -> String {
        format!("ship:{}", self.0)
    }
}

impl std::fmt::Display for ShipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Errors specific to dispatching and tracking runs.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
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
        "the most likely cause is that `{workflow}` does not stamp the nonce into its `run-name`. \
         gh-ship finds your run by looking for `ship:<id>` in the run title, because dispatching \
         returns no run id. Check with `gh ship validate`, and confirm the workflow exists on the \
         dispatched branch — `workflow_dispatch` reads the workflow file from that ref, not from \
         your default branch."
    )
}

/// Dispatch with a caller-supplied nonce.
///
/// `prepare` stages its work on a branch named after the nonce, so the branch,
/// the dispatch and the resulting run all carry one identifier. That is what
/// makes an abandoned staging branch traceable to the run that abandoned it.
pub fn dispatch_as(
    gh: &Gh,
    workflow: &WorkflowRef,
    branch: &str,
    ship_id: &ShipId,
    inputs: &[(&str, String)],
) -> Result<ShipId, RunError> {
    let mut args: Vec<String> = vec![
        "workflow".into(),
        "run".into(),
        // The API resolves a workflow by filename, name or numeric id —
        // never by slug — so the id is what must go on the wire.
        workflow.id.clone(),
        "--ref".into(),
        branch.into(),
        "-f".into(),
        format!("{}={}", super::workflow::SHIP_ID_INPUT, ship_id),
    ];
    for (key, value) in inputs {
        args.push("-f".into());
        args.push(format!("{key}={value}"));
    }

    gh.run_scoped(&args)?;
    Ok(ship_id.clone())
}

/// Poll until a run carrying `ship_id` appears.
pub fn find(
    gh: &Gh,
    workflow: &WorkflowRef,
    branch: &str,
    ship_id: &ShipId,
    timeout: Duration,
    mut on_wait: impl FnMut(Duration),
) -> Result<Run, RunError> {
    let started = Instant::now();
    let mut interval = POLL_MIN;

    loop {
        if let Some(run) = list(gh, workflow, branch)?
            .into_iter()
            .find(|r| r.title.contains(&ship_id.marker()))
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

/// Poll a known run until it reaches a terminal state.
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
    fn nonces_are_short_and_unique() {
        let a = ShipId::generate();
        let b = ShipId::generate();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 12, "short enough to read in a run title");
        assert!(a.as_str().chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn marker_is_what_the_workflow_template_emits() {
        let id = ShipId::new("abc123");
        assert_eq!(id.marker(), "ship:abc123");
        // The template interpolates `ship:${{ inputs.ship_id }}`, so a
        // rendered title looks like this:
        let title = "prepare-release (ship:abc123)";
        assert!(title.contains(&id.marker()));
    }

    #[test]
    fn markers_do_not_match_a_different_nonce() {
        let mine = ShipId::new("aaaaaaaaaaaa");
        let theirs = "prepare-release (ship:bbbbbbbbbbbb)";
        assert!(
            !theirs.contains(&mine.marker()),
            "a concurrent dispatch must never be mistaken for ours"
        );
    }

    #[test]
    fn run_status_predicates() {
        let queued = Run {
            id: 1,
            title: "t".into(),
            status: "queued".into(),
            conclusion: String::new(),
            url: String::new(),
            head_branch: "release/next".into(),
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
            "displayTitle": "prepare-release (ship:deadbeef1234)",
            "status": "completed",
            "conclusion": "success",
            "url": "https://github.com/o/r/actions/runs/42",
            "headBranch": "release/next"
        }]"#;
        let runs: Vec<Run> = serde_json::from_str(json).unwrap();
        assert_eq!(runs[0].id, 42);
        assert!(runs[0].succeeded());
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
        assert!(help.contains("run-name"), "{help}");
        assert!(
            help.contains("from that ref"),
            "dispatching a ref whose workflow file lacks the trigger is the second \
             most common cause and must be mentioned: {help}"
        );
    }
}
