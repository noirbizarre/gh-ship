//! Shared helpers for integration tests.
//!
//! Everything here is hermetic: tests run the real binary in a
//! throwaway directory with colour disabled, so snapshots are stable
//! across machines and CI.

#![allow(dead_code)]

pub mod stub;

use std::path::Path;

use assert_cmd::Command;

pub use stub::GhStub;

/// A workflow satisfying the gh-ship contract, used by lifecycle tests.
pub const CONFORMING_WORKFLOW: &str = r#"name: prepare-release
run-name: prepare-release (ship:${{ inputs.ship_id }})
on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
      dry_run: { required: false, type: boolean, default: false }
  workflow_call:
    inputs:
      ship_id: { required: true, type: string }
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#;

/// The smallest configuration gh-ship accepts.
pub const MINIMAL_CONFIG: &str = "version: 1\nworkflows:\n  prepare: prepare-release\n";

/// Lay out a repository on disk, without installing a `gh` stub.
///
/// Setup tests need only the files; lifecycle tests wrap this with a stub.
pub fn layout(config: Option<&str>, workflows: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let wf = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&wf).unwrap();
    for (name, body) in workflows {
        std::fs::write(wf.join(name), body).unwrap();
    }
    if let Some(c) = config {
        std::fs::write(dir.path().join(".github/ship.yml"), c).unwrap();
    }
    dir
}

/// A repository laid out on disk, with a stubbed `gh` on PATH.
pub struct Repo {
    pub dir: tempfile::TempDir,
    pub stub: stub::Installed,
    /// Extra environment for [`Repo::ship`], set by the `in_*` builders.
    env: Vec<(String, String)>,
}

impl Repo {
    /// Create a repository with the given config and a stubbed `gh`.
    pub fn new(config: &str, stub: GhStub) -> Self {
        let dir = layout(
            Some(config),
            &[("prepare-release.yml", CONFORMING_WORKFLOW)],
        );
        let installed = stub.install(dir.path());
        Self {
            dir,
            stub: installed,
            env: Vec::new(),
        }
    }

    /// Set an environment variable for subsequent [`Repo::ship`] calls.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Pin the base branch, as `--base` would.
    pub fn base(self, branch: &str) -> Self {
        self.with_env("SHIP_BASE_BRANCH", branch)
    }

    /// Pretend to be a GitHub Actions run triggered on a branch.
    pub fn in_ci(self, branch: &str) -> Self {
        self.with_env("GITHUB_ACTIONS", "true")
            .with_env("GITHUB_REF", &format!("refs/heads/{branch}"))
            .with_env("GITHUB_REF_NAME", branch)
    }

    /// Pretend to be a GitHub Actions run triggered by a pull request
    /// targeting `base`.
    pub fn in_pr(self, base: &str, head: &str) -> Self {
        self.with_env("GITHUB_ACTIONS", "true")
            .with_env("GITHUB_REF", "refs/pull/1/merge")
            .with_env("GITHUB_BASE_REF", base)
            .with_env("GITHUB_HEAD_REF", head)
    }

