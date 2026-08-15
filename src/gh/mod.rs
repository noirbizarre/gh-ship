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

/// A short random token, unique per invocation.
///
/// Twelve hex characters of a v4 UUID: short enough to read back in a branch
/// name or a log line, wide enough that two concurrent releases cannot
/// collide.
///
/// Two things need one, and they need the same one. `prepare` names its
/// staging branch after it, which is what makes the branch — and therefore
/// the run dispatched on it — belong to exactly one invocation, and a branch
/// left behind by a failed run traceable to it. [`run::dispatch`] uses it as
/// the throwaway value for the legacy `ship_id` input, which nothing reads
/// back.
pub fn short_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Read a `SHIP_*` knob.
///
/// `None` covers both "unset" and "unparsable": a typo falls back to the
/// documented default rather than failing a release over an environment
/// variable.
pub(crate) fn env_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok()?.parse().ok()
}

/// Read a `SHIP_*` knob expressed in whole seconds.
///
/// Every duration knob gh-ship exposes is seconds-as-integer, so the parsing
/// lives here rather than being re-derived per module.
pub(crate) fn env_duration(key: &str) -> Option<std::time::Duration> {
    env_parsed::<u64>(key).map(std::time::Duration::from_secs)
}
