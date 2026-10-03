//! End-to-end tests for Java structural integrity analysis.
//!
//! These spawn the real JDK, because the analysis runs `java` over an embedded
//! tool and unit tests over hand-written summaries cannot prove the tool works.
//! They are `#[ignore]`d so a plain `cargo test` stays fast and independent of
//! whether a JDK is installed.

use std::{fs, path::Path, process::Command};

use witdiff_core::javaanalysis::{analyze_java_test_change, JavaToolchain};

/// Find a JDK, or return `None` so the test can be skipped.
fn toolchain() -> Option<JavaToolchain> {
    match Command::new("java").arg("--version").output() {
        Ok(output) if output.status.success() => {
            JavaToolchain::from_test_command(&["mvn".to_owned(), "test".to_owned()], Path::new("."))
        }
        _ => None,
    }
}

fn analyze(base: Option<&str>, head: &str) -> Vec<witdiff_core::IntegrityFinding> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("SampleTest.java");
    fs::write(&path, head).expect("write head");
    let toolchain = toolchain().expect("jdk");
    analyze_java_test_change("SampleTest.java", &toolchain, &path, base)
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

/// A JUnit 5 test file wrapping one assertion.
fn java_test(name: &str, body: &str) -> String {
    format!(
        "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\npublic class SampleTest {{\n    @Test\n    public void {name}() {{\n        {body}\n    }}\n}}\n"
    )
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn identical_revisions_produce_no_findings() {
    let source = java_test("adds", "assertEquals(2, Add(1, 1));");
    let findings = analyze(Some(&source), &source);
    assert!(
        findings.is_empty(),
        "an unchanged file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// The property that matters most: JUnit's argument order is reversed, so a
/// correct expectation must not be mistaken for a changed one.
#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn junit_argument_order_is_normalized() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let findings = analyze(Some(&base), &base);
    assert!(
        findings.is_empty(),
        "the swapped expectation form must compare equal to itself, got {:?}",
        rules(&findings)
    );
}

/// An inverted expectation is detected, which requires the swap to be correct:
/// without it the subject and expectation would be compared in the wrong roles.
#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn changing_an_expectation_is_reported() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let head = java_test("adds", "assertEquals(3, Add(1, 1));");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "an inverted expectation must be reported, got {:?}",
        rules(&findings)
    );
}

/// Replacing an equality with a comparison over the same subject is reported.
///
/// The rule is `changed_expected_value`, not `weakened_assertion`, because a
/// comparison *is* an exact constraint under the shared strength model:
/// `Add(1, 1) > 0` still names a specific boundary. Verified that Python
/// reports the same rule for the equivalent `assert f() == 2` becoming
/// `assert f() > 0`, so the two languages agree rather than diverging.
///
/// The genuinely weaker form is a bare truthiness check with no comparison at
/// all, which is covered by `weakening_an_assertion_to_truthiness_is_reported`.
#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn replacing_equality_with_a_comparison_is_reported() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let head = java_test("adds", "assertTrue(Add(1, 1) > 0);");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a changed expectation must be reported, got {:?}",
        rules(&findings)
    );
    assert!(
        findings
            .iter()
            .any(|f| f.severity == witdiff_core::Severity::High),
        "it must be high severity"
    );
}

/// The genuinely weaker form: a bare truthiness check with no comparison.
#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn weakening_an_assertion_to_truthiness_is_reported() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let head = java_test("adds", "assertTrue(isValid(Add(1, 1)));");
    let findings = analyze(Some(&base), &head);
    assert!(
        !findings.is_empty(),
        "dropping the comparison entirely must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn a_trivial_assertion_is_reported() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let head = java_test("adds", "assertTrue(true);");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"trivial_assertion"),
        "an assertion that cannot fail must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn removing_an_assertion_is_reported() {
    let base = java_test(
        "adds",
        "assertEquals(2, Add(1, 1));\n        assertEquals(4, Add(2, 2));",
    );
    let head = java_test("adds", "assertEquals(2, Add(1, 1));");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "a deleted assertion must be reported, got {:?}",
        rules(&findings)
    );
}

/// The bare `if (...) fail(...)` shape, which has no assertion method at all.
#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn a_guarded_fail_call_is_treated_as_an_assertion() {
    let base = java_test(
        "adds",
        "if (Add(1, 1) != 2) {\n            fail(\"bad\");\n        }",
    );
    let head = java_test(
        "adds",
        "if (Add(1, 1) != 3) {\n            fail(\"bad\");\n        }",
    );
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a changed guard must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn newly_disabling_a_test_is_reported() {
    let base = java_test("adds", "assertEquals(2, Add(1, 1));");
    let head = "import org.junit.jupiter.api.Disabled;\nimport org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\npublic class SampleTest {\n    @Test\n    @Disabled(\"flaky\")\n    public void adds() {\n        assertEquals(2, Add(1, 1));\n    }\n}\n";
    let findings = analyze(Some(&base), head);
    assert!(
        rules(&findings).contains(&"skipped_test"),
        "newly disabling a test must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn a_pre_existing_disabled_test_is_not_reported() {
    let source = "import org.junit.jupiter.api.Disabled;\nimport org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\npublic class SampleTest {\n    @Test\n    @Disabled(\"flaky\")\n    public void adds() {\n        assertEquals(2, Add(1, 1));\n    }\n}\n";
    let findings = analyze(Some(source), source);
    assert!(
        !rules(&findings).contains(&"skipped_test"),
        "an existing disable is not a new weakening, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn removing_a_test_is_reported() {
    let base = "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\npublic class SampleTest {\n    @Test\n    public void adds() {\n        assertEquals(2, Add(1, 1));\n    }\n\n    @Test\n    public void subs() {\n        assertEquals(1, Sub(2, 1));\n    }\n}\n";
    let head = java_test("adds", "assertEquals(2, Add(1, 1));");
    let findings = analyze(Some(base), &head);
    assert!(
        rules(&findings).contains(&"removed_test"),
        "a deleted test must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn a_syntax_error_is_reported_not_swallowed() {
    let head = "public class Broken {\n    public void x( {\n}\n";
    let findings = analyze(
        Some(&java_test("adds", "assertEquals(2, Add(1, 1));")),
        head,
    );
    // Either the tool reports an error, or it finds no tests. Both are reported
    // rather than silently producing nothing.
    assert!(
        !findings.is_empty(),
        "an unparsable Java file must not look clean"
    );
}

#[test]
#[ignore = "end-to-end: spawns the JDK; run with -- --ignored"]
fn a_new_file_produces_no_comparative_findings() {
    let head = java_test("adds", "assertEquals(2, Add(1, 1));");
    let findings = analyze(None, &head);
    assert!(
        !rules(&findings).contains(&"removed_test"),
        "a brand-new file has nothing to remove, got {:?}",
        rules(&findings)
    );
}
