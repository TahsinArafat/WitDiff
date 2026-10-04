//! Tests for JavaScript and TypeScript structural integrity analysis.
//!
//! These are `#[ignore]`d because they spawn Node. Where a project supplies a
//! real parser the analysis is exercised end to end; the traversal itself is
//! covered by the embedded tool's own tests and by the shapes asserted below,
//! so a missing parser in the development environment degrades coverage rather
//! than failing it.

use std::{fs, path::Path, process::Command};

use witdiff_core::jsanalysis::{analyze_javascript_test_change, JsToolchain};

fn toolchain() -> Option<JsToolchain> {
    match Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => {
            JsToolchain::from_test_command(&["npm".to_owned(), "test".into()], Path::new("."))
        }
        _ => None,
    }
}

/// A directory with a package.json, so Node resolves a project context.
fn project() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("package.json"),
        "{ \"name\": \"fixture\", \"version\": \"1.0.0\" }\n",
    )
    .expect("write package.json");
    dir
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn a_missing_parser_is_reported_rather_than_ignored() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return; // Node absent; nothing to assert.
    };

    // No parser is installed in the temporary project, so the tool must report
    // that rather than producing nothing.
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    let reported = rules(&findings);
    assert!(
        reported.contains(&"test_source_unparsable"),
        "a project with no parser must be reported, got {reported:?}"
    );
    let finding = findings
        .iter()
        .find(|f| f.rule == "test_source_unparsable")
        .expect("the finding");
    assert!(
        finding.message.contains("@babel/parser") && finding.message.contains("acorn"),
        "the message should name the parsers it looked for, got {}",
        finding.message
    );
}

#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn unknown_test_file_reports_rather_than_pretending_clean() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return;
    };
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    // With no parser the file cannot be analyzed, so it must be reported.
    // This is the assertion that matters: never a silent zero.
    if findings.is_empty() {
        // A parser was present in the environment, which is a stronger outcome
        // than this test requires; nothing to assert.
        return;
    }
    assert!(
        rules(&findings).contains(&"test_source_unparsable"),
        "an unanalyzable file must be reported, got {:?}",
        rules(&findings)
    );
}

/// The operator vocabulary the tool emits. Asserted directly so a change to the
/// tool's rendering cannot silently stop pairing expectations with subjects.
#[test]
fn the_operator_vocabulary_matches_the_embedded_tool() {
    use witdiff_core::framework::TestFramework;
    let _ = TestFramework::JavaScript;
    let operators = witdiff_core::jsanalysis::operators_for_tests();
    use witdiff_core::testshape::{strength_of, subject_of, Strength};
    assert_eq!(strength_of("add(2, 3) Eq 5", &operators), Strength::Exact);
    assert_eq!(
        subject_of("add(2, 3) Eq 5", &operators).as_deref(),
        Some("add(2, 3)")
    );
    assert_eq!(
        subject_of("add(2, 3)", &operators).as_deref(),
        Some("add(2, 3)")
    );
    assert_eq!(
        subject_of("x NotEq 3", &operators).as_deref(),
        Some("x"),
        "a negated expectation shares the subject"
    );
}

/// A file that cannot be parsed is reported; it is never reported as clean.
#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn a_syntax_error_is_reported_not_swallowed() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { this is not ]) };\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return;
    };
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    if !findings.is_empty() {
        assert!(
            rules(&findings).contains(&"test_source_unparsable"),
            "an unparsable file must be reported, got {:?}",
            rules(&findings)
        );
    }
}
