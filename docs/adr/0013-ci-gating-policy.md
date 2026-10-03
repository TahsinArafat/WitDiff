# ADR-0013: CI gating distinguishes "nothing to prove" from "proof failed"

Status: accepted

## Context

M5 adds ecosystem integrations, starting with a GitHub Actions workflow and PR
annotations (PG-502). The integration's job is to turn a receipt into a pass or
fail on a pull request, plus annotations a reviewer can act on.

That requires a policy decision the core deliberately does not make. The CLI
exposes `--strict`, which exits 2 whenever the status is not exactly `verified`.
Measured against a repository with no changed tests:

```text
$ witdiff verify --base HEAD --strict
  status           : no_changed_tests
$ echo $?
2
```

So a documentation-only or refactor-only pull request fails the check. The
receipt is correct — there genuinely were no changed tests, and no proof was
attempted — but reporting that as a gate failure is wrong. It would also be
self-defeating: a check that fails on unrelated pull requests is a check people
disable, and a disabled check proves nothing.

## Decision

CI gating treats statuses as three classes, not two:

1. **Satisfied** — `verified`, and `verified_with_warnings` unless the operator
   opts into strictness.
2. **Nothing to prove** — `no_changed_tests`. Passes by default, and the summary
   says so explicitly so the pass is not mistaken for a proof.
3. **Failed** — `not_verified`, `head_failed`, `base_incompatible`. Fails.

`base_incompatible` fails rather than passing. It means the transplanted test
could not compile against the base revision, so no behavioral proof was
established; treating undecidable as acceptable would let the one case ADR-0003
exists to flag pass silently.

The distinction is expressed in the CLI rather than only in the workflow file,
so every consumer — GitHub, MCP, a local script — gets the same policy:

- `--strict` keeps its current meaning and is unchanged: exit non-zero unless
  exactly `verified`.
- A new opt-in, `--fail-on-no-changed-tests`, restores the old
  fail-on-everything behavior for repositories that require a test change per
  pull request.

This is a behavior change to `--strict` for the `no_changed_tests` case only.
It is called out here because it is the kind of change that could hide a
regression: if `no_changed_tests` were reached *because WitDiff failed to detect
changed tests*, passing would be wrong.

That risk is bounded by the fact that a false `no_changed_tests` is already
visible: the receipt and the summary list the changed files and the test globs
that were applied, so an operator can see that tests existed and were not
classified. The workflow summary therefore prints the changed-file counts
alongside the verdict.

## Consequences

- A docs-only pull request passes and says "nothing to prove", instead of
  failing on a correct receipt.
- A repository that wants to require a test change per PR sets
  `--fail-on-no-changed-tests` and gets the old behavior explicitly.
- The annotation and summary are generated from the receipt, not from the exit
  code, so a reviewer sees *why* rather than only *that* something failed.
- Annotations are emitted as GitHub workflow commands from a CLI subcommand
  rather than from a workflow `run:` block assembling `echo "::error ..."`.
  Assembling them in YAML would mean every consumer re-implements escaping of
  the message text, and receipt messages contain quotes, backticks and newlines
  that must be escaped or the annotation is malformed.
- No hosted service and no network access from the core (invariant 4). The
  workflow runs the CLI, and the CLI only reads the local repository.
- Signed receipts (PG-503) remain undesigned. This ADR deliberately does not
  anticipate them, because the signing question is about what attests to a
  receipt, which is independent of how a status is rendered in CI.

## Alternatives considered

- **Keep failing on `no_changed_tests`.** Rejected: it fails correct receipts on
  unrelated pull requests, which teaches operators to ignore the check.
- **Pass everything except `head_failed`.** Rejected: it would pass
  `not_verified`, which is the case where a proof was attempted and failed. That
  is the one outcome the tool exists to report.
- **Pass `base_incompatible` as "unknown".** Rejected: undecidable is not
  acceptable as a gate. It is actionable — the change and its test must be
  separated — and failing surfaces that.
- **Put the policy in the workflow YAML.** Rejected: every consumer would
  re-implement it, and the MCP wrapper would drift from CI.
- **Have the CLI call the GitHub API directly.** Rejected: it would put network
  access and a vendor credential into the core's execution path, breaking
  invariant 4 and the process boundary in ADR-0004. Emitting workflow commands
  on stdout keeps the CLI a pure function of the repository.
