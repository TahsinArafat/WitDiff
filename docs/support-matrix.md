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

| | Rust | Python | Go | Java | Ruby | JS/TS |
| --- | --- | --- | --- | --- | --- | --- |
| **Red/green proof** | yes | yes | yes | yes | yes | yes |
| Failure classification | yes | yes | yes | yes | yes | yes |
| Compile-error vs test-failure | yes | yes | yes | yes | yes | yes |
| **Integrity findings (structural)** | yes | yes | yes | yes | **yes** | **yes*** |
| Inline `#[cfg(test)]` transplant | yes | n/a | n/a | n/a | n/a | n/a |
| **Mutation analysis** | yes | no | no | no | no | no |
| Targeted test selection | yes | no | no | no | no | no |
| `init` project detection | yes | yes | yes | yes | yes | yes |

JavaScript and TypeScript need a parser *in the project*, because Node ships
none. A Jest or Vitest project already has one — see the section below.

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

## Python structural analysis (ADR-0016)

Python test files are analyzed structurally, using the interpreter named by the
project's own test command. The rules mirror Rust so a reader learns one model:
`removed_assertion`, `weakened_assertion`, `changed_expected_value`,
`trivial_assertion`, `removed_test`, `skipped_test`.

This closed the gap recorded here previously. Measured before the work, a test
changed from `assert is_even(3)` to `assert True` alongside a real bug fix
produced `verified` with zero findings. It now produces:

```text
status    : not_verified
integrity : 2 finding(s), 2 high severity
  high test_a.py [trivial_assertion]  test `test_odd` contains an assertion that cannot fail
  high test_a.py [removed_assertion]  an assertion was removed from test `test_odd`
```

Reformatting still produces no finding: the analyzer compares normalized
structure, not text, so `assert f() == 1` and `assert f()  ==  1` are identical.

Two limitations to know:

- The interpreter must be discoverable from the test command. A command that
  hides it (a wrapper script, `uv run`, a container entrypoint) yields an
  explicit `test_source_unparsable` finding saying the file was not analyzed,
  rather than silent silence or a wrong guess.
- Python's `assert` is compiled out under `python -O`. WitDiff analyzes the
  source, so it reports what the test says rather than what a particular
  interpreter invocation would execute.

## Go structural analysis (ADR-0017)

Go test files are analyzed with `go/ast` from the standard library, run through
`go run` over an embedded script. The rules are the same as Python and Rust,
because all three share one comparison engine ([`testshape`]), so a fix to the
pairing logic cannot be applied to only some languages.

Go has no assertion keyword, which shapes the analysis. A Go test fails by
calling `t.Errorf` or `t.Fatalf`, and what matters is the condition guarding the
call:

```go
if got != want {          // this is the assertion
    t.Errorf("got %d", got)
}
```

The failure message is not the assertion. Verified end to end: rewording
`t.Errorf("got %d", ...)` to `t.Errorf("addition is wrong: %d", ...)` produces no
finding, because comparing messages would report a reworded message as a changed
expectation and would miss an inverted condition entirely.

Detected: `removed_assertion` (including the Go-specific case of a test whose
guard was deleted so it asserts nothing), `weakened_assertion`,
`changed_expected_value`, `trivial_assertion`, `removed_test`, `skipped_test`.

`t.Skip` is the Go form of an ignored test and is reported only when newly
added, so a pre-existing skip does not flood every receipt that touches the
file.

## Java structural analysis (ADR-0018)

Java test files are analyzed with the JDK's own parser (`com.sun.source` via
`JavacTask`), run through `java` over an embedded tool. Only the public API is
used, so no `--add-exports` flag and no internal `com.sun.tools.javac` package
is needed, and it works on any JDK.

**JUnit reverses the argument order**, which is the detail that would silently
break everything. `assertEquals(expected, actual)` puts the expectation first,
unlike Python's `assert actual == expected` and Go's `if actual != want`. The
tool swaps them so the shared engine compares like with like. Verified end to
end: `assertEquals(2, Add(1, 1))` is normalized to `Add(1, 1) Eq 2`, and the
analyzer reports `changed_expected_value` for it exactly as Python does.

Detected: `removed_assertion`, `weakened_assertion`, `changed_expected_value`,
`trivial_assertion`, `removed_test`, `skipped_test`.

Recognized forms: `assertEquals`/`assertNotEquals`/`assertSame`/`assertNotSame`
(with the swap), `assertTrue`/`assertFalse`, `assertNull`/`assertNotNull`,
`assertArrayEquals`, and the bare `if (cond) fail(...)` shape. `@Disabled` and
`@Ignore` are the skip markers, reported only when newly added.

Requirements: a **JDK**, not a bare JRE, because the analysis compiles the tool
in memory. A JRE-only environment is reported explicitly rather than mistaken
for a syntax error.

## JavaScript and TypeScript (ADR-0021)

Node ships **no built-in parser**, so this is the one language whose parser comes
from the project rather than the runtime. WitDiff tries `@babel/parser`, then
`acorn`, then `typescript`, in that order — all produce an ESTree tree, so one
traversal serves all three, and `typescript` also covers `.ts`/`.tsx`.

