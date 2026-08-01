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

use gh_ship::cli::{Cli, PreviewArgs};
use gh_ship::gh::repo;
use gh_ship::gh::workflow::DRY_RUN_INPUT;
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, print_rendered, report_nothing_to_release, run_workflow};

pub fn run(cli: &Cli, args: &PreviewArgs, theme: &Theme) -> Result<()> {
    let ctx = Context::load(cli, theme)?;

    eprintln!("{}", logger::action(theme, "previewing", ctx.repo_slug()));
    eprintln!(
        "{}",
        logger::skip(theme, "dry run — nothing on GitHub will be modified")
    );

    // Preview runs on a branch that already exists so it never creates
    // one. The release branch is preferred when present, because that is
    // where a real prepare would run and the result should match.
    let branch = preview_branch(&ctx)?;

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

    let rendered = render::render(&ctx.config.settings.pull_request, &artifact)?;

    if args.json {
        let out = serde_json::json!({
            "artifact": artifact,
            "pull_request": {
                "title": rendered.title,
                "body": rendered.body,
                "labels": rendered.labels,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
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

/// Choose a branch to dispatch the preview on.
///
/// `workflow_dispatch` reads the workflow definition from the ref it is
/// given, so the ref has to exist. Preferring the release branch when it
/// exists means a preview reflects what a real prepare would produce.
fn preview_branch(ctx: &Context) -> Result<String> {
    let release = ctx.release_branch();
    if repo::branch_exists(&ctx.gh, ctx.repo_slug(), release)? {
        return Ok(release.to_string());
    }
    Ok(ctx.base_branch().to_string())
}
