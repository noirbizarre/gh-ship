//! Command implementations.
//!
//! Each module owns one subcommand end to end: argument interpretation,
//! orchestration, and output — except `context`, which holds the
//! orchestration `prepare`, `preview` and `release` share. Nothing here holds state between runs —
//! gh-ship keeps zero local state, so every command reconstructs what it
//! needs from the artifact it is given or from GitHub itself.

pub mod context;
pub mod init;
pub mod prepare;
pub mod preview;
pub mod release;
pub mod status;
pub mod validate;

use std::path::{Path, PathBuf};

/// Infer the repository root from the config path.
///
/// `.github/ship.yml` → the directory containing `.github`. A config passed
/// with `--config` from elsewhere falls back to the current directory, which
/// is the best guess available.
pub(crate) fn repo_root(config: &Path) -> PathBuf {
    config
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == ".github"))
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A commit SHA abbreviated for display.
///
/// Seven characters is git's own default abbreviation and what GitHub shows,
/// so it is what a reader will recognise.
pub(crate) fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_is_the_grandparent_of_a_dot_github_config() {
        assert_eq!(repo_root(Path::new(".github/ship.yml")), Path::new("."));
        assert_eq!(
            repo_root(Path::new("/src/proj/.github/ship.yml")),
            Path::new("/src/proj")
        );
    }

    #[test]
    fn repo_root_falls_back_to_cwd_for_unusual_paths() {
        assert_eq!(repo_root(Path::new("ship.yml")), Path::new("."));
        assert_eq!(repo_root(Path::new("/tmp/custom.yml")), Path::new("."));
    }

    #[test]
    fn short_sha_abbreviates_to_seven_characters() {
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
    }

    #[test]
    fn short_sha_leaves_already_short_values_alone() {
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(short_sha(""), "");
    }
}
