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

    // Refuse to start a second release on top of one already merged.
    if pending_release(&ctx, &release_branch, &base_branch)? {
        return Ok(());
    }

    sync_release_branch(&ctx, &release_branch, &base_branch)?;

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

    // Labels must exist before the PR references them: `gh pr create`
    // fails outright on an unknown label, which would lose the PR over
    // pure decoration.
    let labels = ensure_labels(&ctx, &rendered.labels);

    let existing = repo::find_pull_request(&ctx.gh, &release_branch, &base_branch)?;

    match existing.filter(|pr| pr.is_open()) {
        Some(pr) => {
            eprintln!(
                "{}",
                logger::action(theme, "updating", &format!("PR #{}", pr.number))
            );
            repo::update_pull_request(&ctx.gh, pr.number, &rendered.title, &body, &labels)?;
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
                &labels,
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

/// Resolve configured labels to ones that actually exist, creating what
/// is missing and warning about anything that could not be created.
fn ensure_labels(ctx: &Context, wanted: &[String]) -> Vec<String> {
    let theme = &ctx.theme;
    let (usable, dropped) = repo::ensure_labels(&ctx.gh, wanted);

    for label in &dropped {
        eprintln!(
            "{}",
            logger::warn(
                theme,
                &format!(
                    "label `{label}` does not exist and could not be created — \
                          opening the PR without it"
                )
            )
        );
    }
    if !dropped.is_empty() {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                "creating labels needs `issues: write`; create them by hand or drop \
                 them from `pull_request.labels`"
            )
        );
    }

    usable
}

/// Whether a merged Release PR is still waiting on `gh ship release`.
///
/// Between merging the Release PR and running `gh ship release` the tag does
/// not exist yet, so a changelog tool still reports the same version as
/// unreleased. Preparing again in that window starts a *second* release for a
/// version already merged, quietly clobbering the first.
///
/// This matters most under automation — a push-triggered prepare hits this
/// window on the very push that merges the Release PR — but the hole is real
/// either way, so the guard lives here rather than in a workflow condition.
///
/// It is also why this cannot be a commit-message check: the Release PR lands
/// as `Merge pull request #N…`, or as the PR title when squashed. Neither
/// carries the release prefix.
///
/// Returns `true` when the caller should stop. Stopping is a **success**:
/// nothing is wrong, the release simply needs finishing, and an orchestrator
/// workflow must not go red on every push until someone ships it.
fn pending_release(ctx: &Context, release_branch: &str, base_branch: &str) -> Result<bool> {
    let theme = &ctx.theme;

    let Some(pr) = repo::find_pull_request(&ctx.gh, release_branch, base_branch)? else {
        return Ok(false);
    };
    if !pr.is_merged() {
        return Ok(false);
    }

    // A merged PR with no artifact cannot be released from; `gh ship status`
    // already reports that, and preparing afresh is the way out.
    let Some(artifact) = render::extract_artifact(&pr.body) else {
        return Ok(false);
    };
    let Some(tag) = artifact.tag().filter(|_| artifact.changed) else {
        return Ok(false);
    };

    if repo::release_exists(&ctx.gh, tag)? {
        // Released already: that cycle is complete, so a new one may start.
        return Ok(false);
    }

    eprintln!(
        "{}",
        logger::ok(
            theme,
            &format!("release {tag} is prepared and merged, but not yet published")
        )
    );
    eprintln!("{}", logger::detail_url(theme, "pr", &pr.url));
    eprintln!(
        "{}",
        logger::skip(
            theme,
            "run `gh ship release` to publish it — preparing now would start a \
             second release for a version already merged"
        )
    );
    Ok(true)
}

/// Bring the release branch in line with the base branch.
///
/// Creating the branch when missing is not enough. Once it exists, every later
/// prepare would run against whatever it contained the first time — so a
/// changelog tool sees a history that is missing everything merged into the base
/// since, produces byte-identical output, and the Release PR silently stops
/// updating. The failure is invisible: the workflow succeeds, gh-ship reports
/// "Release PR updated", and nothing changes. It also gets worse the longer a
/// Release PR stays open, which is exactly when it matters.
///
/// So the branch is reset to the base on every run. That is safe because it is
/// machine-managed and disposable: the version bump and the changelog are
/// regenerated from scratch each time, so nothing is lost, and the Release PR
/// always ends up as exactly one commit on top of the base.
fn sync_release_branch(ctx: &Context, release: &str, base: &str) -> Result<()> {
    let theme = &ctx.theme;
    let base_sha = repo::branch_sha(&ctx.gh, ctx.repo_slug(), base)?;

    if !repo::branch_exists(&ctx.gh, ctx.repo_slug(), release)? {
        eprintln!(
            "{}",
            logger::action(theme, "creating branch", &format!("{release} from {base}"))
        );
        repo::create_branch_at(&ctx.gh, ctx.repo_slug(), release, &base_sha)?;
        eprintln!("{}", logger::ok(theme, &format!("created {release}")));
        return Ok(());
    }

    // Announce it: force-updating a branch deserves to be visible, even when it
    // is the expected behaviour.
    eprintln!(
        "{}",
        logger::action(
            theme,
            "resetting",
            &format!("{release} to {base}@{}", short_sha(&base_sha))
        )
    );
    repo::reset_branch(&ctx.gh, ctx.repo_slug(), release, &base_sha)?;
    Ok(())
}

/// Abbreviate a SHA the way git does.
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// Dispatch without waiting.
///
/// Useful when the prepare workflow is slow and the caller would rather
/// poll later with `gh ship status`. The Release PR is *not* created,
/// because the artifact it needs does not exist yet — re-running
/// `gh ship prepare` afterwards completes the job.
fn dispatch_only(ctx: &Context, branch: &str) -> Result<()> {
    let theme = &ctx.theme;
    let workflow = ctx.workflow(ctx.config.prepare_workflow());

    eprintln!(
        "{}",
        logger::action(theme, "dispatching", &workflow.to_string())
    );
    let ship_id = gh_ship::gh::run::dispatch(&ctx.gh, &workflow, branch, &[])?;

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
