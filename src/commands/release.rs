//! `gh ship release` — tag, publish, and release.
//!
//! # Why gh-ship does this, and not the publish workflow
//!
//! Tagging and creating the release from CI is the conventional split, so doing
//! it here needs justifying — and it is the first thing anyone refactoring this
//! module will want to undo.
//!
//! **The release notes are generated before the merge and must survive it.** The
//! prepare workflow runs the changelog tool against the *release branch*; the
//! publish workflow checks out the *tag*, which is post-merge history. Were the
//! publish workflow to regenerate them, what shipped could legitimately differ
//! from what was reviewed in the Release PR. The artifact carries them so that
//! what ships is what was read.
//!
//! It is also why the artifact has `release.name`, `release.prerelease` and
//! `release.make_latest` at all: they exist for this code. Moving release
//! creation into the workflow would strand them in a published v1 schema.
//!
//! The cost is the ordering below. `gh release create <tag> <assets…>` already
//! does draft → upload → publish atomically; this module reimplements that
//! because it does not hold the assets. Buying the native behaviour would mean
//! downloading every binary and re-uploading it, pushing release payloads
//! through whichever machine runs the command.
//!
//! # Why draft-first
//!
//! The naive ordering — create the release, then build and upload assets
//! — publishes an empty release. Everyone watching the repository is
//! notified immediately, and the assets appear minutes later. Anyone who
//! reacts quickly downloads nothing.
//!
//! So gh-ship creates the release as a **draft**, dispatches the publish
//! workflow to attach assets to it, and only then undrafts it. A draft
//! release is invisible to watchers and to the releases API, yet
//! `gh release upload` still works against it. That is the only ordering
//! where a release becomes visible complete.
//!
//! # Why the tag is created explicitly
//!
//! A draft release does **not** create its git ref — the tag appears only when
//! the release is published. Since the publish workflow is dispatched on that
//! tag and checks it out, relying on `gh release create` to produce it would
//! fail on the first release, after the release object already existed.
//!
//! So gh-ship creates `refs/tags/<tag>` itself, before the release. Tagging is
//! then something gh-ship does rather than a side effect it hopes for, and the
//! step is idempotent so a partial failure is recoverable by re-running.
//!
//! # Why the merge commit, not the branch tip
//!
//! The release branch tip that `prepare` saw is **not** what lands on
//! the base branch when a PR is squash- or rebase-merged: GitHub creates
//! a new commit. Tagging the remembered SHA would tag a commit that is
//! not on the base branch. gh-ship therefore always reads
//! `mergeCommit.oid` from the merged PR.

use miette::Result;

use gh_ship::artifact::Artifact;
use gh_ship::cli::{Cli, ReleaseArgs};
use gh_ship::gh::repo::{self, PullRequest};
use gh_ship::gh::workflow::SHIP_ID_INPUT;
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, report_nothing_to_release};
use super::short_sha;

pub fn run(cli: &Cli, args: &ReleaseArgs, theme: Theme) -> Result<()> {
    let ctx = Context::load(cli, theme)?;

    eprintln!("{}", logger::action(theme, "releasing", ctx.repo_slug()));

    let release_branch = ctx.release_branch().to_string();
    let base_branch = ctx.base_branch().to_string();

    // --- 1. Find the Release PR -----------------------------------------
    let pr = repo::find_pull_request(&ctx.gh, &release_branch, &base_branch)?.ok_or_else(|| {
        miette::miette!(
            code = "ship::release::no_pr",
            help = "run `gh ship prepare` first, or check `gh ship status`",
            "no Release PR found for `{release_branch}` → `{base_branch}`"
        )
    })?;

    // --- 2. Recover the artifact ----------------------------------------
    //
    // From the PR body, which is where `prepare` embedded it. This is why
    // gh-ship needs no local state and why this command works days later
    // on a different machine.
    let artifact = recover_artifact(&pr)?;

    if !artifact.changed {
        report_nothing_to_release(theme);
        return Ok(());
    }

    let tag = artifact
        .tag()
        .expect("schema guarantees a tag when changed");
    let version = artifact.version().unwrap_or(tag);

    eprintln!("{}", logger::detail(theme, "version", version));
    eprintln!("{}", logger::detail(theme, "tag", tag));

    // --- 3. Ensure the PR is merged -------------------------------------
    let pr = ensure_merged(&ctx, pr, args.merge)?;

    let target = pr.merged_sha().ok_or_else(|| {
        miette::miette!(
            code = "ship::release::no_merge_commit",
            help = "GitHub reported the PR as merged but returned no merge commit; \
                    retry in a moment, or check `gh ship status`",
            "could not determine the commit PR #{} merged as",
            pr.number
        )
    })?;
    eprintln!("{}", logger::detail(theme, "merged as", short_sha(target)));

    // --- 4. Tag the merge commit -----------------------------------------
    //
    // Before the release, not as a side effect of it: a draft release does not
    // create the git ref, and the publish workflow is dispatched on this tag
    // and checks it out.
    eprintln!("{}", logger::action(theme, "tagging", tag));
    repo::create_tag(&ctx.gh, ctx.repo_slug(), tag, target)?;

    // --- 5. Create the release (draft by default) ------------------------
    if repo::release_exists(&ctx.gh, tag)? {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                &format!("release {tag} already exists — not recreating")
            )
        );
    } else {
        create_release(&ctx, &artifact, tag, target)?;
    }

    // --- 6. Publish workflow, then undraft -------------------------------
    let draft = ctx.config.draft_release();

    match ctx.config.publish_workflow() {
        Some(publish) if draft => {
            run_publish(&ctx, publish, tag)?;
            eprintln!("{}", logger::action(theme, "publishing", tag));
            repo::publish_release(&ctx.gh, tag)?;
            eprintln!("{}", logger::ok(theme, "release published"));
        }
        Some(publish) => {
            // Not drafting means the release is already visible; the
            // publish workflow still runs, it just cannot beat the
            // notification.
            eprintln!(
                "{}",
                logger::warn(
                    theme,
                    "release.draft is false — watchers were notified before assets were uploaded"
                )
            );
            run_publish(&ctx, publish, tag)?;
        }
        None if draft => {
            // A draft with no publish workflow would sit invisible
            // forever, so undraft it immediately.
            eprintln!("{}", logger::action(theme, "publishing", tag));
            repo::publish_release(&ctx.gh, tag)?;
            eprintln!("{}", logger::ok(theme, "release published"));
        }
        None => {}
    }

    eprintln!();
    eprintln!("{}", logger::ok(theme, &format!("shipped {tag}")));
    Ok(())
}

