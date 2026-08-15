//! `gh ship status` — where does the current release stand?
//!
//! This is a **pure query**. It mutates nothing, dispatches nothing, and
//! waits for nothing.
//!
//! It is also the payoff of keeping zero local state: everything shown
//! here is reconstructed from GitHub — does the release branch exist, is
//! there a Release PR, what did the last prepare run do, what artifact
//! is embedded in the PR, does the release already exist. So `status`
//! gives the same answer on a laptop that has never seen this release as
//! it does on the machine that started it.

use miette::Result;
use serde::Serialize;

use gh_ship::artifact::Artifact;
use gh_ship::cli::{Cli, StatusArgs};
use gh_ship::gh::{repo, run};
use gh_ship::logger;
use gh_ship::render;
use gh_ship::style::Theme;

use super::context::Context;
use super::short_sha;

/// The reconstructed state of a release.
#[derive(Debug, Serialize)]
pub struct Status {
    pub repository: String,
    pub base_branch: String,
    /// How the base branch was arrived at — the first thing to look at
    /// when a release line turns out to be the wrong one.
    pub base_branch_origin: gh_ship::detect::Origin,
    /// The `branches` entry that matched, when release lines are configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_rule: Option<String>,
    /// Whether that entry carried its own `release_branch`.
    ///
    /// Not serialised: `release_branch` above already reports the
    /// resolved name, which is what a machine reader wants. This only
    /// exists so the human report can say where that name came from.
    #[serde(skip)]
    pub branch_rule_override: bool,
    pub release_branch: String,
    pub release_branch_exists: bool,
    pub pull_request: Option<PullRequestStatus>,
    pub artifact: Option<Artifact>,
    pub last_run: Option<RunStatus>,
    pub release_exists: bool,
    /// What to do next, in plain language.
    pub next: String,
}

#[derive(Debug, Serialize)]
pub struct PullRequestStatus {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: String,
    pub merged_sha: Option<String>,
}

impl PullRequestStatus {
    fn is_merged(&self) -> bool {
        repo::state::is_merged(&self.state)
    }

    fn is_closed(&self) -> bool {
        repo::state::is_closed(&self.state)
    }
}

#[derive(Debug, Serialize)]
pub struct RunStatus {
    pub id: u64,
    pub title: String,
    pub status: String,
    pub conclusion: String,
    pub url: String,
}

pub fn run(cli: &Cli, args: &StatusArgs, theme: Theme) -> Result<()> {
    let ctx = Context::load(cli, args.base.base.as_deref(), theme)?;
    let status = collect(&ctx)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).expect("status output is always serialisable")
        );
        return Ok(());
    }

    report(&status, theme);
    Ok(())
}

fn collect(ctx: &Context) -> Result<Status> {
    let release_branch = ctx.release_branch().to_string();
    let base_branch = ctx.base_branch().to_string();

    let branch_exists = repo::branch_exists(&ctx.gh, &release_branch)?;

    let pr = repo::find_pull_request(&ctx.gh, &release_branch, &base_branch)?;

    // The artifact lives in the PR body, which is the whole reason
    // gh-ship needs no local state.
    let artifact = pr.as_ref().and_then(|p| render::extract_artifact(&p.body));

    // The most recent prepare run on the release branch, whether or not
    // gh-ship started it.
    let last_run = if branch_exists {
        run::list(
            &ctx.gh,
            &ctx.workflow(ctx.config.prepare_workflow()),
            &release_branch,
        )
        .ok()
        .and_then(|runs| runs.into_iter().next())
        .map(|r| RunStatus {
            id: r.id,
            title: r.title,
            status: r.status,
            conclusion: r.conclusion,
            url: r.url,
        })
    } else {
        None
    };

    let release_exists = match artifact.as_ref().and_then(|a| a.tag()) {
        Some(tag) => repo::release_exists(&ctx.gh, tag).unwrap_or(false),
        None => false,
    };

    let pull_request = pr.as_ref().map(|p| PullRequestStatus {
        number: p.number,
        url: p.url.clone(),
        title: p.title.clone(),
        state: p.state.clone(),
        merged_sha: p.merged_sha().map(str::to_string),
    });

    let next = next_step(
        branch_exists,
        pull_request.as_ref(),
        artifact.as_ref(),
        release_exists,
    );

    Ok(Status {
        repository: ctx.repo_slug().to_string(),
        base_branch,
        base_branch_origin: ctx.base_origin(),
        branch_rule: ctx
            .line()
            .entry
            .and_then(|i| ctx.config.branches().get(i))
            .map(|rule| rule.branch.clone()),
        branch_rule_override: ctx
            .line()
            .entry
            .and_then(|i| ctx.config.branches().get(i))
            .is_some_and(|rule| rule.release_branch.is_some()),
        release_branch,
        release_branch_exists: branch_exists,
        pull_request,
        artifact,
        last_run,
        release_exists,
        next,
    })
}

