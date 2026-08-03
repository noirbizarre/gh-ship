//! Everything that talks to GitHub.
//!
//! gh-ship shells out to the GitHub CLI for **all** GitHub access. It
//! implements no REST client and handles no tokens: `gh` already solves
//! authentication, enterprise hosts, SSO, and rate limiting, and
//! reimplementing that would be a second product.
//!
//! The one thing gh-ship reads directly from disk is workflow files,
//! because their trigger configuration is not exposed by any API in a
//! usable form.

pub mod cli;
pub mod repo;
pub mod run;
pub mod workflow;

pub use cli::{Gh, GhError};

/// Read a `SHIP_*` knob expressed in whole seconds.
///
/// Every duration knob gh-ship exposes is seconds-as-integer, so the parsing
/// lives here rather than being re-derived per module. `None` covers both
/// "unset" and "unparseable": a typo falls back to the documented default
/// rather than failing a release over an environment variable.
pub(crate) fn env_duration(key: &str) -> Option<std::time::Duration> {
    std::env::var(key)
        .ok()?
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}
