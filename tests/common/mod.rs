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

/// A repository laid out on disk, with a stubbed `gh` on PATH.
pub struct Repo {
    pub dir: tempfile::TempDir,
    pub stub: stub::Installed,
}

impl Repo {
    /// Create a repository with the given config and a stubbed `gh`.
    pub fn new(config: &str, stub: GhStub) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let wf = dir.path().join(".github/workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("prepare-release.yml"), CONFORMING_WORKFLOW).unwrap();
        std::fs::write(dir.path().join(".github/ship.yml"), config).unwrap();
        let installed = stub.install(dir.path());
        Self {
            dir,
            stub: installed,
        }
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
            .env("SHIP_RUN_TIMEOUT", "10");
        for (k, v) in &self.stub.env {
            cmd.env(k, v);
        }
        let output = cmd.args(args).output().expect("gh-ship runs");
        Outcome {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Build a `gh-ship` invocation with a deterministic environment.
///
/// `NO_COLOR` / `TERM=dumb` force plain ASCII, and `SHIP_CONFIG` is
/// cleared so a developer's shell cannot leak into a test.
pub fn ship() -> Command {
    let mut cmd = Command::cargo_bin("gh-ship").expect("binary builds");
    cmd.env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env_remove("SHIP_CONFIG")
        .env_remove("SHIP_REPO")
        .env_remove("RUST_BACKTRACE");
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

    let output = ship()
        .current_dir(&dir)
        .arg("validate")
        .arg(&file)
        .output()
        .expect("gh-ship runs");

    Outcome {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
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
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
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
        (r"ship/prepare-[0-9a-f]{12}", "ship/prepare-[nonce]"),
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
