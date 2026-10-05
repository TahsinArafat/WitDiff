# ADR-0023: PHP structural integrity analysis

Status: accepted

## Context

WitDiff supports structural integrity analysis for six languages through one
shared rule engine (`witdiff_core::testshape`). PHP was unsupported: there was
no `PhpToolchain`, no `php` framework in `TestFramework`, and no test-glob
detection. A PHP project therefore received the red/green proof but **no
integrity findings at all** — the rules that detect a change weakening its own
tests simply did not run.

This was reported by someone building a WordPress theme plugin, where the
absence was not a nicety: a plugin change that quietly replaced a real
assertion with a self-satisfying one is exactly the failure WitDiff exists to
catch.

The previous assessment in `docs/support-matrix.md` recorded PHP as
"partially" supported, on the reasoning that `token_get_all` is a tokenizer and
therefore "enough for line-based rules, not for the structural comparison the
other languages get". That assessment was wrong, and it was wrong by not being
tested: see Consequences.

## Decision

PHP joins the shared rule engine as a seventh language.

### The parser comes from the runtime, like Ruby and Python

`token_get_all` ships in every PHP installation. The summary tool is embedded
in the binary and written to a temporary file, so no Composer dependency, no
`vendor/` requirement for the analysis itself, and nothing for the project to
install. This is the same seam ADR-0020 established for `ripper`.

### The tool is a tokenizer, so structure is tracked by depth

There is no tree to walk. A PHPUnit method body opens at the first `{` after its
parameter list and closes at the matching `}`. A Pest example is not a
declaration at all but a call — `test('name', function () { … })` — so its body
opens at the first `{` **inside the call's parentheses**, because the closure is
an argument.

### Both assertion styles normalize to subject-first

PHPUnit writes `assertEquals(expected, actual)` and Pest writes
`expect($x)->toBe($y)`. Both name the expectation first, the reverse of Python
and Go. The tool swaps them, for the same reason and with the same consequence
as JUnit and Minitest: without the swap every changed expectation reads as a
removal plus an addition rather than as a change.

### Failure classification is measured against real runners, not remembered ones

PHPUnit 10.5.66 and Pest 3 were installed and run. The markers are recorded in
`docs/support-matrix.md`. The load-bearing difference: Pest prints **no
`FAILURES!` marker**, so a Pest test that errors on something other than an
assertion is classified from the `Tests:  1 failed` count line alone. Without
that, Pest classified as `CommandFailure`, which cannot produce a proof — Pest
projects would have silently lost their red/green evidence while appearing to
work.

### Generated suite artifacts are build output

PHPUnit writes `.phpunit.result.cache` and Pest writes
`vendor/pestphp/pest/.temp/test-results`, both created by the act of running the
suite. Both are classified as build output, alongside `node_modules/` and
`.venv/`. Without this, a PHP verification failed its own freshness gate on the
cache its suite had just written.

### PHP project detection precedes JavaScript

A WordPress plugin commonly ships a `package.json` for build tooling while its
tests are PHPUnit or Pest. `composer.json`, `phpunit.xml`, `phpunit.xml.dist`,
`pest.php` and a committed `vendor/bin/phpunit` are all PHP signals, checked
before the JavaScript branch for the same reason Java is checked before it.

## Consequences

**The support matrix's previous PHP assessment is superseded.** It claimed the
tokenizer was insufficient for structural comparison. Running the tool against
real PHP showed the tokenizer was sufficient for every shape the rule engine
needs — test methods, Pest examples, assertions and their compared arguments.
The matrix row is struck through and replaced.

**Three bugs were found by running, not by reading.** Each is a token-shape
assumption that a hand-written fixture would have encoded as truth:

- PHP 8 emits `->` as a single `T_OBJECT_OPERATOR` token. An implementation
  scanning for `-` followed by `>` finds nothing, and every Pest expectation
  reads as a bare subject.
- A Pest closure body sits *inside* the call's parentheses. Scanning past the
  closing `)` finds no body, so every Pest example reports zero assertions.
- `expect((new Calc(1, 2))->add())->toBe(3)` contains a `T_OBJECT_OPERATOR` in
  the *subject*. Matching the first one reads `add` as the matcher and yields
  `Eq 3` attached to the wrong expression.

