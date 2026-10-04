# ADR-0020: Ruby integrity analysis via `ripper`

Status: accepted

## Context

The support matrix recorded Ruby as the cheapest remaining addition, because
`ripper` ships in the same standard library as the minitest already recognized.
This closes the last language gap that does not require its own decision about
parser availability.

## Evidence

- **`ripper` ships with the interpreter.** No gem is needed, so the analysis
  works in any project that can run its tests — the same property that made
  Python, Go and Java tractable.
- **`Ripper.sexp` produces a full tree with positions.** Verified against real
  source: `[:method_add_arg, [:fcall, [:@ident, "add", [1, 16]]], ...]`, which
  carries both the method name and its line.
- **Minitest and RSpec put the expectation first**, like JUnit:
  `assert_equal 2, add(1, 1)`. Confirmed against the Minitest API.
- **Ruby needed no compilation step**, unlike Java, so it is the cheapest
  adapter: one `ruby summarize.rb <path>` per revision per file.

Two bugs were found by running the tool against real Ruby rather than by reading
it, and both are recorded because they are the kind that fail silently:

1. `assertion?` called `start_with?` on a value that can be an `Array` for a
   bare method call, which raised `NoMethodError` and made every file
   unanalyzable. The tool now guards the type.
2. A bare call renders as `[:fcall, [:@ident, ...]]`, which the renderer did not
   unwrap, so `add(1, 1)` came out as `fcall(1, 1)`. The name was lost, which
   would have made every such assertion compare as a different subject.

Neither was visible in the first manual test, which used `Compute.call(...)` — a
`method_add_arg` rather than an `fcall`.

## Decision

Analyze Ruby through `ripper` over an embedded tool, mirroring Java, with the
shared rule engine doing the comparison.

1. **The interpreter comes from the project's test command**, recognizing
   `ruby`, `rake`, `rspec`, `bundle` and `minitest`, so a repository that can run
   its tests can analyze them.

2. **Minitest's argument order is swapped in the tool.** `assert_equal expected,
   actual` becomes the subject-first `actual Eq expected` the shared engine
   expects. Getting this wrong would report every expectation change as a
   removal plus an addition rather than as a changed expectation — the same
   silent failure mode as JUnit.

3. **Bindings and guards are captured**, so a rebound subject is not reported as
   a removal and a deleted `raise` guard is reported as `removed_error_check`.
   Ruby's `raise`/`flunk` inside a conditional is the guard form, since it fails
   the test without an assertion macro.

4. **Assertion methods are recognized by prefix.** `assert_*` and `refute_*` are
   Minitest's and RSpec's families; `flunk` is the bare failure call. A method
   that is not recognized contributes nothing rather than being guessed at.

## Consequences

- Ruby gains the same rules as Rust, Python, Go and Java, including
  `removed_error_check` and `trivial_assertion`. Verified end to end that
  `assert_equal 2, add(1, 1)` becoming `assert true` is reported.
- RSpec is recognized by the framework classifier and by `init`, but its
  `expect(x).to eq(y)` form is **not** yet normalized: the assertion methods
  handled are the Minitest-style ones. An RSpec file therefore produces few or
  no findings rather than wrong ones, and this is recorded in the support matrix
  rather than implied to work.
- The rendering is stable rather than pretty. `r.nil?` renders with a trailing
  `.()`, which is cosmetic; comparison only needs the same input to produce the
  same output, and it does.
- No new dependency, and no gem install step.

## Alternatives considered

- **Require a gem such as `parser`.** Rejected: `ripper` is already present, so a
  gem would add an install step whose absence silently degrades the analysis.
- **Regex over Ruby source.** Rejected for the same reason as every other
  language: it cannot tell `assert_equal 2, add(1, 1)` from `assert_equal add(1,
  1), 2`, which have opposite meanings.
- **Compare the failure message.** Rejected: a reworded message would be a false
  `changed_expected_value` and an inverted expectation would be missed.
- **Handle RSpec's `expect(...).to` in the same change.** Deferred, not
  rejected: it is a different assertion shape and deserves its own tests against
  real RSpec output, which is not installed in this environment.
