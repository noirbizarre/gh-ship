# CLI Reference

```
gh ship <COMMAND>
```

## Global options

| Option | Environment | Meaning |
|---|---|---|
| `-c`, `--config <PATH>` | `SHIP_CONFIG` | Config path. Default `.github/ship.yml`. |
| `-R`, `--repo <OWNER/REPO>` | `SHIP_REPO` | Target repository. Defaults to the current one. |
| `-v`, `--verbose` | | More detail. Repeat for more. |

## Waiting

`preview`, `prepare` and `release` all block on a workflow run they dispatched.
Two environment variables cap how long they wait:

| Variable | Default | Meaning |
|---|---|---|
| `SHIP_APPEAR_TIMEOUT` | `90` | Seconds to wait for a dispatched run to *appear*. |
| `SHIP_RUN_TIMEOUT` | `3600` | Seconds to wait for a run to *finish*. |

Both take a whole number of seconds. Raise `SHIP_RUN_TIMEOUT` if your publish
workflow legitimately takes more than an hour; the appear timeout only covers
the gap between dispatching and GitHub queueing the run, so it rarely needs
touching.

When a command runs inside a workflow of your own, set `timeout-minutes` on the
job *below* these values, so a stuck run fails the job visibly rather than
sitting until GitHub's own six-hour limit.

## Transient failures

GitHub occasionally answers a valid query with a 502 or 504 — most often on the
GraphQL endpoint behind the `--json` flags — and is fine again seconds later.
gh-ship retries those rather than failing the release:

| Variable | Default | Meaning |
|---|---|---|
| `SHIP_GH_RETRIES` | `3` | Extra attempts for a read-only `gh` call that fails transiently. `0` disables. |
| `SHIP_GH_RETRY_DELAY` | `1` | Seconds before the first retry. Doubles per attempt, capped at 8. |

Only **read-only** calls are retried — `list`, `view`, `status`, `download` and
`GET` requests to `gh api`. A gateway timeout means GitHub stopped *answering*,
not that it stopped *acting*: a timed-out `pr create` may well have created the
pull request, so retrying it would open a second one. Writes still fail on the
first error, and are safe to re-run by hand because every gh-ship command is
idempotent.

Rate limits are not retried either. They clear in minutes rather than seconds,
so the error tells you to wait instead of spending the budget for nothing.

## Colour

gh-ship colours its output when it believes something will render it, following
the same variables as `gh` itself — there is no `--color` flag.

| Variable | Effect |
|---|---|
| `NO_COLOR` | Set to anything non-empty, colour is off. Wins over everything else. |
| `CLICOLOR=0` | Colour off. |
| `CLICOLOR_FORCE` | Set to anything but `0`, colour on even without a terminal. |
| `TERM=dumb` | Colour off, unless `CLICOLOR_FORCE` overrides it. |

Otherwise gh-ship colours its output when stderr is a terminal, **or when
`GITHUB_ACTIONS=true`**. Actions gives every process a pipe rather than a
terminal but renders ANSI in its logs, so workflow output is coloured without
any configuration.

Only the human-readable output on stderr is ever coloured. `--json` goes to
stdout and never is, so it stays safe to pipe.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. **Including `changed: false`.** |
| `1` | Failure. |

!!! note "`nothing to release` is not a failure"

    A scheduled release job that finds nothing to ship exits `0`. Treating it as a
    failure would make every quiet week look broken.

---

## `gh ship init`

Make a repository gh-ship enabled.

```console
$ gh ship init [--force]
```

Detects your repository, lists workflows gh-ship can dispatch, explains the ones it
cannot, offers to generate templates, and writes a documented `.github/ship.yml`.

| Option | Meaning |
|---|---|
| `--force` | Overwrite an existing configuration. |

Requires an interactive terminal. In automation, write the config directly.

---

## `gh ship validate`

Check a release artifact, or the setup.

```console
$ gh ship validate [FILE]
```

**With a file** — validates it against the
[artifact schema](specifications/release-artifact.md). Requires **no network, no
repository, and no GitHub authentication**.

**Without a file** — validates `.github/ship.yml` and checks that the workflows it
names satisfy the [contract](workflows.md).

```console
$ gh ship validate ship.release.json
✔ ship.release.json is a valid release artifact
  version: 1.4.0
  tag: v1.4.0
```