The third is the general lesson restated: a fixture can only assert what
somebody already thought to write down. See the same argument in the JavaScript
ADR.

**A fourth bug is the one that mattered most, and it was not a token shape.**
A WordPress-style test whose only check is
`$this->expectException(RuntimeException::class)` was summarized with **zero
assertions**. Deleting the guard therefore changed nothing a comparison could
see, and the test went from asserting to asserting nothing with no finding.

The red/green proof still passed in that case, and that is correct on its own
terms — the experiment genuinely did distinguish two revisions. But the test had
stopped constraining anything: the code was changed to stop throwing, and the
guard was deleted so the test would stay green. PHPUnit itself flags that shape
(`Tests: 1, Assertions: 0, Risky: 1.`); WitDiff read it as `verified`.

The shared engine already had the right rule — a test that goes from asserting
to asserting nothing is a `removed_assertion` — and it stayed silent only
because the summary reported nothing for it to remove. **Fixing the summary was
enough; no new rule was needed.** That is the strongest argument for keeping
one rule engine across languages: the defect was a language adapter under-
reporting, not a gap in the rules.

An exception expectation is now rendered as `expects-exception`, which counts as
an assertion without inventing a comparison the test never wrote.

**A test that has never failed proves nothing, and one of ours did.** The
PHPUnit count helper was added, then broken in three ways, and no test noticed
twice. It was dead code that its own doc comment described as load-bearing. The
fix was not to keep the helper but to **find a case only it can classify** —
which required installing Pest and discovering that Pest prints neither
`FAILURES!` nor `failed asserting that`. The helper is now genuinely
load-bearing, and breaking its Pest arm fails
`pest_non_assertion_error_is_recognized`.

**An existing test was narrower than it looked.**
`every_supported_framework_recognizes_its_own_failure_output` enumerated four
frameworks in a hand-written list and had never been extended when Java and
Ruby shipped. Adding PHP did not extend it either. It is now driven by
`TestFramework::all()` with a length assertion, so the next framework cannot be
silently skipped.

**Dependencies must be reachable from the base worktree.** This is not
PHP-specific and was already true for every language with gitignored
dependencies. The base experiment runs in a `git worktree`, which contains only
committed files; with `vendor/` gitignored the base control run fails with
`Could not open input file: vendor/bin/phpunit` and the receipt reports
`base control: FAIL`, which reads as "the base is broken" rather than "the base
could not start". **Recorded as an open gap, not fixed here** — fixing it means
changing what the base experiment executes, which is a semantic decision about
verification rather than a language addition.

## Evidence

Run against PHP 8.5.9, PHPUnit 10.5.66 and Pest 3, on real repositories created
for this ADR:

- **PHPUnit red/green proof**: a base whose `add()` returned `a - b`, fixed on
  head with `assertEquals(5, (new Calc())->add(2, 3))` →
  `status: verified`, `red_green_proven: true`, base control `PASS`, base+tests
  `FAIL`.
- **PHPUnit vacuous test**: the same fix with
  `assertEquals($calc->add(2, 3), $calc->add(2, 3))`, green under plain PHPUnit
  (`OK (2 tests, 2 assertions)`) → `status: not_verified`, note "changed tests
  also pass on the base revision; they do not prove the behavioral change".
- **Pest red/green proof**: same shape, `framework = "pest"` →
  `status: verified`, `red_green_proven: true`.
- **A gutted WordPress-style test is now caught.** A base test asserting only
  `expectException`, changed on head so the code stops throwing and the guard is
  deleted → `high … [removed_assertion] test 'testMissingFileThrows' no longer
  asserts anything; every check was removed`, and the note "red/green behavior
  was observed, but high-severity test-integrity findings block verification".
  Before the fix this same repository was reported `verified`.
- **Pest weakening detection**: changing `toBe(5)` to `toBe(999)` →
  `high tests/CalcTest.php [changed_expected_value] test 'adds two numbers'
  changed the expectation from 'add(2, 3) Eq 5' to 'add(2, 3) Eq 999'`.

Tests: 7 unit tests in `phpanalysis.rs`, 4 new classifier tests and 3 Pest
classifier tests in `framework.rs`, and 11 end-to-end tests in
`tests/php_analysis.rs`, three of which run a **real PHPUnit** via
`WITDIFF_PHPUNIT`.