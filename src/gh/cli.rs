//! The GitHub CLI subprocess wrapper.
//!
//! Every GitHub interaction funnels through [`Gh`]. Keeping it in one
//! place means there is exactly one spot that knows how to build a
//! command, how to scope it to a repository, and how to turn a non-zero
//! exit into a diagnostic that names the likely cause.

use std::ffi::OsStr;
use std::process::Command;

use miette::Diagnostic;
use thiserror::Error;

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
        let mut cmd = Command::new("gh");
        cmd.args(args);
        if scoped && let Some(repo) = &self.repo {
            cmd.arg("--repo").arg(repo);
        }

        let display = display_args(args, scoped.then_some(self.repo.as_deref()).flatten());

        let output = cmd.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GhError::NotFound
            } else {
                GhError::Failed {
                    args: display.clone(),
                    stderr: e.to_string(),
                    help: None,
                }
            }
        })?;

        if output.status.success() {
            return String::from_utf8(output.stdout).map_err(|e| GhError::Decode {
                args: display,
                message: e.to_string(),
            });
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(classify(&display, &stderr))
    }

    /// Run `gh` and parse stdout as JSON.
    pub fn json<T, S>(&self, args: &[S]) -> Result<T, GhError>
    where
        T: serde::de::DeserializeOwned,
        S: AsRef<OsStr>,
    {
        let out = self.run_scoped(args)?;
        serde_json::from_str(&out).map_err(|e| GhError::Decode {
            args: display_args(args, self.repo.as_deref()),
            message: e.to_string(),
        })
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
        Some(
            "the token lacks the required permissions. gh-ship needs `repo` and `workflow` scopes."
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

    #[test]
    fn explains_permission_failures() {
        let e = classify(
            "workflow run",
            "HTTP 403: Resource not accessible by integration",
        );
        let GhError::Failed { help, .. } = &e else {
            panic!("expected Failed")
        };
        assert!(help.as_ref().unwrap().contains("workflow"), "{help:?}");
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

    #[test]
    fn display_args_appends_repo() {
        assert_eq!(
            display_args(&["run", "list"], Some("o/r")),
            "run list --repo o/r"
        );
        assert_eq!(display_args(&["run", "list"], None), "run list");
    }

    #[test]
    fn missing_gh_binary_is_reported_clearly() {
        // An empty PATH guarantees `gh` cannot be found.
        let gh = Gh::new(None);
        let saved = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "") };
        let result = gh.run(&["repo", "view"]);
        if let Some(p) = saved {
            unsafe { std::env::set_var("PATH", p) };
        }
        assert!(matches!(result, Err(GhError::NotFound)), "{result:?}");
    }
}
