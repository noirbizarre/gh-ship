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

## Tokens

!!! warning "The default token cannot trigger workflows"

    GitHub deliberately prevents `GITHUB_TOKEN` from triggering further workflow
    runs, to avoid infinite loops. A commit or PR authored with it will **not**
    start your CI.

    If the Release PR must be tested before merging, you need a different token.

Options, best first:

1. **A GitHub App token.** Scoped, rotatable, attributable. Mint it in the workflow
   with [`actions/create-github-app-token`](https://github.com/actions/create-github-app-token).
2. **A fine-grained PAT** with `contents: write` and `pull_requests: write`, stored
   as the `SHIP_TOKEN` secret.
3. **Nothing.** Accept that the Release PR shows no CI results.

The generated template prefers `SHIP_TOKEN` when present:

```yaml
- uses: actions/checkout@v6
  with:
    token: ${{ secrets.SHIP_TOKEN || secrets.GITHUB_TOKEN }}
```

gh-ship never sees, stores, or manages this secret. It is between you and GitHub.

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
