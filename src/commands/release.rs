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

use miette::{Diagnostic, Result};
use thiserror::Error;

use gh_ship::artifact::Artifact;
use gh_ship::cli::{Cli, ReleaseArgs};
use gh_ship::gh::repo::{self, PullRequest};
use gh_ship::gh::run::{self, ShipId};
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::{Context, dispatch_and_wait, report_nothing_to_release, wait_for_run};
use super::short_sha;

/// Everything that can stop a release before it starts.
///
/// These are enumerated rather than raised ad hoc so the whole
/// `ship::release::*` code namespace is visible in one place.
#[derive(Debug, Error, Diagnostic)]
pub enum ReleaseError {
    #[error("no Release PR found for `{release_branch}` → `{base_branch}`")]
    #[diagnostic(
        code(ship::release::no_pr),
        help("run `gh ship prepare` first, or check `gh ship status`")
    )]
    NoPullRequest {
        release_branch: String,
        base_branch: String,
    },

    #[error("PR #{number} does not carry a release artifact")]
    #[diagnostic(
        code(ship::release::no_artifact),
        help(
            "gh-ship embeds the release artifact in the Release PR body as an HTML comment, and \
             reads it back here. It is missing, which usually means the body was edited or the PR \
             was not created by gh-ship. Re-run `gh ship prepare` to restore it."
        )
    )]
    NoArtifact { number: u64 },

    #[error("PR #{number} carries a release artifact with no tag")]
    #[diagnostic(
        code(ship::release::no_tag),
        help(
            "the artifact embedded in the Release PR body reports a change but names no tag, \
             which usually means the body was edited. Re-run `gh ship prepare` to restore it."
        )
    )]
    NoTag { number: u64 },

    #[error("could not determine the commit PR #{number} merged as")]
    #[diagnostic(
        code(ship::release::no_merge_commit),
        help(
            "GitHub reported the PR as merged but returned no merge commit; retry in a moment, \
             or check `gh ship status`"
        )
    )]
    NoMergeCommit { number: u64 },

    #[error("PR #{number} is `{state}` — it was closed without merging")]
    #[diagnostic(
        code(ship::release::pr_closed),
        help("run `gh ship prepare` to start a new release")
    )]
    PullRequestClosed { number: u64, state: String },

    #[error("PR #{number} is still open")]
    #[diagnostic(
        code(ship::release::pr_open),
        help("merge it first, or pass `--merge` to have gh-ship merge it: {url}")
    )]
    PullRequestOpen { number: u64, url: String },
}

