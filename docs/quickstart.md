# Quick Start

The goal is a gh-ship enabled repository in under a minute.

## 1. Initialise

```console
$ gh ship init
```

`init` will:

- detect your repository,
- list workflows that gh-ship can actually dispatch,
- explain the ones it cannot use, and why,
- offer to generate templates for anything missing,
- write a documented `.github/ship.yml`.

## 2. Make the workflow yours

The generated `prepare-release.yml` contains placeholder steps. Replace them with
whatever your project already uses — cargo-release, Commitizen, git-cliff,
semantic-release, a shell script. gh-ship does not care.

The only parts you must keep are the [contract](workflows.md): the
`workflow_dispatch` trigger, the `ship_id` input, the `run-name`, and the artifact
upload.

## 3. Check the setup

```console
$ gh ship validate
✔ .github/ship.yml is valid
  release branch: release/next
  prepare: prepare-release (prepare-release.yml)
✔ workflows satisfy the gh-ship contract
```

This catches the mistakes that would otherwise surface mid-release as a confusing
timeout.

## 4. Preview

```console
$ gh ship preview
```

Runs your prepare workflow with `dry_run: true` and renders the Release PR it would
produce. **Nothing on GitHub is modified**: no branch, no PR, no tag, no release.

## 5. Prepare

```console
$ gh ship prepare
```

Creates the release branch if needed, runs your prepare workflow for real, and opens
(or updates) the Release PR.

Re-running `prepare` is safe. It is the supported way to refresh a Release PR.

## 6. Review and merge

Read the PR. Merge it when you are happy.

## 7. Ship

```console
$ gh ship release
```

Tags the merge commit, creates the release as a draft, dispatches your publish
workflow to attach assets, then makes the release visible.

## Where am I?

At any point:

```console
$ gh ship status
▶ status of acme/widgets

  base branch: main
  release branch: release/next
  release pr: #7 Release 1.4.0 [open]
  url: https://github.com/acme/widgets/pull/7
  version: 1.4.0
  tag: v1.4.0

- next: review and merge the Release PR, then run `gh ship release`
```

`status` is a pure query. It reconstructs everything from GitHub, so it gives the
same answer on any machine.
