# Support matrix

What WitDiff actually does per language, verified rather than intended. Read
this before adopting it for a non-Rust project.

The product has three capabilities that can be adopted independently:

1. **Red/green proof** — the core claim: the changed tests pass here and fail on
   the base revision. This is what the tool exists for.
2. **Test-integrity findings** — warnings that the change weakened its own tests
   (`removed_assertion`, `ignored_test`, `removed_match_arm`, …).
3. **Changed-code mutation** — supplementary evidence about how tightly the
   tests constrain the changed code.

Capability 1 is language-neutral. Capabilities 2 and 3 are **Rust-only**.

## Matrix

| | Rust | Python (pytest) | Go | JS/TS (Jest, Vitest) |
| --- | --- | --- | --- | --- |
| **Red/green proof** | yes | yes | yes | yes |
| Failure classification | yes | yes | yes | yes |
| Compile-error vs test-failure | yes | yes | yes | yes |
| **Integrity findings (structural)** | yes | no | no | no |
| **Integrity findings (line-based)** | yes | no | no | no |
| Inline `#[cfg(test)]` transplant | yes | n/a | n/a | n/a |
| **Mutation analysis** | yes | no | no | no |
| Targeted test selection | yes | no | no | no |
| `init` project detection | yes | yes | yes | yes |

"n/a" means the concept does not apply: inline test modules are a Rust idiom.
Python, Go and JavaScript keep tests in separate files, which the whole-file
transplant already handles.

## What "red/green proof" means per language

The transplant copies changed dedicated test files onto a detached worktree at
the base revision and runs the same test command. Nothing in that path is
Rust-specific: it is `git diff` for paths, `git apply`, and a copied file. The
only language-specific part is deciding *why* the run failed, which is why the
per-framework classifiers exist (ADR-0012).

Verified end to end against real toolchains:

- **pytest** — a repository whose test passes on the base and fails after the
  change reaches `verified`, with `base_run.failure_kind = test_failure`.
  Before ADR-0012 the same output classified as `command_failure`, which cannot
  produce a proof.
- **Go** — likewise, using real `go test` output.

JavaScript classification is covered by unit tests against captured Jest and
Vitest output. It has **not** been verified end to end against a real runner in
this environment, because neither is installed. Treat it as tested but not
proven until someone runs it.

## The honest gap: weakened tests are not detected outside Rust

This is the most important limitation to understand before adopting WitDiff for
a non-Rust project.

Measured on a Python repository: a test was changed from `assert is_even(3)` to
`assert True`, alongside a genuine bug fix. WitDiff reported:

```text
status           : verified
red_green_proven : true
integrity_findings: 0
```

The red/green proof was correct — the *other* test in the file did fail on base
— so `verified` is not a false claim about the proof. But the gutted assertion
was not reported at all.

The cause is concrete: the line-based fallback matches Rust macro syntax only.

```rust
["assert!(", "assert_eq!(", "assert_ne!(", "debug_assert!(", "debug_assert_eq!("]
```

Python's `assert x`, Go's `if got != want { t.Errorf(...) }`, and Jest's
`expect(x).toBe(y)` match none of those. So for non-Rust files WitDiff produces
**no integrity findings in either direction** — it does not warn incorrectly,
and it does not warn at all.

What this means in practice:

- You still get the core value: a real proof that the changed tests fail without
  the change.
- You do not get warned when a change also weakens its tests. A reviewer or
  agent must notice that themselves.

For Rust, the structural analyzer (ADR-0006) catches this, along with changed
expected values, removed `match` arms, newly ignored tests and trivial
assertions.

## What is not supported anywhere

- Test frameworks beyond the four above. An unrecognized `framework` value is a
  configuration error rather than a silent fallback (ADR-0012), so a JUnit or
  RSpec project fails loudly instead of producing a wrong-conservative result.
- Multi-language monorepos with per-directory test commands. One
  `test_command` applies to the whole repository, so a repo mixing Rust and
  Python needs two WitDiff configurations or a wrapper script.
- Targeted test selection outside Rust. Cargo's `--test <target>` has a real
  implementation (ADR-0008); other frameworks run the full suite and the receipt
  says `full_suite`.

## Adding a language

Two steps, in this order:

1. **Failure classification** — add a `TestFramework` variant with patterns
   keyed on that framework's own structural output markers, plus tests using
   captured real output including a passing run. This alone makes red/green
   proofs work.
2. **Integrity analysis** — parse the language and compare structure between
   revisions, as `rustanalysis.rs` does with `syn`. This is the larger piece:
   it is a real parser plus a comparison model, not a pattern list.

A line-based fallback for a new language is *not* worth adding. Measured: the
one that exists matches Rust syntax specifically and contributes nothing for any
other language, while giving the impression that tests are being checked.
