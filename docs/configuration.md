# Configuration

gh-ship is configured by `.github/ship.yml`. Generate it with
[`gh ship init`](cli.md#gh-ship-init), or write it by hand.

Only `version` and `workflows.prepare` are required.

## Editor support

Add the modeline as the first line and your editor will offer completion and
flag mistakes as you type. `gh ship init` writes it for you.

```yaml
# $schema: https://noirbizarre.github.io/gh-ship/schema/config/v1.json
```

It is a comment, so it costs nothing at parse time and older gh-ship versions
ignore it. This `$schema:` form is understood by both yaml-language-server
(VS Code, Neovim) and JetBrains IDEs. The schema is for your editor;
`gh ship validate` keeps its own checks, which explain problems in more detail
than a schema error can.

## Minimal

```yaml
# $schema: https://noirbizarre.github.io/gh-ship/schema/config/v1.json
version: 1
workflows:
  prepare: prepare-release
```

## Complete

```yaml
# $schema: https://noirbizarre.github.io/gh-ship/schema/config/v1.json
version: 1

# Branch on which the release is staged. gh-ship stages each release on a
# throwaway branch and moves this one onto the result, so do not push to it
# yourself — anything else pushed there is discarded.
release_branch: release/next

# The branches gh-ship releases from, one release line each.
# Omit it and the repository's default branch is the only line.
branches: [main]

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
| `release_branch` | template | `release/next` | Branch the release is staged on. |
| `branches` | list | repo default | Base branches to release from — see [Release lines](#release-lines). |
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
.github/workflows/prepare-release.yml    ->   prepare-release
.github/workflows/prepare-release.yaml   ->   prepare-release
```

Either extension is discovered, and both yield the same slug.

The `name:` inside the workflow is display only, so it is free to carry emoji
(`🚀 Prepare Release`) and can be changed without touching this file. A full
filename or a display name is still accepted, but the slug is the stable identity
and what `gh ship init` writes.

!!! warning "These must be dispatchable"

    A workflow named here **must** declare `on: workflow_dispatch`. A
    `workflow_call`-only workflow cannot be started through the API at all.
    See [Workflows](workflows.md).

## Release lines

Most projects release from one branch. Some need more: a `1.x` line kept alive
for security fixes while `main` moves on to `2.0`. `branches` lists the base
branches gh-ship releases from, and `release_branch` becomes a template rendered
once per line — so nothing is duplicated.

```yaml
version: 1
branches: [main, "release/*"]
release_branch: "next/{{ match }}"
workflows:
  prepare: prepare-release
```

That gives two independent releases in flight:

| Base branch | Release branch | Release PR |
|---|---|---|
| `main` | `next/main` | `next/main` → `main` |
| `release/1.x` | `next/1.x` | `next/1.x` → `release/1.x` |

Each line gets its own release branch, its own Release PR and its own staging
branches, so two prepares can run at once without touching each other.

### Entries

An entry containing `*` is a **glob**; anything else is an **exact branch
name**. A glob may contain at most one `*`, and `*` matches `/` — so
`release/*` covers `release/1.x` as well as `release/1/x`.

Exact entries are matched first, whatever their order in the file, then globs in
the order they are written, first match winning. Writing `main` alongside `*`
therefore means what it looks like: `main` is special.

A branch matching no entry is refused, rather than released from by accident.

### The `release_branch` template

`release_branch` is a MiniJinja template with two variables:

| Variable | Meaning |
|---|---|
| `{{ branch }}` | The full base branch name, e.g. `release/1.x`. |
| `{{ match }}` | What the `*` captured, e.g. `1.x`. For an exact entry, the branch itself. |

!!! warning "With several lines, it must vary"

    A constant `release_branch` would put every line on the same branch, where
    they would silently overwrite each other's Release PR. gh-ship refuses a
    configuration with two or more `branches` entries whose `release_branch`
    renders the same for every one of them.

    Two *templates* that happen to collide — different bases rendering to the
    same name — cannot be detected up front. Keep `{{ match }}` in the name and
    they cannot.

### Which branch am I on?

`prepare`, `preview`, `release` and `status` need to know which line they are
working on. They look, in order:

1. `--base <branch>`, or `SHIP_BASE_BRANCH`.
2. The GitHub Actions environment — the branch a PR targets on a
   `pull_request` event, otherwise the branch of the run. A run on a tag names
   no branch, and is not guessed at.
3. The local checkout's current branch.
4. The repository's default branch.

Detection only happens when `branches` is configured. Without it there is one
line and nothing to select, so the repository default branch is used exactly as
before — being on a feature branch never retargets your Release PR.

`gh ship status` reports which it used:

```
base branch: release/1.x (detected from CI)
release line: release/*
```

### Migrating from `base_branch`

`base_branch` was replaced by `branches`, which is the same idea at any arity:

```yaml
# before
base_branch: develop

# after
branches: [develop]
```

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

!!! warning "`release_branch` is the exception"

    The `pull_request` templates above describe a release, so the artifact is
    their context. `release_branch` names a branch, and is rendered before any
    workflow has run — there is no artifact yet. Its context is the branch:
    `{{ branch }}` and `{{ match }}`, and nothing else. See
    [Release lines](#the-release_branch-template).

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
