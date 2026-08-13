# Architecture

gh-ship is intentionally small. The interesting decisions are about what it refuses
to do.

## Principles

**GitHub-first.** Every GitHub interaction goes through the `gh` CLI. gh-ship
implements no REST client and handles no tokens, because `gh` already solves
authentication, enterprise hosts, SSO, and rate limiting.

**Convention over configuration.** The only required config is `version` and
`workflows.prepare`.

**Never execute user release logic.** gh-ship has no `run:` key and no shell
execution. Your workflows do the work.

**Never manage secrets.**

**Zero local state.** Everything is reconstructed from GitHub.

## The anti-goal

!!! danger "This must never become a workflow engine"

    gh-ship's predecessor was a custom workflow DSL, and it failed in an
    instructive way: the DSL kept growing ad-hoc escape hatches for control flow it
    could not express, its built-in actions were perpetually incomplete so users
    fell back to raw shell anyway, and everything was stringly typed.

    The lesson is that a workflow engine is a bottomless product. GitHub Actions
    already exists and is better at it. gh-ship orchestrates *around* it and stops
    there.

## Layout

```
src/
  artifact/        # the protocol: model, embedded schema, validation, span lookup
  gh/              # everything that talks to GitHub, via the gh CLI
    cli.rs         #   subprocess wrapper and error classification
    workflow.rs    #   workflow discovery and contract checking
    run.rs         #   dispatch, correlation, polling
    repo.rs        #   branches, PRs, releases
  commands/        # one module per subcommand
    context.rs     #   the shared dispatch → wait → validate sequence
  config.rs        # .github/ship.yml
  detect.rs        # which branch are we releasing from?
  branches.rs      # base branch -> release line
  render.rs        # PR templating and artifact embedding
  templates.rs     # rendering the workflows `init` writes
  style / logger / suggest
schemas/           # the published JSON Schemas — release artifact and config —
                   # embedded via include_str!
templates/         # the workflow templates `init` renders and emits, one
                   # MiniJinja source per role, branching on the token strategy
```

Errors are defined per module as `thiserror` enums rather than centrally, so
each carries its own miette diagnostic payload — code, help text and source
span — next to the code that can raise it.

## Key mechanisms

### Resolving the release line

Every lifecycle command starts by answering "which release am I working on?".
`detect.rs` finds the base branch — the `--base` flag, the GitHub Actions
environment, then the checkout's `.git/HEAD` — and `branches.rs` matches it
against the configured lines to produce a `Line { base, release }`. `Context`
does this once, so no command below it knows whether the repository has one
release line or five.

`detect.rs` reads `.git/HEAD` directly rather than shelling out to `git`. `gh`
is the only subprocess gh-ship spawns, and keeping it that way costs ten lines
and buys a detection story with no `PATH` dependency and no way to fetch.

### Dispatch correlation

`gh workflow run` returns 204 No Content. There is no API that maps a dispatch to a
run.

gh-ship generates a nonce, passes it as the `ship_id` input, and requires the
workflow to stamp it into `run-name`. It then polls the run list and matches on that
nonce. `gh ship validate` refuses a workflow that would break this, so the failure
surfaces at setup time rather than mid-release.

The alternative — newest run after the dispatch timestamp — is wrong under
concurrency, and wrong rarely enough to survive testing.

### Zero state, via the PR body

`gh ship release` needs the artifact `gh ship prepare` validated, possibly days
later and on a different machine.

Rather than a state file, gh-ship embeds the artifact in the Release PR body inside
an HTML comment:

```
<!-- ship:artifact
{"schemaVersion":1,"changed":true,"version":"1.4.0","tag":"v1.4.0"}
-->
```

Invisible when rendered, durable, and it outlives artifact retention. It also means
`gh ship status` is a pure query with no cache to go stale.

### Who creates the release

gh-ship tags the merge commit and creates the GitHub Release. The publish
workflow only builds and attaches assets.

The conventional choice is the other one — most release tooling tags and releases
from CI — so the split deserves a reason.

**Release notes are generated before the merge and must survive it.** The prepare
workflow runs your changelog tool against the *release branch*; the publish
workflow checks out the *tag*, which is post-merge history. If the publish
workflow regenerated the notes, what shipped could legitimately differ from what
was reviewed and approved in the Release PR. The artifact exists so that what
ships is what was read.

