//! A hermetic native `gh` stub, driven by environment variables.
//!
//! The executable in tests/support/gh_stub.rs implements the small GitHub
//! surface used by the CLI. Invocation logs let tests assert on mutations,
//! correlation and authentication without credentials or network access.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        let env = [
            ("STUB_DEFAULT_BRANCH", "main"),
            ("STUB_REPO", "acme/widgets"),
            ("STUB_RUN_STATUS", "completed"),
            ("STUB_RUN_CONCLUSION", "success"),
            ("STUB_BRANCH_EXISTS", "1"),
            ("STUB_PR_EXISTS", "0"),
            ("STUB_RELEASE_EXISTS", "0"),
            ("STUB_TAG_EXISTS", "0"),
            ("STUB_RUN_FOUND", "1"),
            ("STUB_PR_BODY_JSON", "\"\""),
            ("STUB_LABELS", ""),
            ("STUB_LABEL_CREATE_FAILS", "0"),
            ("STUB_ARTIFACT", r#"{"schemaVersion":1,"changed":false}"#),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        Self { env }
    }

    fn set(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
    pub fn run_status(self, status: &str, conclusion: &str) -> Self {
        self.set("STUB_RUN_STATUS", status)
            .set("STUB_RUN_CONCLUSION", conclusion)
    }
    pub fn run_never_appears(self) -> Self {
        self.set("STUB_RUN_FOUND", "0")
    }
    pub fn push_run(self) -> Self {
        self.set("STUB_PUSH_RUN", "1")
    }
    pub fn requires_ship_id(self) -> Self {
        self.set("STUB_REQUIRES_SHIP_ID", "1")
    }
    pub fn existing_run(self, status: &str, conclusion: &str) -> Self {
        self.set("STUB_EXISTING_RUN", "1")
            .set("STUB_EXISTING_RUN_STATUS", status)
            .set("STUB_EXISTING_RUN_CONCLUSION", conclusion)
    }
    pub fn last_run(self, status: &str, conclusion: &str) -> Self {
        self.existing_run(status, conclusion)
            .run_on("ship/prepare-deadbeef1234")
    }
    pub fn run_on(self, branch: &str) -> Self {
        self.set("STUB_EXISTING_RUN_BRANCH", branch)
    }
    pub fn artifact(self, json: &str) -> Self {
        self.set("STUB_ARTIFACT", json)
    }
    pub fn no_artifact(self) -> Self {
        self.artifact("")
    }
    pub fn artifact_wrong_filename(self, name: &str) -> Self {
        self.set("STUB_ARTIFACT_NAME", name)
    }
    pub fn repo(self, slug: &str) -> Self {
        self.set("STUB_REPO", slug)
    }
    pub fn default_branch(self, branch: &str) -> Self {
        self.set("STUB_DEFAULT_BRANCH", branch)
    }
    pub fn branch_exists(self, yes: bool) -> Self {
        self.set("STUB_BRANCH_EXISTS", bool_env(yes))
    }
    pub fn pr_exists(self, yes: bool) -> Self {
        self.set("STUB_PR_EXISTS", bool_env(yes))
    }
    pub fn pr_state(self, state: &str) -> Self {
        self.pr_exists(true).set("STUB_PR_STATE", state)
    }
    pub fn pr_body(self, body: &str) -> Self {
        self.pr_exists(true)
            .set("STUB_PR_BODY_JSON", serde_json::to_string(body).unwrap())
    }
    pub fn merge_commit(self, sha: &str) -> Self {
        self.set("STUB_MERGE_SHA", sha)
    }
    pub fn pr_identity(self, json: &str) -> Self {
        self.set("STUB_PR_IDENTITY", json)
    }
    pub fn viewed_pr_merged(self) -> Self {
        self.set("STUB_VIEW_PR_STATE", "MERGED")
    }
    pub fn release_exists(self, yes: bool) -> Self {
        self.set("STUB_RELEASE_EXISTS", bool_env(yes))
    }
    pub fn draft_release(self) -> Self {
        self.release_exists(true).set("STUB_RELEASE_DRAFT", "true")
    }
    pub fn labels(self, names: &[&str]) -> Self {
        self.set("STUB_LABELS", names.join(","))
    }
    pub fn label_create_fails(self) -> Self {
        self.set("STUB_LABEL_CREATE_FAILS", "1")
    }
    pub fn pr_create_rejects_unknown_labels(self) -> Self {
        self.set("STUB_PR_STRICT_LABELS", "1")
    }
    pub fn tag_exists(self) -> Self {
        self.set("STUB_TAG_EXISTS", "1")
    }
    pub fn tag_target(self, sha: &str) -> Self {
        self.tag_exists().set("STUB_TAG_SHA", sha)
    }
    pub fn tag_creation_error(self, message: &str) -> Self {
        self.set("STUB_TAG_ERROR", message)
    }
    pub fn stale_staging_branch(self, name: &str) -> Self {
        self.set("STUB_STALE_STAGING", name)
    }
    pub fn head_already_signed(self) -> Self {
        self.set("STUB_HEAD_VERIFIED", "true")
    }
    pub fn cannot_sign(self) -> Self {
        self.set("STUB_SIGN_VERIFIED", "false")
    }
    pub fn branch_delete_fails(self) -> Self {
        self.set("STUB_BRANCH_DELETE_FAILS", "1")
    }
    pub fn unauthenticated(self) -> Self {
        self.set("STUB_UNAUTHENTICATED", "1")
    }
    pub fn flaky(self, cmd: &str, count: u32) -> Self {
        self.set("STUB_FLAKY_CMD", cmd)
            .set("STUB_FLAKY_COUNT", count.to_string())
    }
    pub fn app_rejects_jwt(self) -> Self {
        self.set("STUB_APP_REJECTS_JWT", "1")
    }
    pub fn app_not_installed(self) -> Self {
        self.set("STUB_APP_NOT_INSTALLED", "1")
    }
    pub fn rejects_token(self, token: &str) -> Self {
        self.set("STUB_BAD_TOKEN", token)
    }

    /// Install the Cargo-built helper under the platform's gh executable name.
    pub fn install(self, dir: &Path) -> Installed {
        let bin = dir.join("stubbin");
        std::fs::create_dir_all(&bin).expect("create stub bin dir");
        let path = bin.join(format!("gh{}", std::env::consts::EXE_SUFFIX));
        #[cfg(unix)]
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_gh-ship-test-gh"), &path)
            .expect("link native gh stub");
        #[cfg(not(unix))]
        std::fs::copy(env!("CARGO_BIN_EXE_gh-ship-test-gh"), &path)
            .expect("install native gh stub");
        let log = dir.join("gh-calls.log");
        let mut env = self.env;
        env.insert("STUB_LOG".into(), log.to_string_lossy().into_owned());
        Installed { bin, env, log }
    }
}

pub struct Installed {
    pub bin: PathBuf,
    pub env: BTreeMap<String, String>,
    pub log: PathBuf,
}

impl Installed {
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
    pub fn tokens(&self) -> Vec<String> {
        let mut path = self.log.clone().into_os_string();
        path.push(".tokens");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
    pub fn installation_tokens(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .zip(self.tokens())
            .filter(|(call, _)| !call.contains("Authorization: Bearer"))
            .map(|(_, token)| token)
            .collect()
    }
    pub fn called_with(&self, fragments: &[&str]) -> bool {
        self.calls()
            .iter()
            .any(|call| fragments.iter().all(|f| call.contains(f)))
    }
}

fn bool_env(yes: bool) -> String {
    if yes { "1".into() } else { "0".into() }
}
