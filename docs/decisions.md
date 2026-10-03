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
