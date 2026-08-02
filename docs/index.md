<p align="center" markdown>
  ![gh-ship](images/logo.svg){ width="520" }
</p>

**The GitHub Release Orchestrator.**

gh-ship orchestrates the lifecycle of GitHub Releases around workflows you already
own. It is a [GitHub CLI](https://cli.github.com) extension, and it feels like one.

```console
$ gh ship prepare
▶ preparing acme/widgets
▶ staging on ship/prepare-8f2c1a9e4b07 from main
▶ dispatching prepare-release on ship/prepare-8f2c1a9e4b07
  ship id: 8f2c1a9e4b07
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

## The division of labour

gh-ship orchestrates. Your workflows perform the work.

| gh-ship does | your workflow does |
|---|---|
| create the release branch | bump the version |
| dispatch workflows | generate the changelog |
| wait for them and correlate runs | update files |
| validate the release artifact | commit and push |
| render the Release PR | |
| tag and create the GitHub Release | |

## What gh-ship deliberately is not

!!! warning "This is not a workflow engine"

    There is no DSL, no step registry, no `run:` key, and there never will be.
    gh-ship reuses GitHub Actions rather than reinventing it.

- It does not replace GitHub Actions.
- It does not replace Commitizen, git-cliff, cargo-release, semantic-release, or
  Changesets. Keep using them.
- It never manages secrets — authentication is `gh`'s job.
- It never knows how your project versions itself.
- It never generates changelogs.

## The protocol

Your workflow and gh-ship communicate through exactly one thing: a JSON artifact
named [`ship.release.json`](specifications/release-artifact.md).

```json
{
  "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
  "schemaVersion": 1,
  "changed": true,
  "version": "1.4.0",
  "tag": "v1.4.0",
  "release": { "notes": "## What's Changed\n\n* ..." }
}
```

That is the whole contract. It is versioned, schema-validated, and any tool can
produce it.

## Next

<div class="grid cards" markdown>

- **[Installation](installation.md)** — get the extension.
- **[Quick Start](quickstart.md)** — shipping in a minute.
- **[Configuration](configuration.md)** — `.github/ship.yml`.
- **[Workflows](workflows.md)** — the contract your
  workflows must satisfy.

</div>
