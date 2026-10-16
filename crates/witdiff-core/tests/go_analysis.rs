//! End-to-end tests for Go structural integrity analysis.
//!
//! These spawn the real Go toolchain, because the analysis runs `go run` over an
//! embedded script and unit tests over hand-written summaries cannot prove the
//! script works. They are `#[ignore]`d so a plain `cargo test` stays fast and
//! independent of whether Go is installed.

use std::{fs, path::Path, process::Command};

use witdiff_core::goanalysis::{analyze_go_test_change, GoToolchain};

/// Find the Go toolchain, or return `None` so the test can be skipped.
fn toolchain() -> Option<GoToolchain> {
    match Command::new("go").arg("version").output() {
        Ok(output) if output.status.success() => {
            GoToolchain::from_test_command(&["go".to_owned(), "test".to_owned()], Path::new("."))
        }
        _ => None,
    }
}

fn analyze(base: Option<&str>, head: &str) -> Vec<witdiff_core::IntegrityFinding> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("sample_test.go");
    fs::write(&path, head).expect("write head");
    let toolchain = toolchain().expect("go toolchain");
    analyze_go_test_change("sample_test.go", &toolchain, &path, base)
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

/// A Go test in its idiomatic form: a guarded failure call.
fn go_test(name: &str, guard: &str) -> String {
    format!(
        "package sample\n\nimport \"testing\"\n\nfunc {name}(t *testing.T) {{\n    if {guard} {{\n        t.Errorf(\"failed\")\n    }}\n}}\n"
    )
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn identical_revisions_produce_no_findings() {
    let source = go_test("TestAdd", "Add(1, 1) != 2");
    let findings = analyze(Some(&source), &source);
    assert!(
        findings.is_empty(),
        "an unchanged file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// The core value: an inverted expectation is detected.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn changing_a_guarded_expectation_is_reported() {
    let base = go_test("TestAdd", "Add(1, 1) != 2");
    let head = go_test("TestAdd", "Add(1, 1) != 3");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "an inverted expectation must be reported, got {:?}",
        rules(&findings)
    );
}

/// Removing the guard entirely: the failure call is now unconditional, so the
/// test no longer checks anything meaningful.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn removing_the_guard_is_reported() {
    let base = go_test("TestAdd", "Add(1, 1) != 2");
    let head = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    t.Log(\"no check\")\n}\n";
    let findings = analyze(Some(&base), head);
    assert!(
        !findings.is_empty(),
        "a test whose guard was removed must be reported"
    );
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn removing_a_test_is_reported() {
    // One file cannot hold two `package` clauses, so the second test is built
    // into the same file rather than concatenated as a whole declaration.
    let base = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    if Add(1, 1) != 2 {\n        t.Errorf(\"failed\")\n    }\n}\n\nfunc TestSub(t *testing.T) {\n    if Sub(2, 1) != 1 {\n        t.Errorf(\"failed\")\n    }\n}\n";
    let head = go_test("TestAdd", "Add(1, 1) != 2");
    let findings = analyze(Some(base), &head);
    assert!(
        rules(&findings).contains(&"removed_test"),
        "a deleted test must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn newly_skipping_a_test_is_reported() {
    let base = go_test("TestAdd", "Add(1, 1) != 2");
    let head = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    t.Skip(\"not ready\")\n    if Add(1, 1) != 2 {\n        t.Errorf(\"failed\")\n    }\n}\n";
    let findings = analyze(Some(&base), head);
    assert!(
        rules(&findings).contains(&"skipped_test"),
        "newly skipping a test must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn a_pre_existing_skip_is_not_reported() {
    let source = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    t.Skip(\"not ready\")\n    if Add(1, 1) != 2 {\n        t.Errorf(\"failed\")\n    }\n}\n";
    let findings = analyze(Some(source), source);
    assert!(
        !rules(&findings).contains(&"skipped_test"),
        "an existing skip is not a new weakening, got {:?}",
        rules(&findings)
    );
}

/// A trivial guard asserts nothing: `if true` always fails the test.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn a_trivial_guard_is_reported() {
    let base = go_test("TestAdd", "Add(1, 1) != 2");
    let head = go_test("TestAdd", "true");
    let findings = analyze(Some(&base), &head);
    assert!(
        !findings.is_empty(),
        "a guard that cannot meaningfully fail must be reported"
    );
}

/// Go's failure message is not the assertion. Rewording it must not be
/// reported as a changed expectation, which is why the script walks up to the
/// enclosing `if` rather than reading the message.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn rewording_a_failure_message_is_not_a_finding() {
    let base = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    if Add(1, 1) != 2 {\n        t.Errorf(\"got %d\", Add(1, 1))\n    }\n}\n";
    let head = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    if Add(1, 1) != 2 {\n        t.Errorf(\"addition is wrong: %d\", Add(1, 1))\n    }\n}\n";
    let findings = analyze(Some(base), head);
    assert!(
        findings.is_empty(),
        "reworded message text is not a changed expectation, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn a_syntax_error_is_reported_not_swallowed() {
    let findings = analyze(
        Some(&go_test("TestAdd", "Add(1, 1) != 2")),
        "package sample\n\nfunc TestAdd( {\n",
    );
    assert!(
        rules(&findings).contains(&"test_source_unparsable"),
        "an unparsable file must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn a_new_file_produces_no_comparative_findings() {
    let head = go_test("TestAdd", "Add(1, 1) != 2");
    let findings = analyze(None, &head);
    assert!(
        !rules(&findings).contains(&"removed_test"),
        "a brand-new file has nothing to remove, got {:?}",
        rules(&findings)
    );
}

/// Regression, matching the Rust and Python cases: introducing a binding
/// reported the surviving assertion as removed. A pure refactor became a
/// high-severity finding, which blocks verification.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn rebinding_a_subject_is_not_a_removed_assertion() {
    let base = go_test("TestAdd", "Compute() != 4");
    let head = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    v := Compute()\n    if v != 4 {\n        t.Errorf(\"failed\")\n    }\n}\n";
    let findings = analyze(Some(&base), head);
    assert!(
        findings.is_empty(),
        "a rebinding asserts the same thing, got {:?}",
        rules(&findings)
    );
}

/// The dangerous direction: resolution must not hide a real change.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn rebinding_does_not_hide_a_real_change() {
    let base = go_test("TestAdd", "Compute() != 4");
    let head = "package sample\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n    v := Compute()\n    if v != 5 {\n        t.Errorf(\"failed\")\n    }\n}\n";
    let findings = analyze(Some(&base), head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a real expectation change must still be reported, got {:?}",
        rules(&findings)
    );
}

/// Go has no assertion keyword, so the guard condition IS the assertion and the
/// engine compares it whole. Negating a guard therefore reads as a changed
/// expectation rather than as the same check.
///
/// Measured before this test existed, with the real Go toolchain:
///   `isEven(2)` -> `!isEven(2)`   High removed_assertion
///   `!isEven(2)` -> `isEven(2)`   High removed_assertion
///   `isEven(2)` -> guard deleted  High removed_assertion x2
///
/// A variant fix that stripped the `!` without a matching space made all of
/// those report ZERO, silently allowing the real weakening through. So this is
/// pinned as measured rather than tidied up: the report is blunt, not wrong.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn negating_a_guard_is_reported_as_a_changed_expectation() {
    let base = go_test("TestEven", "isEven(2)");
    let head = go_test("TestEven", "!isEven(2)");
    assert!(
        !analyze(Some(&base), &head).is_empty(),
        "adding a negation to a guard changes when the test fails and must be reported"
    );
    assert!(
        !analyze(Some(&head), &base).is_empty(),
        "removing a negation from a guard must be reported too"
    );
}

/// A testify test in its idiomatic form.
fn testify_test(name: &str, call: &str) -> String {
    format!(
        "package sample\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n)\n\nfunc {name}(t *testing.T) {{\n    {call}\n}}\n"
    )
}

/// testify is the most used Go assertion library, and its calls are not
/// `t.Error*`, so nothing recognized them: every testify-only test summarized
/// with zero assertions AND zero guards, which is why a test whose only
/// assertion was deleted could not be reported at all.
///
/// Measured before `testifyCall` existed, with the real toolchain:
///   assert.Equal -> []   require.NoError -> []   assert.True -> []
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn testify_assertions_are_seen() {
    let base = testify_test("TestAdd", "assert.Equal(t, 5, Add(2, 3))");
    let head = testify_test("TestAdd", "assert.Equal(t, 6, Add(2, 3))");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "inverting a testify expectation must be reported, got {:?}",
        rules(&findings)
    );
}

/// And a testify test that loses its assertion is reported, which it could not
/// be before: with zero assertions on both sides there was nothing to compare.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn deleting_a_testify_assertion_is_reported() {
    let base = testify_test("TestAdd", "assert.Equal(t, 5, Add(2, 3))");
    let head = testify_test("TestAdd", "t.Log(\"no check\")");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "removing the only assertion must be reported, got {:?}",
        rules(&findings)
    );
}

/// `require` aborts where `assert` continues, but both assert the same thing,
/// so a package swap must not read as a change to what the test checks.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn swapping_assert_for_require_is_not_reported() {
    let base = testify_test("TestAdd", "assert.Equal(t, 5, Add(2, 3))");
    let head = testify_test("TestAdd", "require.Equal(t, 5, Add(2, 3))")
        .replace("testify/assert", "testify/require");
    let findings = analyze(Some(&base), &head);
    assert!(
        findings.is_empty(),
        "require and assert make the same claim, got {:?}",
        rules(&findings)
    );
}