**The working directory is not enough.** Node resolves `require` relative to the
script's own location, so a tool run from a temporary directory cannot see the
project's `node_modules` even when its working directory is the project. The
project path is passed explicitly and each parser is required by absolute path.
This was a bug that nearly shipped.

When no parser is present, the file is **reported as not analyzed**, naming the
packages WitDiff looked for:

```text
warning a.test.js [test_source_unparsable] no JavaScript parser available;
        install one of @babel/parser, acorn or typescript as a project dependency
```

It never falls back to a weaker analysis, because that is the failure the
line-based fallback already demonstrates.

`expect(x).toBe(y)` normalizes to the subject-first form the shared engine
compares; `toBeTruthy`, `toContain` and the other predicate matchers constrain
the subject alone, so they normalize to the subject; `.not.toBe(y)` normalizes
as an inequality.

**Not verified against a real parser here.** This development environment has no
JavaScript parser and no way to install one, so the end-to-end path is verified
through the parser-absence contract — that a project without a parser is
reported, which is the branch that matters most — plus hand-written ESTree cases
for the traversal. Those caught a real bug: `.not.toBe` nests one level deeper
than `.toBe`, and reading the matcher without unwrapping returned nothing for
every negated expectation. A run against real acorn in CI would close the gap.

## Other languages: assessment, not support

These were checked directly on this machine. None is supported; the point is to
record what each would actually require, so a future decision is informed rather
than guessed.

| Language | Parser available without extra install? | What it would take |
| --- | --- | --- |
| **TypeScript** | No — needs `typescript` in `node_modules` | Same problem as JavaScript. A TS project using Vitest does have `typescript` installed, so this is more tractable than plain JS |
| **PHP** | Partially — `token_get_all` always ships; `ext-ast` does not | The tokenizer gives tokens, not a tree. Enough for line-based rules, not for the structural comparison the other languages get |
| **.NET / C#** | Yes — Roslyn ships with the SDK | Feasible in principle, but Roslyn is a large API and the analysis would likely need a helper project rather than a single portable source file |
| **ASP / ASP.NET** | n/a | A framework, not a test language. ASP.NET tests are xUnit/NUnit/MSTest, which are C# and covered by the .NET row |

## Ruby structural analysis (ADR-0020)

Ruby test files are analyzed with `ripper` from the standard library, so no gem
and no install step is needed. Minitest's `assert_equal expected, actual` puts
the expectation first, and the tool swaps it to the subject-first form the
shared engine uses.

Detected: `removed_assertion`, `weakened_assertion`, `changed_expected_value`,
`trivial_assertion`, `removed_test`, `skipped_test`, `removed_error_check`.
Verified end to end that `assert_equal 2, add(1, 1)` becoming `assert true` is
reported.

**RSpec is supported too.** `expect(x).to eq(y)` and `expect(x).not_to eq(y)`
normalize to the same canonical subject-first form as Minitest, so a project
using either style gets the same rules. RSpec examples are `it "..." do` blocks
rather than `def`, so they need their own collection, and both styles share one
body extraction so the two forms cannot drift in what they detect.

Verified against `Ripper.sexp` rather than a live RSpec run: RSpec could not be
installed in the development environment (`gem install` is blocked), so the
normalization is verified against the parser's own output for every form, and
the end-to-end tests exercise the analyzer end to end with RSpec sources. A
predicate matcher such as `be_truthy` normalizes to the subject alone, since that
is what it checks.

Recognized output, verified against real runs:

- Minitest's `1 runs, 1 assertions, 1 failures, 0 errors, 0 skips`, including a
  passing run that must not classify as a failure;
- RSpec's `2 examples, 1 failure`, which uses the singular form;
- Ruby's `LoadError`, which is a compile failure rather than a test failure.

`init` writes `rake test` for a Minitest project and `bundle exec rspec` when an
`.rspec` file or a `spec/` directory is present.

### When a toolchain is missing

A verification needs the project's test command to run. When that program is
absent, WitDiff still writes a receipt rather than aborting: the status is
`not_verified`, `head_run.failure_kind` is `spawn_failure`, and every integrity
finding is preserved, because those are computed before the run and do not
depend on it.

`--install-toolchains` additionally tries to obtain the missing program, but
only from the project's own committed installer — a Java repository's `mvnw` or
`gradlew`, which downloads an exactly pinned distribution. WitDiff never picks a
version, and never runs `npm install`, `pip install` or `go install`, because
those execute project-controlled scripts. See ADR-0019.

For each ecosystem the practical fix when nothing is committed:

| Ecosystem | Install |
| --- | --- |
| Rust | `rustup toolchain install` (honours `rust-toolchain.toml`) |
| Python | `uv sync`, or `pip install -r requirements.txt` |
| Node | `npm ci` |
| Go | the toolchain self-installs the version in `go.mod` |
| Java | commit `mvnw`/`gradlew`; otherwise install Maven or Gradle and a JDK |

## The line-based fallback

The fallback that exists for unparsable Rust files matches Rust macro syntax
only:

```rust
["assert!(", "assert_eq!(", "assert_ne!(", "debug_assert!(", "debug_assert_eq!("]
```

Confirmed directly: Python's `assert x`, Go's `t.Errorf` and Jest's
`expect(x).toBe(y)` match none of those. It contributes nothing outside Rust,
which is why each language gets real structural analysis rather than a widened
pattern list.

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
