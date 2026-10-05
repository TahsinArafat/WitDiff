//! Test-framework adapters: failure classification and optional targeted
//! invocation.
//!
//! See ADR-0012. Before this module existed, `classify_failure` matched cargo
//! and rustc output only. Measured against real framework output, three of four
//! frameworks misclassified a genuine test failure:
//!
//! ```text
//! pytest   -> CommandFailure   (should be TestFailure)
//! cargo    -> TestFailure
//! jest     -> CommandFailure   (should be TestFailure)
//! go       -> CommandFailure   (should be TestFailure)
//! ```
//!
//! That is not cosmetic. Only `TestFailure` on the base experiment yields a
//! red/green proof; `CommandFailure` produces `not_verified`. A pytest
//! repository therefore could not obtain a proof at all, and the receipt said
//! the failure was unrecognized rather than that the adapter was missing.
//!
//! ## Scope
//!
//! An adapter supplies classification, and optionally a targeted invocation
//! strategy. Discovery is deliberately absent: it is already expressed by
//! `test_globs` and `extra_test_paths`, which are language-neutral. See ADR-0012
//! for why the trait is this narrow.
//!
//! ## The dangerous direction
//!
//! A permissive matcher can report `TestFailure` for something that was not a
//! test failure, which would turn a broken invocation into a proof. Every
//! pattern below keys on a structural marker of the framework's own output, and
//! each classifier is tested against captured real output — including a
//! passing run that must classify as success.

use crate::model::FailureKind;

/// A test framework WitDiff knows how to interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestFramework {
    /// `cargo test`, and rustc for compile errors.
    Cargo,
    /// `pytest` and compatible runners.
    Pytest,
    /// Jest and Vitest.
    JavaScript,
    /// `go test`.
    Go,
    /// Maven Surefire, Gradle test, and JUnit runners.
    Java,
    /// Minitest and RSpec.
    Ruby,
    /// PHPUnit and Pest.
    Php,
}

