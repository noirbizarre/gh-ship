# Workflows

gh-ship dispatches workflows you own. This page is the contract they must satisfy.

`gh ship validate` checks every rule here, so you never have to discover a violation
during a release.

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
.github/workflows/prepare-release.yaml   ->   prepare: prepare-release
```

Not by the `name:` in the workflow. That means the display name is yours to
decorate:

```yaml
name: 🚢 Prepare Release
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

### 3. It must upload the artifact

```yaml
- uses: actions/upload-artifact@v4
  with:
    name: ship-release
    path: ship.release.json
    if-no-files-found: error
```

Both names are part of the [protocol](specifications/release-artifact.md).

### 4. It must respect `dry_run`

When `dry_run` is `true`, the workflow must produce the artifact but **not** commit
or push. This is what makes `gh ship preview` safe.

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

```yaml
on:
  workflow_dispatch:
    inputs:
      ship_id: { required: true, type: string }
      tag: { required: true, type: string }

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
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
    environment:
      name: release
      deployment: false
    steps:
      - uses: actions/checkout@v7
      - run: gh extension install noirbizarre/gh-ship
        env:
          GH_TOKEN: ${{ github.token }}
      - run: gh ship prepare
        env:
          GH_TOKEN: ${{ secrets.SHIP_TOKEN || secrets.GITHUB_TOKEN }}
```

A push with nothing to release costs one prepare-release run reporting
`changed: false`, and exits 0.

!!! tip "Merging the Release PR does not start another one"

    The merge is itself a push to your default branch, so this workflow runs
    again — but `gh ship prepare` detects that a merged Release PR is still
    awaiting `gh ship release` and stops. See
    [`gh ship prepare`](cli.md#gh-ship-prepare).

    `cancel-in-progress: false` matters: `prepare` blocks on a dispatched run,
    and cancelling it would orphan that run rather than stop it.

## Tokens

!!! warning "The default token cannot trigger workflows"

    GitHub deliberately prevents `GITHUB_TOKEN` from triggering further workflow
    runs, to avoid infinite loops. A commit or PR authored with it will **not**
    start your CI.

    If the Release PR must be tested before merging, you need a different token.

Options, best first:

1. **A GitHub App token.** Scoped, rotatable, attributable. Mint it in the workflow
   with [`actions/create-github-app-token`](https://github.com/actions/create-github-app-token).
2. **A fine-grained PAT**, stored as the `SHIP_TOKEN` secret.
3. **Nothing.** Accept that the Release PR shows no CI results.

The generated template prefers `SHIP_TOKEN` when present:

```yaml
- uses: actions/checkout@v7
  with:
    token: ${{ secrets.SHIP_TOKEN || secrets.GITHUB_TOKEN }}
```

gh-ship never sees, stores, or manages this secret. It is between you and GitHub.

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

    | Permission | Access |
    |---|---|
    | `metadata` | read |
    | `contents` | write |
    | `actions` | write |
    | `pull_requests` | write |
    | `issues` | write |

!!! danger "Actions: write is the one people miss"

    Without it, `gh ship prepare` fails at the very first step:

    ```
    × `gh workflow run prepare-release.yaml --ref release/next` failed:
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

Every rule above is checked, with an explanation of why it exists:

```
× workflow `prepare-release.yml` does not declare `on: workflow_dispatch`
  help: gh-ship starts workflows through the API, which can only start workflows
        declaring `on: workflow_dispatch`. A `workflow_call`-only workflow — what
        is usually called a reusable workflow — cannot be started this way.
        Declare both triggers to keep it reusable *and* dispatchable.
```
