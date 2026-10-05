# Roadmap

The roadmap is organized around proof quality, not feature count.

## M0 — repository foundation

- [x] Rust workspace and CLI
- [x] config loading
- [x] Git repository/base discovery
- [x] changed-file classification
- [x] dedicated test classification
- [x] JSON receipt model
- [x] agent documentation

## M1 — red/green proof (implemented in starter)

- [x] run HEAD test command
- [x] create detached base worktree
- [x] transplant tracked dedicated tests
- [x] copy untracked dedicated tests
- [x] run pristine-base control command
- [x] run identical command on base+changed-tests
- [x] distinguish test failure vs compile failure
- [x] remove worktree
- [x] workspace fingerprint freshness
- [x] basic integrity findings
- [x] strict/non-strict exit behavior

### M1 hardening — first agent tasks

- [x] add Git inspection integration test using a temporary repository
- [x] add full red/green end-to-end integration tests using a deterministic fixture runner
- [x] ensure worktree cleanup also occurs if patch application or test execution fails
- [x] verify behavior with spaces/non-UTF8 paths where platforms permit
- [x] improve renamed/deleted test handling
- [x] add bounded execution timeout without risking stdout/stderr pipe deadlock
- [x] normalize status display to stable kebab/snake strings instead of Rust debug output

## M2 — Rust-aware test analysis

- [x] parse Rust syntax instead of line heuristics
- [x] detect weakened comparisons, changed expected values, removed match arms
- [x] associate changed tests with test names
- [x] run only changed tests where equivalence is safe
- [x] support inline `#[cfg(test)] mod tests` transplant safely (ADR-0010)

See ADR-0006 (structural analysis), ADR-0007 (test-only transplant boundary),
ADR-0008 (targeted selection refuses rather than approximates), ADR-0009
(NUL-delimited Git paths), and ADR-0010 (inline test transplant).

## M3 — changed-code mutation proof

- [x] identify changed functions
- [x] mutation operators: equality flip, comparison boundary, logical flip, boolean literal flip, condition negation, numeric substitution
- [x] run changed tests against mutants
- [x] receipt fields for generated/killed/survived/not_compiled/timeout/skipped mutants
- [x] deterministic mutant IDs
- [x] cache by source/test fingerprint

All six planned operators are implemented. Mutation is opt-in
(`verification.mutation`) and supplementary: it never changes `status`. See
ADR-0011.

## M4 — language adapters

- [x] pytest (failure classification and structural integrity analysis)
- [x] Go test (failure classification and structural integrity analysis)
- [x] Java / JUnit (failure classification and structural integrity analysis)
- [x] Ruby / Minitest / RSpec (failure classification and structural integrity analysis)
- [x] Vitest/Jest (failure classification and structural integrity analysis)
- [x] test framework capability trait (classification and optional targeted invocation)
- [x] framework-specific failure classification

Verified end to end against real toolchains: pytest, Go, Java, Ruby/RSpec and
JavaScript/TypeScript repositories each reach `verified` and report integrity
findings, where before ADR-0012 the same pytest failure classified as
`CommandFailure` and could not produce a proof at all.

Structural integrity analysis exists for **Rust (ADR-0006), Python (ADR-0016),
Go (ADR-0017), Java (ADR-0018), Ruby (ADR-0020) and JavaScript/TypeScript
(ADR-0021)**. All share one rule engine (`witdiff_core::testshape`), so no
language can disagree about what a weakening is.

Every language above is now verified against its **real** parser or runner, not
a hand-written fixture. Doing so found nine bugs — six in JavaScript, two in
Ruby's classifier, and one in the shared rule engine that had affected every
language using it.

`verify_languages_end_to_end.rs` additionally drives the whole red/green chain
against real temporary repositories for **all five**: pytest, Go, Ruby/RSpec,
JavaScript/TypeScript and Java. Java reaches `VerifiedWithWarnings` rather than
`Verified` because a script-shaped test command yields no Java toolchain, so
structural analysis is unavailable — which the receipt reports. See
`docs/support-matrix.md`.

Still open:

- **Targeted invocation** for non-Rust frameworks, so the full suite runs and
  the receipt says `full_suite`.
- **Mutation** outside Rust; the operators are defined over Rust syntax.

See `docs/support-matrix.md` for the per-language detail, including what Java,
.NET, PHP, Ruby and TypeScript would each require.

## M5 — ecosystem integrations

- [x] GitHub Actions reusable workflow (`.github/workflows/witdiff.yml`)
- [x] PR annotations/check summary (`--github-annotations`; see ADR-0013)
- [x] universal agent skill/instruction package (`examples/agent-instruction.txt`)
- [x] MCP server wrapping core methods (`crates/witdiff-mcp`, ADR-0014)
- [x] content digest and revision binding for receipts (ADR-0015 prerequisites)
- [x] signature over the verification digest (ADR-0022)

CI gating treats "nothing to prove" as distinct from "the proof failed", so a
documentation-only pull request no longer fails the check. The policy lives in
the CLI rather than the workflow file, so CI, MCP and local scripts agree.

Signed receipts are implemented (ADR-0022, superseding the ADR-0015 design). The
signature is a detached Ed25519 signature over the domain separator, the status
and the content digest, computed by `ed25519-dalek` in Rust — no external
runtime. It is tamper-evidence, not non-repudiation, and the operator supplies
the key; WitDiff never creates or stores one.

## M6 — advanced evidence

- [ ] coverage of changed branches as evidence (not as sole proof)
- [x] dependency/version/environment fingerprint
- [x] sandboxed verification runner
- [x] provenance chain across multiple verification stages
- [x] policy file for organization-specific gates

Shipped so far in M6:

- **Environment evidence** — the receipt records the configured test program, its
  version, the toolchain that participates in it, and a digest of every
  dependency manifest present. It answers *where* the evidence came from, which
  the digest deliberately does not, and it is kept out of the digest so that
  upgrading Python cannot make an older receipt report itself stale.
- **Gate policy** — `[gate]` in `witdiff.toml` commits which results pass, so a
  repository states it once rather than restating flags on every invocation.
  Flags may only tighten it.
- **Provenance chain** — `.witdiff/provenance.json` links each receipt's digest
  to the one before it, so a sequence of verifications can be checked as a
  sequence rather than one at a time. Editing, dropping or reordering an entry
  breaks the links that follow, and both `verify` and `receipt` report it.
  Verified against a real tampering: rewriting a recorded digest is caught.
- **Sandboxed runner** — `sandbox_image` rewrites every run into a named
  container, so the candidate-controlled `test_command` no longer executes on the
  host. Named so a timeout can remove it rather than leaving it running; an
  allow-listed environment; `--network none` on request. The image must carry the
  toolchain, because which one a project needs cannot be inferred.


## Explicitly postponed

- cloud control plane;
- generic agent orchestration;
- AI-generated correctness scores;
- automatic code fixes;
- dashboard before receipt semantics are mature.
