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
removed assertions, changed expected values, weakened assertions, deleted
tests, and removed `match` arms.

`match` arms are compared as a multiset of `(pattern, guard)` pairs,
grouped by normalized scrutinee rather than paired expression by
expression. Grouping is what makes the comparison robust to a `match`
being split, merged, or reordered, while still catching any arm that no
longer exists anywhere on the same scrutinee. Collapsing specific arms
into a wildcard is reported, because the specific arms are then absent.
A guard is part of an arm's identity: dropping one makes the arm apply
to more inputs, so the guarded form counts as removed.

A `match` that disappeared entirely is deliberately not reported. A
`match` rewritten as an `if`/`else` chain is a common refactor, and any
assertion inside the removed arms is already caught by the assertion
rules; reporting the structural removal too would turn a faithful
refactor into a false positive.

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
- Weakening comparisons consume identical assertions before pairing by
  subject. Without that ordering, a newly added assertion whose subject
  collides with an existing one — a second `match` arm asserting a
  different expected value over the same expression — would be reported as
  if the existing assertion had been rewritten. The ordering was found by
  writing the added-arm test, not by inspection.

## Alternatives considered

- **Keep line heuristics and tune them.** Rejected: the failure mode is
  unrepresentable noise, which no amount of tuning fixes, because the input
  lacks the information needed.
- **Require agents to supply test names.** Rejected: the names would be
  self-reported, and the entire point of WitDiff is that agent testimony is not
  evidence.