# Workflows

gh-ship dispatches workflows you own. This page is the contract they must satisfy.

`gh ship validate` checks rules 1 to 3 below, so you never have to discover those
violations during a release. Rule 4 — uploading the artifact — is the one thing it
cannot check, because it depends on what your job steps actually do at runtime.

## The contract

```yaml
name: prepare-release

# 1. Correlation. Required.
run-name: prepare-release (ship:${{ inputs.ship_id }})

on:
  # 2. Dispatchable. Required.
  workflow_dispatch:
    inputs:
      # 3. The nonce input. Required.
      ship_id:
        required: true
        type: string
      dry_run:
        required: false
        type: boolean
        default: false

  # Optional, recommended: keeps the workflow reusable too.
  workflow_call:
    inputs:
      ship_id: { required: true, type: string }
      dry_run: { required: false, type: boolean, default: false }
```

### 0. gh-ship refers to it by filename slug

`.github/ship.yml` names workflows by their **filename without the extension**:

```
.github/workflows/prepare-release.yml    ->   prepare: prepare-release
.github/workflows/prepare-release.yaml   ->   prepare: prepare-release
```

Either extension works, and both resolve to the same slug — which is why
`gh ship init` writing `.yml` costs you nothing if your repository uses `.yaml`.

Not by the `name:` in the workflow. That means the display name is yours to
decorate:

```yaml
name: 🚀 Prepare Release
```

and renaming it never breaks a release. `gh ship validate` prints the slug, with
the display name beside it when it differs.

### 1. It must be dispatchable

gh-ship starts workflows through the GitHub API, which can only start workflows
declaring `on: workflow_dispatch`.

!!! danger "A reusable workflow is not enough"

    A workflow declaring only `on: workflow_call` — what people usually mean by
    "reusable" — **cannot be started through the API at all**. This is the single
    most common setup mistake.

    Declare **both** triggers to have it both ways: dispatchable by gh-ship, and
    reusable by your other workflows.

### 2. It must stamp the nonce into `run-name`

```yaml
run-name: prepare-release (ship:${{ inputs.ship_id }})
```

This looks decorative. It is not.

`gh workflow run` — and the REST endpoint behind it — returns **204 No Content**. No
run id, no URL. There is no API that says "the dispatch you just made became run
12345".

The obvious workaround is to list recent runs and take the newest. That is wrong
whenever a teammate dispatches concurrently, a schedule fires, a push lands at the
same moment, or GitHub queues the run late. It fails rarely enough to pass testing
and often enough to corrupt a release.

So gh-ship generates a nonce, passes it as `ship_id`, and finds its run by looking
for `ship:<nonce>` in the run title. Explicit beats guessing.

### 3. It must accept and respect `dry_run`

```yaml
on:
  workflow_dispatch:
    inputs:
      dry_run: { required: false, type: boolean, default: false }
```

When `dry_run` is `true`, the workflow must produce the artifact but **not** commit
or push. This is what makes `gh ship preview` safe.

Declaring the input is checked; *respecting* it is up to you. The declaration
matters on its own, because GitHub refuses a dispatch carrying an input the
workflow does not declare — so without it `gh ship preview` fails mid-command.

This rule applies to the prepare workflow only, which is the one `preview`
dispatches.

### 4. It must upload the artifact

!!! warning "Not checked by `gh ship validate`"

    This is the one rule gh-ship cannot verify statically: whether an artifact is
    produced depends on what your steps do, not on what the workflow declares.
    A missing upload surfaces during `gh ship prepare` instead.

```yaml
- uses: actions/upload-artifact@v7
  with:
    name: ship-release
    path: ship.release.json
    if-no-files-found: error
```

Both names are part of the [protocol](specifications/release-artifact.md).

## The prepare workflow

Responsible for everything gh-ship refuses to know about:

1. Work out the next version.
2. Generate the changelog.
3. Update files.
4. Commit and push to the release branch.
5. Write and upload `ship.release.json`.

```yaml
- name: Write the release artifact
  run: |
    jq -n \
      --arg version "$VERSION" \
      --arg tag "v$VERSION" \
      --rawfile notes NOTES.md \
      '{
        "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
        schemaVersion: 1,
        changed: true,
        version: $version,
        tag: $tag,
        release: { notes: $notes }
      }' > ship.release.json

- name: Validate before uploading
  env:
    GH_TOKEN: ${{ github.token }}
  run: |
    gh extension install noirbizarre/gh-ship
    gh ship validate ship.release.json
```

