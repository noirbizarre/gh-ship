//! Workflow discovery and contract checking.
//!
//! Two facts about a workflow matter to gh-ship, and neither is
//! comfortably available from the API:
//!
//! 1. **Is it dispatchable?** A workflow that only declares
//!    `on: workflow_call` — what people usually mean by "reusable" —
//!    *cannot* be started by the API at all. Naming one in `ship.yml` is
//!    the single most likely setup mistake, so it is checked up front.
//!
//! 2. **Does it declare a correlating `run-name`?** `gh workflow run`
//!    returns nothing: no run id, no URL. The only reliable way to find
//!    the run we just started is to have the workflow stamp our nonce
//!    into its own name. That makes `run-name` part of the protocol.
//!
//! Both are checked by parsing the workflow YAML, because the answer has
//! to be available before anything is dispatched.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The dispatch input carrying gh-ship's correlation nonce.
pub const SHIP_ID_INPUT: &str = "ship_id";

/// The dispatch input that suppresses mutations, used by `preview`.
pub const DRY_RUN_INPUT: &str = "dry_run";

/// Directory holding workflow definitions.
pub const WORKFLOW_DIR: &str = ".github/workflows";

/// A workflow definition found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// The workflow's `name:`, falling back to the file stem.
    pub name: String,
    /// Whether it declares `on: workflow_dispatch`.
    pub dispatchable: bool,
    /// Whether it declares `on: workflow_call`.
    pub callable: bool,
    /// Its `run-name:` expression, if any.
    pub run_name: Option<String>,
    /// `workflow_dispatch` input names.
    pub inputs: Vec<String>,
}

impl Workflow {
    /// The identifier to pass to `gh workflow run`.
    ///
    /// The filename is preferred over the display name: it is unique,
    /// stable, and unambiguous when two workflows share a `name:`.
    pub fn id(&self) -> String {
        self.path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.name.clone())
    }

    /// Whether the `run-name` interpolates gh-ship's nonce, which is
    /// what makes dispatch→run correlation reliable.
    pub fn correlates(&self) -> bool {
        self.run_name
            .as_deref()
            .is_some_and(|n| n.contains(SHIP_ID_INPUT))
    }

    /// Whether it accepts the nonce input at all.
    pub fn accepts_ship_id(&self) -> bool {
        self.inputs.iter().any(|i| i == SHIP_ID_INPUT)
    }

    /// Whether it accepts the dry-run input, required by `gh ship preview`.
    pub fn accepts_dry_run(&self) -> bool {
        self.inputs.iter().any(|i| i == DRY_RUN_INPUT)
    }

    /// Every way this workflow violates the gh-ship contract.
    pub fn contract_violations(&self) -> Vec<Violation> {
        let mut v = Vec::new();
        if !self.dispatchable {
            v.push(Violation::NotDispatchable);
        }
        if !self.accepts_ship_id() {
            v.push(Violation::MissingShipIdInput);
        }
        if !self.correlates() {
            v.push(Violation::MissingRunName);
        }
        v
    }
}

/// A way a workflow fails to satisfy the gh-ship contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    NotDispatchable,
    MissingShipIdInput,
    MissingRunName,
}

impl Violation {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotDispatchable => "does not declare `on: workflow_dispatch`",
            Self::MissingShipIdInput => "does not declare a `ship_id` input",
            Self::MissingRunName => "does not stamp `ship_id` into its `run-name`",
        }
    }

    pub fn help(&self) -> &'static str {
        match self {
            Self::NotDispatchable => {
                "gh-ship starts workflows through the API, which can only start workflows \
                 declaring `on: workflow_dispatch`. A `workflow_call`-only workflow — what is \
                 usually called a reusable workflow — cannot be started this way. Declare both \
                 triggers to keep it reusable *and* dispatchable."
            }
            Self::MissingShipIdInput => {
                "add a required `ship_id` string input under `workflow_dispatch.inputs`; \
                 gh-ship passes a nonce there to find the run it started"
            }
            Self::MissingRunName => {
                "add `run-name: <name> (ship:${{ inputs.ship_id }})`. `gh workflow run` returns \
                 no run id, so the nonce in the run name is how gh-ship locates your run instead \
                 of guessing from timestamps."
            }
        }
    }
}

