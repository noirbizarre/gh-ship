# FAQ

## Why not just use GitHub Actions for everything?

You should. gh-ship dispatches your Actions workflows; it does not replace them.

What Actions does not give you is the *outside* of the loop: creating the release
branch, correlating a dispatch to its run, validating what came back, rendering and
updating a Release PR, and sequencing the release so assets land before watchers are
notified. That orchestration is awkward to express inside a workflow and pleasant to
express in a CLI.

## Why not use `workflow_call` reusable workflows?

Because they cannot be started. `on: workflow_call` workflows are callable *from
another workflow*, not from the API. `gh workflow run` against one fails.

Declare both `workflow_dispatch` and `workflow_call` and you get both properties.
The generated templates do exactly that.

## Why is `run-name` mandatory?

Because `gh workflow run` returns no run id, so gh-ship would otherwise have to
*guess* which run is yours — and any heuristic based on timestamps breaks under
concurrency.

See [Workflows](workflows.md) for the full explanation.

## Why an artifact instead of job outputs?

Job outputs cap at 1 MB in aggregate, are awkward to produce from a matrix or a
composite action, and require escaping gymnastics for multi-line values that corrupt
Markdown. Release notes hit all three problems.

An artifact is a plain file. It can be written with `jq`, checked into a fixture,
validated offline, diffed in review, and produced by a tool that has never heard of
gh-ship.

## Why doesn't gh-ship bump my version?

Because there is no version scheme it could implement that would not be wrong for
someone. Semver, CalVer, a build number, a date, a monorepo with independent
versions — your project already answers this, usually with a tool.

gh-ship treats `version` as an opaque non-empty string. It never parses it.

## Why doesn't gh-ship generate changelogs?

git-cliff, release-drafter, Changesets, semantic-release, and conventional-changelog
all do this well and disagree about how. Picking one would be picking a fight.

## Will you support monorepos?

Not in protocol v1, which describes exactly one release.

Multi-package releasing is not an incremental addition — it changes the shape of the
artifact, the PR, and the tagging model. It would be a v2 with a different document,
not a compatible extension of this one.

## Why does my Release PR have no CI results?

Because GitHub's default `GITHUB_TOKEN` cannot trigger further workflow runs. This
is a deliberate loop-prevention measure and applies to anything it authors.

Use a GitHub App token or a fine-grained PAT as `SHIP_TOKEN`. See
[Tokens](workflows.md#tokens).

## Does gh-ship store anything locally?

No. **Zero local state.**

Everything is reconstructed from GitHub. The release artifact is embedded in the
Release PR body as an HTML comment, so `gh ship release` works days later, on
another machine, run by someone else.

## What happens if my laptop dies mid-`prepare`?

Nothing is lost. The workflow keeps running on GitHub. Run `gh ship status` to see
where things stand, and `gh ship prepare` again to pick up.

## Can two people run `gh ship prepare` at once?

No — run them one at a time.

Each `prepare` sweeps abandoned `ship/prepare-*` staging branches, and the sweep
cannot tell an abandoned branch from one a concurrent run is actively using. The
second run would delete the first run's branch out from under it.

gh-ship releases one at a time by design; the Release PR is the lock. Serialise
with `concurrency: group: ship`, as the sample workflow in
[Workflows](workflows.md) does.

## Why does gh-ship create the tag and the release, rather than my workflow?

Most release tooling tags and releases from CI, so this is a fair thing to
question.

Because the release notes are generated **before** the merge and have to survive
it. Your prepare workflow runs your changelog tool against the release branch;
the publish workflow checks out the tag, which is post-merge history. If the
publish workflow regenerated the notes, what shipped could legitimately differ
from what you reviewed and approved in the Release PR.

The artifact carries the notes so that what ships is what was read. That is also
why it carries `release.name`, `release.prerelease` and `release.make_latest`:
they exist for gh-ship to create the release with.

It is not free. `gh release create <tag> <assets…>` already does
draft → upload → publish in one step, and gh-ship reimplements that sequence
because it does not hold the assets. See
[Who creates the release](architecture.md#who-creates-the-release).

## Why draft the release first?

Publishing first notifies every watcher of a release with no assets attached.
Anyone who reacts quickly downloads nothing.

A draft is invisible to watchers, yet `gh release upload` still works against it,
so assets can be attached before anyone is told. Then gh-ship undrafts it.

The tag is created explicitly, before the draft. A draft release does **not**
create its git ref — the tag appears only when the release is published — and the
publish workflow is dispatched on that tag and checks it out.

## Why tag the merge commit rather than the branch tip?

Because a squash or rebase merge creates a **new** commit. The release branch tip
gh-ship saw during `prepare` is not what lands on your base branch, so tagging it
would tag a commit that is not on the branch you released from.

gh-ship always reads `mergeCommit.oid` from the merged PR.

## Can another tool produce `ship.release.json`?

Yes, and that is the point. The [protocol](specifications/release-artifact.md) is
public, versioned, and schema-validated. gh-ship owns it; anyone may implement it.

`jq` is a sufficient implementation.

## Does `gh ship validate` need network access?

No. No network, no repository, no GitHub authentication. The schema is embedded in
the binary. This is enforced by a test.

## Can I use gh-ship without the GitHub CLI?

Only for `validate`. Everything else is GitHub orchestration, and gh-ship delegates
all GitHub access to `gh` rather than implementing a REST client and handling tokens
itself.
