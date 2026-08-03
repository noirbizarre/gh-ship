//! The GitHub CLI subprocess wrapper.
//!
//! Every GitHub interaction funnels through [`Gh`]. Keeping it in one
//! place means there is exactly one spot that knows how to build a
//! command, how to scope it to a repository, and how to turn a non-zero
//! exit into a diagnostic that names the likely cause.

use std::ffi::OsStr;
use std::process::Command;
use std::time::Duration;

use miette::Diagnostic;
use thiserror::Error;

/// How many *extra* attempts a transient failure earns.
///
/// GitHub occasionally answers a perfectly valid query with a 502 or 504,
/// especially on the GraphQL endpoint the `--json` flags use. Those clear
/// within seconds, so failing the whole release for one of them wastes a
/// human's afternoon on a problem that fixed itself.
pub const RETRIES: u32 = 3;

/// The delay before the first retry. Doubles per attempt, capped at
/// [`RETRY_MAX_DELAY`].
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

/// The ceiling on the retry backoff.
const RETRY_MAX_DELAY: Duration = Duration::from_secs(8);

/// The retry count, overridable via `SHIP_GH_RETRIES`. `0` disables retrying.
fn retries() -> u32 {
    std::env::var("SHIP_GH_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(RETRIES)
}

/// The initial retry delay, overridable via `SHIP_GH_RETRY_DELAY` (seconds).
///
/// Mostly a test knob: the suite sets it to `0` so it never sleeps through a
/// backoff it is not measuring.
fn retry_delay() -> Duration {
    super::env_duration("SHIP_GH_RETRY_DELAY").unwrap_or(RETRY_DELAY)
}

/// Errors from invoking the GitHub CLI.
#[derive(Debug, Error, Diagnostic)]
pub enum GhError {
    #[error("the GitHub CLI (`gh`) is not installed or not on PATH")]
    #[diagnostic(
        code(ship::gh::missing),
        help(
            "gh-ship is a gh extension and delegates all GitHub access to it — install it from https://cli.github.com"
        )
    )]
    NotFound,

    #[error("not authenticated with GitHub")]
    #[diagnostic(
        code(ship::gh::auth),
        help("run `gh auth login`, or set GH_TOKEN in CI")
    )]
    NotAuthenticated,

    #[error("not inside a GitHub repository")]
    #[diagnostic(
        code(ship::gh::no_repo),
        help("run this from a repository clone, or pass `--repo OWNER/REPO`")
    )]
    NoRepository,

    #[error("`gh {args}` failed: {stderr}")]
    #[diagnostic(code(ship::gh::failed))]
    Failed {
        args: String,
        stderr: String,
        #[help]
        help: Option<String>,
    },

    #[error("could not read the output of `gh {args}`: {message}")]
    #[diagnostic(
        code(ship::gh::decode),
        help(
            "this usually means the installed `gh` is older than gh-ship expects; try `gh --version` and upgrade"
        )
    )]
    Decode { args: String, message: String },
}

/// A configured GitHub CLI invoker.
#[derive(Debug, Clone, Default)]
pub struct Gh {
    /// `OWNER/REPO`, or `None` to let `gh` infer it from the checkout.
    repo: Option<String>,
}

impl Gh {
    pub fn new(repo: Option<String>) -> Self {
        Self { repo }
    }

    /// The explicitly configured repository, if any.
    pub fn repo(&self) -> Option<&str> {
        self.repo.as_deref()
    }

    /// Run `gh` with the given arguments and capture stdout.
    ///
    /// `--repo` is *not* injected automatically: several `gh` commands
    /// reject it, so each call site opts in via [`Self::run_scoped`].
    pub fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String, GhError> {
        self.exec(args, false)
    }

    /// Run `gh`, injecting `--repo` when one is configured.
    pub fn run_scoped<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String, GhError> {
        self.exec(args, true)
    }

    fn exec<S: AsRef<OsStr>>(&self, args: &[S], scoped: bool) -> Result<String, GhError> {
        let repo = scoped.then_some(self.repo.as_deref()).flatten();
        let display = display_args(args, repo);

        let attempts = retries();
        let mut delay = retry_delay();

        for attempt in 0..=attempts {
            // A `Command` cannot be reused after `output()`, so it is built
            // fresh per attempt. The arguments are identical every time: a
            // retry must ask the same question, not a slightly different one.
            let mut cmd = Command::new("gh");
            cmd.args(args);
            if let Some(repo) = repo {
                cmd.arg("--repo").arg(repo);
            }

            // A spawn failure is never retried: `gh` being missing or
            // unrunnable is not a condition that clears on its own.
            let output = cmd.output().map_err(|e| spawn_error(&display, &e))?;

            if output.status.success() {
                return String::from_utf8(output.stdout).map_err(|e| GhError::Decode {
                    args: display,
                    message: e.to_string(),
                });
            }

            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

            if attempt < attempts && is_retryable(args, &stderr) {
                std::thread::sleep(delay);
                delay = std::cmp::min(delay * 2, RETRY_MAX_DELAY);
                continue;
            }

            return Err(retried(classify(&display, &stderr), attempt));
        }

        unreachable!("the loop returns on its last iteration")
    }