/// Read the artifact back out of the Release PR body.
fn recover_artifact(pr: &PullRequest) -> Result<Artifact> {
    render::extract_artifact(&pr.body).ok_or_else(|| {
        miette::miette!(
            code = "ship::release::no_artifact",
            help = "gh-ship embeds the release artifact in the Release PR body as an HTML \
                    comment, and reads it back here. It is missing, which usually means the \
                    body was edited or the PR was not created by gh-ship. Re-run \
                    `gh ship prepare` to restore it.",
            "PR #{} does not carry a release artifact",
            pr.number
        )
    })
}

/// Make sure the Release PR is merged, merging it if asked to.
fn ensure_merged(ctx: &Context, pr: PullRequest, merge: bool) -> Result<PullRequest> {
    let theme = ctx.theme;

    if pr.is_merged() {
        return Ok(pr);
    }

    if !pr.is_open() {
        return Err(miette::miette!(
            code = "ship::release::pr_closed",
            help = "run `gh ship prepare` to start a new release",
            "PR #{} is `{}` — it was closed without merging",
            pr.number,
            pr.state
        ));
    }

    if !merge {
        return Err(miette::miette!(
            code = "ship::release::pr_open",
            help = format!(
                "merge it first, or pass `--merge` to have gh-ship merge it: {}",
                pr.url
            ),
            "PR #{} is still open",
            pr.number
        ));
    }

    eprintln!(
        "{}",
        logger::action(theme, "merging", &format!("PR #{}", pr.number))
    );
    repo::merge_pull_request(&ctx.gh, pr.number)?;
    eprintln!("{}", logger::ok(theme, "Release PR merged"));

    // Re-read the PR: the merge commit only exists after merging, and a
    // squash merge means it is a commit that did not exist before.
    let merged = repo::view_pull_request(&ctx.gh, pr.number)?;
    Ok(merged)
}

fn create_release(ctx: &Context, artifact: &Artifact, tag: &str, target: &str) -> Result<()> {
    let theme = ctx.theme;
    let draft = ctx.config.draft_release();

    eprintln!(
        "{}",
        logger::action(
            theme,
            if draft {
                "creating draft release"
            } else {
                "creating release"
            },
            tag
        )
    );

    let url = repo::create_release(
        &ctx.gh,
        tag,
        target,
        artifact.release_name().unwrap_or(tag),
        artifact.notes(),
        draft,
        artifact.is_prerelease(),
    )?;

    eprintln!("{}", logger::ok(theme, "release created"));
    if !url.is_empty() {
        eprintln!("{}", logger::detail_url(theme, "release", &url));
    }
    Ok(())
}

/// Dispatch the publish workflow and wait for it.
///
/// Dispatched on the tag rather than a branch: the publish workflow
/// should build exactly what is being released, not whatever the branch
/// has drifted to since.
fn run_publish(ctx: &Context, workflow_name: &str, tag: &str) -> Result<()> {
    let theme = ctx.theme;
    let workflow = ctx.workflow(workflow_name);
    eprintln!(
        "{}",
        logger::skip(
            theme,
            "assets are uploaded to the draft before it becomes visible"
        )
    );

    let inputs = [("tag", tag.to_string())];
    let ship_id = gh_ship::gh::run::dispatch(&ctx.gh, &workflow, tag, &inputs)?;
    eprintln!(
        "{}",
        logger::action(theme, "dispatching", &format!("{workflow} on {tag}"))
    );
    eprintln!("{}", logger::detail(theme, SHIP_ID_INPUT, ship_id.as_str()));

    let found = gh_ship::gh::run::find(
        &ctx.gh,
        &workflow,
        tag,
        &ship_id,
        gh_ship::gh::run::appear_timeout(),
        |_| {},
    )?;
    eprintln!("{}", logger::detail_url(theme, "run", &found.url));

    let mut last = String::new();
    gh_ship::gh::run::wait(
        &ctx.gh,
        &workflow,
        &found,
        gh_ship::gh::run::complete_timeout(),
        |_, current| {
            if current.status != last {
                last = current.status.clone();
                eprintln!("{}", logger::detail(theme, "status", &current.status));
            }
        },
    )?;

    eprintln!("{}", logger::ok(theme, &format!("{workflow} succeeded")));
    Ok(())
}
