//! `gh ship preview` — see the Release PR without creating anything.
//!
//! Preview cannot guess: the version and the release notes come from the
//! user's own tooling, so the only honest way to show what a release
//! would look like is to actually run the prepare workflow.
//!
//! It does so with `dry_run: true`, which the workflow contract requires
//! to mean "produce the artifact, but do not commit or push". Nothing on
//! GitHub is mutated: no branch, no PR, no tag, no release.

use miette::Result;
use serde::Serialize;

use gh_ship::artifact::Artifact;
use gh_ship::cli::{Cli, PreviewArgs};
use gh_ship::gh::workflow::DRY_RUN_INPUT;
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, print_rendered, report_nothing_to_release, run_workflow};

pub fn run(cli: &Cli, args: &PreviewArgs, theme: Theme) -> Result<()> {
    let ctx = Context::load(cli, theme)?;

    eprintln!("{}", logger::action(theme, "previewing", ctx.repo_slug()));
    eprintln!(
        "{}",
        logger::skip(theme, "dry run — nothing on GitHub will be modified")
    );

    // Preview runs on a branch that already exists so it never creates one:
    // the base branch, always. `prepare` stages on a throwaway branch cut from
    // the base, so the base *is* what a real prepare runs against. Preferring
    // a stale release branch would make preview report a history that no
    // longer matches reality — the same silent-staleness bug, in the one
    // command whose whole job is to tell the truth without changing anything.
    let branch = ctx.base_branch().to_string();

    let artifact = run_workflow(
        &ctx,
        ctx.config.prepare_workflow(),
        &branch,
        &[(DRY_RUN_INPUT, "true".to_string())],
    )?;

    if !artifact.changed {
        report_nothing_to_release(theme);
        return Ok(());
    }

    let rendered = render::render(ctx.config.pull_request(), &artifact)?;

    if args.json {
        let out = Preview {
            pull_request: PreviewPullRequest {
                title: &rendered.title,
                body: &rendered.body,
                labels: &rendered.labels,
            },
            artifact: &artifact,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).expect("preview output is always serialisable")
        );
        return Ok(());
    }

    eprintln!();
    eprintln!(
        "{}",
        logger::detail(theme, "version", artifact.version().unwrap_or("?"))
    );
    eprintln!(
        "{}",
        logger::detail(theme, "tag", artifact.tag().unwrap_or("?"))
    );
    print_rendered(theme, &rendered.title, &rendered.body, &rendered.labels);
    eprintln!(
        "{}",
        logger::skip(theme, "run `gh ship prepare` to open the Release PR")
    );

    Ok(())
}

/// `gh ship preview --json`.
///
/// Typed rather than an ad-hoc `json!` literal so the shape is tied to what
/// `render` actually produces and cannot drift when a field is renamed.
#[derive(Serialize)]
struct Preview<'a> {
    artifact: &'a Artifact,
    pull_request: PreviewPullRequest<'a>,
}

#[derive(Serialize)]
struct PreviewPullRequest<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
}
