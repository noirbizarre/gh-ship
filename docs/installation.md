# Installation

gh-ship is a [GitHub CLI](https://cli.github.com) extension, and that is the only
way it is distributed.

```console
$ gh extension install noirbizarre/gh-ship
```

Upgrade:

```console
$ gh extension upgrade ship
```

You get `gh ship`, using the GitHub authentication you already have. There is no
separate binary to download, no package on crates.io, and nothing to keep in sync.

## In GitHub Actions

Your prepare workflow should validate the artifact before uploading it. Runners
already have `gh` and a token, so installing is one line:

```yaml
- name: Validate the release artifact
  env:
    GH_TOKEN: ${{ github.token }}
  run: |
    gh extension install noirbizarre/gh-ship
    gh ship validate ship.release.json
```

!!! tip "Validation is fully self-contained"

    Installing needs `gh`, but *running* `gh ship validate FILE` does not need
    much of anything: no network access, no repository, and no GitHub
    authentication. The schema is embedded in the binary.

    So it is safe as the very first step of any job, and the check itself works
    on any CI system. This is enforced by a test that runs it under `env -i` with
    an empty `PATH`.

## Requirements

- The [GitHub CLI](https://cli.github.com).
- Authentication: `gh auth login` locally, or `GH_TOKEN` in CI. See
  [what the token must be allowed to do](workflows.md#what-the-token-must-be-allowed-to-do).
- Release workflows that declare `on: workflow_dispatch` — see
  [Workflows](workflows.md).

## Building from source

You do not need this to use gh-ship. It is here because gh-ship's own workflows
do it, and that can look like a contradiction:

```yaml
- run: cargo build --release
- run: ./target/release/gh-ship prepare
```

That is bootstrapping, not a second installation route. gh-ship releases itself,
and an extension cannot install itself before its own first release exists. Once
a release is published, those workflows will use `gh extension install` like
everyone else.