    /// Make the tempdir look like a git checkout of `branch`.
    ///
    /// Writing the file is the whole of the local-git surface gh-ship
    /// reads, which is what makes detection hermetically testable
    /// without a `git` binary.
    pub fn with_git_head(self, branch: &str) -> Self {
        let git = self.path().join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
        self
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run gh-ship in this repository, with the stub taking priority on
    /// PATH so a real `gh` can never be reached.
    pub fn ship(&self, args: &[&str]) -> Outcome {
        let path = format!(
            "{}:{}",
            self.stub.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let mut cmd = ship();
        cmd.current_dir(self.path())
            .env("PATH", path)
            // Keep the "run never appeared" path fast: the production
            // default is 90s, which would dominate the suite.
            .env("SHIP_APPEAR_TIMEOUT", "3")
            .env("SHIP_RUN_TIMEOUT", "10")
            // Retries are exercised for their behaviour, not their timing:
            // sleeping through the real backoff would add seconds per test.
            .env("SHIP_GH_RETRY_DELAY", "0");
        for (k, v) in &self.stub.env {
            cmd.env(k, v);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        Outcome::run(cmd.args(args))
    }
}

/// Environment that must never leak into a test.
///
/// The `GITHUB_*` entries matter more than they look: this suite runs
/// *on* GitHub Actions, where they are all set, so base-branch detection
/// would resolve CI's own branch and every branch test would behave
/// differently on a developer machine than in CI. Tests that want CI
/// detection opt in explicitly via [`Repo::in_ci`] / [`Repo::in_pr`].
const SCRUBBED: &[&str] = &[
    "SHIP_CONFIG",
    "SHIP_REPO",
    "SHIP_BASE_BRANCH",
    "RUST_BACKTRACE",
    "GITHUB_ACTIONS",
    "GITHUB_REF",
    "GITHUB_REF_NAME",
    "GITHUB_BASE_REF",
    "GITHUB_HEAD_REF",
];

/// Build a `gh-ship` invocation with a deterministic environment.
///
/// `NO_COLOR` / `TERM=dumb` force plain ASCII, and [`SCRUBBED`] is
/// cleared so neither a developer's shell nor CI can leak into a test.
pub fn ship() -> Command {
    let mut cmd = Command::cargo_bin("gh-ship").expect("binary builds");
    cmd.env("NO_COLOR", "1").env("TERM", "dumb");
    for key in SCRUBBED {
        cmd.env_remove(key);
    }
    cmd
}

/// Run `gh ship validate <fixture>` and return combined output.
///
/// Paths are normalised to the bare filename so snapshots do not embed
/// the absolute path of whoever ran the tests.
pub fn validate_fixture(relative: &str) -> Outcome {
    let path = fixture_path(relative);
    let dir = path.parent().expect("fixture has a parent").to_path_buf();
    let file = path.file_name().expect("fixture has a name").to_owned();

    Outcome::run(ship().current_dir(&dir).arg("validate").arg(&file))
}

/// Absolute path to a file under `tests/fixtures`.
pub fn fixture_path(relative: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

/// Every fixture file in a directory, sorted for deterministic order.
pub fn fixtures_in(relative: &str) -> Vec<std::path::PathBuf> {
    let dir = fixture_path(relative);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures found in {}", dir.display());
    files
}

/// The result of running the binary.
///
/// `stderr` is deliberately private: every human-facing line goes there, so
/// [`Outcome::diagnostics`] is the single accessor the suite reads it
/// through. Moving where diagnostics go should be one edit, not forty.
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    stderr: String,
}

impl From<std::process::Output> for Outcome {
    fn from(output: std::process::Output) -> Self {
        Self {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl Outcome {
    /// Run a prepared command and capture what it said.
    pub fn run(cmd: &mut Command) -> Self {
        cmd.output().expect("gh-ship runs").into()
    }

    /// Stderr, which is where all human-facing output goes.
    pub fn diagnostics(&self) -> &str {
        &self.stderr
    }

    pub fn succeeded(&self) -> bool {
        self.code == 0
    }
}

/// Insta filters for values that legitimately change between runs.
///
/// The correlation nonce is random by design, and elapsed times vary, so
/// both are redacted rather than allowed to make snapshots flaky.
pub fn redactions() -> Vec<(&'static str, &'static str)> {
    vec![
        (r"ship id: [0-9a-f]{12}", "ship id: [nonce]"),
        (r"ship:[0-9a-f]{12}", "ship:[nonce]"),
        // The staging branch is named after the nonce.
        (r"ship/prepare-[0-9a-z]+", "ship/prepare-[nonce]"),
        (r"\d+m \d+s", "[dur]"),
        (r"\d+\.\d{2}s", "[dur]"),
        (r"\d+ms", "[dur]"),
        (r"<1ms", "[dur]"),
        (r"/tmp/\.tmp\w+", "[tmp]"),
    ]
}

/// Run `f` with the standard redactions applied to any snapshot taken.
pub fn with_redactions(f: impl FnOnce()) {
    let mut settings = insta::Settings::clone_current();
    for (pattern, replacement) in redactions() {
        settings.add_filter(pattern, replacement);
    }
    settings.bind(f);
}
