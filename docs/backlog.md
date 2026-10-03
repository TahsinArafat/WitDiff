# Agent-ready backlog

These tasks are written so multiple coding agents can pick up work without relying on prior chat history.

## P0 — compile, lint, smoke, harden

Status: complete. Covered by `crates/witdiff-core/tests/verify_end_to_end.rs`.

### PG-001: full verification integration test — DONE

**Goal:** exercise `verify_repository` end-to-end in a temporary real Git/Rust repository.

**Invariant:** `Verified` requires HEAD green, pristine BASE green, BASE+changed-tests red.

**Acceptance:**

- fixture with buggy base, fixed working tree, untracked dedicated regression test -> `Verified`;
- same test made to pass on base -> `NotVerified`;
- base intentionally broken before transplant -> `NotVerified`;
- test references new API and base fails compilation -> `BaseIncompatible`;
- high-severity integrity finding blocks `Verified` when configured.

These tests spawn real `cargo` builds and are therefore `#[ignore]`d by default so
that a plain `cargo test` stays offline-capable. Run them with:

```bash
cargo test --workspace --all-features -- --ignored --test-threads=1
```

### PG-002: guaranteed worktree cleanup — DONE

**Goal:** use an explicit RAII guard so worktrees are cleaned on every `Result` path and during future refactors.

**Acceptance:**

- patch-apply failure does not leave `git worktree list` entry;
- runner spawn failure does not leave an entry;
- `--keep-worktree` deliberately retains it and prints exact path.

Implemented as `witdiff_core::git::WorktreeGuard`. `Drop` performs best-effort
removal; `remove()` surfaces an explicit error when the caller can act on it, and
`retain()` hands the path back for `--keep-worktree`.

### PG-004: bounded execution timeout — DONE

**Goal:** a suite that never terminates must not hang WitDiff forever.

**Design:** stdout and stderr are drained on dedicated threads from the moment
the child starts, and only the waiting thread observes the deadline. The obvious
implementation — spawn, wait for exit with a deadline, then read the pipes —
deadlocks: a child that writes more than one pipe buffer (64 KiB) blocks in
`write`, and the parent will not read until the child exits. Measured
empirically: exactly 65536 bytes are delivered before the child parks forever.

**Semantics:** expiry kills the child and returns a result with
`timed_out: true` and `failure_kind: "timeout"`, not a tool error. A hung suite
is a fact about the candidate, so it is reported. `Timeout` is accepted by no
proof path, and a timed-out run is never a success.

**Acceptance:**

- a command that never terminates is killed at its deadline;
- output larger than one pipe buffer does not deadlock the reader;
- a missing program remains an explicit error, not a timeout;
- a timed-out head run yields `head_failed` and never `verified`.

### PG-003: stable human status formatting — DONE

**Goal:** replace debug enum formatting (`VerifiedWithWarnings`) with public snake/kebab strings aligned with receipt values.

Implemented as `VerificationStatus::as_str` and `Severity::as_str` plus `Display`
impls. A unit test asserts the console token is byte-identical to the serialized
receipt token, so the two audiences cannot silently diverge.

## P1 — Rust-aware tests

### PG-101: syntax-aware inline test detection

Status: not started. Detection of likely inline `#[cfg(test)]` edits is still the
line-based heuristic in `GitRepo::inline_test_hints`, which reports a file as a
hint rather than parsing it.

Use a Rust parser to identify changed functions/tests inside `#[cfg(test)]` modules without classifying unrelated production edits as test edits.

**Non-goal:** transplant yet.

### PG-102: inline test transplantation

Safely apply only test-module changes to base while excluding production changes from the same file.

This is difficult; require an ADR before choosing AST rewrite vs patch-hunk reconstruction.

### PG-103: targeted cargo test adapter

Status: done in 1.0, opt-in. `witdiff_core::selection` narrows a cargo test
command to the cargo targets of the changed dedicated tests, gated on
`verification.targeted_test_selection` (default `false`).

Full-suite evidence is preserved, never silently replaced: when the command
cannot be narrowed without changing what cargo runs — notably under
`--all-targets`, which overrides `--test` — the full suite runs and a note
records why. The receipt states which variant produced the evidence via
`test_selection` and `effective_test_command`. See ADR-0008.

## P2 — test integrity

### PG-201: AST assertion-delta analyzer

Status: mostly done in 1.0. Shipped as `witdiff_core::rustanalysis`, producing
`removed_assertion`, `weakened_assertion`, `changed_expected_value`,
`removed_test`, `trivial_assertion`, `ignored_test`, `added_should_panic`,
`test_source_unparsable`, and the informational `unignored_test` /
`empty_test_body`. Every finding carries a rule ID and a line number.

Still open from the original list:

- removed error checks (a distinct rule from removed assertions);
- mock substitution around changed behavior.

The analyzer does not resolve `let` bindings, so an assertion rewritten as
`assert_eq!(compute(), 4)` -> `assert_eq!(v, 4)` is reported as a removal. That
is deliberate and conservative, not an oversight.

### PG-202: policy configuration

Allow organizations to mark integrity rules as block/warn/ignore without changing deterministic observations.

## P3 — changed-code mutation

### PG-301: mutation IR

Define deterministic mutant ID, source span, operator, original/replacement representation.

### PG-302: Rust mutation operators

Initial operators:

- `==` <-> `!=`;
- `<` <-> `<=`, `>` <-> `>=`;
- boolean literal flip;
- condition negation;
- `&&` <-> `||`;
- simple numeric return substitutions.

### PG-303: mutation receipt extension

Introduce `witdiff.receipt.v2` only after v1 remains readable. Do not mutate v1 semantics in-place.

## P4 — adapters

### PG-401: pytest adapter
### PG-402: Vitest/Jest adapter
### PG-403: Go test adapter

Each adapter owns:

- dedicated test discovery;
- failure classification;
- targeted invocation capability;
- framework/environment fingerprint additions.

## P5 — integrations

### PG-501: generic MCP server

Thin protocol wrapper. It may expose `inspect`, `verify`, and `receipt`; it must call `witdiff-core` and must not create an alternative verification implementation.

### PG-502: GitHub check annotations

Map deterministic findings to PR annotations. Avoid a hosted service initially.

### PG-503: signed receipt

Design before implementation: identify what is signed, key management assumptions, and replay semantics.
