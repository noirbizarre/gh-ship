//! A hermetic `gh` stub.
//!
//! gh-ship funnels every GitHub interaction through the `gh` binary, so
//! replacing `gh` on `PATH` with a script gives complete control over
//! what GitHub "says" — with no network, no credentials, and no fixture
//! recording to keep in sync.
//!
//! The stub is driven entirely by environment variables so a test can
//! describe a scenario declaratively:
//!
//! ```ignore
//! GhStub::new()
//!     .run_status("completed", "success")
//!     .artifact(r#"{"schemaVersion":1,"changed":false}"#)
//!     .install(&dir);
//! ```

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A configurable fake `gh`.
pub struct GhStub {
    env: BTreeMap<String, String>,
}

impl Default for GhStub {
    fn default() -> Self {
        Self::new()
    }
}

impl GhStub {
    pub fn new() -> Self {
        let mut env = BTreeMap::new();
        env.insert("STUB_DEFAULT_BRANCH".into(), "main".into());
        env.insert("STUB_REPO".into(), "acme/widgets".into());
        env.insert("STUB_RUN_STATUS".into(), "completed".into());
        env.insert("STUB_RUN_CONCLUSION".into(), "success".into());
        env.insert("STUB_BRANCH_EXISTS".into(), "1".into());
        env.insert("STUB_PR_EXISTS".into(), "0".into());
        env.insert("STUB_RELEASE_EXISTS".into(), "0".into());
        env.insert("STUB_TAG_EXISTS".into(), "0".into());
        // By default the dispatched run is found immediately, carrying
        // whatever nonce gh-ship passed.
        env.insert("STUB_RUN_FOUND".into(), "1".into());
        env.insert("STUB_PR_BODY_JSON".into(), "\"\"".into());
        // Labels that already exist in the fake repository.
        env.insert("STUB_LABELS".into(), String::new());
        env.insert("STUB_LABEL_CREATE_FAILS".into(), "0".into());
        env.insert(
            "STUB_ARTIFACT".into(),
            r#"{"schemaVersion":1,"changed":false}"#.into(),
        );
        Self { env }
    }

    /// The run's terminal state.
    pub fn run_status(mut self, status: &str, conclusion: &str) -> Self {
        self.env.insert("STUB_RUN_STATUS".into(), status.into());
        self.env
            .insert("STUB_RUN_CONCLUSION".into(), conclusion.into());
        self
    }

    /// Make `gh run list` never return a matching run, simulating a
    /// workflow that does not stamp the nonce into its `run-name`.
    pub fn run_never_appears(mut self) -> Self {
        self.env.insert("STUB_RUN_FOUND".into(), "0".into());
        self
    }

    /// A run that already exists before the command runs — a previous
    /// dispatch, or one a human started or re-ran from the GitHub UI.
    ///
    /// Without this, `gh run list` only reports runs on a ref this test
    /// actually dispatched on, which is what makes "is there already a
    /// publish run for this tag?" a meaningful question.
    pub fn existing_run(mut self, status: &str, conclusion: &str) -> Self {
        self.env.insert("STUB_EXISTING_RUN".into(), "1".into());
        self.env
            .insert("STUB_EXISTING_RUN_STATUS".into(), status.into());
        self.env
            .insert("STUB_EXISTING_RUN_CONCLUSION".into(), conclusion.into());
        self
    }

    /// The last run `gh ship status` reports. Same mechanism as
    /// [`Self::existing_run`], named for how `status` reads it.
    pub fn last_run(self, status: &str, conclusion: &str) -> Self {
        self.existing_run(status, conclusion)
    }

    /// The contents of `ship.release.json` the run "uploaded".
    pub fn artifact(mut self, json: &str) -> Self {
        self.env.insert("STUB_ARTIFACT".into(), json.into());
        self
    }

    /// Make artifact download fail, simulating a workflow that forgot to
    /// upload one.
    pub fn no_artifact(mut self) -> Self {
        self.env.insert("STUB_ARTIFACT".into(), String::new());
        self
    }