Nothing to release? Say so, and gh-ship stops cleanly with exit 0:

```bash
jq -n '{schemaVersion: 1, changed: false}' > ship.release.json
```

## The publish workflow

Optional. Dispatched by `gh ship release` **after** the release exists as a draft
and **before** it becomes visible, so it can attach assets to a release nobody has
been notified about yet.

It receives the `tag` as an input, and should check that out rather than a branch —
you want to build exactly what is being released.

Rules 1 and 2 apply here exactly as they do to the prepare workflow: it must be
dispatchable, and it must stamp `ship_id` into its `run-name` or `gh ship release`
will never find the run it started. Only rule 3 (`dry_run`) is prepare-only.

```yaml
run-name: 📦 Publish Release (ship:${{ inputs.ship_id }})

on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
      tag: { required: true, type: string }
  # Optional, recommended: keeps the workflow reusable too. The generated
  # template declares both.
  workflow_call:
    inputs:
      ship_id: { required: true, type: string }
      tag: { required: true, type: string }

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ inputs.tag }}

      - run: ./build.sh

      - env:
          GH_TOKEN: ${{ github.token }}
        run: gh release upload "${{ inputs.tag }}" dist/* --clobber
```

## The `release` environment

The generated workflows both run in an environment named `release`, which is where
`SHIP_TOKEN` lives. GitHub creates the environment the first time a workflow
references it.

They declare it differently, on purpose:

```yaml
# prepare-release: secrets, but no deployment record.
environment:
  name: release
  deployment: false
```

```yaml
# publish-release: this one really is a deployment.
environment:
  name: release
  url: ${{ github.server_url }}/${{ github.repository }}/releases/tag/${{ inputs.tag }}
```

By default, referencing an environment creates a GitHub deployment object.
`deployment: false` opts out while still granting the environment's secrets and
still honouring wait timers and required reviewers — see
[Control deployments](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/control-deployments).

Preparing a release is not a deployment, so it should not appear in your deployment
history. Publishing one is, so it does — and its `url` links straight to the release.

!!! warning "Custom deployment protection rules"

    `deployment: false` is incompatible with custom deployment protection rules
    (the GitHub App kind), which need a deployment object to function. A job
    combining the two fails immediately. Wait timers and required reviewers are
    unaffected.

## Releasing on every push

To have gh-ship check for a release on every push to your default branch, add a
workflow that runs `gh ship prepare`. It performs no release work itself — it
dispatches your prepare workflow and manages the Release PR.

```yaml
name: 🚢 Ship

on:
  push:
    branches: [main]

permissions:
  contents: write
  actions: write
  pull-requests: write
  issues: write

concurrency:
  group: ship
  cancel-in-progress: false

jobs:
  prepare:
    runs-on: ubuntu-latest
    # `gh ship prepare` blocks on the dispatched run until `SHIP_RUN_TIMEOUT`
    # expires, 60 minutes by default. Capping below that makes a stuck run
    # fail the job visibly. See [Waiting](cli.md#waiting).
    timeout-minutes: 30
    environment:
      name: release
      deployment: false
    # Set once so every `gh` invocation picks up SHIP_TOKEN when configured.
    # With a GitHub App token this has to move to each step instead — see
    # [Using a GitHub App](#using-a-github-app).
    env:
      GH_TOKEN: ${{ secrets.SHIP_TOKEN || secrets.GITHUB_TOKEN }}
    steps:
      - uses: actions/checkout@v7
      - run: gh extension install noirbizarre/gh-ship
      - run: gh ship prepare
```

Add a second job to release when that PR merges, and the whole lifecycle runs
itself:

```yaml
on:
  push:
    branches: [main]
  pull_request:
    types: [closed]

jobs:
  prepare:
    if: github.event_name == 'push'
    # ... gh ship prepare

  release:
    # `closed` fires for abandoned pull requests too, so check it merged.
    if: >-
      github.event_name == 'pull_request'
      && github.event.pull_request.merged
      && github.event.pull_request.head.ref == 'release/next'
    # ... gh ship release
```

So:

| Event | What happens |
|---|---|
| push to your default branch | `gh ship prepare` opens or updates the Release PR |
| the Release PR merges | `gh ship release` tags, drafts, attaches assets, publishes |

A push with nothing to release costs one prepare-release run reporting
`changed: false`, and exits 0.

