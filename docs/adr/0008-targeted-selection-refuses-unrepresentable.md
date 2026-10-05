# ADR-0008: Targeted test selection must refuse rather than approximate

Status: accepted

## Context

Running only the changed tests is much faster than running the whole suite, and
the roadmap has wanted it since M2.

The obvious implementation — append `--test <name>` to the configured command —
is unsound. Verified against cargo during this work: `--all-targets` overrides
`--test`. A configured command of `cargo test --all-targets --all-features` would
still build and run every target, so the receipt would name one test target
while the entire suite executed. The run would look targeted, the claim would be
false, and nothing in the output would reveal it.

Two further mismatches follow from cargo's target model. Only direct children of
a `tests/` directory are separate integration-test targets; files nested deeper
are modules of their parent target and cannot be selected. And a unit-test module
inside `src/` is not a target at all.

## Decision

Targeted selection is opt-in (`verification.targeted_test_selection`, default
`false`) and is attempted only when the configured command can be narrowed
without changing what actually runs. Otherwise WitDiff keeps the full suite and
records a note explaining why narrowing did not happen.

Narrowing is refused when:

- the command is not a cargo invocation, or does not invoke `cargo test`;
- the command uses `--all-targets`, which overrides `--test`;
- the command already selects tests explicitly;
- any changed dedicated test file does not map to a selectable cargo target.

The refusal cases are enumerated in `SelectionOutcome` and covered by unit
tests, so each is a stated decision rather than an untested branch.

## Consequences

- A receipt states which command produced the evidence: `test_selection` is
  `full_suite` or `targeted`, and `effective_test_command` records the exact
  argument vector used.
- Narrowing never silently replaces full-suite evidence. When it is declined,
  the full suite runs, and the note says so.
- Narrowing is narrower evidence. A failing narrowed run speaks only about the
  tests that ran, so the receipt distinguishes the two cases rather than
  presenting them as the same claim.
- Defaulting to `false` keeps 1.0 behavior identical to 0.1 for existing
  repositories; enabling it is an explicit decision.

## Alternatives considered

- **Always narrow and trust cargo.** Rejected: it produces receipts that assert
  something narrower than what ran, which is a false claim in the machine-
  readable artifact.
- **Require a separate configured target command.** Rejected as unnecessary for
  now: the refusal path already yields correct full-suite evidence, and a second
  configuration knob would add a way to be wrong without adding proof.
