<p align="center">
  <img src="docs/images/logo.svg" alt="gh-ship logo" />
</p>
<p align="center">
  Ship GitHub releases your way.
</p>
<p align="center">
  A GitHub CLI extension that orchestrates Release PRs and GitHub Actions release workflows.
</p>
<p align="center">
  <a href="https://github.com/noirbizarre/gh-ship/actions/workflows/ci.yaml">
    <img src="https://github.com/noirbizarre/gh-ship/actions/workflows/ci.yaml/badge.svg" alt="CI">
  </a>
  <a href="https://codecov.io/gh/noirbizarre/gh-ship">
    <img src="https://codecov.io/gh/noirbizarre/gh-ship/graph/badge.svg" alt="Codecov">
  </a>
  <img src="https://img.shields.io/github/v/release/noirbizarre/gh-ship" alt="Release">
  <img src="https://img.shields.io/github/license/noirbizarre/gh-ship" alt="License">
</p>

---

# gh-ship

**The GitHub Release Orchestrator.**

A [GitHub CLI](https://cli.github.com) extension that orchestrates the lifecycle of
GitHub Releases around workflows you already own.

```console
$ gh ship prepare
▶ preparing acme/widgets
▶ staging on ship/prepare-8f2c1a9e4b07 from main
▶ dispatching prepare-release on ship/prepare-8f2c1a9e4b07
  run: https://github.com/acme/widgets/actions/runs/42
▶ waiting for prepare-release
✔ prepare-release succeeded
▶ downloading ship-release
✔ artifact is valid
▶ updating release/next to a1b2c3d
▶ opening Release PR
✔ Release PR opened
  pr: https://github.com/acme/widgets/pull/7
```

---

## What it is

gh-ship orchestrates. Your workflows do the work.

| gh-ship does | your workflow does |
|---|---|
| create the release branch | bump the version |
| dispatch workflows | generate the changelog |
| wait and correlate runs | update files |
| validate the release artifact | commit and push |
| render the Release PR | |
| tag and create the GitHub Release | |

## What it is not

- **Not a workflow engine.** There is no DSL, no step registry, no `run:` key.
- **Not a replacement for GitHub Actions.** It dispatches your workflows.
- **Not a replacement for** Commitizen, git-cliff, cargo-release, semantic-release,
  Changesets, or anything else. Keep using them.
- **It never manages secrets.** Authentication is `gh`'s job.
- **It never knows how you version.** `1.4.0`, `2026.08.1`, `banana` — all fine.
- **It never generates changelogs.**

## Install

```console
$ gh extension install noirbizarre/gh-ship
```

## Quick start

```console
$ gh ship init        # under a minute
$ gh ship validate    # check the setup
$ gh ship preview     # see the Release PR, change nothing
$ gh ship prepare     # open the Release PR
# ...review, merge...
$ gh ship release     # tag, publish, release
```

## How it works

Your workflow and gh-ship communicate through exactly one thing: a JSON artifact.

```json
{
  "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
  "schemaVersion": 1,
  "changed": true,
  "version": "1.4.0",
  "tag": "v1.4.0",
  "release": {
    "notes": "## What's Changed\n\n* ..."
  }
}
```

Your workflow uploads it as `ship.release.json` in an artifact named `ship-release`.
That is the whole protocol. It is [versioned and
specified](docs/specifications/release-artifact.md), and any tool can produce it —
`jq` is enough.

Validate it before uploading, and a protocol mistake becomes a red workflow with a
precise error instead of a confusing failure later:

```console
$ gh ship validate ship.release.json
× the artifact has unknown field `tags`
   ╭─[ship.release.json:5:3]
 5 │   "tags": "v1.4.0",
   ·   ───┬──
   ·      ╰── not allowed here
  help: did you mean `tag`?
```

`gh ship validate FILE` needs **no network, no repository, and no GitHub
authentication**, so it works in any CI system.

## Configuration

A trimmed `.github/ship.yml` — `gh ship init` writes a documented one for you:

```yaml
# $schema: https://noirbizarre.github.io/gh-ship/schema/config/v1.json
version: 1

release_branch: release/next

workflows:
  prepare: prepare-release
  publish: publish-release

pull_request:
  title: "chore(release): {{ version }}"
  header: |
    This PR prepares the next release.
  footer: |
    Generated automatically by gh-ship.
  labels: [release]
```

Only `version` and `workflows.prepare` are required.

The PR title is also the release commit message: GitHub composes the squash
commit from it and appends `(#42)`, so the default lands in history as
`chore(release): 1.4.0 (#42)` — see
[The PR title is the release commit message](https://noirbizarre.github.io/gh-ship/configuration/#the-pr-title-is-the-release-commit-message).

Maintaining several release lines at once — a `1.x` branch alive while `main`
moves on — is a two-line addition; see
[Release lines](https://noirbizarre.github.io/gh-ship/configuration/#release-lines).

## Commands

| Command | What it does |
|---|---|
| `gh ship init` | Detect workflows, generate templates, write the config. |
| `gh ship validate [FILE]` | Check an artifact, or the setup and its workflows. |
| `gh ship preview` | Dry-run the prepare workflow and render the PR. Mutates nothing. |
| `gh ship prepare` | Run the prepare workflow, open or update the Release PR. |
| `gh ship status` | Where the release stands. A pure query. |
| `gh ship release` | Tag the merge commit, draft the release, publish assets, then make it visible. Requires the Release PR to be merged, or pass `--merge`. |

## Two things worth knowing

### Your workflows must be dispatchable

gh-ship starts workflows through the API, which can only start workflows declaring
`on: workflow_dispatch`. A `workflow_call`-only workflow — what people usually mean
by "reusable" — cannot be started this way. Declare both to have it both ways.

That is the only structural requirement. gh-ship finds the run it started from
the ref it dispatched to and the run ids that were not there a moment before, so
your workflows need no `run-name` convention and no correlation input.

Your prepare workflow must also declare a `dry_run` boolean input — it is what
`gh ship preview` sets to produce the artifact without committing anything.

`gh ship validate` checks all three.

### The token

GitHub's default `GITHUB_TOKEN` **cannot trigger other workflows**. A Release PR it
authors will not run your CI. If the Release PR must be tested before merging,
supply a GitHub App token or a fine-grained PAT. `gh ship init` asks which you
want and generates the matching workflow: an App mints a scoped, self-revoking
token in the job with
[`actions/create-github-app-token`](https://noirbizarre.github.io/gh-ship/workflows/#using-a-github-app),
a PAT lives in the `SHIP_TOKEN` secret.

## Design notes

**Zero local state.** Everything is reconstructed from GitHub. The release artifact
is embedded in the Release PR body as an HTML comment, so `gh ship release` works
days later, on another machine, run by someone else.

**Draft-first releases.** gh-ship creates the release as a draft, lets your publish
workflow attach assets to it, and only then makes it visible. Publishing first would
notify every watcher of an empty release.

**The merge commit, not the branch tip.** A squash merge creates a new commit, so
gh-ship always reads `mergeCommit.oid` rather than trusting a SHA it saw earlier.

**`changed: false` is a success.** A scheduled release job that finds nothing to
ship exits 0.

## Documentation

<https://noirbizarre.github.io/gh-ship>

## License

MIT