---

## `gh ship preview`

Dry-run the prepare workflow and render the Release PR.

```console
$ gh ship preview [--json]
```

Dispatches the prepare workflow with `dry_run: true`, waits, downloads the artifact,
and renders the PR to stdout.

**Nothing on GitHub is modified**: no branch, no PR, no tag, no release.

| Option | Meaning |
|---|---|
| `--json` | Emit the artifact and rendered PR as JSON. |

---

## `gh ship prepare`

Run the prepare workflow and open or update the Release PR.

```console
$ gh ship prepare [--no-wait]
```

1. Stops if a merged Release PR is still awaiting `gh ship release`.
2. Sweeps staging branches left behind by earlier runs.
3. Cuts a throwaway staging branch, `ship/prepare-<nonce>`, from the base branch
   (`workflow_dispatch` needs the ref to exist).
4. Dispatches the prepare workflow **on that staging branch** and waits.
5. Downloads and validates the artifact.
6. Stops with exit 0 if `changed: false`.
7. Moves the release branch onto the staged release commit, then sweeps again.
8. Opens or updates the Release PR, embedding the artifact in its body.

Re-running is safe, and is the supported way to refresh a Release PR: the
release branch moves onto the new commit and the existing PR is updated in
place.

!!! warning "Run one at a time"

    Step 2 deletes every `ship/prepare-*` branch, including one a concurrent
    run is using. Serialise with `concurrency: group: ship` — the sample
    workflow in [Workflows](workflows.md) does.

!!! note "Why a staging branch"

    The work used to happen on the release branch, reset to base first. That
    left it momentarily identical to its base, and GitHub closes a pull request
    whose head becomes contained in its base — so the Release PR was closed and
    reopened on every prepare. Staging elsewhere and promoting afterwards means
    the release branch is never equal to the base.

| Option | Meaning |
|---|---|
| `--no-wait` | Dispatch and return. The PR is not created; run `prepare` again later. |

!!! note "It refuses while a release is pending"

    If the Release PR has been **merged** but `gh ship release` has not run yet,
    `prepare` stops and says so, exiting `0`.

    In that window the tag does not exist, so your changelog tool still reports the
    same version as unreleased — preparing again would start a second release for a
    version you already merged. Run `gh ship release` to finish, and the next
    `prepare` proceeds normally.

    Exiting `0` is deliberate: this is an expected state, and a workflow that runs
    `prepare` on every push should not go red until someone ships.

!!! note "It also skips right after a release"

    Merging the Release PR is itself a push to your base branch, so a
    push-triggered `prepare` runs on it. If the release is already published and
    its merge commit is **still the tip** of the base branch, there is nothing on
    top of it to release: `prepare` says so and exits `0` without dispatching the
    prepare workflow.

    The check compares the merge commit recorded on the pull request against the
    branch tip, so it holds whether you merge, squash or rebase. The next commit
    to land makes `prepare` proceed as usual.

---

## `gh ship status`

Show where the current release stands.

```console
$ gh ship status [--json]
```

A **pure query**. It dispatches nothing, waits for nothing, and changes nothing.

Everything is reconstructed from GitHub — branch, PR, last run, the artifact
embedded in the PR body — so the answer is the same on any machine.

| Option | Meaning |
|---|---|
| `--json` | Emit machine-readable JSON. |

---

## `gh ship release`

Tag, publish, and release the merged Release PR.

```console
$ gh ship release [--merge]
```

1. Finds the Release PR and recovers the artifact from its body.
2. Requires the PR to be merged (or merges it with `--merge`).
3. Tags the **merge commit** — never a remembered SHA, because a squash merge
   creates a new one.
4. Creates the GitHub Release as a **draft**.
5. Dispatches the publish workflow so it can attach assets.
6. Makes the release visible.

!!! note "The tag is created before the release, deliberately"

    A draft release does not create its git ref — the tag only appears when the
    release is published. Since the publish workflow is dispatched on that tag
    and checks it out, gh-ship creates `refs/tags/<tag>` itself first.

    The step is idempotent, so re-running after a failure part-way through is
    safe.

| Option | Meaning |
|---|---|
| `--merge` | Merge the Release PR if it is still open. |

Idempotent: if the release already exists, it is not recreated.