    /// Run `gh` and parse stdout as JSON.
    ///
    /// `--repo` is *not* injected, mirroring [`Self::run`]: `gh api` rejects
    /// it outright, so each call site opts in via [`Self::json_scoped`].
    pub fn json<T, S>(&self, args: &[S]) -> Result<T, GhError>
    where
        T: serde::de::DeserializeOwned,
        S: AsRef<OsStr>,
    {
        self.decode(args, false)
    }

    /// Run `gh` and parse stdout as JSON, injecting `--repo` when configured.
    pub fn json_scoped<T, S>(&self, args: &[S]) -> Result<T, GhError>
    where
        T: serde::de::DeserializeOwned,
        S: AsRef<OsStr>,
    {
        self.decode(args, true)
    }

    fn decode<T, S>(&self, args: &[S], scoped: bool) -> Result<T, GhError>
    where
        T: serde::de::DeserializeOwned,
        S: AsRef<OsStr>,
    {
        let out = self.exec(args, scoped)?;
        serde_json::from_str(&out).map_err(|e| GhError::Decode {
            args: display_args(args, scoped.then_some(self.repo.as_deref()).flatten()),
            message: e.to_string(),
        })
    }
}

/// Map a failure to *spawn* `gh` onto an error.
///
/// Separate from [`classify`], which interprets what `gh` said: this one runs
/// when `gh` never ran at all. `NotFound` is worth singling out because it means
/// the extension is installed but its host is not, and the fix is "install the
/// GitHub CLI" rather than anything about the command.
fn spawn_error(display: &str, e: &std::io::Error) -> GhError {
    if e.kind() == std::io::ErrorKind::NotFound {
        GhError::NotFound
    } else {
        GhError::Failed {
            args: display.to_string(),
            stderr: e.to_string(),
            help: None,
        }
    }
}

/// Map a `gh` failure onto a specific error where we can recognise it.
///
/// Recognising these matters: the generic "command failed" message sends
/// people hunting through `gh` docs, while "run `gh auth login`" is
/// immediately actionable.
fn classify(display: &str, stderr: &str) -> GhError {
    let lower = stderr.to_lowercase();

    if lower.contains("not logged")
        || lower.contains("authentication")
        || lower.contains("gh auth login")
    {
        return GhError::NotAuthenticated;
    }
    if lower.contains("not a git repository") || lower.contains("could not determine") {
        return GhError::NoRepository;
    }

    let help = if lower.contains("could not add label")
        || lower.contains("label") && lower.contains("not found")
    {
        Some(
            "the label does not exist in this repository. gh-ship normally creates missing \
             labels, which needs `issues: write`; otherwise create it by hand or remove it \
             from `pull_request.labels` in .github/ship.yml."
                .to_string(),
        )
    } else if lower.contains("404") || lower.contains("could not resolve to a repository") {
        Some(
            "the repository does not exist, or your token cannot see it. On a private repository \
             GitHub returns 404 rather than 401, so this often means missing scopes rather than a typo."
                .to_string(),
        )
    } else if lower.contains("403") || lower.contains("resource not accessible") {
        // Name both token models. Saying only "repo and workflow scopes" is
        // classic-PAT vocabulary and is useless to anyone holding a
        // fine-grained token, which is the default GitHub now offers.
        Some(
            "the token lacks the required permissions.\n\
             Fine-grained PAT — repository permissions: Actions: read and write, \
             Contents: read and write, Pull requests: read and write, \
             Issues: read and write, Metadata: read.\n\
             Classic PAT — scopes: `repo` and `workflow`.\n\
             Dispatching a workflow needs Actions: write specifically; content \
             access alone is not enough.\n\
             See https://noirbizarre.github.io/gh-ship/workflows/#what-the-token-must-be-allowed-to-do"
                .to_string(),
        )
    } else if lower.contains("rate limit") {
        Some("you have hit GitHub's API rate limit; wait and retry".to_string())
    } else {
        None
    };

    GhError::Failed {
        args: display.to_string(),
        stderr: stderr.to_string(),
        help,
    }
}