/// Minimal projection of a workflow file.
///
/// Deliberately forward-compatible: unknown keys are ignored, because a
/// workflow is the user's file and gh-ship has no business validating
/// anything beyond the handful of keys it depends on.
#[derive(Debug, Deserialize)]
struct RawWorkflow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "run-name")]
    run_name: Option<String>,
    // `on` is YAML 1.1's boolean `true`. serde_norway parses YAML 1.2
    // core schema so it stays a string, but we accept both spellings to
    // survive either behaviour.
    #[serde(default, rename = "on", alias = "true")]
    on: Option<serde_norway::Value>,
}

/// Parse a workflow definition.
///
/// Returns `None` when the file is not valid YAML: an unparseable
/// workflow is GitHub's problem to report, not gh-ship's.
pub fn parse(path: &Path, text: &str) -> Option<Workflow> {
    let raw: RawWorkflow = serde_norway::from_str(text).ok()?;

    let name = raw.name.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    let (dispatchable, callable, inputs) = analyse_triggers(raw.on.as_ref());

    Some(Workflow {
        path: path.to_path_buf(),
        name,
        dispatchable,
        callable,
        run_name: raw.run_name,
        inputs,
    })
}

/// Extract trigger facts from the `on:` value.
///
/// `on:` has three legal shapes — a bare string, a sequence of strings,
/// and a mapping — and all three appear in the wild.
fn analyse_triggers(on: Option<&serde_norway::Value>) -> (bool, bool, Vec<String>) {
    use serde_norway::Value;

    let mut dispatchable = false;
    let mut callable = false;
    let mut inputs = Vec::new();

    match on {
        Some(Value::String(s)) => {
            dispatchable = s == "workflow_dispatch";
            callable = s == "workflow_call";
        }
        Some(Value::Sequence(items)) => {
            for item in items {
                if let Value::String(s) = item {
                    dispatchable |= s == "workflow_dispatch";
                    callable |= s == "workflow_call";
                }
            }
        }
        Some(Value::Mapping(map)) => {
            for (key, value) in map {
                let Value::String(trigger) = key else {
                    continue;
                };
                match trigger.as_str() {
                    "workflow_dispatch" => {
                        dispatchable = true;
                        collect_inputs(value, &mut inputs);
                    }
                    "workflow_call" => {
                        callable = true;
                        collect_inputs(value, &mut inputs);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    inputs.sort();
    inputs.dedup();
    (dispatchable, callable, inputs)
}

fn collect_inputs(trigger: &serde_norway::Value, out: &mut Vec<String>) {
    use serde_norway::Value;
    let Value::Mapping(m) = trigger else { return };
    let Some(Value::Mapping(inputs)) = m.get(Value::String("inputs".into())) else {
        return;
    };
    for key in inputs.keys() {
        if let Value::String(name) = key {
            out.push(name.clone());
        }
    }
}

/// Discover every workflow under `root/.github/workflows`.
///
/// Sorted by name for deterministic listings and stable prompts.
pub fn discover(root: &Path) -> Vec<Workflow> {
    let dir = root.join(WORKFLOW_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .collect();
    paths.sort();

    let mut workflows: Vec<Workflow> = paths
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).ok()?;
            let relative = p.strip_prefix(root).unwrap_or(p);
            parse(relative, &text)
        })
        .collect();

    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    workflows
}

/// Find a workflow by `name:` or by filename.
pub fn find<'a>(workflows: &'a [Workflow], needle: &str) -> Option<&'a Workflow> {
    workflows
        .iter()
        .find(|w| w.name == needle)
        .or_else(|| workflows.iter().find(|w| w.id() == needle))
        .or_else(|| {
            workflows.iter().find(|w| {
                w.path
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy() == needle)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(text: &str) -> Workflow {
        parse(Path::new(".github/workflows/prepare.yml"), text).expect("parses")
    }

    const CONFORMING: &str = r#"
name: prepare-release
run-name: prepare-release (ship:${{ inputs.ship_id }})
on:
  workflow_dispatch:
    inputs:
      ship_id:
        required: true
        type: string
      dry_run:
        required: false
        type: boolean
        default: false
  workflow_call:
    inputs:
      ship_id:
        required: true
        type: string
jobs:
  prepare:
    runs-on: ubuntu-latest
    steps: []
"#;

    #[test]
    fn conforming_workflow_has_no_violations() {
        let w = wf(CONFORMING);
        assert_eq!(w.name, "prepare-release");
        assert!(w.dispatchable);
        assert!(w.callable, "declaring workflow_call too keeps it reusable");
        assert!(w.accepts_ship_id());
        assert!(w.accepts_dry_run());
        assert!(w.correlates());
        assert_eq!(w.contract_violations(), vec![]);
        assert_eq!(w.id(), "prepare.yml");
    }

    #[test]
    fn call_only_workflow_is_not_dispatchable() {
        // This is *the* setup mistake: a "reusable workflow" cannot be
        // started by the API.
        let w = wf(
            "name: x\non:\n  workflow_call:\n    inputs:\n      ship_id:\n        type: string\n",
        );
        assert!(!w.dispatchable);
        assert!(w.callable);
        assert!(
            w.contract_violations()
                .contains(&Violation::NotDispatchable)
        );
    }

    #[test]
    fn detects_missing_run_name_correlation() {
        let w = wf(
            "name: x\nrun-name: just a name\non:\n  workflow_dispatch:\n    inputs:\n      ship_id:\n        type: string\n",
        );
        assert!(!w.correlates());
        assert_eq!(w.contract_violations(), vec![Violation::MissingRunName]);
    }

    #[test]
    fn detects_missing_ship_id_input() {
        let w =
            wf("name: x\nrun-name: x (ship:${{ inputs.ship_id }})\non:\n  workflow_dispatch:\n");
        assert!(!w.accepts_ship_id());
        assert!(
            w.contract_violations()
                .contains(&Violation::MissingShipIdInput)
        );
    }

    #[test]
    fn handles_bare_string_trigger() {
        let w = wf("name: x\non: workflow_dispatch\n");
        assert!(w.dispatchable);
        assert!(w.inputs.is_empty());
    }

    #[test]
    fn handles_sequence_trigger() {
        let w = wf("name: x\non: [push, workflow_dispatch]\n");
        assert!(w.dispatchable);
    }

    #[test]
    fn handles_yaml_1_1_on_parsed_as_boolean_true() {
        // Some YAML parsers turn the `on:` key into boolean `true`.
        // Whichever way it lands, the trigger analysis must work.
        let w = parse(
            Path::new("w.yml"),
            "name: x\ntrue:\n  workflow_dispatch:\n    inputs:\n      ship_id:\n        type: string\n",
        )
        .unwrap();
        assert!(w.dispatchable, "the `on:`/`true:` alias must be handled");
    }

    #[test]
    fn name_falls_back_to_filename() {
        let w = parse(Path::new(".github/workflows/release.yaml"), "on: push\n").unwrap();
        assert_eq!(w.name, "release");
    }

    #[test]
    fn workflow_without_triggers_is_inert() {
        let w = wf("name: x\n");
        assert!(!w.dispatchable);
        assert!(!w.callable);
    }

    #[test]
    fn invalid_yaml_is_ignored_rather_than_fatal() {
        assert!(parse(Path::new("w.yml"), "\tnot: [valid").is_none());
    }

    #[test]
    fn find_matches_name_filename_and_stem() {
        let workflows = vec![wf(CONFORMING)];
        assert!(find(&workflows, "prepare-release").is_some());
        assert!(find(&workflows, "prepare.yml").is_some());
        assert!(find(&workflows, "prepare").is_some());
        assert!(find(&workflows, "nope").is_none());
    }

    #[test]
    fn discover_reads_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let wfdir = dir.path().join(WORKFLOW_DIR);
        std::fs::create_dir_all(&wfdir).unwrap();
        std::fs::write(wfdir.join("b.yml"), CONFORMING).unwrap();
        std::fs::write(wfdir.join("a.yaml"), "name: aaa\non: push\n").unwrap();
        std::fs::write(wfdir.join("README.md"), "not a workflow").unwrap();

        let found = discover(dir.path());
        assert_eq!(found.len(), 2, "non-YAML files must be ignored");
        assert_eq!(found[0].name, "aaa", "results are sorted by name");
        assert_eq!(found[1].name, "prepare-release");
        assert_eq!(
            found[1].path,
            Path::new(".github/workflows/b.yml"),
            "paths are relative to the repository root"
        );
    }

    #[test]
    fn discover_on_a_repo_without_workflows_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(dir.path()).is_empty());
    }

    #[test]
    fn violations_explain_the_reusable_workflow_trap() {
        let help = Violation::NotDispatchable.help();
        assert!(help.contains("workflow_call"), "{help}");
        assert!(help.contains("reusable"), "{help}");
    }
}
