# AGENTS.md — WitDiff engineering contract

This file is the authoritative entry point for coding agents working in this repository. Read it before editing code. Then read `docs/product.md`, `docs/architecture.md`, and `docs/verification-model.md`.

For what is supported per language, and which limitations are deliberate, read `docs/support-matrix.md`. It records that structural integrity analysis works for Rust, Python, Go and Java; that Ruby and JavaScript have classification but no analysis; and that the line-based fallback matches Rust syntax exclusively.

Structural analyzers share one rule engine, `witdiff_core::testshape`. Add a language by supplying a summary and an operator vocabulary, not by copying rules.

## Mission

WitDiff is **deterministic verification infrastructure for AI-written code**. The product must independently establish evidence; it must not ask an LLM whether its own work is correct.

The core v0.1 invariant is:

> A newly changed dedicated regression test is credible only when the configured test command passes on the current workspace and fails for a recognized test reason when the test-only change is transplanted onto the base revision.

## Non-negotiable invariants

1. Never set `Verified` because a model, comment, commit message, or agent transcript says something passed.
2. Never treat a base compile failure as equivalent to a behavioral regression failure unless a future explicit proof mode defines that behavior.
3. Verification evidence is stale if the workspace fingerprint changes during the run.
4. Core verification must remain usable without network access.
5. Agent/vendor-specific integrations must not leak into `witdiff-core`.
6. Prefer observable facts: Git objects, exit codes, test output, diffs, hashes, and tool-produced artifacts.
7. Do not silently ignore unsupported cases. Return a conservative status and a note.
8. Preserve backward compatibility of `witdiff.receipt.v1` unless an ADR explicitly introduces a new schema version.

## Repository layout

```text
crates/witdiff-core   deterministic domain logic and Git/test execution
crates/witdiff-cli    user-facing binary only
docs/                   product, architecture, verification, roadmap, agent guidance
examples/               sample config and output
.github/                 CI and issue templates
```

Do not create a framework-shaped directory tree without an implemented need. New crates require a concrete isolation reason (platform boundary, protocol boundary, or heavy optional dependency).

## Required workflow for every change

1. Read the relevant docs and existing implementation.
2. State the invariant affected by your change in the PR/commit description.
3. Make the smallest coherent change.
4. Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

5. If WitDiff can dogfood the change, also run:

```bash
cargo run -p witdiff -- inspect --base HEAD~1
cargo run -p witdiff -- verify --base HEAD~1
```

6. Update docs and the receipt schema when behavior changes.
7. Do not claim the task is complete if any required command was not actually executed. State exactly what was not run and why.

## Coding rules

- Rust stable; `rust-version` in the workspace is the lower bound.
- No `unsafe` without an ADR plus a safety comment explaining every invariant.
- Avoid shelling through `sh -c`; spawn programs with explicit argument vectors.
- Paths are repository-relative strings in receipts; OS paths stay `Path`/`PathBuf` internally.
- All external process failures need actionable error context.
- New proof decisions require unit tests for positive, negative, and ambiguous cases.
- Prefer enums over booleans when states have semantic meaning.
- Never parse model prose to establish verification state.
- Keep stdout human-readable; `--json` must stay machine-stable.

## What agents should work on next

Read `docs/roadmap.md`. High-value tasks currently are:

1. removed-error-check detection in the structural analyzer;
2. the remaining mutation operators (condition negation, numeric return substitution);
3. structural integrity analysis for Ruby — classification and `init` detection
   are done, and `ripper` ships with the runtime, so it follows ADR-0017's shape;
   and for JavaScript, which needs its own decision because Node ships no
   parser;
4. the signed-receipt prerequisites in ADR-0015: a content digest over the
   verified inputs, then a checkable binding from receipt to revision. Key
   management is an open question and must be settled before implementation;
5. reporting a stale stored receipt (PG-504).

Integration tests, syntax-aware integrity analysis including `match`-arm
comparison, targeted test selection, inline `#[cfg(test)]` transplantation
(ADR-0010), changed-code mutation (ADR-0011), framework-specific failure
classification (ADR-0012), CI gating with annotations (ADR-0013), and the MCP
server (ADR-0014) are done;
see `docs/roadmap.md` for what shipped.

## What not to build yet

- chat UI;
- model provider SDKs in the core;
- generic multi-agent orchestration;
- cloud dashboard;
- opaque “AI confidence score”;
- auto-fix logic before verification is mature;
- telemetry that uploads source code by default.

## Definition of done

A feature is done only when:

- behavior is implemented;
- tests cover success and failure paths;
- errors are explicit;
- CLI/JSON output is documented if changed;
- no unsupported case is silently represented as verified;
- `cargo fmt`, `clippy -D warnings`, and tests pass in a Rust-capable environment.
