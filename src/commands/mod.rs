//! Command implementations.
//!
//! Each module owns one subcommand end to end: argument interpretation,
//! orchestration, and output. Nothing here holds state between runs —
//! gh-ship keeps zero local state, so every command reconstructs what it
//! needs from the artifact it is given or from GitHub itself.

pub mod context;
pub mod init;
pub mod prepare;
pub mod preview;
pub mod release;
pub mod status;
pub mod validate;
