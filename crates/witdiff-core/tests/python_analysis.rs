//! End-to-end tests for Python structural integrity analysis.
//!
//! These spawn a real Python interpreter, because the analysis runs the
//! embedded script and unit tests over hand-written summaries cannot prove the
//! script works. They are `#[ignore]`d so a plain `cargo test` stays fast and
//! independent of whether Python is installed; run them with:
//!
//! ```text
//! cargo test --workspace --all-features -- --ignored
//! ```
//!
//! Each test skips itself with a clear message when no interpreter is present,
//! rather than failing for an environment reason.

use std::{fs, path::Path, process::Command};

use witdiff_core::pyanalysis::{analyze_python_test_change, Interpreter};

/// Find an interpreter, or return `None` so the test can be skipped.
fn interpreter() -> Option<Interpreter> {
    let command = vec!["python3".to_owned(), "-m".to_owned(), "pytest".to_owned()];
    let probe = Command::new("python3").arg("-c").arg("import ast").output();
    match probe {
        Ok(output) if output.status.success() => {
            Interpreter::from_test_command(&command, Path::new("."))
        }
        _ => None,
    }
}

/// Write a Python file and analyze it against a base revision.
fn analyze(base: Option<&str>, head: &str) -> Vec<witdiff_core::IntegrityFinding> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("test_sample.py");
    fs::write(&path, head).expect("write head");
    let interpreter = interpreter().expect("interpreter");
    analyze_python_test_change("test_sample.py", &interpreter, &path, base)
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn identical_revisions_produce_no_findings() {
    let source = "def test_a():\n    assert f() == 1\n";
    let findings = analyze(Some(source), source);
    assert!(
        findings.is_empty(),
        "an unchanged file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// The property that makes the analysis worth having: reformatting is not a
/// change. This is what the line-based fallback cannot do.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn reformatting_is_not_a_finding() {
    let base = "def test_a():\n    assert f() == 1\n";
    let head = "def test_a():\n    assert f()  ==  1\n";
    let findings = analyze(Some(base), head);
    assert!(
        findings.is_empty(),
        "whitespace-only edits must not register, got {:?}",
        rules(&findings)
    );
}

/// The exact case the support matrix recorded as a gap: a test weakened from an
/// exact assertion to bare truthiness.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn weakening_an_exact_assertion_is_reported() {
    let base = "def test_a():\n    assert f() == 1\n";
    let head = "def test_a():\n    assert f()\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"weakened_assertion"),
        "an exact assertion replaced by a truthiness check must be reported, got {:?}",
        rules(&findings)
    );
    assert!(
        findings
            .iter()
            .any(|f| f.severity == witdiff_core::Severity::High),
        "a weakening is high severity"
    );
}

/// The case measured before this work existed: `assert is_even(3)` becoming
/// `assert True`, which previously produced `verified` with zero findings.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn trivializing_an_assertion_is_reported() {
    let base = "def test_a():\n    assert is_even(3)\n";
    let head = "def test_a():\n    assert True\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"trivial_assertion")
            || rules(&findings).contains(&"weakened_assertion"),
        "an assertion that cannot fail must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn removing_an_assertion_is_reported() {
    let base = "def test_a():\n    assert f() == 1\n    assert g() == 2\n";
    let head = "def test_a():\n    assert f() == 1\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "a deleted assertion must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn changing_the_expected_value_is_reported() {
    let base = "def test_a():\n    assert f() == 1\n";
    let head = "def test_a():\n    assert f() == 2\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "an inverted expectation must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn removing_a_test_is_reported() {
    let base = "def test_a():\n    assert f() == 1\n\n\ndef test_b():\n    assert g() == 2\n";
    let head = "def test_a():\n    assert f() == 1\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"removed_test"),
        "a deleted test must be reported, got {:?}",
        rules(&findings)
    );
}

/// A pre-existing skip is not a new weakening. Without this check, every
/// receipt touching an established test file would be flooded.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_pre_existing_skip_is_not_reported() {
    let source = "import pytest\n\n\n@pytest.mark.skip\ndef test_a():\n    assert f() == 1\n";
    let findings = analyze(Some(source), source);
    assert!(
        !rules(&findings).contains(&"skipped_test"),
        "an existing skip is not a new weakening, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn newly_skipping_a_test_is_reported() {
    let base = "def test_a():\n    assert f() == 1\n";
    let head = "import pytest\n\n\n@pytest.mark.skip\ndef test_a():\n    assert f() == 1\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"skipped_test"),
        "newly skipping a test must be reported, got {:?}",
        rules(&findings)
    );
}

