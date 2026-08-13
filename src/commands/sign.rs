//! `gh ship sign` — re-create a commit so GitHub signs it.
//!
//! A commit made on a runner is never signed. Git signs with a key,
//! there is no key on a runner, and the token only authenticates the
//! push — so `git commit && git push` produces an unsigned commit under
//! a GitHub App token exactly as it does under `GITHUB_TOKEN`.
//!
//! GitHub *will* sign a commit it creates itself on behalf of a bot, but
//! only when the request carries no identity of its own. Measured, four
//! routes to the same tree:
//!
//! | How the commit is made                          | Verified |
//! |-------------------------------------------------|----------|
//! | `git commit` + `git push`                       | no       |
//! | `POST /git/commits` *with* author or committer  | no       |
//! | `POST /git/commits` without them                | **yes**  |
//! | GraphQL `createCommitOnBranch`                  | **yes**  |
//!
//! Two consequences shape this command.
//!
//! Signing and choosing the author are mutually exclusive: supplying an
//! identity is precisely what suppresses the signature, so the author
//! becomes whoever the token belongs to. For a prepare workflow that is
//! the App or `github-actions[bot]` — the identity the workflow was
//! setting by hand anyway.
//!
//! And it only works for a **bot**. The same call under a human's token
//! comes back unsigned. That is why this is a command the workflow runs,
//! and not something `prepare` does while promoting: `prepare` is
//! supported from a laptop, and there it could not sign.

use miette::{Diagnostic, Result};
use thiserror::Error;

use gh_ship::cli::{Cli, SignArgs};
use gh_ship::detect;
use gh_ship::gh::cli::Gh;
use gh_ship::gh::repo;
use gh_ship::logger;
use gh_ship::style::Theme;

use super::{repo_root, short_sha};

/// Why signing could not proceed.
#[derive(Debug, Error, Diagnostic)]
pub enum SignError {
    #[error("cannot tell which branch to sign")]
    #[diagnostic(
        code(ship::sign::no_ref),
        help(
            "pass it explicitly — `gh ship sign <branch>`. In a workflow this is read from \
             `GITHUB_REF`, which names no branch on a `pull_request` event or a tag; outside \
             one it comes from the checkout, which a detached HEAD does not provide."
        )
    )]
    NoRef,

    #[error("GitHub returned the commit unsigned")]
    #[diagnostic(
        code(ship::sign::not_signed),
        help(
            "GitHub only signs commits it creates for a bot: the default `GITHUB_TOKEN`, or a \
             GitHub App installation token. A personal access token or a user login cannot \
             produce one, whatever its permissions. `{branch}` was left untouched.\n\nTo sign as \
             yourself instead, give the runner a GPG or SSH key and set `commit.gpgsign`."
        )
    )]
    NotSigned { branch: String },
}

pub fn run(cli: &Cli, args: &SignArgs, theme: Theme) -> Result<()> {
    let root = repo_root(&cli.config);
    let branch =
        detect::checked_out_branch(args.reference.as_deref(), &root).ok_or(SignError::NoRef)?;

    // `gh api` cannot take `--repo`, so the repository has to be resolved
    // into the URL path before anything else.
    let gh = Gh::new(cli.repo.clone());
    let repository = repo::repository(&gh)?;
    let gh = gh.scoped_to(&repository.name_with_owner);

    let head = repo::commit_at(&gh, &branch)?;

    // Someone who signs with their own key has already solved this, and
    // re-creating their commit would replace a signature they chose with
    // one they did not. Doing nothing is the correct answer.
    if head.verified {
        eprintln!(
            "{}",
            logger::skip(
                theme,
                &format!("{} on {branch} is already signed", short_sha(&head.sha))
            )
        );
        return Ok(());
    }

    eprintln!(
        "{}",
        logger::action(
            theme,
            "signing",
            &format!("{branch} at {}", short_sha(&head.sha))
        )
    );

    // Same tree, same parents, same message: only the signature and the
    // identity differ. Earlier commits keep their shas, so a branch whose
    // release is one commit — which is what the templates produce — comes
    // out identical apart from being signed.
    let signed = repo::create_signed_commit(&gh, &head.message, &head.tree, &head.parents)?;

    // The new commit is unreferenced at this point, so failing here costs
    // nothing but the object. Moving the branch onto an unsigned rewrite
    // would be worse than not trying: it changes the author for no gain.
    if !signed.verified {
        return Err(SignError::NotSigned { branch }.into());
    }

    repo::reset_branch(&gh, &branch, &signed.sha)?;

    eprintln!(
        "{}",
        logger::ok(theme, &format!("signed as {}", short_sha(&signed.sha)))
    );
    Ok(())
}