That also explains why the artifact carries `release.name`, `release.prerelease`
and `release.make_latest`: they exist so gh-ship can create the release. Moving
release creation into the workflow would strand them in a published v1 schema,
where they cannot be removed without a v2.

**The honest cost.** `gh release create <tag> <assets…>` already performs
draft → upload → publish atomically, so gh-ship's create-draft → dispatch →
undraft sequence reimplements it. gh-ship could only use the native behaviour by
holding the assets itself — downloading every binary from the workflow and
re-uploading it — which would push release payloads through whatever machine ran
`gh ship release`. The ordering below is the price of notes fidelity, not an
accident.

### Staging the release commit

`prepare` does not build the release commit on the release branch. It cuts a
throwaway branch from the base, dispatches the prepare workflow there, and then
moves the release branch onto the resulting commit in a single update.

The obvious alternative — reset the release branch to its base and let the
workflow rebuild it in place — was tried, and closes the Release PR. GitHub
closes a pull request whose head becomes contained in its base, and a branch
reset to its base is exactly that. Every prepare closed the Release PR and
opened a replacement.

Staging avoids the empty state: the release branch moves from its previous
release commit straight to the new one, and is never equal to the base.

It also fixes a subtler problem. `workflow_dispatch` reads the workflow
definition from the ref it is given, so dispatching on the release branch meant a
stale branch ran a stale copy of the workflow — the sort of thing that produces
"I fixed it, and it still fails". The staging branch is cut from the base, so the
definition is always current.

Staging branches are named `ship/prepare-<nonce>`, sharing the correlation nonce
with the dispatch and the run, and are deleted once the release branch has moved.
A run that fails part-way leaves one behind, so each prepare sweeps them first.

With [release lines](configuration.md#release-lines) configured the name also
carries the line — `ship/prepare-<base>-<nonce>` — and the sweep is filtered to
that same prefix. Two lines can then prepare concurrently without either one
deleting the ref the other's `workflow_dispatch` is running on.

### Draft-first releases

Tag the merge commit → create the release as a draft → dispatch the publish
workflow → undraft.

A draft is invisible to watchers, yet `gh release upload` still works against it.
That is the only ordering where a release becomes visible complete, rather than
notifying everyone about an empty release whose assets arrive minutes later.

The tag is created explicitly, first. A draft release does **not** create its git
ref — the tag appears only when the release is published — and the publish
workflow is dispatched on that tag and checks it out.

### Diagnostics

Errors are [miette](https://docs.rs/miette) diagnostics with source spans, so a
schema violation points at the offending byte of the user's JSON:

```
× `/release` has unknown field `note`
   ╭─[ship.release.json:8:5]
 8 │     "note": "## What's Changed\n"
   ·     ───┬──
   ·        ╰── not allowed here
  help: did you mean `notes`?
```

Because `serde_json` discards positions, `artifact/span.rs` re-scans the raw text to
resolve a JSON Pointer to a byte range.

Every user-facing error carries a diagnostic code and a `help` that says what to do,
not just what happened.

## Dependencies

| Crate | Why |
|---|---|
| `clap` | CLI |
| `serde`, `serde_json`, `serde_norway` | JSON and YAML |
| `boon` | JSON Schema 2020-12 |
| `miette`, `thiserror` | diagnostics |
| `minijinja` | PR templating |
| `demand` | `init` prompts |
| `uuid` | correlation nonces |
| `owo-colors`, `strsim` | output and "did you mean?" |

No async runtime. The work is dominated by waiting on GitHub, and a synchronous
poll loop expresses that more honestly than an executor would.

## Testing

**Unit tests** for pure logic — schema validation, span lookup, templating, workflow
parsing, error classification.

**A fixture corpus** at `tests/fixtures/artifacts/{valid,invalid}` that is the
executable specification of the protocol. Every rejection's diagnostic is
snapshotted, so a regression in message quality shows up as a diff.

**A hermetic `gh` stub.** Because every GitHub interaction goes through the `gh`
binary, replacing it on `PATH` with a script gives complete control over what GitHub
"says" — with no network, no credentials, and no recorded fixtures to keep in sync.
Tests describe scenarios declaratively:

```rust
GhStub::new()
    .pr_state("MERGED")
    .merge_commit("abc1234")
    .artifact(r#"{"schemaVersion":1,"changed":true,...}"#)
```

The stub logs every invocation, so tests assert on what gh-ship actually asked
GitHub to do — including what it must *not* do, like `preview` never calling
`pr create`.
