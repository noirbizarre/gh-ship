//! `gh-ship` — the GitHub Release Orchestrator.
//!
//! gh-ship orchestrates the lifecycle of a GitHub Release around
//! workflows the project already owns. It creates the release branch,
//! dispatches your workflow, waits for it, validates the artifact it
//! produced, renders the Release PR, and creates the GitHub Release.
//!
//! It is emphatically **not** a workflow engine. There is no DSL, no
//! step registry, no `run:` key. If you find yourself wanting to add
//! one, the project has failed.
//!
//! # Responsibilities
//!
//! | gh-ship does | your workflow does |
//! |---|---|
//! | create the release branch | bump the version |
//! | dispatch workflows | generate the changelog |
//! | wait and correlate runs | update files |
//! | validate the artifact | commit and push |
//! | render the Release PR | |
//! | create the GitHub Release | |
//!
//! # The protocol
//!
//! The boundary between the two columns is a single JSON document,
//! [`artifact::ARTIFACT_FILE`], uploaded as a workflow artifact. It is
//! versioned, schema-validated, and documented — see [`artifact`].

// Diagnostic-bearing error variants carry rich miette payloads
// (`NamedSource<String>`, `SourceSpan`, suggestion strings) so they
// comfortably exceed clippy's 128-byte threshold. Boxing them would
// defeat ergonomic `?` usage everywhere validation happens, so the lint
// is silenced crate-wide rather than per-function.
#![allow(clippy::result_large_err)]

pub mod artifact;
pub mod branches;
pub mod cli;
pub mod config;
pub mod detect;
pub mod gh;
pub mod logger;
pub mod render;
pub mod style;
pub mod suggest;
pub mod templates;
