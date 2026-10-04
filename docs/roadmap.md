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
- [x] Vitest/Jest (failure classification only)
- [x] test framework capability trait (classification and optional targeted invocation)
- [x] framework-specific failure classification

Verified end to end against real toolchains: pytest, Go and Java repositories
each reach `verified` and report integrity findings, where before ADR-0012 the
same pytest failure classified as `CommandFailure` and could not produce a proof
at all.

Structural integrity analysis exists for **Rust (ADR-0006), Python (ADR-0016),
Go (ADR-0017) and Java (ADR-0018)**. It shares one rule engine
(`witdiff_core::testshape`), so the languages cannot disagree about what a
weakening is.

Still open:

- **JavaScript/TypeScript integrity analysis.** Node ships no parser, so this
  needs its own decision rather than following the Go and Java shape; see the
  support matrix.
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
- [ ] the signature itself, pending the key-management decision ADR-0015 records

CI gating treats "nothing to prove" as distinct from "the proof failed", so a
documentation-only pull request no longer fails the check. The policy lives in
the CLI rather than the workflow file, so CI, MCP and local scripts agree.

Signed receipts are designed but not implemented (ADR-0015). The design found
that a signature over the receipt as currently shaped would attest that a run
happened, not which code was verified: on a clean tree the workspace fingerprint
is exactly SHA-256 of the empty string, so it distinguishes nothing. A content
digest over the verified inputs and a checkable revision binding must land
first; key management and the goal of the attestation are still open.

## M6 — advanced evidence

- [ ] coverage of changed branches as evidence (not as sole proof)
- [ ] dependency/version/environment fingerprint
- [ ] sandboxed verification runner
- [ ] provenance chain across multiple verification stages
- [ ] policy file for organization-specific gates

## Explicitly postponed

- cloud control plane;
- generic agent orchestration;
- AI-generated correctness scores;
- automatic code fixes;
- dashboard before receipt semantics are mature.