    /// Upload an artifact under the wrong filename.
    pub fn artifact_wrong_filename(mut self, name: &str) -> Self {
        self.env.insert("STUB_ARTIFACT_NAME".into(), name.into());
        self
    }

    pub fn repo(mut self, slug: &str) -> Self {
        self.env.insert("STUB_REPO".into(), slug.into());
        self
    }

    pub fn default_branch(mut self, branch: &str) -> Self {
        self.env.insert("STUB_DEFAULT_BRANCH".into(), branch.into());
        self
    }

    pub fn branch_exists(mut self, yes: bool) -> Self {
        self.env.insert("STUB_BRANCH_EXISTS".into(), bool_env(yes));
        self
    }

    pub fn pr_exists(mut self, yes: bool) -> Self {
        self.env.insert("STUB_PR_EXISTS".into(), bool_env(yes));
        self
    }

    pub fn pr_state(mut self, state: &str) -> Self {
        self.env.insert("STUB_PR_EXISTS".into(), "1".into());
        self.env.insert("STUB_PR_STATE".into(), state.into());
        self
    }

    /// Set the PR body.
    ///
    /// JSON-encoded here rather than in the shell stub: escaping
    /// newlines and quotes correctly in POSIX `sh` is a losing battle,
    /// and PR bodies always contain both.
    pub fn pr_body(mut self, body: &str) -> Self {
        self.env.insert("STUB_PR_EXISTS".into(), "1".into());
        self.env.insert(
            "STUB_PR_BODY_JSON".into(),
            serde_json::to_string(body).expect("string encodes"),
        );
        self
    }

    pub fn merge_commit(mut self, sha: &str) -> Self {
        self.env.insert("STUB_MERGE_SHA".into(), sha.into());
        self
    }

    pub fn release_exists(mut self, yes: bool) -> Self {
        self.env.insert("STUB_RELEASE_EXISTS".into(), bool_env(yes));
        self
    }

    /// Labels that already exist in the repository.
    pub fn labels(mut self, names: &[&str]) -> Self {
        self.env.insert("STUB_LABELS".into(), names.join(","));
        self
    }

    /// Make `gh label create` fail, simulating a token without
    /// `issues: write`.
    pub fn label_create_fails(mut self) -> Self {
        self.env
            .insert("STUB_LABEL_CREATE_FAILS".into(), "1".into());
        self
    }

    /// Make `gh pr create` reject an unknown label, which is what GitHub
    /// actually does and what cost a real Release PR.
    pub fn pr_create_rejects_unknown_labels(mut self) -> Self {
        self.env.insert("STUB_PR_STRICT_LABELS".into(), "1".into());
        self
    }

    /// Make tag creation report "already exists", as GitHub does when
    /// re-running after a partially completed release.
    pub fn tag_exists(mut self) -> Self {
        self.env.insert("STUB_TAG_EXISTS".into(), "1".into());
        self
    }

    /// Report a leftover staging branch, as an abandoned run would.
    pub fn stale_staging_branch(mut self, name: &str) -> Self {
        self.env.insert("STUB_STALE_STAGING".into(), name.into());
        self
    }

    /// Refuse branch deletions, as a protected ref or a missing scope does.
    pub fn branch_delete_fails(mut self) -> Self {
        self.env
            .insert("STUB_BRANCH_DELETE_FAILS".into(), "1".into());
        self
    }

    /// Simulate `gh` being unauthenticated.
    pub fn unauthenticated(mut self) -> Self {
        self.env.insert("STUB_UNAUTHENTICATED".into(), "1".into());
        self
    }

    /// Make the first `count` invocations of `cmd` fail with GitHub's 504.
    ///
    /// `cmd` is the subcommand pair as the stub sees it, e.g. `"pr list"`.
    /// Use a count larger than the retry budget to model an outage rather
    /// than a blip.
    pub fn flaky(mut self, cmd: &str, count: u32) -> Self {
        self.env.insert("STUB_FLAKY_CMD".into(), cmd.into());
        self.env
            .insert("STUB_FLAKY_COUNT".into(), count.to_string());
        self
    }

