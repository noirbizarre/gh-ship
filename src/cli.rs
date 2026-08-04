//! Command-line surface.
//!
//! Every command here is implemented. There are no stubs and no
//! documented-but-missing commands — the reference project this one
//! learns from drifted that way and it eroded trust in the docs.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Default configuration path, relative to the repository root.
pub const DEFAULT_CONFIG: &str = ".github/ship.yml";

/// The GitHub Release Orchestrator.
#[derive(Debug, Parser)]
#[command(
    name = "gh-ship",
    bin_name = "gh ship",
    version,
    about = "Orchestrate GitHub Releases: dispatch your workflows, render the Release PR, ship the release.",
    long_about = "gh ship orchestrates the GitHub Release lifecycle around workflows you already own.\n\n\
                  It never bumps versions, never writes changelogs, and never runs your release logic.\n\
                  Your GitHub Actions workflows do that, and hand back a `ship.release.json` artifact.",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(long, short, global = true, default_value = DEFAULT_CONFIG, env = "SHIP_CONFIG")]
    pub config: PathBuf,

    /// Repository in `OWNER/REPO` format. Defaults to the current
    /// repository, as resolved by the GitHub CLI.
    #[arg(long, short = 'R', global = true, env = "SHIP_REPO")]
    pub repo: Option<String>,

    /// Increase verbosity. Repeat for more detail.
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Make this repository gh-ship enabled.
    Init(InitArgs),

    /// Check the configuration, or a release artifact.
    Validate(ValidateArgs),

    /// Dry-run the prepare workflow and show the Release PR it would produce.
    Preview(PreviewArgs),

    /// Run the prepare workflow and open or update the Release PR.
    Prepare(PrepareArgs),

    /// Show where the current release stands.
    Status(StatusArgs),

    /// Tag, publish and release the merged Release PR.
    Release(ReleaseArgs),
}

/// The base branch to release from.
///
/// Flattened into the lifecycle commands rather than made global: it is
/// meaningless to `init` and `validate`, and a flag that is silently
/// ignored is worse than one that is refused.
#[derive(Debug, Clone, clap::Args)]
pub struct BaseArgs {
    /// Base branch to release from.
    ///
    /// Selects the release line when `branches` is configured.
    /// Otherwise defaults to the repository's default branch.
    #[arg(long, value_name = "BRANCH", env = "SHIP_BASE_BRANCH")]
    pub base: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ReleaseArgs {
    /// Merge the Release PR if it is still open.
    #[arg(long)]
    pub merge: bool,

    #[command(flatten)]
    pub base: BaseArgs,
}

#[derive(Debug, clap::Args)]
pub struct PrepareArgs {
    /// Dispatch the workflow but do not wait for it to finish.
    #[arg(long)]
    pub no_wait: bool,

    #[command(flatten)]
    pub base: BaseArgs,
}

#[derive(Debug, clap::Args)]
pub struct StatusArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    #[command(flatten)]
    pub base: BaseArgs,
}

#[derive(Debug, clap::Args)]
pub struct PreviewArgs {
    /// Emit the artifact and rendered PR as JSON.
    #[arg(long)]
    pub json: bool,

    #[command(flatten)]
    pub base: BaseArgs,
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Overwrite an existing configuration.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, clap::Args)]
pub struct ValidateArgs {
    /// A `ship.release.json` artifact to validate.
    ///
    /// When omitted, validates the gh-ship configuration and the
    /// workflows it references instead.
    ///
    /// Validating a file requires no network, no repository, and no
    /// GitHub authentication, so it is safe to run early in CI.
    pub artifact: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn validate_accepts_an_optional_artifact_path() {
        let cli = Cli::try_parse_from(["gh-ship", "validate"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Validate(ValidateArgs { artifact: None })
        ));

        let cli = Cli::try_parse_from(["gh-ship", "validate", "ship.release.json"]).unwrap();
        let Command::Validate(a) = cli.command else {
            panic!("expected validate")
        };
        assert_eq!(a.artifact.unwrap().to_str(), Some("ship.release.json"));
    }

    #[test]
    fn config_defaults_to_dot_github() {
        let cli = Cli::try_parse_from(["gh-ship", "validate"]).unwrap();
        assert_eq!(cli.config.to_str(), Some(DEFAULT_CONFIG));
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from(["gh-ship", "validate", "-R", "owner/repo", "-vv"]).unwrap();
        assert_eq!(cli.repo.as_deref(), Some("owner/repo"));
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn base_is_accepted_by_every_lifecycle_command() {
        for command in ["prepare", "preview", "release", "status"] {
            let cli = Cli::try_parse_from(["gh-ship", command, "--base", "release/1.x"])
                .unwrap_or_else(|e| panic!("{command} should accept --base: {e}"));
            let base = match cli.command {
                Command::Prepare(a) => a.base.base,
                Command::Preview(a) => a.base.base,
                Command::Release(a) => a.base.base,
                Command::Status(a) => a.base.base,
                other => panic!("unexpected command {other:?}"),
            };
            assert_eq!(base.as_deref(), Some("release/1.x"));
        }
    }

    #[test]
    fn base_is_refused_where_it_would_be_ignored() {
        for command in ["init", "validate"] {
            assert!(
                Cli::try_parse_from(["gh-ship", command, "--base", "main"]).is_err(),
                "{command} has no base branch to speak of"
            );
        }
    }
}
