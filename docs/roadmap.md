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
- [ ] verify behavior with spaces/non-UTF8 paths where platforms permit
- [ ] improve renamed/deleted test handling
- [x] add bounded execution timeout without risking stdout/stderr pipe deadlock
- [x] normalize status display to stable kebab/snake strings instead of Rust debug output

## M2 — Rust-aware test analysis

- [ ] parse Rust syntax instead of line heuristics
- [ ] detect weakened comparisons, removed match arms, changed expected values
- [ ] support inline `#[cfg(test)] mod tests` transplant safely
- [ ] associate changed tests with test names
- [ ] run only changed tests where equivalence is safe

## M3 — changed-code mutation proof

- [ ] identify changed functions
- [ ] mutation operators: comparison boundary, bool literal, condition negation, logical op, return-value substitutions
- [ ] run changed tests against mutants
- [ ] receipt fields for generated/killed/survived mutants
- [ ] deterministic mutant IDs
- [ ] cache by source/test fingerprint

## M4 — language adapters

- [ ] pytest
- [ ] Vitest/Jest
- [ ] Go test
- [ ] test framework capability trait
- [ ] framework-specific failure classification

## M5 — ecosystem integrations

- [ ] GitHub Actions reusable workflow
- [ ] PR annotations/check summary
- [ ] universal agent skill/instruction package
- [ ] MCP server wrapping core methods
- [ ] signed/attested receipts

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
