# AGENTS.md — WitDiff engineering contract

This file is the authoritative entry point for coding agents working in this repository. Read it before editing code. Then read `docs/product.md`, `docs/architecture.md`, and `docs/verification-model.md`.

For what is supported per language, and which limitations are deliberate, read `docs/support-matrix.md`. It records that structural integrity analysis works for Rust, Python, Go, Java, Ruby (Minitest and RSpec) and JavaScript/TypeScript; that JavaScript needs a parser in the project because Node ships none; and that the line-based fallback matches Rust syntax exclusively.

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

Read `docs/roadmap.md`. Every roadmap milestone is ticked; what is left is
feature scope rather than defect:

1. **Targeted test invocation outside Rust.** Cargo's `--test <target>` has a
   real implementation (ADR-0008); every other framework runs the full suite and
   the receipt says `full_suite`. Narrowing must be provably equivalent to what
   the framework would otherwise run, or it must refuse — ADR-0008 is the
   precedent.
2. **Mutation outside Rust.** The operators are defined over Rust syntax
   (ADR-0011). Other languages get red/green proof and structural analysis, but
   no mutation signal.
3. **Integrity-rule policy (PG-202).** Organizations marking an individual rule
   block/warn/ignore. Distinct from `[gate]`, which decides which *statuses*
   pass; this would decide how a *finding* is weighted. It must change only how
   an observation is reported, never the observation itself.
4. **Mock substitution around changed behaviour**, the last item open from the
   original integrity list.

## What has already been established

The three verification gaps recorded in `docs/support-matrix.md` were closed by
installing the real dependencies — `acorn`, `typescript`, a live RSpec — and
running the analyzers against them. That found nine bugs: six in JavaScript, two
in Ruby's classifier, and one in the shared rule engine (`testshape`) that had
affected every language using it:

- JavaScript normalized no `Literal` node, so `toBe(2)` and `toBe(3)` rendered
  identically and **no expectation change was ever reported**;
- `.not.toBe` produced no assertion at all, so inverting one went unreported;
- `test.skip`, `test.only` and `test.each` produced no test function;
- Node's `assert` module was invisible;
- every `.ts`/`.tsx` file either crashed or analyzed as empty;
- Ruby classified a single-example red suite as an unrecognized command
  failure, and a suite that failed to load as a behavioural regression;
- `testshape` compared only the right-hand side of an assertion, so
  `Eq 1` → `NotEq 1` — inverting an assertion — reported nothing.

Signing runs entirely in Rust through `ed25519-dalek` (ADR-0022), so no
JavaScript runtime is needed to sign or verify a receipt.

**The lesson worth carrying:** a hand-written output fixture can only assert what
somebody already thought to write down. It cannot show you that real RSpec
prints `1 example, 1 failure` in the singular, or that a real parser emits
`Literal` where the code expected `NumericLiteral`. **Write the test, run it
against the real tool, and believe the output over the code.**

CI runs the `--ignored` suite with every toolchain installed, so this work cannot
rot silently. Four things had to be fixed for it to run there at all, all found
by running it rather than by reading it:

- the reusable workflow's exit code was never captured, because GitHub runs an
  unspecified shell as `bash -e` and a non-zero verify aborted the step first —
  so a failed gate was reported as a tool malfunction;
- a run whose only movement was build output failed the gate while the receipt
  called those paths irrelevant to the verification;
- the JavaScript tests need `npm ci`, or they skip;
- the Java fixture needs a JUnit 5 runtime, and a JUnit classpath whose
  versions agree. Mixed versions print `0 tests found` and exit 0.

The Java fixture also found something worth remembering: **mixing JUnit
platform versions fails silently.** A `1.14.4` platform with a `6.0.1` Jupiter
engine compiles, runs, prints `0 tests found` and exits 0 — a green run that
tested nothing. Version-align any JUnit classpath you assemble.

Anything not listed above — integration tests, syntax-aware integrity analysis
including `match`-arm comparison, targeted test selection, inline
`#[cfg(test)]` transplantation (ADR-0010), changed-code mutation (ADR-0011),
framework-specific failure classification (ADR-0012), CI gating with
annotations (ADR-0013), the MCP server (ADR-0014), coverage, provenance,
sandboxing and the gate policy — is done. See `docs/roadmap.md`.

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
