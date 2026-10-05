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
- **Ruby / RSpec** — likewise, against a live RSpec 3.13.
- **JavaScript / TypeScript** — likewise, against a real Node runner with a
  Jest-shaped summary and a real `acorn` in the project's `node_modules`.

`crates/witdiff-core/tests/verify_languages_end_to_end.rs` runs the whole chain
for each of them against a real temporary Git repository: HEAD green, pristine
base control green, and base-plus-transplanted-test red **for a recognized test
reason**. Verifying an analyzer against a real parser is not the same as
proving the verification works, and this file makes that distinction explicit.

JavaScript classification is also covered by unit tests against captured Jest
and Vitest output.

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

**Verified end to end against a real parser.** This was the last gap in the
JavaScript row, closed by installing `acorn` and `typescript` as dev
dependencies and running the analyzer against them.

Running the tool for real found six bugs, all of which had passed the previous
verification. Each is now covered by a test that runs a real parser:

- **ESTree's `Literal` was not normalized.** The tool handled Babel's
  `NumericLiteral`/`StringLiteral` but not acorn's `Literal`, so every literal
  rendered as the bare word `Literal`. `expect(x).toBe(2)` and
  `expect(x).toBe(3)` were therefore **identical strings**, and no expectation
  change was ever reported. This was the whole capability, silently absent.
- **`expect(x).not.toBe(1)` produced no assertion at all.** acorn puts `.not`
  between the call and the matcher, so the previous unwrap missed it. Inverting
  an assertion — which makes it pass for the wrong reason — reported nothing.
- **`test.skip` produced no function at all.** Only the bare `test(...)` form
  was recognized, so newly skipping a test was invisible. `test.only` and
  `test.each` were missed the same way.
- **Node's `assert` module was invisible.** `assert.strictEqual(a, 1)` and
  `assert.deepEqual(a, 1)` are member calls, and only a bare `assert*(...)`
  form was recognized.
- **TypeScript files crashed or analyzed as empty.** The TypeScript AST tags
  nodes with `kind`, not `type`, so the ESTree traversal visited nothing; with
  `setParentNodes` on, the tree was cyclic and the traversal recursed until
  `Maximum call stack size exceeded`. Literal kinds also reverse-map to the
  alias `FirstLiteralToken`, so `2` normalized to that word.
- **`.tsx` was rejected.** Every file was written to disk as `input.js`, so the
  parser never saw a `.tsx` extension and rejected valid JSX. The extension is
  load-bearing and is now preserved.

One bug was **not** in the JavaScript analyzer at all. Inverting an assertion
changed nothing, because the shared engine's `expectation_of` returned only the
right-hand side, making `Eq 1` and `NotEq 1` compare equal. That affected every
language using the operator-string form; Rust was unaffected only because its
analyzer compares `macro_name`. Fixed in `testshape`, so all languages get it.

`acorn` and `typescript` are now dev dependencies and the JS end-to-end tests
run against both. Measured: the two parsers produce byte-identical summaries
for the same source, which is what makes one traversal trustworthy for both.

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

**Verified against a live RSpec.** RSpec 3.13 was installed and run, which
closed the last unverified claim in the Ruby row and found two classifier bugs:

- **A single-example red suite was classified as an unrecognized command
  failure.** The summary matcher required the plural `" examples,"`, but real
  RSpec prints `1 example, 1 failure` for a one-example suite. Because an
  unrecognized failure cannot produce a proof, every single-example Ruby suite
  silently lost its red/green evidence. Minitest has the same singular shape
  (`1 run, 1 failures, 0 errors`) and the same gap.
- **A suite that failed to load was reported as a behavioural regression.** Real
  RSpec reports a `LoadError` as `0 examples, 0 failures, 1 error occurred
  outside of examples` — a non-zero *error* count on a summary line whose
  failure count is zero. The test-failure check ran first and claimed it. Ruby
  now checks compile failure **before** test failure, the reverse of every
  other framework, because a load error is never behavioural evidence.

Also recorded, since no output fixture could ever have shown it: RSpec's
default pattern is `**/*_spec.rb`. A spec file named `calc.rb` runs **zero**
examples and exits **successfully** — a silent pass with no output to copy. The
same source named `calc_spec.rb` fails. That difference is now a test pair.

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

## Known verification gaps

| # | Not verified against | Verified instead by | Where | ADR |
| --- | --- | --- | --- | --- |
| 1 | ~~a live **RSpec** run~~ | **closed: verified against RSpec 3.13** | Ruby: framework classification | ADR-0020 |
| 2 | ~~a real **`acorn`** / `@babel/parser`~~ | **closed: verified against real acorn and typescript** | JavaScript | ADR-0021 |
| 3 | ~~`ed25519-dalek`~~ | **closed: signing and verification run entirely in Rust** | signature over status and digest | ADR-0022 |

Those three were blocked by the environment, not by design: `gem install` and
cargo cache writes were denied, and macOS ships LibreSSL, which does not
implement Ed25519 at all. With the restrictions lifted, each was verified
against the real tool, and each found real bugs.

### What remains

**Red/green proof is end to end for all five supported languages.** Each reaches
a proof against a real temporary repository with a real toolchain: `Verified`
for pytest, Go, Ruby and JavaScript, and `VerifiedWithWarnings` for Java — see
below for why Java is one notch lower.

Python additionally has negative tests: a test that passes on the base is
reported `not_verified`, and a gutted assertion is caught rather than accepted.
The others have the positive path only.

**Java proves red/green but loses structural analysis.** Its red/green chain is
verified against a real JUnit 5 platform, so the failure classification and the
transplant are proven. The status is `VerifiedWithWarnings` rather than
`Verified` for one reason: when the configured test command is a script rather
than `java`/`mvn`/`gradle`, `JavaToolchain::from_test_command` cannot derive a
toolchain, so the structural comparison does not run and the receipt says so
with a `test_source_unparsable` finding. Fewer findings, never wrong ones — but
a real limitation, and a wrapper script is common in CI.

**A shared cargo target directory breaks a second run.** The two workspaces are
the same crate written by two revisions, so they write the same output path.
After the base experiment rebuilds the buggy source, the shared artifact is
newer than the workspace's own `src/lib.rs`, cargo treats it as fresh, and a
**second** `verify` in the same directory reports `head_failed` for a workspace
that compiles green. Measured: three consecutive runs against isolated target
directories were each `verified`; with a shared absolute target directory the
second run flipped.

The direction is safe — a false gate failure, never a false pass — but it is a
confusing break, and it is exactly what CI does when `CARGO_TARGET_DIR` points at
a cache shared across worktrees. Prefer isolated target directories, or rebuild
before trusting a repeat run.

Two things the Java work found by running the real thing, neither visible from
reading the code:

- **Mixed JUnit versions fail silently.** A `1.14.4` platform with a `6.0.1`
  Jupiter engine compiles the tests, runs them, prints `0 tests found`, and
  exits 0. A green run that executed nothing is exactly the result this project
  must not accept, and no amount of reading file names reveals it.
- **Build output must be gitignored**, or the first `javac` mutates the
  workspace and the fingerprint correctly reports stale evidence.

The recurring lesson is the one this project is built on: every bug above was
found by running the thing, and every one of them had already passed whatever
verification its ADR claimed. A hand-written fixture can only assert what
somebody already thought to write down; it cannot show that a real runner prints
`1 example, 1 failure` in the singular, or that a parser emits `Literal` where
the code expected `NumericLiteral`.

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