!!! tip "Merging the Release PR does not start another one"

    The merge is itself a push to your default branch, so this workflow runs
    again — but `gh ship prepare` detects that a merged Release PR is still
    awaiting `gh ship release` and stops. See
    [`gh ship prepare`](cli.md#gh-ship-prepare).

    `cancel-in-progress: false` matters: both commands block on a dispatched
    run, and cancelling would orphan that run rather than stop it.

    Merging fires **both** triggers, since the merge is also a push. The
    concurrency queue keeps them from interleaving, and either order is
    correct — if `prepare` goes first it stops at the pending-release guard, and
    if `release` goes first the next `prepare` finds nothing to release.

## Tokens

!!! warning "The default token cannot trigger workflows"

    GitHub deliberately prevents `GITHUB_TOKEN` from triggering further workflow
    runs, to avoid infinite loops. A commit or PR authored with it will **not**
    start your CI.

    If the Release PR must be tested before merging, you need a different token.

Options, best first:

1. **[A GitHub App token](#using-a-github-app).** Scoped, rotatable, attributable, and
   nothing long-lived is stored. Minted per job with
   [`actions/create-github-app-token`](https://github.com/actions/create-github-app-token).
2. **A fine-grained PAT**, stored as the `SHIP_TOKEN` secret.
3. **Nothing.** Accept that the Release PR shows no CI results.

`gh ship init` asks which of the three you want and generates the matching workflow.

With a PAT, the generated template prefers `SHIP_TOKEN` when present:

```yaml
- uses: actions/checkout@v7
  with:
    token: ${{ secrets.SHIP_TOKEN || secrets.GITHUB_TOKEN }}
```

gh-ship never sees, stores, or manages this secret. It is between you and GitHub.

### Using a GitHub App

An App is the best of the three because nothing durable is stored: the workflow
mints an installation token at the start of the job, and the action revokes it
when the job ends. What you keep is the private key, which is useless without a
workflow run to use it in.

Setting one up:

1. [Register a GitHub App](https://docs.github.com/apps/creating-github-apps/setting-up-a-github-app/creating-a-github-app),
   granting it the repository permissions in the
   [GitHub App tab](#what-the-token-must-be-allowed-to-do) below.
2. Install it on the repository you release.
3. Store its **Client ID** as the `APP_CLIENT_ID` variable — it is not a secret —
   and its private key as the `APP_PRIVATE_KEY` secret. Put both in the `release`
   environment, alongside your other release secrets.

```yaml
jobs:
  prepare:
    runs-on: ubuntu-latest
    # Well below the token's one-hour lifetime. See the warning below.
    timeout-minutes: 30
    # Load-bearing twice over: it gates the secret, and an environment
    # variable resolves only in a job that declares its environment.
    environment:
      name: release
      deployment: false
    steps:
      # An installation token, minted for this job and revoked when it ends.
      # The private key never leaves the secret.
      - uses: actions/create-github-app-token@v3
        id: app-token
        with:
          client-id: ${{ vars.APP_CLIENT_ID }}
          private-key: ${{ secrets.APP_PRIVATE_KEY }}

      - uses: actions/checkout@v7
        with:
          token: ${{ steps.app-token.outputs.token }}

      - run: gh extension install noirbizarre/gh-ship
        env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}

      - run: gh ship prepare
        env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}
```

!!! warning "The App token cannot go in the job's `env:`"

    Everywhere else these docs set `GH_TOKEN` once at the job level, because a
    per-step `GH_TOKEN` is easy to forget on one step and silently fall back to
    the default token.

    An App token cannot be set that way. It is a *step output*, and a job-level
    `env:` block is evaluated before any step has run, so
    `${{ steps.app-token.outputs.token }}` there resolves to the empty string —
    and `gh` falls back to `GITHUB_TOKEN` without complaining. Set it on every
    step that runs `gh`, and check you have not missed one.

!!! danger "Installation tokens expire after one hour"

    `gh ship prepare` and `gh ship release` both block on a dispatched workflow
    run, for up to `SHIP_RUN_TIMEOUT` — 60 minutes by default. A job that runs
    that long outlives its own token, and fails at whichever call happens to
    come after the hour.

    Cap `timeout-minutes` well below 60. `skip-token-revoke` does not help: it
    stops the action revoking the token early, it does not extend its lifetime.

!!! warning "An environment variable is invisible outside its environment"

    Keeping `APP_CLIENT_ID` in the `release` environment rather than at
    repository level means a job that does not declare
    `environment: release` cannot see it. `${{ vars.APP_CLIENT_ID }}` there
    expands to the empty string, and the mint step fails with a confusing
    error rather than "variable not set".

    So the `environment:` key is not only about gating: it is what makes the
    credentials resolve at all. Every job that mints a token needs it.

!!! tip "`app-id` also works"

    `actions/create-github-app-token` accepts the legacy `app-id` input, which
    takes the App's numeric ID rather than its Client ID. `client-id` is what
    the action recommends, and what these examples use.

#### Committing as the App

Your prepare workflow commits the version bump. By default that commit is
attributed to whichever identity `git config` names — usually
`github-actions[bot]`, copied from an example — which is misleading once the
push is authenticated as your App.

The App's bot user has a numeric id, and the pair makes the commit attribute to
the App and show as verified:

```yaml
- name: Get the App's user id
  id: bot
  run: echo "id=$(gh api "/users/${{ steps.app-token.outputs.app-slug }}[bot]" --jq .id)" >> "$GITHUB_OUTPUT"
  env:
    GH_TOKEN: ${{ steps.app-token.outputs.token }}

- name: Commit and push
  run: |
    git config user.name  '${{ steps.app-token.outputs.app-slug }}[bot]'
    git config user.email '${{ steps.bot.outputs.id }}+${{ steps.app-token.outputs.app-slug }}[bot]@users.noreply.github.com'
    git add -A
    git commit -m "chore(release): ${{ steps.version.outputs.version }}"
    git push origin HEAD
```

### What the token must be allowed to do

=== "Fine-grained PAT"

    Repository permissions:

    | Permission | Access | Why |
    |---|---|---|
    | **Metadata** | Read-only | Resolve the repository |
    | **Contents** | Read and write | Create the release branch, read refs, merge the Release PR, create and edit the release |
    | **Actions** | Read and write | **Dispatch your workflow** and download its artifact |
    | **Pull requests** | Read and write | List, create and update the Release PR |
    | **Issues** | Read and write | Create missing labels |

=== "Classic PAT"

    | Scope | Why |
    |---|---|
    | `repo` | Branch, PR, release and label access |
    | `workflow` | Dispatch workflows, and push commits that touch `.github/workflows/` |

=== "GitHub App"

    Repository permissions:

    | Permission | Access | Why |
    |---|---|---|
    | `metadata` | read | Resolve the repository |
    | `contents` | write | Create the release branch, read refs, merge the Release PR, create and edit the release |
    | `actions` | write | **Dispatch your workflow** and download its artifact |
    | `pull_requests` | write | List, create and update the Release PR |
    | `issues` | write | Create missing labels |

    !!! note "Installation permissions are not the App's permissions"

        They are fixed when the App is installed. Adding a permission to the App
        later does not grant it to existing installations until an account
        administrator approves it — so an App that looks correctly configured can
        still mint a token that lacks `actions: write`.

!!! danger "Actions: write is the one people miss"

    Without it, `gh ship prepare` fails at the very first step:

    ```
    × `gh workflow run prepare-release.yaml --ref ship/prepare-8f2c1a9e4b07` failed:
      HTTP 403: Resource not accessible by personal access token
    ```

    Dispatching a workflow is an Actions write, not a Contents write. A token
    with full repository content access will still fail here.

Three of these are counter-intuitive enough to be worth stating outright:

- **Merging the Release PR needs `Contents`, not `Pull requests`.** GitHub lists
  `PUT /repos/{owner}/{repo}/pulls/{number}/merge` under Contents.
- **Label creation may already be covered.** `POST /repos/{owner}/{repo}/labels`
  is listed under both Issues and Pull requests, so `Pull requests: write` may
  suffice. Grant `Issues: write` if labels still fail — gh-ship degrades
  gracefully and opens the PR without them either way.
- **`Workflows: write` may be needed too.** GitHub lists the ref-creating and
  release endpoints under a separate `Workflows` permission. Grant it if a
  release commit could ever modify a file under `.github/workflows/`.

The authoritative mapping is GitHub's
[permissions required for fine-grained personal access tokens](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens).

## Permissions

The prepare workflow pushes commits:

```yaml
permissions:
  contents: write
```

The publish workflow uploads release assets:

```yaml
permissions:
  contents: write
```

## Checking your work

```console
$ gh ship validate
```

Rules 1 to 3 are checked, with an explanation of why each exists:

```
× workflow `prepare-release` does not declare `on: workflow_dispatch`
  help: gh-ship starts workflows through the API, which can only start workflows
        declaring `on: workflow_dispatch`. A `workflow_call`-only workflow — what
        is usually called a reusable workflow — cannot be started this way.
        Declare both triggers to keep it reusable *and* dispatchable.
```
