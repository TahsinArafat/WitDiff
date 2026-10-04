# Architecture decisions

This file is a lightweight index. Add formal ADRs under `docs/adr/` when decisions become costly to reverse.

- ADR-0001: deterministic core before AI integrations
- ADR-0002: Git worktrees for base experiments
- ADR-0003: compile failure is not behavioral red evidence in v0.1
- ADR-0004: CLI + JSON is the compatibility boundary for coding agents
- ADR-0005: the receipt schema identifier is witdiff.receipt.v1
- ADR-0006: Rust test changes are compared with a real parser, not diff lines
- ADR-0007: the transplant contains test changes only; ineligible files are reported
- ADR-0008: targeted test selection refuses rather than approximating a narrow run
- ADR-0009: Git path output is read NUL-delimited and decoding loss is recorded
- ADR-0010: inline `#[cfg(test)]` tests are transplanted by span splicing, or not
  at all
- ADR-0011: mutation is a supplementary signal, never a proof
- ADR-0012: failure classification is framework-specific and explicitly selected
- ADR-0013: CI gating distinguishes "nothing to prove" from "proof failed"
- ADR-0014: the MCP server is a hand-written stdio adapter, not an SDK client
- ADR-0015: a signed receipt can attest a run, not a revision (proposed; not
  implemented)
- ADR-0016: Python integrity analysis uses the interpreter's own AST
- ADR-0017: Go integrity analysis uses `go/ast`, behind a shared rule engine
- ADR-0018: Java integrity analysis uses the JDK's parser, swapping JUnit's
  argument order at the boundary
- ADR-0019: a missing toolchain is reported, and installed only from the
  project's own pin