/// Whether a failed invocation is worth attempting again.
///
/// Both halves must hold. Transience alone is not enough: a 504 says GitHub
/// gave up on *answering*, not that it gave up on *acting*, so a timed-out
/// `pr create` may well have created the pull request. Retrying reads is free;
/// retrying writes invents duplicates.
fn is_retryable<S: AsRef<OsStr>>(args: &[S], stderr: &str) -> bool {
    is_read_only(args) && is_transient(stderr)
}

/// Whether the failure is GitHub having a bad moment rather than a real answer.
///
/// Rate limits are deliberately absent: they clear in minutes, not seconds, so
/// hammering them is both useless and rude. They already carry their own help.
fn is_transient(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();

    if lower.contains("rate limit") {
        return false;
    }

    const SIGNS: &[&str] = &[
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
        "we couldn't respond to your request in time",
        "internal server error",
        "server error",
        "connection reset",
        "connection refused",
        "timeout awaiting",
        "i/o timeout",
        "tls handshake timeout",
        "unexpected eof",
        "temporary failure in name resolution",
    ];

    SIGNS.iter().any(|sign| lower.contains(sign))
}

/// Whether the failure means "this does not exist" rather than "this went
/// wrong".
///
/// A missing branch, tag or release is an ordinary answer to a question,
/// not an error, so several helpers turn it into `Ok(false)` / `Ok(None)`.
/// The vocabulary is centralised here for the same reason [`is_transient`]
/// is: GitHub rewords its messages, and one table is one place to fix.
pub(super) fn is_not_found(error: &GhError) -> bool {
    const SIGNS: &[&str] = &["404", "not found", "reference does not exist"];
    matches_stderr(error, SIGNS)
}

/// Whether the failure means the thing we asked to create is already there.
///
/// The creations gh-ship performs are idempotent by intent — re-running a
/// release must not fail because the branch it wanted already exists.
pub(super) fn is_already_exists(error: &GhError) -> bool {
    const SIGNS: &[&str] = &["already exists", "422", "reference already exists"];
    matches_stderr(error, SIGNS)
}

fn matches_stderr(error: &GhError, signs: &[&str]) -> bool {
    let GhError::Failed { stderr, .. } = error else {
        return false;
    };
    let lower = stderr.to_lowercase();
    signs.iter().any(|sign| lower.contains(sign))
}

/// Whether an invocation only *reads* from GitHub.
///
/// Fails closed: anything unrecognised counts as a mutation, so a new call
/// site has to be added here deliberately rather than inheriting retries by
/// accident.
fn is_read_only<S: AsRef<OsStr>>(args: &[S]) -> bool {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();

    let Some(first) = args.first() else {
        return false;
    };

    // `gh api` carries its verb in a flag rather than a subcommand.
    if first == "api" {
        let mut verb_is_get = true;
        let mut iter = args.iter().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                // A body implies a write, whatever the method says.
                "-f" | "-F" | "--field" | "--raw-field" | "--input" => return false,
                "-X" | "--method" => {
                    verb_is_get = iter.next().is_some_and(|m| m.eq_ignore_ascii_case("GET"));
                }
                other => {
                    if let Some(method) = other.strip_prefix("--method=") {
                        verb_is_get = method.eq_ignore_ascii_case("GET");
                    }
                }
            }
        }
        return verb_is_get;
    }

    const READS: &[&str] = &["list", "view", "status", "download", "diff", "checks"];

    // The subcommand, positionally. Scanning past flags instead would be
    // worse, not better: a flag's *value* is indistinguishable from a
    // subcommand, so `pr --repo o/r list` would read `o/r` as the verb.
    // Every call site in gh-ship spells the verb second, and `--repo` is
    // appended at the end by `exec`.
    args.get(1)
        .is_some_and(|verb| READS.contains(&verb.as_str()))
}

/// Note on an exhausted error that it was already retried.
///
/// Without this the log says "`gh pr list` failed", and the reader's first
/// instinct is to retry by hand — which we already did, several times.
fn retried(error: GhError, attempt: u32) -> GhError {
    if attempt == 0 {
        return error;
    }

    let GhError::Failed { args, stderr, help } = error else {
        return error;
    };

    let note = format!(
        "this looked transient, so it was retried {attempt} more \
         time{} before giving up",
        if attempt == 1 { "" } else { "s" }
    );

    GhError::Failed {
        args,
        stderr,
        help: Some(match help {
            Some(help) => format!("{help}\n{note}"),
            None => note,
        }),
    }
}