impl TestFramework {
    /// Stable token used in configuration and errors.
    pub fn as_str(self) -> &'static str {
        match self {
            TestFramework::Cargo => "cargo",
            TestFramework::Pytest => "pytest",
            TestFramework::JavaScript => "javascript",
            TestFramework::Go => "go",
            TestFramework::Java => "java",
            TestFramework::Ruby => "ruby",
            TestFramework::Php => "php",
        }
    }

    /// Parse a configured framework name.
    ///
    /// Returns `None` for an unrecognized name so the caller can fail loudly.
    /// Falling back to the Rust classifier would silently produce the
    /// wrong-conservative answer this module exists to fix.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "cargo" | "rust" => Some(TestFramework::Cargo),
            "pytest" | "py.test" | "python" => Some(TestFramework::Pytest),
            "jest" | "vitest" | "javascript" | "js" | "ts" => Some(TestFramework::JavaScript),
            "go" | "gotest" | "go test" => Some(TestFramework::Go),
            "java" | "junit" | "maven" | "mvn" | "gradle" => Some(TestFramework::Java),
            "ruby" | "minitest" | "rspec" | "rake" => Some(TestFramework::Ruby),
            "php" | "phpunit" | "pest" => Some(TestFramework::Php),
            _ => None,
        }
    }

    /// Every framework WitDiff can classify, for diagnostics.
    pub fn all() -> [TestFramework; 7] {
        [
            TestFramework::Cargo,
            TestFramework::Pytest,
            TestFramework::JavaScript,
            TestFramework::Go,
            TestFramework::Java,
            TestFramework::Ruby,
            TestFramework::Php,
        ]
    }

    /// The default framework for a declared project language.
    ///
    /// Only `rust` has a default, because that is the language the existing
    /// default configuration builds. Other languages must name their framework
    /// explicitly rather than inheriting a guess.
    pub fn for_language(language: &str) -> Option<Self> {
        match language.trim().to_ascii_lowercase().as_str() {
            "rust" => Some(TestFramework::Cargo),
            _ => None,
        }
    }

    /// Classify a finished run from its captured output.
    ///
    /// A successful run is classified by the caller; this only distinguishes
    /// *why* a run failed.
    pub fn classify(self, stdout: &str, stderr: &str, exit_code: Option<i32>) -> FailureKind {
        let combined = format!("{stdout}\n{stderr}");
        let lower = combined.to_lowercase();

        // A framework's test-failure marker is checked before any compile
        // marker, because a run can print both (a compile error inside a test
        // file is still reported by pytest as a collection error).
        // A compile failure is checked FIRST for Ruby, the reverse of every other
        // framework.
        //
        // Measured against real RSpec: a file that fails to load prints
        // `0 examples, 0 failures, 1 error occurred outside of examples` — a
        // non-zero *error* count on the summary line. Checking the test-failure
        // markers first therefore claimed it as a behavioural failure, and a
        // suite that never loaded was reported as evidence that the code
        // behaves differently. The `LoadError` itself is in the output, but it
        // was never consulted.
        if self.is_compile_failure(&lower) {
            return FailureKind::CompileError;
        }
        if self.is_test_failure(&lower) {
            return FailureKind::TestFailure;
        }

        // Some frameworks signal failure only through the exit code and a
        // summary line this classifier does not know. Report that honestly
        // rather than guessing at TestFailure.
        let _ = exit_code;
        FailureKind::CommandFailure
    }

    fn is_test_failure(self, lower: &str) -> bool {
        match self {
            TestFramework::Cargo => {
                lower.contains("test result: failed")
                    || lower.contains("failures:")
                    || lower.contains("tests failed")
                    || lower.contains("panicked at")
            }
            TestFramework::Pytest => {
                // pytest's summary line, e.g. "1 failed, 2 passed in 0.03s",
                // and the short-summary section header.
                lower.contains("= failures =")
                    || lower.contains("short test summary info")
                    || lower.contains("assertionerror")
                    || has_pytest_failed_count(lower)
            }
            TestFramework::JavaScript => {
                // Jest and Vitest both print a "Tests:" summary where a failed
                // count appears, e.g. "Tests: 1 failed, 1 passed, 2 total".
                lower.contains("● ") && lower.contains("failed") || has_jest_failed_count(lower)
            }
            TestFramework::Go => {
                lower.contains("--- fail:")
                    || lower.starts_with("fail")
                    || lower.contains("\nfail\t")
                    || lower.contains("panic:")
            }
            TestFramework::Ruby => {
                // Minitest prints "N runs, N assertions, N failures, N errors,
                // N skips"; RSpec prints "N examples, N failures".
                has_ruby_failure_count(lower)
                    || lower.contains("minitest::assertion")
                    || lower.contains("rspec::expectations")
                    || lower.contains(") failure:")
                    || lower.contains(") error:")
            }
            TestFramework::Java => {
                // Surefire's summary line, e.g.
                // "Tests run: 2, Failures: 1, Errors: 0, Skipped: 0".
                // Both Maven and Gradle print a non-zero Failures or Errors
                // count, and both print the assertion error class name.
                has_surefire_failure_count(lower)
                    || lower.contains("assertionfailederror")
                    || lower.contains("assertionerror")
                    || lower.contains("comparisonfailure")
                    || lower.contains("<<< failure!")
                    || lower.contains("tests completed, ") && lower.contains(" failed")
            }
            TestFramework::Php => {
                // Measured against PHPUnit 10.5.66. A failing assertion prints
                // `There was 1 failure:` and ends with `FAILURES!`; a thrown
                // exception prints `ERRORS!`. Both carry a non-zero count on the
                // `Tests: N, Assertions: N, Failures: F, Errors: E` summary
                // line, which is what the count helper keys on.
                has_phpunit_failure_count(lower)
                    || lower.contains("failures!")
                    || lower.contains("errors!")
                    || lower.contains(") failure:")
                    || lower.contains("failed asserting that")
            }
        }
    }

    fn is_compile_failure(self, lower: &str) -> bool {
        match self {
            TestFramework::Cargo => {
                lower.contains("could not compile")
                    || lower.contains("error[e")
                    || lower.contains("error: expected")
            }
            TestFramework::Pytest => {
                // A syntax error or a collection failure, as distinct from an
                // assertion failure.
                lower.contains("syntaxerror")
                    || lower.contains("error collecting")
                    || lower.contains("importerror")
            }
            TestFramework::JavaScript => {
                lower.contains("cannot find module")
                    || lower.contains("failed to compile")
                    || lower.contains("syntaxerror")
                    || lower.contains("tsc")
            }
            TestFramework::Go => {
                lower.contains("build failed")
                    || lower.contains("cannot find package")
                    || lower.contains("undefined:")
                    || lower.contains("# ") && lower.contains(".go:")
            }
            TestFramework::Ruby => {
                lower.contains("(loaderror)")
                    || lower.contains("syntaxerror")
                    || lower.contains("cannot load such file")
                    || lower.contains("nameerror")
            }
            TestFramework::Java => {
                // Maven prints "COMPILATION ERROR" and Gradle prints
                // "compileJava FAILED" or a Kotlin/Java compile error.
                lower.contains("compilation error")
                    || lower.contains("compilation failure")
                    || lower.contains("compilejava failed")
                    || lower.contains("compiletestjava failed")
                    || lower.contains("cannot find symbol")
                    || lower.contains("error: ';' expected")
            }
            TestFramework::Php => {
                // Measured: PHPUnit 10 reports a file it cannot parse as
                // `An error occurred inside PHPUnit.` with
                // `Message:  syntax error, ...` and exits 255, printing no
                // summary line at all. A missing class or trait is the other
                // way a suite fails before any test runs.
                lower.contains("an error occurred inside phpunit")
                    || lower.contains("syntax error")
                    || lower.contains("parse error")
                    || lower.contains("fatal error")
                    || lower.contains("could not find class")
                    || lower.contains("class \"") && lower.contains("not found")
            }
        }
    }
}

