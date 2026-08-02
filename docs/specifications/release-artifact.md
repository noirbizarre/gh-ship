# Release Artifact Specification

**Version 1** · Schema: <https://noirbizarre.github.io/gh-ship/schema/release/v1.json>

The release artifact is the public protocol between a GitHub Actions workflow and
gh-ship. It is the *only* channel between them.

gh-ship owns this protocol. Any tool may produce a conforming artifact.

---

## Why an artifact, and not job outputs

GitHub Actions job outputs are limited to 1 MB in aggregate and are awkward to
produce from a matrix or a composite action. Release notes routinely exceed what
outputs comfortably carry, and multi-line values require escaping gymnastics that
corrupt Markdown.

An artifact is a plain file. It can be written with `jq`, checked into a test
fixture, validated locally, diffed in a PR, and produced by a tool that has never
heard of gh-ship.

## Transport

The workflow uploads an artifact named **`ship-release`** containing a single file
named **`ship.release.json`**.

```yaml
- name: Validate the artifact before uploading
  run: gh ship validate ship.release.json

- uses: actions/upload-artifact@v7
  with:
    name: ship-release
    path: ship.release.json
```

Validating before uploading turns a protocol mistake into a red workflow with a
precise error, instead of a confusing failure in gh-ship minutes later.

---

## Document

```json
{
  "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
  "schemaVersion": 1,
  "changed": true,
  "version": "1.4.0",
  "tag": "v1.4.0",
  "release": {
    "name": "Release v1.4.0",
    "notes": "## What's Changed\n\n* Add `gh ship preview`\n",
    "prerelease": false,
    "make_latest": true
  },
  "pull_request": {
    "title": "Release v1.4.0",
    "labels": ["release"]
  }
}
```

### Root

| Field | Type | Required | Meaning |
|---|---|---|---|
| `$schema` | string | no | Editor metadata. Enables autocompletion in VS Code and JetBrains IDEs. Ignored by gh-ship. |
| `schemaVersion` | integer | **yes** | Protocol version. Must be `1`. Authoritative — see [Versioning](#versioning). |
| `changed` | boolean | **yes** | Whether there is anything to release. |
| `version` | string | when `changed` | The version being released. |
| `tag` | string | when `changed` | The git tag to create. |
| `release` | object | no | GitHub Release content. |
| `pull_request` | object | no | Release PR overrides. |

### `release`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | the tag | Release title. |
| `notes` | string | `""` | Release body, GitHub Flavored Markdown. |
| `prerelease` | boolean | `false` | Mark as a pre-release. |
| `make_latest` | boolean | GitHub's | Mark as the repository's latest release. |

!!! note "Omitting `make_latest` is not the same as setting it to `true`"

    When the field is absent gh-ship passes nothing, so GitHub applies its own
    default (`legacy`: the release with the highest semver tag wins). Set it
    explicitly if you need the outcome pinned.

### `pull_request`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `title` | string | from config | Overrides `pull_request.title` in `.github/ship.yml`. |
| `body` | string | rendered | When set, used verbatim; header/notes/footer assembly is skipped. |
| `labels` | array of string | `[]` | Applied in addition to configured labels. |

---

## `changed` and the release identity

`changed` is the workflow's answer to "is there anything to release?".

- **`changed: false`** — `version` and `tag` are rejected: promising nothing to
  release while naming a release is a schema error. gh-ship prints
  `nothing to release`, **exits 0**, and touches no branch, PR, tag or release.
- **`changed: true`** — `version` and `tag` are required. Promising a release
  without identifying it is a schema error.

`changed: false` is a success, not a failure. A scheduled release job that finds
nothing to ship should go green.

## What gh-ship does *not* do with these values

gh-ship deliberately knows nothing about how your project versions itself:

- **`version` is never parsed.** It is not required to be semver. CalVer, a date,
  a build number, or `banana` are all accepted. gh-ship only requires a non-empty,
  single-line string with no leading or trailing whitespace.
- **`tag` is never derived from `version`.** If your convention is `v1.4.0`, or
  `release-1.4.0`, or `1.4.0`, that is your workflow's business. gh-ship only
  requires a non-empty string with no whitespace.
- **`notes` are never generated.** Use git-cliff, release-drafter, Changesets,
  `gh api` — whatever you already use.

---

## Extensions

Any property whose name begins with `x-` is permitted at every level and ignored
by gh-ship:

```json
{
  "schemaVersion": 1,
  "changed": true,
  "version": "1.0.0",
  "tag": "v1.0.0",
  "x-generator": "my-release-tool@2.1.0",
  "release": { "x-commit-count": 42 }
}
```

Everything else is rejected. This is a deliberate trade: strictness means a typo
like `"tags"` fails loudly instead of silently doing nothing, and `x-` gives
third-party tools a sanctioned place to put their own metadata.

```
× the artifact has unknown field `tags`
   ╭─[ship.release.json:5:3]
 5 │   "tags": "v1.4.0",
   ·   ───┬──
   ·      ╰── not allowed here
  help: did you mean `tag`?
```

---

## Versioning

`schemaVersion` is **authoritative**. `$schema` is editor metadata; if the two
disagree, gh-ship follows `schemaVersion` and ignores `$schema`.

A gh-ship build refuses any `schemaVersion` it does not implement, and says which
direction to move:

```
× unsupported artifact schema version: 2
  help: this artifact speaks protocol v2, but this gh-ship only understands v1.
        Upgrade gh-ship: `gh extension upgrade ship`
```

Version 1 describes exactly **one** release. Multi-package and monorepo releases
are out of scope by design; supporting them would be a version 2 with a different
shape, not a compatible addition to this one.

---

## Validation

```console
$ gh ship validate ship.release.json
```

This command requires **no network, no repository, and no GitHub authentication**.
The schema is embedded in the binary. It is safe to run as the first step of any
job, and in CI systems that are not GitHub Actions.

Exit codes:

| Code | Meaning |
|---|---|
| `0` | Valid. (Including `changed: false`.) |
| `1` | Invalid, or unreadable. |

## Editor support

Add `$schema` to get completion and inline validation:

```json
{
  "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
  "schemaVersion": 1,
  "changed": false
}
```

---

## Minimal producers

Nothing about this protocol requires a tool. `jq` is enough:

```bash
jq -n --arg version "$VERSION" --arg tag "v$VERSION" --rawfile notes NOTES.md '{
  "$schema": "https://noirbizarre.github.io/gh-ship/schema/release/v1.json",
  schemaVersion: 1,
  changed: true,
  version: $version,
  tag: $tag,
  release: { notes: $notes }
}' > ship.release.json
```

Nothing to release:

```bash
jq -n '{schemaVersion: 1, changed: false}' > ship.release.json
```
