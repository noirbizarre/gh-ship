//! `gh ship prepare` — run the prepare workflow and open the Release PR.
//!
//! The sequence, and why it is this order:
//!
//! 1. **Refuse to start** while a merged Release PR is still awaiting
//!    `gh ship release`: a second run would bury the pending release.
//! 2. **Sweep staging branches** left behind by earlier runs.
//! 3. **Cut a throwaway staging branch**, `ship/prepare-<nonce>`, from
//!    the base branch. `workflow_dispatch` reads the workflow definition
//!    from the ref it is given, so the ref must exist *before*
//!    dispatching — and cutting it fresh from the base guarantees the
//!    dispatched copy is the current one, which dispatching on the
//!    long-lived release branch did not.
//! 4. **Dispatch on that staging branch and wait.** The workflow bumps
//!    the version, writes the changelog, commits and pushes there.
//!    gh-ship does none of this.
//! 5. **Validate the artifact.** A workflow that reports nothing to
//!    release stops here, successfully.
//! 6. **Promote**: move the release branch onto the staged release
//!    commit, then sweep the staging branch away.
//! 7. **Open or update the Release PR**, embedding the artifact in its
//!    body so `gh ship release` can recover it later without any local
//!    state.
//!
//! Re-running `prepare` is safe and is the supported way to refresh a
//! Release PR: the release branch moves onto the new commit and the
//! existing PR is updated in place.

use miette::Result;

use gh_ship::cli::{Cli, PrepareArgs};
use gh_ship::gh::repo;
use gh_ship::gh::run::ShipId;
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, report_nothing_to_release, run_workflow_as};
use super::short_sha;