/// Whether PHPUnit or Pest reports a non-zero failure or error count.
///
/// The two runners share a `Tests:` prefix and nothing else, which is why this
/// exists rather than two substring checks:
///
/// - PHPUnit 10.5.66: `Tests: 2, Assertions: 2, Failures: 1.`
/// - Pest 3: `Tests:    1 failed, 1 passed (2 assertions)`
///
/// The Pest form is load-bearing. Measured against real Pest, a test that
/// errors on something other than an assertion prints neither `FAILURES!` nor
/// `failed asserting that` — only `FAILED  Tests\CalcTest > undefined method
/// Error` and `Tests:    1 failed (0 assertions)`. Without this helper that
/// classified as `CommandFailure`, which cannot produce a proof, so Pest
/// projects silently lost their red/green evidence.
///
/// A passing run is `Tests: 1 passed (1 assertions)` and a fully skipped run
/// is `Tests: 1 skipped (0 assertions)`; neither contains a non-zero count.
fn has_phpunit_failure_count(lower: &str) -> bool {
    for line in lower.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("tests:") {
            continue;
        }
        for label in ["failures:", "errors:"] {
            if let Some(index) = trimmed.find(label) {
                let count: String = trimmed[index + label.len()..]
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if count.parse::<u64>().unwrap_or(0) > 0 {
                    return true;
                }
            }
        }
        // Pest's wording: a bare `N failed` on the same line.
        if let Some(index) = trimmed.find(" failed") {
            let count: String = trimmed[..index]
                .chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect();
            if count
                .chars()
                .rev()
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
                > 0
            {
                return true;
            }
        }
    }
    false
}

/// Whether pytest's summary reports a non-zero failed count.
///
/// Looks for the `<n> failed` token that precedes `passed`/`error` in pytest's
/// summary, so a *passing* run (`2 passed in 0.01s`) does not match.
fn has_pytest_failed_count(lower: &str) -> bool {
    for line in lower.lines() {
        let line = line.trim();
        if !line.contains(" in ") && !line.contains("=") {
            continue;
        }
        if let Some(index) = line.find(" failed") {
            let prefix = &line[..index];
            let count: String = prefix
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if count
                .chars()
                .rev()
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
                > 0
            {
                return true;
            }
        }
    }
    false
}

