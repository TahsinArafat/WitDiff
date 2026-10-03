# ADR-0011: Changed-code mutation as a supplementary signal, never a proof

Status: accepted

## Context

M3 of the roadmap adds changed-code mutation testing. The idea is to answer a
question the red/green experiment cannot: a transplanted test can fail on the
base revision for the *right* reason yet still barely constrain the changed
code. A test that asserts one example of a function's behavior may be killed by
almost any mutation, or by none, depending on how tightly it pins the contract.

Mutation testing measures that tightness. WitDiff mutates the changed production
code, runs the suite against each mutant, and records whether the tests noticed.

That power brings a specific danger. Mutation results are easy to state as a
score, and a score reads like a proof. It is not one, and treating it as one
would break the project's central invariant: verification rests on observable
red/green behavior, not on a heuristic number.

## Evidence

The mechanics were exercised against real crates before any of this was
designed, and three findings shaped the decision.

### A killed mutant is only a kill on a test failure

Observed: a mutant flipping `value % 2 == 0` to `value % 2 != 0` produced a test
assertion failure, while a mutant introducing a syntax error (`value >>> 0`)
produced `could not compile`. Those are different facts. A mutant that does not
compile was never executed, so it says nothing about whether the tests constrain
behavior. Counting it as "killed" would inflate the signal with mutants that
were never observably wrong.

### A surviving mutant is real evidence of a weak test

Observed: mutating `value % 2 == 0` to `value % 2 < 1` left a test that asserted
only `is_even(2)` passing. The test does not constrain that operator at all. This
is the signal worth surfacing, and it is invisible to every other rule WitDiff
has.

### Mutation must never touch test code

Observed, and initially mistaken for a false negative: a naive text replacement
of a string literal rewrote the *test's own* expected value alongside the
production literal, so the assertion still matched and the mutant appeared to
survive. Mutating the oracle changes the question being asked. A mutant that
edits test code is not a mutant at all; it is a different test.

### The first real run found a gap in WitDiff's own code

Run against WitDiff itself, the first mutation analysis generated 25 mutants
from the changed production lines and surfaced one survivor: flipping
`mutation: false` to `true` in `VerificationConfig::default` broke no test, even
though ADR-0011, the README and the example configuration all state that
mutation is off by default.

That is precisely the class of gap mutation exists to find — a documented
invariant with nothing pinning it. The fix was a test asserting the documented
defaults, which now kills the mutant. It is recorded here because it is the
strongest available evidence that the signal is real rather than theoretical.

## Decision

Mutation is an **opt-in, supplementary report**. It never changes
`status`, never contributes to `red_green_proven`, and cannot produce
`verified`. A run with mutation enabled that finds surviving mutants still
reports the same red/green status it would have reported without them, plus a
separate mutation summary.

Scope is the changed production code only:

1. Mutants are generated from the *head* revision of files classified as
   production, restricted to functions the diff actually changed.
2. A mutant is applied by rewriting a byte span in the head source, so exactly
   one operator instance changes and the author's formatting is preserved.
3. Mutation is confined to the changed function bodies. Editing a test's own
   code is made impossible by construction: test modules are never part of a
   mutation span (ADR-0006 already locates them; ADR-0010 already handles inline
   ones).

Each mutant is classified into exactly one of:

- **killed** — the suite failed with `FailureKind::TestFailure`;
- **survived** — the suite passed;
- **not_compiled** — the suite failed to build, so the mutant was never
  executed;
- **timeout** — the deadline was exceeded; the mutant is unclassified, and a
  suite that does not terminate has demonstrated nothing;
- **skipped** — a bound was reached before this mutant ran.

`not_compiled`, `timeout` and `skipped` are *not* successes and are never
counted as kills. The receipt reports all five counts, so a consumer can see how
much of the mutant set was actually decided.

Bounds are mandatory and explicit, because mutation cost is unbounded:

- a maximum number of mutants per run;
- a maximum number of mutants per changed function;
- the existing per-run timeout applies to each mutant run;
- deterministic ordering (by path, then byte offset), so a truncated run
  truncates the same way every time.

A mutant carries a deterministic ID derived from the file path, the byte span
and the operator. The same revision pair therefore produces the same IDs, which
makes a result comparable across runs without storing state.

## Consequences

- The receipt gains a mutation section. It is additive and optional, so
  `witdiff.receipt.v1` remains readable and a receipt without mutation still
  parses (invariant 8). PG-303 predicted a v2; that is not needed, because
  nothing about existing v1 semantics changes.
- Every mutant costs a full test run. This is why the feature is opt-in and
  bounded by default rather than always on.
- A surviving mutant is a *question*, not a verdict. The finding names the
  operator, the span and the original/replacement text so a human or agent can
  judge whether the test should have caught it. Some mutants are equivalent to
  the original program and can never be killed by any test; WitDiff does not
  attempt equivalence detection and says so rather than guessing.
- Caching by source and test fingerprint is required rather than optional. A
  mutant's outcome is determined by the mutated source, the test code and the
  test command; those three fingerprints are the cache key, so a re-run against
  an unchanged revision pair does not repeat the suite. Without this the feature
  is too slow to be used, and a slow feature that is skipped is worse than one
  that is absent.
- Mutation depends on the head suite passing. If the head run fails, or the
  pristine-base control fails, no mutants are generated: a baseline that is not
  green cannot distinguish a killed mutant from a pre-existing failure.

## Alternatives considered

- **Report a mutation score as the headline.** Rejected: it invites reading a
  percentage as a verdict. The receipt reports counts and per-mutant detail so a
  consumer can reason about what was decided.
- **Treat `not_compiled` as killed.** Rejected on measurement: it inflates the
  kill count with mutants that were never executed.
- **Text search-and-replace for mutation.** Rejected on measurement: it rewrote
  a test's own literal and produced a mutant that appeared to survive. Byte
  spans chosen from a parsed tree are what make "production only" enforceable.
- **Mutate the whole changed file rather than changed functions.** Rejected:
  cost grows with file size rather than with the change, and an unrelated
  function's mutants say nothing about this change.
- **Always-on mutation.** Rejected: the cost is a full suite run per mutant,
  which makes the default verification path unusably slow.
- **A separate `witdiff-mutate` crate now.** Deferred, not rejected. The
  architecture note allows a new crate when a stable boundary appears. The
  operators and the runner are not yet stable enough to justify the split, and
  splitting early would freeze an interface before it has been exercised.
