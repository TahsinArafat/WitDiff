# ADR-0017: Go integrity analysis via `go/ast`, and a shared comparison engine

Status: accepted

## Context

ADR-0016 closed the integrity gap for Python. The support matrix recorded Go
next: measured, a Go test whose guard was deleted, or whose expectation was
inverted, produced no finding at all.

Go is a different shape than Python. There is no assertion keyword. A test fails
by calling `t.Errorf`, `t.Fatalf`, `t.Error` or `t.Fatal`, and the condition
that matters is the `if` that guards the call:

```go
if got != want {
    t.Errorf("got %d, want %d", got, want)
}
```

Comparing the failure *message* would be wrong in both directions: rewording a
message would be reported as a changed expectation, and an inverted condition
would be missed entirely because the message text is unchanged.

## Evidence

- **`go/ast` ships with the toolchain.** No dependency is needed on either
  side, the same property that made Python tractable.
- **`go run` refuses `_test.go` files.** Verified: `go run summarize.go
  sample_test.go` fails with "cannot run *_test.go files". `go/parser` itself
  accepts any filename, so the target is copied to `target.txt` in a temporary
  directory.
- **`go run` requires all named files in one directory.** Verified: naming files
  from two directories fails. The script and target are therefore written side
  by side in a temporary directory, and `go run` completes in about 0.4s.
- **The guard is recoverable.** Verified against a real file: the summarizer
  reports `Add(1, 1) != 2` for the guard, not the message text.

## Decision

1. **Analyze Go through `go run`** over an embedded script, mirroring Python.
   The toolchain is discovered from the project's own test command, so a
   repository that can run `go test` can run the analysis.

2. **The assertion is the guard condition.** The script walks up from a failure
   call to the enclosing `if` and normalizes its condition. A failure call with
   no guard is recorded as the call itself, since it is then unconditional.

3. **Extract the comparison engine into `testshape`.** The rule set is the
   product's semantics and must not drift between languages. Python's analyzer
   was rewritten to use the shared engine as part of this change, and its 14
   end-to-end tests were the check that the extraction preserved behavior.

   The alternative — a copy per language — means a fix to the pairing logic has
   to be applied to every copy, and a missed copy is a silent wrong answer
   rather than a compile error. That is not hypothetical: the false positive
   where a leading negation looked like a removed assertion was found in Python
   by running it on a legitimate fix, and it was already present in the Rust
   analyzer.

4. **The operator vocabulary is per language and supplied by the caller.**
   Python normalizes `==` to `Eq`; Go renders it as `==`. A shared engine that
   hardcoded either would misclassify the other.

## Consequences

- Go gains the same six rules as Python and Rust: `removed_assertion`,
  `weakened_assertion`, `changed_expected_value`, `trivial_assertion`,
  `removed_test`, `skipped_test`.
- A Go-specific rule is now covered by the shared engine: a test whose
  assertions were all removed. It fires only when the base revision *had*
  assertions, so a test that never asserted is not treated as a regression.
- `t.Skip` is the Go form of an ignored test and is reported only when newly
  added, matching the behavior established for Rust's `#[ignore]` and Python's
  `pytest.mark.skip`.
- Analysis cost is one `go run` per revision per file, about 0.4s. Acceptable
  because it runs only for changed test files, and the toolchain's build cache
  makes repeated runs cheaper.
- A project whose test command does not name `go` (a wrapper script, a Makefile
  target) gets an explicit `test_source_unparsable` finding stating the file was
  not analyzed, rather than silence or a wrong guess.
- `changed_expected_value` fires on legitimate work, and this is now documented
  rather than left to surprise a user. Verified: fixing `add` from subtraction
  to addition and updating the expectation reports the rule in Go **and** in
  Rust. The tool cannot distinguish a correct expectation update from one edited
  to make a failing test pass, so it reports and a human decides.

## Alternatives considered

- **Compare failure messages.** Rejected on measurement: a reworded message
  would be a false `changed_expected_value`, and an inverted guard would be
  missed.
- **Regex over Go source.** Rejected: it cannot tell a guard from a message
  string, and it would report reindentation. The Rust fallback already
  demonstrates this failure mode.
- **Keep per-language copies of the rules.** Rejected: the rules are the
  semantics, and duplication makes a fix land in some languages and not others.
- **Build a persistent Go helper binary instead of `go run`.** Rejected for
  now: it needs a build step and a place to cache the artifact. `go run` at
  0.4s is fast enough that the complexity is not yet earned.
- **Add JavaScript in the same change.** Rejected, and recorded in the support
  matrix: Node has no built-in JavaScript parser, verified on this machine. The
  parser would have to come from the project's `node_modules`, so the
  availability question is different from Python and Go and needs its own
  decision.