    /// Write the stub into `dir/bin/gh` and return that bin directory,
    /// plus the environment the stub needs.
    pub fn install(self, dir: &Path) -> Installed {
        let bin = dir.join("stubbin");
        std::fs::create_dir_all(&bin).expect("create stub bin dir");

        let path = bin.join("gh");
        std::fs::write(&path, SCRIPT).expect("write gh stub");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }

        // The stub records every invocation here so tests can assert on
        // what gh-ship actually asked GitHub to do.
        let log = dir.join("gh-calls.log");
        let mut env = self.env;
        env.insert("STUB_LOG".into(), log.to_string_lossy().into_owned());

        Installed { bin, env, log }
    }
}

/// A stub written to disk.
pub struct Installed {
    pub bin: PathBuf,
    pub env: BTreeMap<String, String>,
    pub log: PathBuf,
}

impl Installed {
    /// Every `gh` invocation made, one per line.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Whether any invocation contains all the given fragments.
    pub fn called_with(&self, fragments: &[&str]) -> bool {
        self.calls()
            .iter()
            .any(|c| fragments.iter().all(|f| c.contains(f)))
    }
}

fn bool_env(yes: bool) -> String {
    if yes { "1".into() } else { "0".into() }
}

/// The stub itself.
///
/// POSIX `sh` so it runs anywhere the test suite does. It answers only
/// the subcommands gh-ship actually issues; anything else exits
/// non-zero, which makes an unexpected call a loud test failure rather
/// than a silent success.
const SCRIPT: &str = r#"#!/bin/sh
set -eu

# Record the invocation for assertions. Newlines are folded to spaces so
# each invocation stays a single log line: PR bodies are multi-line, and
# a call that spanned lines could not be matched as one.
if [ -n "${STUB_LOG:-}" ]; then
  printf '%s' "$*" | tr '\n' ' ' >> "$STUB_LOG"
  printf '\n' >> "$STUB_LOG"
fi

if [ "${STUB_UNAUTHENTICATED:-0}" = "1" ]; then
  echo "gh: To get started with GitHub CLI, please run: gh auth login" >&2
  exit 4
fi

REPO="${STUB_REPO:-acme/widgets}"
DEFAULT_BRANCH="${STUB_DEFAULT_BRANCH:-main}"
LABEL_FILE="${STUB_LOG:-/tmp/stub}.labels"

# Fail the first N invocations of a given subcommand with GitHub's 504,
# byte for byte as it broke a real release run. The counter lives on disk
# because each invocation is a fresh process.
if [ -n "${STUB_FLAKY_CMD:-}" ] && [ "$1 ${2:-}" = "$STUB_FLAKY_CMD" ]; then
  FLAKY_FILE="${STUB_LOG:-/tmp/stub}.flaky"
  SEEN="$(cat "$FLAKY_FILE" 2>/dev/null || echo 0)"
  SEEN=$((SEEN + 1))
  printf '%s\n' "$SEEN" > "$FLAKY_FILE"
  if [ "$SEEN" -le "${STUB_FLAKY_COUNT:-0}" ]; then
    echo "HTTP 504: We couldn't respond to your request in time. Sorry about that. Please try resubmitting your request and contact us if the problem persists. (https://api.github.com/graphql)" >&2
    exit 1
  fi
fi

# Labels the repository "has": those seeded by the test, plus any created
# by an earlier invocation in the same test.
#
# Note the trailing newline in the printf: without it `read` returns
# non-zero on the final field and the loop body never runs.
all_labels() {
  printf '%s\n' "${STUB_LABELS:-}" | tr ',' '\n' | while read -r l; do
    if [ -n "$l" ]; then
      printf '%s\n' "$l"
    fi
  done
  if [ -f "$LABEL_FILE" ]; then
    cat "$LABEL_FILE"
  fi
}

