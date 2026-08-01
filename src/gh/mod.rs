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
