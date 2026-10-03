# ADR-0006: Syntax-aware analysis of Rust test changes

Status: accepted

## Context

Test-integrity rules were originally line-oriented: they read a unified diff and
inspected each added or removed line for substrings such as `assert_eq!`.

That approach cannot answer the question the rules exist to ask. An assertion
rewritten across four lines is reported as a removed assertion plus an added
one. An assertion moved to the top of its function is reported as removed and
added. A changed expectation is indistinguishable from any other edit, because
the diff records only that *a* line changed, never what changed *in* it. The
net effect is a rule set that both misses real weakening and reports noise on
harmless reformatting, which trains operators to ignore it.

## Decision

Rust test files are parsed with `syn` and compared structurally between the base
and head revisions. The comparison is over a normalized shape: which functions
carry `#[test]`, which attributes they carry, and which assertions each body
contains, with each assertion reduced to its macro name and whitespace-
normalized argument text.

This yields rules that are statements about code rather than about diffs:
removed assertions, changed expected values, weakened assertions, deleted tests.

## Consequences

- Detection depends on the parsed structure, so reformatting and reordering no
  longer produce findings. This is verified end-to-end, not only in unit tests.
- `syn` becomes a build dependency of `witdiff-core`. It is a compile-time
  dependency only; core verification still requires no network access.
- Undecidable cases resolve toward reporting, not silence. WitDiff does not
  resolve `let` bindings, so `assert_eq!(compute(), 4)` becoming
  `assert_eq!(v, 4)` is reported as a removal. The analyzer cannot prove the
  assertion merely moved, and a false removal is reviewed by a human or agent
  while a missed removal would be a false pass.
- A file that cannot be parsed yields an explicit `test_source_unparsable`
  finding and falls back to the line-oriented rules. It is never reported as
  clean.

## Alternatives considered

- **Keep line heuristics and tune them.** Rejected: the failure mode is
  unrepresentable noise, which no amount of tuning fixes, because the input
  lacks the information needed.
- **Require agents to supply test names.** Rejected: the names would be
  self-reported, and the entire point of WitDiff is that agent testimony is not
  evidence.