pub fn run(cli: &Cli, args: &PrepareArgs, theme: Theme) -> Result<()> {
    let ctx = Context::load(cli, theme)?;

    eprintln!("{}", logger::action(theme, "preparing", ctx.repo_slug()));

    let release_branch = ctx.release_branch().to_string();
    let base_branch = ctx.base_branch().to_string();

    // Read the base tip once: the guard compares it against the merge commit of
    // the last Release PR, and staging cuts its branch from it.
    let base_sha = repo::branch_sha(&ctx.gh, ctx.repo_slug(), &base_branch)?;

    // Refuse to start a release on top of one still in flight, or on top of one
    // that just landed and left nothing behind it.
    if release_in_flight(&ctx, &release_branch, &base_branch, &base_sha)? {
        return Ok(());
    }

    // Clean up after any run that was abandoned before it could tidy up.
    sweep_staging_branches(&ctx);

    // Stage on a throwaway branch cut from the base, rather than resetting the
    // release branch in place. See `stage_branch` for why.
    let ship_id = ShipId::generate();
    let staging = stage_branch(&ship_id);

    eprintln!(
        "{}",
        logger::action(
            theme,
            "staging on",
            &format!("{staging} from {base_branch}")
        )
    );
    repo::create_branch_at(&ctx.gh, ctx.repo_slug(), &staging, &base_sha)?;

    if args.no_wait {
        return dispatch_only(&ctx, &staging, &ship_id);
    }

    let artifact = run_workflow_as(&ctx, ctx.config.prepare_workflow(), &staging, &ship_id, &[])?;

    if !artifact.changed {
        // Nothing was committed, so there is nothing to promote and no reason
        // to leave a release branch behind.
        sweep_staging_branches(&ctx);
        report_nothing_to_release(theme);
        return Ok(());
    }

    // Promote: move the release branch straight from its previous release
    // commit to the new one, in a single update.
    promote(&ctx, &staging, &release_branch)?;
    sweep_staging_branches(&ctx);

    let rendered = render::render(ctx.config.pull_request(), &artifact)?;

    // The artifact rides along in the PR body. This is what makes
    // gh-ship stateless: `release` can run days later, on another
    // machine, by another person, and still know exactly what was
    // prepared.
    let body = render::embed_artifact(&rendered.body, &artifact);

    // Labels must exist before the PR references them: `gh pr create`
    // fails outright on an unknown label, which would lose the PR over
    // pure decoration.
    let labels = ensure_labels(&ctx, &rendered.labels);

    upsert_pull_request(
        &ctx,
        &release_branch,
        &base_branch,
        &rendered.title,
        &body,
        &labels,
    )?;

    eprintln!();
    eprintln!(
        "{}",
        logger::release_identity(theme, artifact.version(), artifact.tag())
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
    let theme = ctx.theme;
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

/// Whether the last Release PR makes a new release cycle pointless right now.
///
/// Merging the Release PR is itself a push to base, so automation runs
/// `prepare` on it. Both ends of that merge need guarding, and both are
/// detected here:
///
/// **Merged, not yet published.** Between merging and `gh ship release` the tag
/// does not exist, so a changelog tool still reports the same version as
/// unreleased. Preparing in that window starts a *second* release for a version
/// already merged, quietly clobbering the first.
///
/// **Merged and published, with nothing after it.** Once the release is out,
/// the merge commit is still the tip of base. There is provably no new commit
/// to release, so dispatching the prepare workflow only to be told
/// `changed: false` is noise.
///
/// The second case is a sha comparison rather than a commit-message check for
/// the same reason the first cannot be one: the Release PR lands as
/// `Merge pull request #N…`, or as the PR title when squashed, or — rebased —
/// as the release commit itself. No message is common to all three, but the
/// merge commit recorded on the pull request is.
///
/// Returns `true` when the caller should stop. Stopping is a **success**:
/// nothing is wrong, and an orchestrator workflow must not go red on every push
/// because the release it just made is still the newest thing on the branch.
fn release_in_flight(
    ctx: &Context,
    release_branch: &str,
    base_branch: &str,
    base_sha: &str,
) -> Result<bool> {
    let theme = ctx.theme;

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
        // Released already. A new cycle may start — unless the merge that
        // carried this release is still the tip of base, in which case there is
        // nothing on top of it to release.
        if pr.merged_sha() != Some(base_sha) {
            return Ok(false);
        }

        eprintln!(
            "{}",
            logger::ok(theme, &format!("release {tag} is already published"))
        );
        eprintln!("{}", logger::detail_url(theme, "pr", &pr.url));
        eprintln!(
            "{}",
            logger::skip(
                theme,
                &format!("nothing new on {base_branch} since it was merged")
            )
        );
        return Ok(true);
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

/// Prefix for the throwaway branches `prepare` stages its work on.
pub const STAGING_PREFIX: &str = "ship/prepare-";

/// The staging branch name for a run.
///
/// Named after the correlation nonce so the branch, the dispatch and the run
/// all carry one identifier — which is what makes a branch left behind by a
/// failed run traceable to the run that abandoned it.
fn stage_branch(ship_id: &ShipId) -> String {
    format!("{STAGING_PREFIX}{ship_id}")
}

/// Move the release branch onto the staged release commit.
///
/// This is why the work is staged elsewhere rather than done on the release
/// branch directly. Resetting the release branch to its base first — which is
/// what gh-ship used to do — leaves it momentarily identical to that base, and
/// GitHub closes a pull request whose head becomes contained in its base. The
/// Release PR was therefore closed on every prepare and a fresh one opened in
/// its place.
///
/// Promoting instead moves the branch from its previous release commit straight
/// to the new one, in a single update. It is never equal to the base, so the PR
/// is never emptied and never closed.
fn promote(ctx: &Context, staging: &str, release: &str) -> Result<()> {
    let theme = ctx.theme;
    let staged_sha = repo::branch_sha(&ctx.gh, ctx.repo_slug(), staging)?;

    if repo::branch_exists(&ctx.gh, ctx.repo_slug(), release)? {
        eprintln!(
            "{}",
            logger::action(
                theme,
                "updating",
                &format!("{release} to {}", short_sha(&staged_sha))
            )
        );
        repo::reset_branch(&ctx.gh, ctx.repo_slug(), release, &staged_sha)?;
    } else {
        eprintln!("{}", logger::action(theme, "creating branch", release));
        repo::create_branch_at(&ctx.gh, ctx.repo_slug(), release, &staged_sha)?;
    }
    Ok(())
}

/// Delete every staging branch.
///
/// Housekeeping, so it never fails a release: a branch that cannot be deleted
/// is reported and otherwise ignored. Sweeping on every run is what stops
/// abandoned runs — a failure part-way, or `--no-wait` — from accumulating
/// branches, and is safe because gh-ship releases one at a time: the Release PR
/// is the lock.
fn sweep_staging_branches(ctx: &Context) {
    for branch in repo::matching_branches(&ctx.gh, ctx.repo_slug(), STAGING_PREFIX) {
        if repo::delete_branch(&ctx.gh, ctx.repo_slug(), &branch).is_err() {
            eprintln!(
                "{}",
                logger::warn(
                    ctx.theme,
                    &format!("could not delete the staging branch `{branch}`")
                )
            );
        }
    }
}

/// Open, update or reopen the Release PR.
///
/// `pull_request.reuse` decides between keeping one PR across prepares — so a
/// release under review keeps its number, comments and review state — and
/// opening a fresh one every time.
fn upsert_pull_request(
    ctx: &Context,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<()> {
    let theme = ctx.theme;
    let reuse = ctx.config.reuse_pull_request();
    let existing = repo::find_pull_request(&ctx.gh, head, base)?;

    // A merged PR belongs to a release that already shipped; never touch it.
    let reusable = existing.filter(|pr| !pr.is_merged());

    match reusable {
        Some(pr) if reuse => {
            if !pr.is_open() {
                eprintln!(
                    "{}",
                    logger::action(theme, "reopening", &format!("PR #{}", pr.number))
                );
                repo::reopen_pull_request(&ctx.gh, pr.number)?;
            }
            eprintln!(
                "{}",
                logger::action(theme, "updating", &format!("PR #{}", pr.number))
            );
            repo::update_pull_request(&ctx.gh, pr.number, title, body, labels)?;
            eprintln!("{}", logger::ok(theme, "Release PR updated"));
            eprintln!("{}", logger::detail_url(theme, "pr", &pr.url));
        }
        Some(pr) if pr.is_open() => {
            // reuse == false: retire the current PR before opening its successor.
            eprintln!(
                "{}",
                logger::action(theme, "closing", &format!("PR #{}", pr.number))
            );
            repo::close_pull_request(&ctx.gh, pr.number)?;
            open_pull_request(ctx, head, base, title, body, labels)?;
        }
        _ => open_pull_request(ctx, head, base, title, body, labels)?,
    }
    Ok(())
}

fn open_pull_request(
    ctx: &Context,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<()> {
    let theme = ctx.theme;
    eprintln!("{}", logger::action(theme, "opening", "Release PR"));
    let url = repo::create_pull_request(&ctx.gh, head, base, title, body, labels)?;
    eprintln!("{}", logger::ok(theme, "Release PR opened"));
    eprintln!("{}", logger::detail_url(theme, "pr", &url));
    Ok(())
}

/// Dispatch without waiting.
///
/// Useful when the prepare workflow is slow and the caller would rather
/// poll later with `gh ship status`. The Release PR is *not* created,
/// because the artifact it needs does not exist yet — re-running
/// `gh ship prepare` afterwards completes the job.
fn dispatch_only(ctx: &Context, branch: &str, ship_id: &ShipId) -> Result<()> {
    let theme = ctx.theme;
    let workflow = ctx.workflow(ctx.config.prepare_workflow());

    eprintln!(
        "{}",
        logger::action(theme, "dispatching", &workflow.to_string())
    );
    let ship_id = gh_ship::gh::run::dispatch_as(&ctx.gh, &workflow, branch, ship_id, &[])?;

    eprintln!("{}", logger::ok(theme, "workflow dispatched"));
    eprintln!("{}", logger::detail(theme, "ship id", ship_id.as_str()));
    eprintln!("{}", logger::detail(theme, "staged on", branch));
    eprintln!();
    eprintln!(
        "{}",
        logger::skip(
            theme,
            "not waiting — the release branch is not updated and no PR is opened; \
             re-run `gh ship prepare` to complete the release"
        )
    );
    eprintln!(
        "{}",
        logger::skip(
            theme,
            &format!("`{branch}` is left behind and swept by the next prepare")
        )
    );
    Ok(())
}
