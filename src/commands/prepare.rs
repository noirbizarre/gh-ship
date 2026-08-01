//! `gh ship prepare` — run the prepare workflow and open the Release PR.
//!
//! The sequence, and why it is this order:
//!
//! 1. **Ensure the release branch exists.** `workflow_dispatch` reads
//!    the workflow definition from the ref it is given, so the ref must
//!    exist *before* dispatching. gh-ship creates it from the base
//!    branch when missing.
//! 2. **Dispatch and wait.** The workflow bumps the version, writes the
//!    changelog, commits and pushes to that branch. gh-ship does none of
//!    this.
//! 3. **Validate the artifact.** A workflow that reports nothing to
//!    release stops here, successfully.
//! 4. **Open or update the Release PR**, embedding the artifact in its
//!    body so `gh ship release` can recover it later without any local
//!    state.
//!
//! Re-running `prepare` is safe and is the supported way to refresh a
//! Release PR: it reuses the branch and updates the existing PR.

use miette::Result;

use gh_ship::cli::{Cli, PrepareArgs};
use gh_ship::gh::repo;
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, report_nothing_to_release, run_workflow};

pub fn run(cli: &Cli, args: &PrepareArgs, theme: &Theme) -> Result<()> {
    let ctx = Context::load(cli, theme)?;

    eprintln!("{}", logger::action(theme, "preparing", ctx.repo_slug()));

    let release_branch = ctx.release_branch().to_string();
    let base_branch = ctx.base_branch().to_string();

    ensure_release_branch(&ctx, &release_branch, &base_branch)?;

    if args.no_wait {
        return dispatch_only(&ctx, &release_branch);
    }

    let artifact = run_workflow(&ctx, ctx.config.prepare_workflow(), &release_branch, &[])?;

    if !artifact.changed {
        report_nothing_to_release(theme);
        return Ok(());
    }

    let rendered = render::render(&ctx.config.settings.pull_request, &artifact)?;

    // The artifact rides along in the PR body. This is what makes
    // gh-ship stateless: `release` can run days later, on another
    // machine, by another person, and still know exactly what was
    // prepared.
    let body = render::embed_artifact(&rendered.body, &artifact);

    let existing = repo::find_pull_request(&ctx.gh, &release_branch, &base_branch)?;

    match existing.filter(|pr| pr.is_open()) {
        Some(pr) => {
            eprintln!(
                "{}",
                logger::action(theme, "updating", &format!("PR #{}", pr.number))
            );
            repo::update_pull_request(
                &ctx.gh,
                pr.number,
                &rendered.title,
                &body,
                &rendered.labels,
            )?;
            eprintln!("{}", logger::ok(theme, "Release PR updated"));
            eprintln!("{}", logger::detail_url(theme, "pr", &pr.url));
        }
        None => {
            eprintln!("{}", logger::action(theme, "opening", "Release PR"));
            let url = repo::create_pull_request(
                &ctx.gh,
                &release_branch,
                &base_branch,
                &rendered.title,
                &body,
                &rendered.labels,
            )?;
            eprintln!("{}", logger::ok(theme, "Release PR opened"));
            eprintln!("{}", logger::detail_url(theme, "pr", &url));
        }
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
    eprintln!();
    eprintln!(
        "{}",
        logger::skip(theme, "review and merge the PR, then run `gh ship release`")
    );

    Ok(())
}

/// Create the release branch if it does not exist yet.
fn ensure_release_branch(ctx: &Context, release: &str, base: &str) -> Result<()> {
    let theme = &ctx.theme;

    if repo::branch_exists(&ctx.gh, ctx.repo_slug(), release)? {
        eprintln!("{}", logger::detail(theme, "branch", release));
        return Ok(());
    }

    eprintln!(
        "{}",
        logger::action(theme, "creating branch", &format!("{release} from {base}"))
    );
    repo::create_branch(&ctx.gh, ctx.repo_slug(), release, base)?;
    eprintln!("{}", logger::ok(theme, &format!("created {release}")));
    Ok(())
}

/// Dispatch without waiting.
///
/// Useful when the prepare workflow is slow and the caller would rather
/// poll later with `gh ship status`. The Release PR is *not* created,
/// because the artifact it needs does not exist yet — re-running
/// `gh ship prepare` afterwards completes the job.
fn dispatch_only(ctx: &Context, branch: &str) -> Result<()> {
    let theme = &ctx.theme;
    let workflow = ctx.config.prepare_workflow();

    eprintln!("{}", logger::action(theme, "dispatching", workflow));
    let ship_id = gh_ship::gh::run::dispatch(&ctx.gh, workflow, branch, &[])?;

    eprintln!("{}", logger::ok(theme, "workflow dispatched"));
    eprintln!("{}", logger::detail(theme, "ship id", ship_id.as_str()));
    eprintln!();
    eprintln!(
        "{}",
        logger::skip(
            theme,
            "not waiting — run `gh ship prepare` again once it finishes to open the Release PR"
        )
    );
    Ok(())
}
