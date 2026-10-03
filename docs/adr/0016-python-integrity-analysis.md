# ADR-0016: Python integrity analysis via the interpreter's own AST

Status: accepted

## Context

The support matrix records the largest gap in the product: structural
test-integrity analysis is Rust-only. For Python, Go and JavaScript, WitDiff
proves that changed tests fail without the change, but cannot warn when a change
also *weakens* its tests.

Measured before designing anything. A Python test was changed from
`assert is_even(3)` to `assert True`, alongside a real bug fix:

```text
status            : verified
red_green_proven  : true
integrity_findings: 0
```

The proof was correct — a second test in the file did fail on the base revision
— but the gutted assertion was not reported at all. The line-based fallback
cannot see it, because it matches Rust macro syntax exclusively
(`assert!(`, `assert_eq!(`, …) and Python's `assert x` matches none of it.

Python is the most valuable language to close this for. It is where the
AI-coding audience largely works, and it is the language where a test asserting
`assert result` instead of `assert result == expected` is a common, plausible
mistake rather than an exotic one.

## Evidence

Two ways to parse Python were evaluated.

**A Rust Python parser (`rustpython-parser`).** It resolves at 0.4.0 against the
workspace's `rust-version = 1.78`, so MSRV is not a blocker the way it was for
`rmcp` in ADR-0014. But it pulls 63 packages, and it could not be compiled in
the development environment at all: the sandbox denies writes to the cargo
cache, so the dependency and its transitive tree cannot be downloaded.

Shipping an analyzer that was never compiled or run is precisely the failure
this project exists to detect, so that option cannot be taken here.

**The interpreter's own `ast` module.** Python ships a parser. `python3 -c`
with a short program can emit a normalized structural summary, and the Python
interpreter is already present in any repository that runs pytest — WitDiff
would be parsing Python for a project whose test command is `python3 -m pytest`.

A prototype confirmed the necessary structure is available:

```text
assert is_even(2)        -> is_even(2)
assert not is_even(3)    -> Not is_even(3)
assert True              -> True
```

Critically, this captures *structure* rather than text: the `Not` operator and
the comparison operator survive, which is what makes a weakening detectable.

`ast.dump` was rejected as the comparison basis even though it is the obvious
choice. Its output embeds `ctx=Load()` nodes and has changed across Python
releases, so two interpreters could disagree about identical code — a
false-positive generator in a tool whose value is that it does not cry wolf.
The prototype instead emits WitDiff's own normalized form, which WitDiff
versions and controls.

## Decision

Analyze Python test files by invoking the configured interpreter to produce a
normalized structural summary, then compare base against head with the same
rule set the Rust analyzer uses.

1. **The interpreter is discovered from the test command**, not hardcoded.
   The command already names `python3`, `python`, or a virtualenv path. Using
   that same interpreter means the analysis runs under the interpreter the
   project actually tests with, and a project with no Python at all is never
   probed.

2. **The analyzer is a pure function over the summary.** The Python side is a
   small, fixed script that emits JSON; all rules, normalization and comparison
   live in Rust. This keeps the semantic decisions in one place and makes the
   Python side replaceable.

3. **Failure to analyze is reported, never silent.** If the interpreter is
   missing, exits non-zero, or emits unusable output, the file yields an
   explicit finding naming why. This reuses the contract ADR-0006 established
   for Rust: a file that cannot be analyzed is never represented as clean.

4. **The rule set mirrors Rust where it applies**, so a reader learns one model:
   `removed_assertion`, `weakened_assertion`, `trivial_assertion`,
   `changed_expected_value`. Python-specific rules (a removed `@pytest.mark.skip`,
   a bare `pass` body) are considered later and are not part of this decision.

5. **The summary format is versioned.** It is written by a script WitDiff
   ships and read by WitDiff, so a format change is a code change on both sides
   and can be detected rather than misparsed.

## Consequences

- Python gains the integrity rules that currently make WitDiff feel Rust-only,
  without adding a 63-package dependency or requiring a network fetch.
- WitDiff now depends on a working interpreter in the verified repository. That
  is a real coupling, and it is bounded: the interpreter comes from the
  project's own configured test command, so a repository that can run its tests
  can run the analysis.
- Analysis correctness is tied to the interpreter's *parse*, not its semantics.
  Two Python versions parse the same source identically for the constructs
  involved; the normalization step removes the version-sensitive parts.
- A project pinning an unusual interpreter (a wrapper script, `uv run python`,
  a container entrypoint) may not be directly invocable. The design must treat
  that as a reported inability to analyze rather than an error that fails
  verification, because it is an analysis limitation and not a failure of the
  proof.
- JavaScript and Go remain without structural analysis. The same interpreter
  approach is available for both (`node` has no built-in AST, but Go ships
  `go/ast`), and is deliberately not decided here.
- The result is an additive receipt change only if new rule names are
  introduced; the receipt's `rule` field is a free-form string, so no schema
  change is required.

## Alternatives considered

- **`rustpython-parser`.** Rejected on the same grounds as `rmcp` in ADR-0014:
  it cannot be compiled or tested in the development environment, so its
  behavior would be unverified. This is a statement about this environment, not
  about the crate, and the decision is worth revisiting where dependencies can
  be fetched.
- **Regex or line heuristics for Python.** Rejected on measurement: the existing
  line-based fallback already demonstrates the failure mode. It cannot tell
  `assert x == 1` from `assert x`, which is exactly the weakening that matters,
  and it would report noise on reformatting.
- **`ast.dump` comparison.** Rejected: version-sensitive output that would make
  two interpreters disagree about identical code.
- **Require the user to install a Python analysis extra.** Rejected: it adds an
  install step whose absence would silently degrade the analysis, and silent
  degradation is what invariant 7 forbids.
- **Do nothing, and document the gap.** Rejected as the end state: the support
  matrix makes the gap known, but Python is the highest-value place to close it,
  and the cost is a script rather than a dependency.