fn display_args<S: AsRef<OsStr>>(args: &[S], repo: Option<&str>) -> String {
    let mut parts: Vec<String> = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();
    if let Some(r) = repo {
        parts.push("--repo".into());
        parts.push(r.into());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn classifies_auth_failures() {
        let e = classify(
            "repo view",
            "gh: To use GitHub CLI in a GitHub Actions workflow, set the GH_TOKEN environment variable. authentication required",
        );
        assert!(matches!(e, GhError::NotAuthenticated), "{e:?}");
    }

    #[test]
    fn classifies_missing_repository() {
        let e = classify("repo view", "fatal: not a git repository (or any parent)");
        assert!(matches!(e, GhError::NoRepository), "{e:?}");
    }

    #[test]
    fn explains_that_404_may_mean_missing_scopes() {
        // This exact confusion cost the predecessor project a debugging
        // session: GitHub answers 404 (not 401) for private repos when
        // the token lacks scope.
        let e = classify("run list", "HTTP 404: Not Found");
        let GhError::Failed { help, .. } = &e else {
            panic!("expected Failed, got {e:?}")
        };
        assert!(help.as_ref().unwrap().contains("404"), "{help:?}");
    }

    /// The 403 help must serve both token models. Naming only classic scopes
    /// is useless to anyone holding a fine-grained token — which is what
    /// GitHub offers by default, and what actually failed in practice.
    #[test]
    fn explains_permission_failures_for_both_token_kinds() {
        let e = classify(
            "workflow run",
            "HTTP 403: Resource not accessible by integration",
        );
        let GhError::Failed { help, .. } = &e else {
            panic!("expected Failed")
        };
        let help = help.as_ref().expect("403 must carry help");

        // Classic PAT vocabulary.
        assert!(help.contains("`repo`"), "{help}");
        assert!(help.contains("`workflow`"), "{help}");

        // Fine-grained PAT vocabulary — every permission gh-ship needs.
        for permission in ["Actions", "Contents", "Pull requests", "Issues", "Metadata"] {
            assert!(
                help.contains(permission),
                "fine-grained permission `{permission}` missing from: {help}"
            );
        }

        // The specific cause of the real-world failure.
        assert!(help.contains("Actions: write"), "{help}");
    }

    /// A fine-grained token reports differently from a GitHub App; both must
    /// be recognised as permission problems.
    #[test]
    fn recognises_the_fine_grained_token_403_wording() {
        let e = classify(
            "workflow run",
            "HTTP 403: Resource not accessible by personal access token",
        );
        let GhError::Failed { help, .. } = &e else {
            panic!("expected Failed")
        };
        assert!(help.as_ref().unwrap().contains("Actions"), "{help:?}");
    }

    #[test]
    fn unknown_failures_stay_generic() {
        let e = classify("repo view", "something weird happened");
        let GhError::Failed { help, stderr, .. } = &e else {
            panic!("expected Failed")
        };
        assert!(help.is_none());
        assert_eq!(stderr, "something weird happened");
    }

    /// The exact failure that killed a real release run: GitHub's GraphQL
    /// endpoint gave up mid-query on a plain read.
    #[test]
    fn the_504_that_broke_a_release_is_retryable() {
        let stderr = "HTTP 504: We couldn't respond to your request in time. Sorry about that. \
                      Please try resubmitting your request and contact us if the problem \
                      persists. (https://api.github.com/graphql)";
        let args = [
            "pr",
            "list",
            "--head",
            "release/next",
            "--base",
            "main",
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "number,url,title,body,state,mergeCommit",
        ];
        assert!(is_retryable(&args, stderr));
    }

    /// Rate limits are not a blip. Retrying them within seconds cannot
    /// succeed, and the error already says to wait.
    #[test]
    fn rate_limits_are_not_retried() {
        assert!(!is_transient("HTTP 403: API rate limit exceeded"));
    }

    /// A real answer is a real answer, however unwelcome.
    #[test]
    fn definite_failures_are_not_transient() {
        assert!(!is_transient("HTTP 404: Not Found"));
        assert!(!is_transient("HTTP 403: Resource not accessible"));
        assert!(!is_transient("something weird happened"));
    }

    /// The whole point of gating on read-only: a 504 means GitHub stopped
    /// *answering*, not that it stopped *acting*. Retrying a write invents
    /// duplicate pull requests, tags and releases.
    #[test]
    fn mutations_are_never_retried_however_transient_the_failure() {
        let stderr = "HTTP 502: Bad gateway";
        for args in [
            vec!["pr", "create", "--title", "x"],
            vec!["release", "create", "v1.0.0"],
            vec!["workflow", "run", "ship.yml"],
            vec!["label", "create", "release"],
            vec!["pr", "merge", "12", "--merge"],
            vec!["api", "-X", "POST", "repos/o/r/git/refs"],
            vec!["api", "repos/o/r/git/refs", "-f", "ref=x"],
        ] {
            assert!(!is_retryable(&args, stderr), "{args:?} must not be retried");
        }
    }

    #[test]
    fn reads_are_recognised() {
        for args in [
            vec!["pr", "list", "--json", "number"],
            vec!["pr", "view", "12"],
            vec!["run", "view", "42", "--json", "status"],
            vec!["run", "download", "42"],
            vec!["release", "view", "v1.0.0"],
            vec!["repo", "view"],
            vec!["auth", "status"],
            vec!["api", "repos/o/r/git/matching-refs/tags"],
            vec!["api", "--method", "GET", "repos/o/r"],
        ] {
            assert!(is_read_only(&args), "{args:?} must count as a read");
        }
    }

    /// The verb is read positionally, so a flag sitting where the
    /// subcommand belongs must not be mistaken for one.
    #[test]
    fn a_misplaced_flag_is_not_a_subcommand() {
        assert!(!is_read_only(&["pr", "--repo", "o/r", "list"]));
    }

    /// An empty invocation cannot be proven safe, so it is not.
    #[test]
    fn unknown_shapes_fail_closed() {
        assert!(!is_read_only::<&str>(&[]));
        assert!(!is_read_only(&["pr"]));
        assert!(!is_read_only(&["totally-new-command", "poke"]));
    }

    /// An exhausted retry has to say so, or the reader's first instinct is
    /// to do by hand what we already did three times.
    #[test]
    fn an_exhausted_error_admits_it_was_retried() {
        let e = retried(classify("pr list", "HTTP 504: gateway timeout"), 3);
        let GhError::Failed { help, stderr, .. } = &e else {
            panic!("expected Failed, got {e:?}")
        };
        assert!(stderr.contains("504"), "the original message must survive");
        assert!(
            help.as_ref().unwrap().contains("retried 3 more times"),
            "{help:?}"
        );
    }

    /// A first-attempt failure is not a retry, and must not claim to be.
    #[test]
    fn a_first_attempt_failure_is_left_alone() {
        let e = retried(classify("pr list", "something weird happened"), 0);
        let GhError::Failed { help, .. } = &e else {
            panic!("expected Failed")
        };
        assert!(help.is_none(), "{help:?}");
    }

    #[test]
    fn display_args_appends_repo() {
        assert_eq!(
            display_args(&["run", "list"], Some("o/r")),
            "run list --repo o/r"
        );
        assert_eq!(display_args(&["run", "list"], None), "run list");
    }

    // The two halves of "gh is not installed", tested without touching the
    // environment. The previous version of this set `PATH` to an empty string
    // for the duration of the call — process-global state, mutated while the
    // rest of the suite runs in parallel, and left empty if `PATH` had been
    // unset to begin with.

    /// A missing binary must produce the actionable error, not a generic one:
    /// the fix is "install the GitHub CLI", which the message has to say.
    #[test]
    fn a_missing_gh_binary_is_reported_clearly() {
        let e = spawn_error("repo view", &io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(e, GhError::NotFound), "{e:?}");

        let help = miette::Diagnostic::help(&e).expect("NotFound carries help");
        assert!(help.to_string().contains("cli.github.com"), "{help}");
    }

    /// Any other spawn failure keeps its original message rather than being
    /// mislabelled as "not installed".
    #[test]
    fn other_spawn_failures_are_not_mistaken_for_a_missing_binary() {
        let e = spawn_error(
            "repo view",
            &io::Error::from(io::ErrorKind::PermissionDenied),
        );
        let GhError::Failed { args, stderr, .. } = &e else {
            panic!("expected Failed, got {e:?}")
        };
        assert_eq!(args, "repo view");
        assert!(!stderr.is_empty());
    }

    /// The half that belongs to the standard library: a binary that is not on
    /// `PATH` really does surface as `NotFound`. Asserted against a name that
    /// cannot exist, rather than by emptying `PATH` for every other test.
    #[test]
    fn spawning_an_absent_binary_yields_not_found() {
        let err = std::process::Command::new("gh-ship-no-such-binary-8f3a1c")
            .output()
            .expect_err("this binary cannot exist");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
