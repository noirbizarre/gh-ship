# Configuration

gh-ship is configured by `.github/ship.yml`. Generate it with
[`gh ship init`](cli.md#gh-ship-init), or write it by hand.

Only `version` and `workflows.prepare` are required.

## Editor support

Add the modeline as the first line and your editor will offer completion and
flag mistakes as you type. `gh ship init` writes it for you.

```yaml
# yaml-language-server: $schema=https://noirbizarre.github.io/gh-ship/schema/config/v1.json
```

It is a comment, so it costs nothing at parse time and older gh-ship versions
ignore it. The schema is for your editor; `gh ship validate` keeps its own
checks, which explain problems in more detail than a schema error can.

## Minimal

```yaml
# yaml-language-server: $schema=https://noirbizarre.github.io/gh-ship/schema/config/v1.json
version: 1
workflows:
  prepare: prepare-release
```

## Complete

```yaml
# yaml-language-server: $schema=https://noirbizarre.github.io/gh-ship/schema/config/v1.json
version: 1

# Branch on which the release is staged. gh-ship stages each release on a
# throwaway branch and moves this one onto the result, so do not push to it
# yourself — anything else pushed there is discarded.
release_branch: release/next

# Branch the Release PR targets.
# Defaults to the repository's default branch.
base_branch: main

workflows:
  prepare: prepare-release
  publish: publish-release

pull_request:
  title: "Release {{ version }}"
  header: |
    This PR prepares the next release.
  footer: |
    Generated automatically by gh-ship.
  labels: [release]
  reuse: true

release:
  draft: true
```

## Reference

### Root

| Key | Type | Default | Meaning |
|---|---|---|---|
| `version` | integer | — | **Required.** Config schema version. Must be `1`. |
| `release_branch` | string | `release/next` | Branch the release is staged on. |
| `base_branch` | string | repo default | Branch the Release PR targets. |
| `workflows` | object | — | **Required.** See below. |
| `pull_request` | object | — | Release PR rendering. |
| `release` | object | — | GitHub Release behaviour. |

### `workflows`

| Key | Type | Meaning |
|---|---|---|
| `prepare` | string | **Required.** Workflow that produces the release artifact. |
| `publish` | string | Optional. Workflow that builds and uploads assets. |

Values are the workflow's **filename without the extension** — its slug:

```
.github/workflows/prepare-release.yaml   ->   prepare-release
```

The `name:` inside the workflow is display only, so it is free to carry emoji
(`🚢 Prepare Release`) and can be changed without touching this file. A full
filename or a display name is still accepted, but the slug is the stable identity
and what `gh ship init` writes.

!!! warning "These must be dispatchable"

    A workflow named here **must** declare `on: workflow_dispatch`. A
    `workflow_call`-only workflow cannot be started through the API at all.
    See [Workflows](workflows.md).

### `pull_request`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `title` | template | `Release {{ version }}` | PR title. |
| `header` | template | — | Markdown prepended to the release notes. |
| `footer` | template | — | Markdown appended after the release notes. |
| `labels` | list | `[]` | Labels applied to the Release PR. |
| `reuse` | boolean | `true` | Reuse the existing Release PR instead of opening a new one each time. |

### Reusing the Release PR

By default a release keeps one pull request for its whole life, so its number,
its comments and its review state survive repeated prepares. A closed but
unmerged Release PR is reopened rather than replaced; a merged one is left alone
and a new PR is opened, because that release has shipped.

```yaml
pull_request:
  reuse: false   # close the open Release PR and open a fresh one each prepare
```

The body is assembled as:

```
{{ header }}

{{ release notes from your workflow }}

{{ footer }}
```

Parts that are absent or empty are omitted, with no stray blank lines.

### `release`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `draft` | boolean | `true` | Create the release as a draft, then publish it after the publish workflow succeeds. |

!!! tip "Leave `draft` alone unless you have a reason"

    Creating the release visible-first notifies every watcher of a release with no
    assets attached. Draft-first is the only ordering where a release becomes
    visible complete.

## Templates

Templates are [MiniJinja](https://docs.rs/minijinja) (Jinja2-compatible).

**The release artifact is the root context.** The vocabulary is exactly what the
[artifact specification](specifications/release-artifact.md) documents:

| Expression | Value |
|---|---|
| `{{ version }}` | `1.4.0` |
| `{{ tag }}` | `v1.4.0` |
| `{{ changed }}` | `true` |
| `{{ release.name }}` | `Release v1.4.0` |
| `{{ release.notes }}` | the changelog your workflow produced |
| `{{ release.prerelease }}` | `false` |

!!! danger "Not `{{ release.version }}`"

    `version` and `tag` are at the **root**, not under `release`. `release` holds
    only the GitHub Release fields.

Examples:

```yaml
pull_request:
  title: "Release {{ version }}"
  header: |
    Shipping `{{ tag }}`.
    {% if release.prerelease %}
    :warning: This is a pre-release.
    {% endif %}
```

## Overrides from the artifact

A workflow can override rendering per release by setting `pull_request` in the
artifact. Artifact values win over config:

```json
{
  "schemaVersion": 1,
  "changed": true,
  "version": "2.0.0",
  "tag": "v2.0.0",
  "pull_request": {
    "title": "Release 2.0.0 — breaking changes",
    "labels": ["breaking"]
  }
}
```

Setting `pull_request.body` in the artifact replaces the body entirely, skipping
header/footer assembly.

Labels from both sources are merged, without duplicates.
