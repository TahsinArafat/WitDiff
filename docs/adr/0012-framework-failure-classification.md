# ADR-0012: Framework-specific failure classification behind a narrow capability boundary

Status: accepted

## Context

M4 of the roadmap adds adapters for pytest, Vitest/Jest and Go test. The
backlog describes each adapter as owning "dedicated test discovery; failure
classification; targeted invocation capability; framework/environment
fingerprint additions" — a wide surface.

Before designing that surface, the failure-classification path was measured
against real framework output. The result reframes the milestone: this is a
correctness gap, not a missing feature.

## Evidence

The existing `classify_failure` matches cargo and rustc output specifically:

```rust
combined.contains("test result: failed")
    || combined.contains("failures:")
    || combined.contains("tests failed")
```

Classifying the canonical failure text of four frameworks with the current
implementation produced:

```text
pytest   -> CommandFailure
cargo    -> TestFailure
jest     -> CommandFailure
go       -> CommandFailure
```

Three of four misclassify a real test failure. That matters because the
distinction is load-bearing, not cosmetic: only `TestFailure` on the base
experiment yields a red/green proof. `CommandFailure` produces `not_verified`
and the note "base+changed-tests command failed, but not with a recognized test
assertion failure".

So a repository using pytest, Jest or Go does not merely lack an adapter — it
gets a *wrong, conservative* answer that looks like a legitimate result. A
regression that genuinely fails on the base revision is reported as
unrecognized. That is the failure mode invariant 7 exists to prevent, in a
milder form: the case is not silently ignored, but the receipt cannot say what
happened.

## Decision

Adapters supply **classification and targeted invocation only**. Discovery stays
config-driven.

1. A `TestFramework` selects a failure classifier and, optionally, a targeted
   invocation strategy. That is the whole trait.

   Discovery is deliberately excluded. It is already expressed by
   `test_globs`, `extra_test_paths` and the changed-file classifier, all of
   which are language-neutral and already configurable. Adding a discovery
   method to the trait would duplicate a working mechanism behind a worse
   interface, and would freeze a plugin API before it has been exercised — the
   same reasoning that deferred a `witdiff-mutate` crate in ADR-0011.

2. Classification is a property of the framework, selected by configuration,
   not sniffed from output.

   Guessing the framework from the command line or from test output would make
   the answer depend on text that the candidate controls. Configuration already
   names the test command, so it can name the framework; the language key
   already exists in `[project]`.

3. An unknown or absent framework classifies conservatively and says so.

   The default classifier keeps today's cargo behavior and remains the Rust
   default. A framework that is not selected is never assumed. When nothing
   matches, the outcome stays `CommandFailure`, which cannot become a proof —
   never a guess at `TestFailure`.

4. Classification patterns must be anchored to framework-specific output
   shapes, not to generic English.

   The risk in a permissive matcher is the opposite error: reporting
   `TestFailure` for something that was not a test failure, which would turn a
   broken invocation into a red/green proof. Each framework's patterns
   therefore key on structural markers (`1 failed`, `--- FAIL:`, `Tests:`) and
   are covered by tests using captured real output, including a
   passing-run case that must classify as success.

## Consequences

- `FailureKind` gains no new variants. The variants describe *evidence*, not
  frameworks, and every framework maps onto the existing set. Adding
  `PytestFailure` and similar would push framework identity into the receipt's
  evidence vocabulary for no gain.
- The default configuration is unchanged, so existing Rust users see no
  behavior change.
- A configured framework name that is not recognized is an explicit error
  rather than a silent fallback to the Rust classifier, because silently using
  the wrong classifier produces exactly the wrong-conservative answer this ADR
  is fixing.
- Targeted invocation is optional per framework. Rust has a real implementation
  (ADR-0008). Others start without one, which means the full suite runs and the
  receipt says `full_suite` — truthful, and never a silent narrowing.
- Structure-aware test-integrity analysis (ADR-0006) and inline transplantation
  (ADR-0010) remain Rust-only and are skipped for other languages. They produce
  no findings rather than wrong findings, and the absence is visible because
  those findings are simply not present. This is a real limitation of the
  adapter work and is recorded rather than papered over.
- Mutation (ADR-0011) remains Rust-only for the same reason: the operators are
  defined over Rust syntax. A non-Rust repository with mutation enabled gets no
  mutants rather than misleading ones.

## Alternatives considered

- **Sniff the framework from the command or output.** Rejected: it makes the
  classification depend on text the candidate can influence, and a wrong guess
  produces a *less* conservative answer, which is the dangerous direction.
- **One combined classifier matching every framework's patterns at once.**
  Rejected: a passing Jest line (`Tests: 1 passed`) and a failing Go line
  (`--- FAIL:`) could both match in a mixed workspace, and the union of patterns
  is much more likely to report `TestFailure` for a non-test failure. Selecting
  one framework keeps the matcher auditable.
- **A full plugin trait with discovery.** Rejected as premature: discovery is
  already config-driven and language-neutral, and freezing the API before
  implementing any non-Rust adapter would lock in guesses about what they need.
- **New `FailureKind` variants per framework.** Rejected: the receipt should
  describe evidence, not the tool that produced it. The framework is already
  visible from the configured command.
- **Defer adapters until a plugin system exists.** Rejected: the classification
  bug means non-Rust repositories get wrong results today, and classification is
  the smallest change that fixes it.
