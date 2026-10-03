# ADR-0010: Inline `#[cfg(test)]` test transplantation by span splicing

Status: accepted

## Context

The red/green experiment copies changed *dedicated* test files (files matching a
test glob, normally under `tests/`) onto a detached base worktree. Files outside
those globs are classified as production code and are never transplanted.

That leaves the most common Rust test layout unsupported. A large share of Rust
tests live in a `#[cfg(test)] mod tests` block inside the same file as the code
they exercise. When such a file changes, WitDiff currently:

1. reports it as a production file,
2. emits the note `possible inline Rust tests changed inside production files`,
3. and reaches `no_changed_tests` — no proof is attempted at all.

The note is honest, and ADR-0007's rule is respected. But the effect is that
WitDiff cannot verify the most idiomatic Rust test change there is, and
`no_changed_tests` is easy to read as "nothing to check" rather than "WitDiff
declined to check".

The transplant is built as `git diff <base> -- <paths>`, which selects whole
files. The question this ADR settles is how to transplant *part* of a file.

## Evidence

Three approaches were implemented against small repositories and observed before
choosing. The findings below are measurements, not expectations.

### Path filtering cannot work

`git apply --include=<pattern>` filters by **path**, not by hunk. Given a file
with both a production change and a test change, `--include` on that path
applies every hunk in it. This was confirmed directly: the production line
`fn prod() -> i32 { 2 }` landed in the worktree alongside the test change.

### Hunk filtering works only when the two edits are far apart

Splitting a patch and keeping only the hunk containing the test change works
when the edits are separated by more than the context window, and `git apply`
relocates the hunk correctly even when the dropped hunk would have shifted line
numbers, because it matches on context rather than trusting the `@@` offsets.

It breaks in two ways that cannot be fixed by better patch parsing:

1. **Hunk boundaries are not semantic.** They are formed by proximity. A
   production edit and a test edit within one context window (3 lines by
   default) land in the *same* hunk. Measured directly: a one-line production
   change immediately above a `#[cfg(test)] mod tests` produced a single hunk
   containing both changes. Keeping the hunk transplants the production fix;
   dropping it discards the test change. There is no third option.
2. **A hunk can contain a deletion that spans both.** When a production change
   removes lines above the test module, the merged hunk's removed-line block
   includes production content, so filtering on "does this hunk touch the test
   module" would still drag production deletions in.

Those two cases are exactly the ones that matter, because a test and the code it
tests are usually adjacent within a file.

### Span splicing works

Parsing the head revision with `syn` (already a dependency for ADR-0006), taking
the byte span of each `#[cfg(test)] mod tests` item, and replacing the
corresponding span in the base revision's source produces the intended file:
base production code plus head test module. Verified by construction and by
running the result:

- The spliced file compiled and the transplanted test **failed at runtime**
  (`assertion left == right failed`) while the base production function was
  still the unchanged one. That is a genuine red result rather than a compile
  error, which is the distinction ADR-0003 exists to preserve.
- Splicing preserves the author's original formatting, because it moves the
  head revision's own bytes rather than re-printing a parsed tree.

## Decision

Transplant inline tests by **span splicing**, and only when every precondition
below holds. When any precondition fails, fall back to today's behavior — the
note plus `no_changed_tests` — rather than attempting a partial transplant.

Preconditions for splicing a file:

1. Both the base and head revisions parse as Rust. If either does not, the file
   is not spliced (and ADR-0006's `test_source_unparsable` handling applies).
2. The base revision contains an item with the same identity as the head
   revision's test module — for example `mod tests` at the same module path.
   A test module that is *new* in head has nothing to splice onto.
3. Every `#[test]` function that changed, appeared, or disappeared in head lies
   inside a spliced test-module span. A change to a test outside any
   `#[cfg(test)]` module means the file is not purely an inline-test change.
4. The file's changed regions are confined to spliced spans. If the diff for
   the file touches code outside every test-module span, the file also carries
   production changes and must not be spliced as test-only.

The resulting file is written to the base worktree in place of the base file,
and the proof proceeds exactly as for a dedicated test file: the same test
command, the same failure classification, the same `verified` /
`base_incompatible` / `head_failed` rules. The transplant stays test-only; only
the *mechanism* for producing it is new.

## Consequences

- Inline `#[cfg(test)]` tests become verifiable, which is the remaining M2 gap.
- The receipt must say that a file was spliced rather than copied, because a
  reviewer reading `changed_files` cannot otherwise tell why a production file
  was involved in the transplant. A new additive field is required; the
  existing `is_test` classification is not changed, because the file is still
  production code in every other respect.
- Files that fail a precondition keep today's conservative outcome and note. The
  note should name the precondition that failed, so the operator can tell a
  genuine "too tangled to separate" from "WitDiff did not try".
- `syn` parsing is already paid for by ADR-0006, so this adds a code path, not a
  dependency.
- Splicing produces source that is not, in general, `rustfmt`-clean, because the
  base and head regions were formatted independently. This does not affect
  verification (compilation and test outcome do not depend on formatting), but
  it does mean a retained worktree inspected by a human may not look formatted.
  WitDiff must not run `rustfmt` to fix it: that would be an unrecorded
  transformation of the evidence, and it would make the result depend on a
  formatter version.
- The base worktree can now contain a file that differs from both revisions.
  That was already true of renaming and copying; this widens it, and it is the
  reason the receipt records the mechanism.

## Alternatives considered

- **Hunk filtering.** Rejected on measured grounds: adjacent production and test
  edits share a hunk, and that adjacency is the normal case rather than an edge
  case. It would silently transplant production changes in exactly the scenario
  the feature exists to serve — the ADR-0007 failure mode, reintroduced through
  a different door.
- **Whole-file transplant of any file containing `#[cfg(test)]`.** Rejected:
  this transplants production changes by construction and would make the
  experiment prove nothing.
- **AST rewrite plus re-emission.** Rejected as the primary mechanism. Printing
  the parsed tree discards the author's formatting and requires either an
  unrecorded reformat pass or a `prettyplease`-style dependency. Span splicing
  achieves the same semantic result while moving the original bytes. The AST is
  still used, but for *locating* spans rather than for rewriting.
- **Keep the note and stay unsupported.** Rejected as the end state: it leaves
  the dominant Rust test layout unverifiable. It remains the correct *fallback*
  whenever a precondition fails.
- **Require tests to live in `tests/`.** Rejected: it constrains how users write
  Rust to suit the tool, and inline tests need access to private items, so the
  constraint is not merely stylistic.
