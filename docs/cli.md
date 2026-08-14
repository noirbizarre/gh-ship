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

## Choosing the release line

`preview`, `prepare`, `release` and `status` accept:

| Option | Environment | Meaning |
|---|---|---|
| `--base <BRANCH>` | `SHIP_BASE_BRANCH` | Base branch to release from. |

With [release lines](configuration.md#release-lines) configured, this selects
the line. Without them it simply overrides the branch the Release PR targets.

gh-ship works the branch out for itself, in order:

1. `--base <BRANCH>`, or `SHIP_BASE_BRANCH`.
2. The GitHub Actions environment — the branch a `pull_request` targets,
   otherwise the branch of the run. A run on a tag names no branch and is not
   guessed at.
3. The local checkout's current branch, read from `.git/HEAD`. No `git`
   binary is needed and nothing is fetched or cloned.
4. The repository's default branch.

Detection only applies when `branches` is configured; otherwise the repository
default branch is used, exactly as before. `gh ship status` always reports which
source it used.

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

Only **read-only** calls are retried — `list`, `view`, `status`, `download`,
`diff`, `checks` and `GET` requests to `gh api`. A gateway timeout means GitHub
stopped *answering*,
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

Only the human-readable output on stderr is ever coloured. Stdout carries the
machine-readable payloads — `--json`, and the rendered PR body from
`gh ship preview` — and is never coloured, so it stays safe to pipe.

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

Scans `.github/workflows/`, lists the workflows gh-ship can dispatch, explains the
ones it cannot, offers to generate templates, and writes a documented
`.github/ship.yml`.

| Option | Meaning |
|---|---|
| `--force` | Overwrite an existing configuration. |

Requires an interactive terminal. In automation, write the config directly.

`init` is entirely local: it never contacts GitHub, so it needs no authentication
and ignores `--repo`.

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
names satisfy the [contract](workflows.md). Requires **no network and no GitHub
authentication** either.

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
and renders the PR: the body on stdout, the title, labels and rules on stderr. So
`gh ship preview > body.md` captures exactly the PR body.

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
3. Cuts a throwaway staging branch, `ship/prepare-<token>`, from the base branch
   (`workflow_dispatch` needs the ref to exist). With
   [release lines](configuration.md#release-lines) configured the name carries
   the line — `ship/prepare-<base>-<token>`, so `release/1.x` stages on
   `ship/prepare-release-1.x-<token>`.
4. Dispatches the prepare workflow **on that staging branch** and waits.
5. Downloads and validates the artifact.
6. Stops with exit 0 if `changed: false`.
7. Moves the release branch onto the staged release commit, then sweeps again.
8. Opens or updates the Release PR, embedding the artifact in its body.

Re-running is safe, and is the supported way to refresh a Release PR: the
release branch moves onto the new commit and the existing PR is updated in
place.

!!! warning "Run one at a time per release line"

    Step 2 deletes staging branches, including one a concurrent run is using,
    so two prepares on the same line must not overlap.

    Without `branches`, the sweep covers every `ship/prepare-*` branch:
    serialise everything with `concurrency: group: ship`, as the sample
    workflow in [Workflows](workflows.md) does.

    With `branches` configured the sweep is scoped to the line's own prefix, so
    different lines never collide and may prepare concurrently. Key the group
    by branch instead — `group: ship-${{ github.ref }}`.

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
5. Dispatches the publish workflow so it can attach assets — unless a run of it
   already succeeded for the tag, in which case it is skipped, or one is still in
   flight, in which case that run is adopted and waited on.
6. Makes the release visible.

!!! note "The tag is created before the release, deliberately"

    A draft release does not create its git ref — the tag only appears when the
    release is published. Since the publish workflow is dispatched on that tag
    and checks it out, gh-ship creates `refs/tags/<tag>` itself first.

    The step is idempotent, so re-running after a failure part-way through is
    safe.

!!! tip "Re-running is cheap"

    Before dispatching, gh-ship lists the runs of the publish workflow on the
    tag. The tag is unique to the release, so any run on it belongs to this
    release — whoever started it.

    - A run that **succeeded** means the assets are up: gh-ship skips the
      dispatch and only makes the release visible.
    - A run still **in flight** is adopted and waited on, rather than raced.
    - Only when every run **failed** — or none exists — is a new one dispatched.

    So if a publish run fails and you re-run it from the GitHub UI, re-running
    the job that calls `gh ship release` finishes the release instead of
    rebuilding it.

| Option | Meaning |
|---|---|
| `--merge` | Merge the Release PR if it is still open. |

Idempotent: if the release already exists, it is not recreated.

---

## `gh ship sign`

Re-create a commit so GitHub signs it.

```console
$ gh ship sign [BRANCH]
```

Meant for the **prepare workflow**, not for you: it re-creates the tip commit of
`BRANCH` through the API, with the same tree, parents and message but no identity
of its own, and moves the branch onto the result. GitHub signs a commit it creates
for a bot, so the commit comes out **Verified** where `git commit` cannot.

`BRANCH` defaults to the branch the workflow was dispatched on — the staging
branch — read from `GITHUB_REF`, or from the checkout outside CI.

```console
$ gh ship sign
▸ signing ship/prepare-8f2c1a9e4b07 at c0ffee0
✔ signed as 51c8ed0
```

Two behaviours worth knowing:

- A commit that is **already signed** is left alone and the command succeeds.
  Re-creating it would replace a signature its author chose with one they did not.
- When GitHub returns the re-created commit **unsigned**, the command fails and
  the branch is **not moved**. That happens when the token is not a bot — a PAT
  or a user login cannot produce a signature, whatever its permissions — and
  moving the branch anyway would change the commit's author for no benefit.

Needs `contents: write`. See [Getting a verified release
commit](workflows.md#getting-a-verified-release-commit) for where it fits in a
workflow, and for the alternatives.

!!! note "Why this is not an option on `prepare`"

    Signing depends on *who is authenticated*, and only a bot gets a signature.
    `gh ship prepare` is supported from a laptop as much as from CI, so a
    `sign:` setting there would either sign only sometimes or stop `prepare`
    working outside CI. The prepare workflow's token is already a bot, so that
    is where the signing belongs.
