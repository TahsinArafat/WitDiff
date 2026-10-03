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

Status: done. ADR-0010 implements span splicing: `syn` locates the
`#[cfg(test)]` module spans in the head revision and those spans replace the
corresponding base spans, so only the test module's bytes move. Any precondition
failure falls back to a reported refusal rather than a partial splice.

`git` hunk filtering was rejected on measured grounds: hunks are formed by
proximity, so an adjacent production and test edit share one hunk and cannot be
separated.

The receipt records `spliced_inline_tests` and `refused_inline_tests`, so a
reader can tell what was transplanted and why anything was not.

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

Status: done (ADR-0011). `witdiff_core::mutation` defines the mutant as a byte
span plus an operator, with a deterministic ID derived from path, span and
operator so the same revision pair yields the same IDs across runs.

### PG-302: Rust mutation operators

Status: mostly done. Shipped: equality flip, comparison boundary, logical flip,
boolean literal flip. Still open: condition negation and simple numeric
return-value substitutions.

Mutants are confined to changed lines and to code outside every `#[cfg(test)]`
module, enforced from the parsed tree rather than by text matching.

### PG-303: mutation receipt extension

Status: done without a v2. The roadmap predicted `witdiff.receipt.v2`, but no v1
semantics needed to change: the mutation report is a new optional field, so
`witdiff.receipt.v1` stays readable and a receipt written before the field
existed still parses.

## P4 — adapters

### PG-401/402/403: pytest, Vitest/Jest, Go adapters

Status: failure classification done (ADR-0012) and Python structural integrity
analysis done (ADR-0016). Targeted invocation not started.

The backlog described each adapter as owning discovery, failure classification,
targeted invocation and fingerprint additions. Only classification was built,
deliberately: discovery is already expressed by `test_globs` and
`extra_test_paths`, which are language-neutral, and adding it to the adapter
trait would duplicate a working mechanism behind a worse interface.

Implemented: `witdiff_core::framework` classifies cargo, pytest, Jest/Vitest and
Go output, selected explicitly via `verification.framework`. Before this, three
of four frameworks misclassified a genuine test failure as `CommandFailure`,
which cannot produce a proof.

Still open:

- targeted invocation for non-Rust frameworks, so a narrowed run is possible
  where the framework supports it;
- structure-aware test-integrity analysis for JavaScript, which now exists for
  Rust (ADR-0006), Python (ADR-0016) and Go (ADR-0017). JavaScript needs its own
  decision because Node ships no parser, unlike the other three runtimes;
- mutation operators (ADR-0011) for other languages.

Until those exist, a non-Rust repository gets a red/green proof and a
line-oriented integrity fallback rather than structural analysis.

## P5 — integrations

### PG-500: CI gating and annotations

Status: done (ADR-0013). `--github-annotations` emits workflow commands for the
status and every finding, with escaping done in the CLI so no consumer
re-implements it. `.github/workflows/witdiff.yml` is a reusable workflow.

The gate policy distinguishes `no_changed_tests` (nothing to prove) from a
failed proof. `--strict` keeps its meaning; `--fail-on-no-changed-tests`
restores the previous fail-on-everything behavior.

### PG-501: generic MCP server

Status: done (ADR-0014). `crates/witdiff-mcp` serves `witdiff_inspect`,
`witdiff_verify` and `witdiff_receipt` over newline-delimited JSON-RPC 2.0 on
stdio, calling `witdiff-core` directly. It returns the receipt unchanged rather
than re-rendering it, and takes its gate verdict from `VerificationStatus::gate`,
so MCP cannot disagree with the CLI or CI.

The official Rust SDK (`rmcp`) was evaluated and rejected: version 3 requires
rustc 1.88 against this workspace's 1.78, so cargo silently substitutes version
2, and it pulls 68 packages including an async runtime. It also could not be
compiled in the development environment, and shipping a dependency whose
behavior was never observed is the failure this project exists to detect.

Implemented scope: `initialize`, `tools/list`, `tools/call`, `ping`. Everything
else returns `Method not found` rather than silence, so a client is told
plainly. Tool failures are results with `isError: true`, never protocol errors,
because a protocol error is invisible to the model.

### PG-502: GitHub check annotations

Map deterministic findings to PR annotations. Avoid a hosted service initially.

### PG-503: signed receipt

Status: designed, deliberately not implemented (ADR-0015).

The design work found that a signature over the receipt as currently shaped
would attest almost nothing. Two prerequisites must exist first:

- **PG-503a: a content digest over the verified inputs** (base and head
  revisions, effective test command, changed test contents, transplanted inline
  modules). Distinct from `workspace_fingerprint`, which is a staleness check
  and not a content hash — on a clean tree it is exactly SHA-256 of the empty
  string, so it does not distinguish one clean tree from another.
- **PG-503b: a checkable binding from receipt to revision**, including for
  uncommitted work, where `head_commit` alone is insufficient.

Key management and the goal of the attestation (tamper-evidence vs
non-repudiation) are open questions that must be decided before implementation.

### PG-504: report when a stored receipt is stale

`witdiff receipt` prints a stored receipt without checking whether it still
describes the working state. Measured: a receipt claiming head `8f4e5f2e` was
printed unchanged while the actual head was `62ec3f5`, with no warning.

This is independent of signing and is a straightforward usability defect. A
reader has to notice the mismatch themselves, and nothing in the output
encourages them to look. The fix is to compare the receipt's `head_commit` and
workspace digest against the current state and report divergence explicitly.