/// Whether a Ruby runner reports a non-zero failure or error count.
///
/// Minitest's summary is `1 runs, 1 assertions, 1 failures, 0 errors, 0 skips`
/// and RSpec's is `2 examples, 1 failure`. A passing run reports zeros, so it
/// must not match.
fn has_ruby_failure_count(lower: &str) -> bool {
    for line in lower.lines() {
        let trimmed = line.trim();
        // The summary line contains a count of runs or examples and at least
        // one of the outcome counts.
        //
        // Both the singular and the plural are matched. Measured against real
        // RSpec: a one-example suite prints `1 example, 1 failure`, and the
        // previous plural-only test skipped it, so an ordinary red suite was
        // classified as an unrecognized command failure — which cannot produce
        // a proof, so every single-example Ruby suite lost its red/green
        // evidence. Minitest has the same singular `1 run, 1 failures, 0
        // errors` shape.
        if !(trimmed.contains(" runs,")
            || trimmed.contains(" run,")
            || trimmed.contains(" examples,")
            || trimmed.contains(" example,")
            || trimmed.starts_with("failures:"))
        {
            continue;
        }
        // Minitest writes "1 failures, 0 errors" and RSpec writes
        // "1 failure" in the singular, so both forms are matched. A passing run
        // reports zeros either way.
        for label in [" failures", " failure", " errors", " error"] {
            if let Some(index) = trimmed.find(label) {
                let count: String = trimmed[..index]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit() || *c == ' ')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if count.trim().parse::<u64>().unwrap_or(0) > 0 {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether Maven Surefire reports a non-zero failure or error count.
///
/// The summary line is `Tests run: N, Failures: F, Errors: E, Skipped: S`, so a
/// *passing* run (`Failures: 0, Errors: 0`) must not match.
fn has_surefire_failure_count(lower: &str) -> bool {
    for line in lower.lines() {
        if !line.contains("tests run:") {
            continue;
        }
        for label in ["failures:", "errors:"] {
            if let Some(index) = line.find(label) {
                let count: String = line[index + label.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if count.parse::<u64>().unwrap_or(0) > 0 {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether Jest or Vitest reports a non-zero failed test count.
fn has_jest_failed_count(lower: &str) -> bool {
    for line in lower.lines() {
        let trimmed = line.trim();
        // Jest writes "Tests:       1 failed, 1 passed, 2 total" and Vitest
        // writes "Tests  1 failed | 1 passed (2)" — the separator differs, so
        // both are accepted rather than only the colon form.
        let is_summary = trimmed.starts_with("tests:")
            || trimmed.starts_with("tests ")
            || trimmed.starts_with("test files")
            || trimmed.starts_with("test suites");
        if !is_summary {
            continue;
        }
        if let Some(index) = trimmed.find(" failed") {
            let prefix = &trimmed[..index];
            let count: String = prefix
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if count
                .chars()
                .rev()
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
                > 0
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every string below is captured from a real run of the named framework,
    // not invented. The ones that are paraphrased are marked as such.

    #[test]
    fn cargo_test_failure_is_recognized() {
        let stdout = "running 1 test\ntest tests::t ... FAILED\n\nfailures:\n\ntest result: FAILED. 0 passed; 1 failed\n";
        assert_eq!(
            TestFramework::Cargo.classify(stdout, "", Some(101)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn cargo_compile_failure_is_recognized() {
        let stderr = "error[E0425]: cannot find function `helper` in this scope\nerror: could not compile `x` (lib test) due to 1 previous error\n";
        assert_eq!(
            TestFramework::Cargo.classify("", stderr, Some(101)),
            FailureKind::CompileError
        );
    }

    /// Captured from a real pytest run.
    #[test]
    fn pytest_test_failure_is_recognized() {
        let stdout = "    def test_fails():\n>       assert 1 + 1 == 3\nE       assert (1 + 1) == 3\n\ntest_sample.py:5: AssertionError\n=========================== short test summary info ============================\nFAILED test_sample.py::test_fails - assert (1 + 1) == 3\n1 failed, 1 passed in 0.02s\n";
        assert_eq!(
            TestFramework::Pytest.classify(stdout, "", Some(1)),
            FailureKind::TestFailure,
            "a real pytest failure must classify as a test failure"
        );
    }

    /// The dangerous direction: a passing pytest run must not match.
    #[test]
    fn passing_pytest_run_is_not_a_test_failure() {
        let stdout = "..                                                                       [100%]\n2 passed in 0.01s\n";
        assert_ne!(
            TestFramework::Pytest.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn pytest_syntax_error_is_a_compile_failure() {
        let stdout = "E   SyntaxError: invalid syntax\n";
        assert_eq!(
            TestFramework::Pytest.classify(stdout, "", Some(2)),
            FailureKind::CompileError
        );
    }

    /// Captured from a real Jest run.
    #[test]
    fn jest_test_failure_is_recognized() {
        let stderr = "  ● sample › fails\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 1 passed, 2 total\n";
        assert_eq!(
            TestFramework::JavaScript.classify("", stderr, Some(1)),
            FailureKind::TestFailure
        );
    }

    /// Vitest writes the same shape to stdout.
    #[test]
    fn vitest_test_failure_is_recognized() {
        let stdout = " FAIL  test/sample.test.ts > adds\nAssertionError: expected 2 to be 3\n\n Test Files  1 failed (1)\n      Tests  1 failed | 1 passed (2)\n";
        assert_eq!(
            TestFramework::JavaScript.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn passing_jest_run_is_not_a_test_failure() {
        let stderr = "Tests:       2 passed, 2 total\n";
        assert_ne!(
            TestFramework::JavaScript.classify("", stderr, Some(0)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn jest_missing_module_is_a_compile_failure() {
        let stderr = "Cannot find module './missing' from 'sample.test.js'\n";
        assert_eq!(
            TestFramework::JavaScript.classify("", stderr, Some(1)),
            FailureKind::CompileError
        );
    }

    /// Captured from a real `go test` run.
    #[test]
    fn go_test_failure_is_recognized() {
        let stdout = "--- FAIL: TestAdd (0.00s)\n    sample_test.go:10: got 2, want 3\nFAIL\nexit status 1\nFAIL\tsample\t0.003s\n";
        assert_eq!(
            TestFramework::Go.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn passing_go_run_is_not_a_test_failure() {
        let stdout = "ok  \tsample\t0.002s\n";
        assert_ne!(
            TestFramework::Go.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn go_build_failure_is_a_compile_failure() {
        let stderr = "# sample\n./sample.go:5:2: undefined: helper\n";
        assert_eq!(
            TestFramework::Go.classify("", stderr, Some(2)),
            FailureKind::CompileError
        );
    }

    /// A framework this classifier has no markers for must not be reported as a
    /// test failure, because that would turn a broken invocation into a proof.
    #[test]
    fn unrecognized_output_is_not_reported_as_a_test_failure() {
        let outcome = TestFramework::Pytest.classify("segmentation fault", "", Some(139));
        assert_eq!(outcome, FailureKind::CommandFailure);
    }

    /// The exact regression this ADR fixes, asserted against every framework.
    ///
    /// Driven by [`TestFramework::all`] rather than a hand-written list, so
    /// adding a framework cannot silently skip this assertion — which is
    /// exactly what had happened: the list still named four frameworks after
    /// Java and Ruby shipped, and the samples below had never been extended to
    /// cover them.
    #[test]
    fn every_supported_framework_recognizes_its_own_failure_output() {
        let samples = [
            (
                TestFramework::Cargo,
                "test result: FAILED. 0 passed; 1 failed\n",
            ),
            (
                TestFramework::Pytest,
                "short test summary info\n1 failed, 1 passed in 0.02s\n",
            ),
            (
                TestFramework::JavaScript,
                "Tests:       1 failed, 1 passed, 2 total\n",
            ),
            (TestFramework::Go, "--- FAIL: TestAdd (0.00s)\nFAIL\n"),
            (
                TestFramework::Java,
                "Tests run: 2, Failures: 1, Errors: 0, Skipped: 0\n",
            ),
            (TestFramework::Ruby, "2 examples, 1 failure\n"),
            (
                TestFramework::Php,
                "FAILURES!\nTests: 2, Assertions: 2, Failures: 1.\n",
            ),
        ];
        assert_eq!(
            samples.len(),
            TestFramework::all().len(),
            "every framework needs a captured sample in this test"
        );
        for (framework, output) in samples {
            assert_eq!(
                framework.classify(output, "", Some(1)),
                FailureKind::TestFailure,
                "{} must recognize its own failure output",
                framework.as_str()
            );
        }
    }

    #[test]
    fn framework_names_round_trip_and_reject_unknown() {
        for framework in TestFramework::all() {
            assert_eq!(
                TestFramework::parse(framework.as_str()),
                Some(framework),
                "{} must parse back to itself",
                framework.as_str()
            );
        }
        assert_eq!(TestFramework::parse("nonsense"), None);
        assert_eq!(TestFramework::parse(""), None);
    }

    #[test]
    fn only_rust_has_a_language_default() {
        assert_eq!(
            TestFramework::for_language("rust"),
            Some(TestFramework::Cargo)
        );
        assert_eq!(TestFramework::for_language("python"), None);
        assert_eq!(TestFramework::for_language("go"), None);
    }
}

#[cfg(test)]
mod java_tests {
    use super::*;

    /// Captured from Maven Surefire output.
    #[test]
    fn maven_test_failure_is_recognized() {
        let stdout = "[ERROR] Tests run: 2, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.05 s <<< FAILURE! -- in CalcTest\n[ERROR] CalcTest.adds -- Time elapsed: 0.01 s <<< FAILURE!\norg.opentest4j.AssertionFailedError: expected: <2> but was: <3>\n\n[INFO] BUILD FAILURE\n";
        assert_eq!(
            TestFramework::Java.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    /// Captured from Gradle output.
    #[test]
    fn gradle_test_failure_is_recognized() {
        let stdout = "CalcTest > adds FAILED\n    org.opentest4j.AssertionFailedError at CalcTest.java:8\n\n2 tests completed, 1 failed\nFAILURE: Build failed with an exception.\n";
        assert_eq!(
            TestFramework::Java.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    /// The dangerous direction: a passing Maven run must not be reported as a
    /// test failure, or a broken invocation could look like a proof.
    #[test]
    fn passing_maven_run_is_not_a_test_failure() {
        let stdout = "[INFO] Tests run: 2, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.03 s -- in CalcTest\n[INFO] BUILD SUCCESS\n";
        assert_ne!(
            TestFramework::Java.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    /// A Maven compile failure is a compile error, not a test failure.
    #[test]
    fn maven_compilation_error_is_a_compile_failure() {
        let stdout = "[ERROR] COMPILATION ERROR :\n[ERROR] /src/test/java/CalcTest.java:[8,9] cannot find symbol\n";
        assert_eq!(
            TestFramework::Java.classify(stdout, "", Some(1)),
            FailureKind::CompileError
        );
    }

    /// Surefire reports `Errors:` separately from `Failures:`, and an error is
    /// also a failing test run.
    #[test]
    fn surefire_errors_count_as_a_test_failure() {
        let stdout = "[ERROR] Tests run: 1, Failures: 0, Errors: 1, Skipped: 0\n";
        assert_eq!(
            TestFramework::Java.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn java_names_parse() {
        for name in ["java", "junit", "maven", "mvn", "gradle"] {
            assert_eq!(
                TestFramework::parse(name),
                Some(TestFramework::Java),
                "{name} should name the Java framework"
            );
        }
        assert_eq!(TestFramework::Java.as_str(), "java");
    }
}

#[cfg(test)]
mod ruby_tests {
    use super::*;

    /// Captured from a real `ruby t.rb` run using the minitest that ships in
    /// Ruby's standard library.
    #[test]
    fn minitest_failure_is_recognized() {
        let stdout = "Run options: --seed 34535\n\n# Running:\n\nF\n\nFinished in 0.000187s, 5347.5936 runs/s\n\n  1) Failure:\nCalcTest#test_adds [t.rb:5]:\nExpected: 3\n  Actual: 2\n\n1 runs, 1 assertions, 1 failures, 0 errors, 0 skips\n";
        assert_eq!(
            TestFramework::Ruby.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    /// The dangerous direction: a passing minitest run reports zeros and must
    /// not be read as a failure, or a broken invocation could look like a proof.
    #[test]
    fn passing_minitest_run_is_not_a_test_failure() {
        let stdout = "Finished in 0.000171s, 5847.9535 runs/s\n\n1 runs, 1 assertions, 0 failures, 0 errors, 0 skips\n";
        assert_ne!(
            TestFramework::Ruby.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    /// Captured from a real Ruby LoadError.
    #[test]
    fn a_ruby_load_error_is_a_compile_failure() {
        let stderr = "kernel_require.rb:54:in `require': cannot load such file -- nope_missing (LoadError)\n";
        assert_eq!(
            TestFramework::Ruby.classify("", stderr, Some(1)),
            FailureKind::CompileError
        );
    }

    /// RSpec's summary shape, which differs from Minitest's.
    #[test]
    fn rspec_failure_is_recognized() {
        let stdout = "Failures:\n  1) Calc adds\n     Failure/Error: expect(add(1, 1)).to eq(3)\n\n2 examples, 1 failure\n";
        assert_eq!(
            TestFramework::Ruby.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn passing_rspec_run_is_not_a_test_failure() {
        let stdout = "2 examples, 0 failures\n";
        assert_ne!(
            TestFramework::Ruby.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn ruby_names_parse() {
        for name in ["ruby", "minitest", "rspec", "rake"] {
            assert_eq!(
                TestFramework::parse(name),
                Some(TestFramework::Ruby),
                "{name}"
            );
        }
        assert_eq!(TestFramework::Ruby.as_str(), "ruby");
    }

    #[test]
    fn php_names_parse() {
        for name in ["php", "phpunit", "pest"] {
            assert_eq!(
                TestFramework::parse(name),
                Some(TestFramework::Php),
                "{name}"
            );
        }
        assert_eq!(TestFramework::Php.as_str(), "php");
    }

    /// Captured from a real PHPUnit 10.5.66 run: one failing assertion among
    /// two passing tests, exit code 1.
    #[test]
    fn phpunit_assertion_failure_is_recognized() {
        let stdout = "PHPUnit 10.5.66 by Sebastian Bergmann and contributors.\n\nRuntime:       PHP 8.5.9\n\nF.                                                                  2 / 2 (100%)\n\nTime: 00:00.008, Memory: 8.00 MB\n\nThere was 1 failure:\n\n1) CalcTest::testAdd\nFailed asserting that 5 matches expected 6.\n\n/private/tmp/phpunitfx/tests/CalcTest.php:5\n\nFAILURES!\nTests: 2, Assertions: 2, Failures: 1.\n";
        assert_eq!(
            TestFramework::Php.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    /// Captured from a real PHPUnit run: a test that throws. This is an error,
    /// not a failed assertion, but both are behavioural — a compile failure is
    /// not, so this must not be classified as one.
    #[test]
    fn phpunit_thrown_error_is_a_test_failure() {
        let stdout = "1) CalcTest::testAdd\nRuntimeException: boom\n\n/private/tmp/phpunitfx/tests/CalcTest.php:5\n\nERRORS!\nTests: 1, Assertions: 0, Errors: 1.\n";
        assert_eq!(
            TestFramework::Php.classify(stdout, "", Some(2)),
            FailureKind::TestFailure
        );
    }

    /// A passing run must never be read as a failure. It prints `OK (N tests,
    /// N assertions)` with no `Tests:` line at all.
    #[test]
    fn passing_phpunit_run_is_not_a_test_failure() {
        let stdout = "PHPUnit 10.5.66 by Sebastian Bergmann and contributors.\n\n..                                                                  2 / 2 (100%)\n\nTime: 00:00.002, Memory: 8.00 MB\n\nOK (2 tests, 2 assertions)\n";
        assert_ne!(
            TestFramework::Php.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
        assert_ne!(
            TestFramework::Php.classify(stdout, "", Some(0)),
            FailureKind::CompileError
        );
    }

    /// Captured from a real PHPUnit run: every test skipped. The count line
    /// reads `Skipped: 1` with zero failures, and the run exits 0. It must not
    /// be mistaken for a red suite, which would make an empty suite look like
    /// evidence.
    #[test]
    fn skipped_phpunit_run_is_not_a_test_failure() {
        let stdout = "S                                                                   1 / 1 (100%)\n\nTime: 00:00.003, Memory: 8.00 MB\n\nOK, but some tests were skipped!\nTests: 1, Assertions: 0, Skipped: 1.\n";
        assert_ne!(
            TestFramework::Php.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    /// Captured from a real PHPUnit run: a file that cannot be parsed. PHPUnit
    /// exits 255 with no summary line, and treating it as a behavioural failure
    /// would let a base that never compiled stand in for a real regression.
    #[test]
    fn unparsable_phpunit_file_is_a_compile_failure() {
        let stdout = "An error occurred inside PHPUnit.\n\nMessage:  syntax error, unexpected token \"{\"\nLocation: /private/tmp/phpunitfx/tests/CalcTest.php:2\n\n#0 /private/tmp/phpunitfx/vendor/phpunit/phpunit/src/Runner/TestSuiteLoader.php(48): PHPUnit\\Runner\\TestSuiteLoader->loadSuiteClassFile()\n";
        assert_eq!(
            TestFramework::Php.classify(stdout, "", Some(255)),
            FailureKind::CompileError
        );
    }

    /// An unrecognized PHP failure must not be guessed at. Reporting
    /// `CommandFailure` is honest and cannot produce a proof.
    #[test]
    fn unknown_php_failure_is_not_guessed() {
        assert_eq!(
            TestFramework::Php.classify("something went wrong\n", "", Some(1)),
            FailureKind::CommandFailure
        );
    }

    /// Captured from a real Pest 3 run: an assertion failure. Pest prints no
    /// `FAILURES!` marker, so this depends on the count line.
    #[test]
    fn pest_assertion_failure_is_recognized() {
        let stdout = "   FAIL  Tests\\CalcTest\n  \u{2a2f} adds\n  \u{2713} passes\n   FAILED  Tests\\CalcTest > adds\n  Failed asserting that 5 is identical to 6.\n\n  Tests:    1 failed, 1 passed (2 assertions)\n  Duration: 0.03s\n";
        assert_eq!(
            TestFramework::Php.classify(stdout, "", Some(1)),
            FailureKind::TestFailure
        );
    }

    /// Captured from a real Pest 3 run: a test that errors on something other
    /// than an assertion — `Error` rather than a failed expectation. This
    /// prints neither `FAILURES!` nor `failed asserting that`, so the count
    /// line is the only signal. Classified as `CommandFailure` before that was
    /// handled, which silently denied every Pest project its proof.
    #[test]
    fn pest_non_assertion_error_is_recognized() {
        let stdout = "   FAILED  Tests\\CalcTest > undefined method                            Error   \n  Tests:    1 failed (0 assertions)\n  Duration: 0.02s\n";
        assert_eq!(
            TestFramework::Php.classify(stdout, "", Some(2)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn passing_pest_run_is_not_a_test_failure() {
        let stdout = "  \u{2713} passes\n  Tests:    1 passed (1 assertions)\n  Duration: 0.03s\n";
        assert_ne!(
            TestFramework::Php.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }

    #[test]
    fn skipped_pest_run_is_not_a_test_failure() {
        let stdout = "  - skips\n  Tests:    1 skipped (0 assertions)\n  Duration: 0.02s\n";
        assert_ne!(
            TestFramework::Php.classify(stdout, "", Some(0)),
            FailureKind::TestFailure
        );
    }
}
