# ADR-0007: The transplant must contain test changes only

Status: accepted

## Context

The red/green experiment works by copying the changed test files onto a detached
worktree at the base revision and running the suite there. The proof is only
meaningful if that copy contains *tests* and nothing else.

Classification is by path glob, so a file counts as a test if its path matches a
test glob. Renames previously contributed both the new and the old path to the
transplant. That is correct for a test renamed to another test location, and
wrong for production code renamed into a test directory: the old production path
was then passed to `git diff` as a pathspec, so the production file's own
changes were transplanted too. The base worktree would receive the fix, the
failure would no longer be attributable to the test, and the receipt would
describe a proof that was never performed.

A second, narrower case is a path that cannot be decoded as UTF-8. Its lossy
form is still a `String`, but it names a file that does not exist, so writing to
it would create a stray file rather than transplant anything.

## Decision

Two conditions make a changed test ineligible for transplant:

1. it was renamed or copied from a path that was not itself a test
   (`previous_is_test == false`), or
2. its path did not survive a UTF-8 round trip (`path_is_lossy == true`).

Ineligible files are excluded from the transplant, recorded by name in the
receipt `notes`, and — when red/green behavior was otherwise observed — the
result is downgraded to `not_verified`. A partial transplant is evidence about
the files it contained, not about the change as a whole.

## Consequences

- The base experiment can no longer be made to pass by moving the fix into the
  transplant.
- A repository that legitimately moves production code into a test directory
  gets a conservative result and an explanation rather than a false proof. The
  operator can then split the change so the production move and the test are
  separate commits.
- `changed_files` gained two additive fields. Both are optional and default to
  the conservative value, so receipts written before this ADR still parse.

## Alternatives considered

- **Transplant production renames and let the red result stand.** Rejected:
  that is precisely the "verified" claim the project exists to distrust.
- **Fail the whole run instead of downgrading.** Rejected: refusing to produce a
  receipt loses the surrounding evidence, which is exactly what a reviewer needs
  in order to decide what to do.