# Installation

## As a `gh` extension

```console
$ gh extension install noirbizarre/gh-ship
```

Upgrade:

```console
$ gh extension upgrade ship
```

This is the recommended route. It gives you `gh ship`, and inherits your existing
GitHub authentication.

## Standalone binary

gh-ship also works as a plain binary called `gh-ship`. This matters for
`gh ship validate`, which needs no `gh`, no network, and no repository — see
[validating in CI](#in-github-actions).

Download a release for your platform from the
[releases page](https://github.com/noirbizarre/gh-ship/releases), or build it:

```console
$ cargo install --git https://github.com/noirbizarre/gh-ship
```

## In GitHub Actions

Your prepare workflow should validate the artifact before uploading it. GitHub
runners already have `gh` and a token, so installing the extension is one line:

```yaml
- name: Validate the release artifact
  env:
    GH_TOKEN: ${{ github.token }}
  run: |
    gh extension install noirbizarre/gh-ship
    gh ship validate ship.release.json
```

!!! tip "Validation is fully self-contained"

    `gh ship validate FILE` performs no network access, needs no repository, and
    needs no GitHub authentication. The schema is embedded in the binary. It is
    safe as the very first step of any job, on any CI system.

## Requirements

- The [GitHub CLI](https://cli.github.com) (for everything except `validate`).
- Authentication: `gh auth login` locally, or `GH_TOKEN` in CI.
- A repository whose release workflows declare `on: workflow_dispatch` — see
  [Workflows](workflows.md).