/// Unparsable input must be reported rather than silently producing nothing,
/// which is the contract ADR-0006 established for Rust.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_syntax_error_is_reported_not_swallowed() {
    let findings = analyze(
        Some("def test_a():\n    assert f() == 1\n"),
        "def test_a(:\n",
    );
    assert!(
        rules(&findings).contains(&"test_source_unparsable"),
        "an unparsable file must be reported, got {:?}",
        rules(&findings)
    );
}

/// Adding an assertion is coverage growth, not a finding.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn adding_an_assertion_is_not_reported() {
    let base = "def test_a():\n    assert f() == 1\n";
    let head = "def test_a():\n    assert f() == 1\n    assert g() == 2\n";
    let findings = analyze(Some(base), head);
    assert!(
        findings.is_empty(),
        "adding an assertion is not a weakening, got {:?}",
        rules(&findings)
    );
}

/// A file new in this revision cannot have weakened anything.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_new_file_produces_no_comparative_findings() {
    let head = "def test_a():\n    assert f() == 1\n";
    let findings = analyze(None, head);
    assert!(
        !rules(&findings).contains(&"removed_assertion")
            && !rules(&findings).contains(&"removed_test"),
        "a brand-new file has nothing to remove, got {:?}",
        rules(&findings)
    );
}

/// Regression: adding a negation strengthens the assertion, but it was reported
/// as a removed assertion plus an addition. Found by running the analyzer on a
/// legitimate fix, not by inspection.
///
/// A false positive here is worse than a miss: `block_on_integrity_findings`
/// means it downgrades a genuinely verified change to `not_verified`.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn adding_a_negation_is_not_a_removed_assertion() {
    let base = "def test_a():\n    assert is_even(3)\n";
    let head = "def test_a():\n    assert not is_even(3)\n";
    let findings = analyze(Some(base), head);
    assert!(
        !rules(&findings).contains(&"removed_assertion"),
        "a negated assertion is a rewrite, not a removal, got {:?}",
        rules(&findings)
    );
    assert!(
        findings.is_empty(),
        "strengthening an assertion must produce no findings at all, got {:?}",
        rules(&findings)
    );
}

/// The same shape through an explicit comparison, which is the more common
/// real-world edit.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn strengthening_an_exact_assertion_is_not_a_finding() {
    let base = "def test_a():\n    assert result == 1\n";
    let head = "def test_a():\n    assert result == 1\n    assert helper() == 2\n";
    let findings = analyze(Some(base), head);
    assert!(
        findings.is_empty(),
        "adding an assertion is coverage growth, got {:?}",
        rules(&findings)
    );
}

/// Regression, matching the Rust case: introducing a binding reported the
/// surviving assertion as removed. A pure refactor became a high-severity
/// finding, which blocks verification and downgrades a proven change.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn rebinding_a_subject_is_not_a_removed_assertion() {
    let base = "def test_a():\n    assert f() == 4\n";
    let head = "def test_a():\n    v = f()\n    assert v == 4\n";
    let findings = analyze(Some(base), head);
    assert!(
        findings.is_empty(),
        "a rebinding asserts the same thing, got {:?}",
        rules(&findings)
    );
}

/// The dangerous direction: resolution must not hide a real change.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn rebinding_does_not_hide_a_real_change() {
    let base = "def test_a():\n    assert f() == 4\n";
    let head = "def test_a():\n    v = f()\n    assert v == 5\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a real expectation change must still be reported, got {:?}",
        rules(&findings)
    );
}

/// A name assigned more than once is not resolvable, so the conservative
/// direction is preserved rather than assuming the first binding holds.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_reassigned_name_is_not_resolved() {
    let base = "def test_a():\n    assert f() == 4\n";
    let head = "def test_a():\n    v = g()\n    v = f()\n    assert v == 4\n";
    let findings = analyze(Some(base), head);
    assert!(
        !rules(&findings).contains(&"changed_expected_value"),
        "a reassigned name must not be reported as a changed expectation, got {:?}",
        rules(&findings)
    );
}
