//! What `gh ship init` actually writes.
//!
//! Six files — two roles times three token strategies — snapshotted in
//! full. The behavioural checks live next to the renderer in
//! `src/templates.rs`; these exist so that editing a template shows up in
//! review as a diff of the *generated workflow*, which is the artefact a
//! user ends up running. A comment reworded in the wrong branch, or an
//! `{% if %}` that quietly swallows a step, is obvious here and nowhere
//! else.

use gh_ship::templates::{Role, TokenStrategy, render};

/// Snapshot every combination `init` can produce.
///
/// One test rather than six, so that a change to shared template prose
/// presents as a single reviewable unit instead of six unrelated
/// failures.
#[test]
fn generated_workflows() {
    for strategy in [
        TokenStrategy::App,
        TokenStrategy::Pat,
        TokenStrategy::Default,
    ] {
        for role in [Role::Prepare, Role::Publish] {
            let name = format!(
                "{}-{}",
                role.filename().trim_end_matches(".yml"),
                match strategy {
                    TokenStrategy::App => "app",
                    TokenStrategy::Pat => "pat",
                    TokenStrategy::Default => "default",
                }
            );
            insta::assert_snapshot!(name, render(role, strategy));
        }
    }
}
