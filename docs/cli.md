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

1. Creates the release branch if missing (`workflow_dispatch` needs the ref to
   exist).
2. Dispatches the prepare workflow and waits.
3. Downloads and validates the artifact.
4. Stops with exit 0 if `changed: false`.
5. Opens or updates the Release PR, embedding the artifact in its body.

Re-running is safe, and is the supported way to refresh a Release PR.

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