pub fn run(cli: &Cli, args: &ReleaseArgs, theme: Theme) -> Result<()> {
    let ctx = Context::load(cli, args.base.base.as_deref(), theme)?;

    eprintln!("{}", logger::action(theme, "releasing", ctx.repo_slug()));

    let release_branch = ctx.release_branch().to_string();
    let base_branch = ctx.base_branch().to_string();

    // --- 1. Find the Release PR -----------------------------------------
    let pr = repo::find_pull_request(&ctx.gh, &release_branch, &base_branch)?.ok_or_else(|| {
        ReleaseError::NoPullRequest {
            release_branch: release_branch.clone(),
            base_branch: base_branch.clone(),
        }
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

    // The schema requires a tag when `changed` is true, but the artifact is
    // recovered from a PR body that a human can edit, so this is a diagnostic
    // rather than an assertion.
    let tag = artifact
        .tag()
        .ok_or(ReleaseError::NoTag { number: pr.number })?;
    let version = artifact.version().unwrap_or(tag);

    eprintln!("{}", logger::detail(theme, "version", version));
    eprintln!("{}", logger::detail(theme, "tag", tag));

    // --- 3. Ensure the PR is merged -------------------------------------
    let pr = ensure_merged(&ctx, pr, args.merge)?;

    let target = pr
        .merged_sha()
        .ok_or(ReleaseError::NoMergeCommit { number: pr.number })?;
    eprintln!("{}", logger::detail(theme, "merged as", short_sha(target)));

    // --- 4. Tag the merge commit -----------------------------------------
    //
    // Before the release, not as a side effect of it: a draft release does not
    // create the git ref, and the publish workflow is dispatched on this tag
    // and checks it out.
    eprintln!("{}", logger::action(theme, "tagging", tag));
    repo::create_tag(&ctx.gh, tag, target)?;

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
    let draft = ctx.config.release().draft;

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
    render::extract_artifact(&pr.body)
        .ok_or_else(|| ReleaseError::NoArtifact { number: pr.number }.into())
}

/// Make sure the Release PR is merged, merging it if asked to.
fn ensure_merged(ctx: &Context, pr: PullRequest, merge: bool) -> Result<PullRequest> {
    let theme = ctx.theme;

    if pr.is_merged() {
        return Ok(pr);
    }

    if !pr.is_open() {
        return Err(ReleaseError::PullRequestClosed {
            number: pr.number,
            state: pr.state.clone(),
        }
        .into());
    }

    if !merge {
        return Err(ReleaseError::PullRequestOpen {
            number: pr.number,
            url: pr.url.clone(),
        }
        .into());
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
    let draft = ctx.config.release().draft;

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
        &repo::NewRelease {
            tag,
            target,
            name: artifact.release_name().unwrap_or(tag),
            notes: artifact.notes(),
            draft,
            prerelease: artifact.is_prerelease(),
            make_latest: artifact.make_latest(),
        },
    )?;

    eprintln!("{}", logger::ok(theme, "release created"));
    if !url.is_empty() {
        eprintln!("{}", logger::detail_url(theme, "release", &url));
    }
    Ok(())
}

/// Dispatch the publish workflow and wait for it — unless one already ran.
///
/// Dispatched on the tag rather than a branch: the publish workflow
/// should build exactly what is being released, not whatever the branch
/// has drifted to since.
///
/// # Why this looks before it dispatches
///
/// Correlation is by nonce, and the nonce is generated per invocation and
/// never persisted. A publish run that failed and was then re-run from the
/// GitHub UI keeps its original `run-name`, and so its original nonce: a
/// re-run of the calling job would match nothing, dispatch a second full
/// build, and re-upload assets that are already there.
///
/// The tag ref makes that unnecessary. It is unique to this release, so
/// every run of the publish workflow on it belongs to this release — no
/// matter who started it or which nonce it carries.
fn run_publish(ctx: &Context, workflow_name: &str, tag: &str) -> Result<()> {
    let theme = ctx.theme;
    let workflow = ctx.workflow(workflow_name);

    let existing = run::list(&ctx.gh, &workflow, tag)?;

    // A success anywhere in the history wins, including the second attempt
    // of a re-run: `gh run view` reports the latest attempt, so there is
    // nothing left to build.
    if let Some(done) = existing.iter().find(|r| r.succeeded()) {
        eprintln!(
            "{}",
            logger::skip(theme, &format!("{workflow} already succeeded for {tag}"))
        );
        eprintln!("{}", logger::detail_url(theme, "run", &done.url));
        return Ok(());
    }

    eprintln!(
        "{}",
        logger::skip(
            theme,
            "assets are uploaded to the draft before it becomes visible"
        )
    );

    if let Some(running) = existing.iter().find(|r| !r.is_finished()) {
        eprintln!(
            "{}",
            logger::skip(theme, &format!("{workflow} is already running for {tag}"))
        );
        eprintln!("{}", logger::detail_url(theme, "run", &running.url));
        wait_for_run(ctx, &workflow, running)?;
        return Ok(());
    }

    // The shared dispatch/find/wait helper does the reporting: a publish
    // that cross-compiles for an hour must look alive throughout, and it
    // must look the same as every other wait gh-ship does.
    let inputs = [("tag", tag.to_string())];
    dispatch_and_wait(ctx, &workflow, tag, &ShipId::generate(), &inputs)?;
    Ok(())
}