case "$1 ${2:-}" in

  "repo view")
    printf '{"nameWithOwner":"%s","defaultBranchRef":{"name":"%s"},"url":"https://github.com/%s"}\n' \
      "$REPO" "$DEFAULT_BRANCH" "$REPO"
    ;;

  "workflow run")
    # Dispatch returns nothing, exactly like the real thing. Capture the
    # nonce so `run list` can echo it back in the run title, and the ref so
    # it only echoes it back on the ref that was actually dispatched.
    prev=""
    for arg in "$@"; do
      case "$arg" in
        ship_id=*) echo "${arg#ship_id=}" > "${STUB_LOG:-/tmp/stub}.shipid" ;;
      esac
      if [ "$prev" = "--ref" ]; then
        echo "$arg" > "${STUB_LOG:-/tmp/stub}.dispatchref"
      fi
      prev="$arg"
    done
    ;;

  "workflow list")
    printf '[]\n'
    ;;

  "run list")
    if [ "${STUB_RUN_FOUND:-1}" = "0" ]; then
      printf '[]\n'
      exit 0
    fi
    BRANCH=""
    prev=""
    for arg in "$@"; do
      if [ "$prev" = "--branch" ]; then BRANCH="$arg"; fi
      prev="$arg"
    done
    REF="$(cat "${STUB_LOG:-/tmp/stub}.dispatchref" 2>/dev/null || echo '')"
    if [ -n "$REF" ] && [ "$REF" = "$BRANCH" ]; then
      # The run this test dispatched, on the ref it was dispatched on.
      SHIP_ID="$(cat "${STUB_LOG:-/tmp/stub}.shipid" 2>/dev/null || echo unknown)"
      printf '[{"databaseId":42,"displayTitle":"prepare-release (ship:%s)","status":"%s","conclusion":"%s","url":"https://github.com/%s/actions/runs/42","headBranch":"%s"}]\n' \
        "$SHIP_ID" "${STUB_RUN_STATUS:-completed}" "${STUB_RUN_CONCLUSION:-success}" "$REPO" "$BRANCH"
    elif [ "${STUB_EXISTING_RUN:-0}" = "1" ]; then
      # A run that was already there before this invocation: a previous
      # dispatch, or one a human started.
      printf '[{"databaseId":41,"displayTitle":"publish-release (ship:preexisting)","status":"%s","conclusion":"%s","url":"https://github.com/%s/actions/runs/41","headBranch":"%s"}]\n' \
        "${STUB_EXISTING_RUN_STATUS:-completed}" "${STUB_EXISTING_RUN_CONCLUSION:-success}" "$REPO" "$BRANCH"
    else
      printf '[]\n'
    fi
    ;;

  "run view")
    SHIP_ID="$(cat "${STUB_LOG:-/tmp/stub}.shipid" 2>/dev/null || echo unknown)"
    printf '{"databaseId":%s,"displayTitle":"prepare-release (ship:%s)","status":"%s","conclusion":"%s","url":"https://github.com/%s/actions/runs/%s","headBranch":"release/next"}\n' \
      "${3:-42}" "$SHIP_ID" "${STUB_RUN_STATUS:-completed}" "${STUB_RUN_CONCLUSION:-success}" "$REPO" "${3:-42}"
    ;;

  "run download")
    if [ -z "${STUB_ARTIFACT:-}" ]; then
      echo "no valid artifacts found to download" >&2
      exit 1
    fi
    DIR=""
    prev=""
    for arg in "$@"; do
      if [ "$prev" = "--dir" ]; then DIR="$arg"; fi
      prev="$arg"
    done
    [ -n "$DIR" ] || DIR="."
    mkdir -p "$DIR"
    printf '%s\n' "$STUB_ARTIFACT" > "$DIR/${STUB_ARTIFACT_NAME:-ship.release.json}"
    ;;

  "pr list")
    if [ "${STUB_PR_EXISTS:-0}" = "0" ]; then
      printf '[]\n'
      exit 0
    fi
    printf '[{"number":7,"url":"https://github.com/%s/pull/7","title":"Release 1.0.0","body":%s,"state":"%s","isDraft":false,"mergeCommit":%s}]\n' \
      "$REPO" \
      "${STUB_PR_BODY_JSON:-\"\"}" \
      "${STUB_PR_STATE:-OPEN}" \
      "$(if [ -n "${STUB_MERGE_SHA:-}" ]; then printf '{"oid":"%s"}' "$STUB_MERGE_SHA"; else printf 'null'; fi)"
    ;;

  "pr view")
    printf '{"number":7,"url":"https://github.com/%s/pull/7","title":"Release 1.0.0","body":%s,"state":"%s","isDraft":false,"mergeCommit":%s}\n' \
      "$REPO" \
      "${STUB_PR_BODY_JSON:-\"\"}" \
      "${STUB_PR_STATE:-OPEN}" \
      "$(if [ -n "${STUB_MERGE_SHA:-}" ]; then printf '{"oid":"%s"}' "$STUB_MERGE_SHA"; else printf 'null'; fi)"
    ;;

  "label list")
    printf '['
    first=1
    for l in $(all_labels); do
      [ "$first" = "1" ] || printf ','
      printf '{"name":"%s"}' "$l"
      first=0
    done
    printf ']\n'
    ;;

  "label create")
    if [ "${STUB_LABEL_CREATE_FAILS:-0}" = "1" ]; then
      echo "HTTP 403: Resource not accessible by integration" >&2
      exit 1
    fi
    # Persist to a file: each stub invocation is its own process, so an
    # exported variable would not survive to the next call.
    printf '%s\n' "$3" >> "$LABEL_FILE"
    ;;

  "pr create")
    if [ "${STUB_PR_STRICT_LABELS:-0}" = "1" ]; then
      # GitHub rejects the whole PR when a label is unknown.
      prev=""
      for arg in "$@"; do
        if [ "$prev" = "--label" ]; then
          found=0
          for l in $(all_labels); do
            if [ "$l" = "$arg" ]; then
              found=1
            fi
          done
          if [ "$found" = "0" ]; then
            echo "could not add label: '$arg' not found" >&2
            exit 1
          fi
        fi
        prev="$arg"
      done
    fi
    printf 'https://github.com/%s/pull/7\n' "$REPO"
    ;;

  "pr edit"|"pr merge"|"pr reopen"|"pr close")
    ;;

  "release view")
    if [ "${STUB_RELEASE_EXISTS:-0}" = "0" ]; then
      echo "release not found" >&2
      exit 1
    fi
    printf '{"tagName":"v1.0.0"}\n'
    ;;

  "release create")
    printf 'https://github.com/%s/releases/tag/v1.0.0\n' "$REPO"
    ;;

  "release edit"|"release upload")
    ;;

  "api "*|"api")
    # `gh api` has no `--repo` flag and exits 1 on an unknown one. Reproducing
    # that here is what catches a call routed through the scoped helper.
    case " $* " in
      *" --repo "*)
        echo "unknown flag: --repo" >&2
        exit 1
        ;;
    esac
    TARGET="${2:-}"
    case "$TARGET" in
      *"/branches/"*)
        [ "${STUB_BRANCH_EXISTS:-1}" = "1" ] || { echo "HTTP 404: Not Found" >&2; exit 1; }
        ;;
      *"/git/ref/"*)
        printf 'a1b2c3d4e5f6\n'
        ;;
      # Staging-branch sweep. Reports one leftover when asked to.
      *"/git/matching-refs/heads/ship/prepare-"*)
        if [ -n "${STUB_STALE_STAGING:-}" ]; then
          printf '[{"ref":"refs/heads/%s"}]\n' "$STUB_STALE_STAGING"
        else
          printf '[]\n'
        fi
        ;;
      # POST creates a ref (branch or tag); PATCH .../git/refs/heads/<branch>
      # force-updates one; DELETE removes one.
      *"/git/refs"*)
        if [ "${STUB_BRANCH_DELETE_FAILS:-0}" = "1" ]; then
          case " $* " in
            *" DELETE "*)
              echo "HTTP 403: Resource not accessible by integration" >&2
              exit 1
              ;;
          esac
        fi
        if [ "${STUB_TAG_EXISTS:-0}" = "1" ]; then
          case "$*" in
            *refs/tags/*)
              echo "HTTP 422: Reference already exists" >&2
              exit 1
              ;;
          esac
        fi
        ;;
      *)
        printf '{}\n'
        ;;
    esac
    ;;

  *)
    echo "gh stub: unexpected invocation: $*" >&2
    exit 127
    ;;
esac
"#;