/// Work out the single most useful next action.
///
/// Ordered from "nothing started" to "all done", because the first
/// matching case is always the right advice.
fn next_step(
    branch_exists: bool,
    pr: Option<&PullRequestStatus>,
    artifact: Option<&Artifact>,
    release_exists: bool,
) -> String {
    if release_exists {
        return "the release exists — nothing to do".into();
    }

    let Some(pr) = pr else {
        return if branch_exists {
            "run `gh ship prepare` to produce the Release PR".into()
        } else {
            "run `gh ship prepare` to start a release".into()
        };
    };

    if pr.is_merged() {
        return match artifact {
            Some(_) => "run `gh ship release` to tag and publish".into(),
            // Without the embedded artifact there is nothing to release
            // from, so point at the recovery path rather than failing
            // cryptically later.
            None => "the Release PR was merged but carries no artifact — \
                     run `gh ship prepare` again on a fresh branch"
                .into(),
        };
    }

    if pr.is_closed() {
        return "the Release PR was closed — run `gh ship prepare` to start again".into();
    }

    "review and merge the Release PR, then run `gh ship release`".into()
}

fn report(status: &Status, theme: Theme) {
    eprintln!("{}", logger::action(theme, "status of", &status.repository));
    eprintln!();

    eprintln!(
        "{}",
        logger::detail(
            theme,
            "base branch",
            &format!(
                "{} ({})",
                status.base_branch,
                status.base_branch_origin.describe()
            )
        )
    );

    if let Some(rule) = &status.branch_rule {
        let line = if status.branch_rule_override {
            format!("{rule} (override)")
        } else {
            rule.clone()
        };
        eprintln!("{}", logger::detail(theme, "release line", &line));
    }

    let branch = if status.release_branch_exists {
        status.release_branch.clone()
    } else {
        format!("{} (does not exist)", status.release_branch)
    };
    eprintln!("{}", logger::detail(theme, "release branch", &branch));

    match &status.pull_request {
        Some(pr) => {
            eprintln!(
                "{}",
                logger::detail(
                    theme,
                    "release pr",
                    &format!("#{} {} [{}]", pr.number, pr.title, pr.state.to_lowercase())
                )
            );
            eprintln!("{}", logger::detail_url(theme, "pr", &pr.url));
            if let Some(sha) = &pr.merged_sha {
                eprintln!("{}", logger::detail(theme, "merged as", short_sha(sha)));
            }
        }
        None => eprintln!("{}", logger::detail(theme, "release pr", "none")),
    }

    match &status.artifact {
        Some(a) if a.changed => {
            eprintln!("{}", logger::release_identity(theme, a.version(), a.tag()));
        }
        Some(_) => eprintln!(
            "{}",
            logger::detail(theme, "artifact", "nothing to release")
        ),
        None => eprintln!("{}", logger::detail(theme, "artifact", "none")),
    }

    if let Some(r) = &status.last_run {
        let state = if r.conclusion.is_empty() {
            r.status.clone()
        } else {
            format!("{} ({})", r.status, r.conclusion)
        };
        eprintln!("{}", logger::detail(theme, "last run", &state));
        eprintln!("{}", logger::detail_url(theme, "run", &r.url));
    }

    if status.release_exists {
        eprintln!("{}", logger::detail(theme, "release", "published"));
    }

    eprintln!();
    eprintln!("{}", logger::skip(theme, &format!("next: {}", status.next)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str) -> PullRequestStatus {
        PullRequestStatus {
            number: 1,
            url: "u".into(),
            title: "t".into(),
            state: state.into(),
            merged_sha: None,
        }
    }

    fn artifact() -> Artifact {
        Artifact {
            schema_version: 1,
            changed: true,
            version: Some("1.0.0".into()),
            tag: Some("v1.0.0".into()),
            release: None,
            pull_request: None,
        }
    }

    #[test]
    fn suggests_prepare_when_nothing_exists() {
        assert!(next_step(false, None, None, false).contains("gh ship prepare"));
    }

    #[test]
    fn suggests_prepare_when_the_branch_exists_but_no_pr_does() {
        let s = next_step(true, None, None, false);
        assert!(s.contains("gh ship prepare"), "{s}");
    }

    #[test]
    fn suggests_merging_an_open_pr() {
        let s = next_step(true, Some(&pr("OPEN")), Some(&artifact()), false);
        assert!(s.contains("merge"), "{s}");
        assert!(s.contains("gh ship release"), "{s}");
    }

    #[test]
    fn suggests_releasing_a_merged_pr() {
        let s = next_step(true, Some(&pr("MERGED")), Some(&artifact()), false);
        assert!(s.contains("gh ship release"), "{s}");
    }

    /// A merged PR whose body lost the artifact cannot be released.
    /// Saying so beats failing later with a confusing message.
    #[test]
    fn flags_a_merged_pr_without_an_artifact() {
        let s = next_step(true, Some(&pr("MERGED")), None, false);
        assert!(s.contains("no artifact"), "{s}");
    }

    #[test]
    fn recognises_a_closed_pr() {
        let s = next_step(true, Some(&pr("CLOSED")), None, false);
        assert!(s.contains("closed"), "{s}");
        assert!(s.contains("start again"), "{s}");
    }

    #[test]
    fn reports_completion_once_the_release_exists() {
        let s = next_step(true, Some(&pr("MERGED")), Some(&artifact()), true);
        assert!(s.contains("nothing to do"), "{s}");
    }
}
