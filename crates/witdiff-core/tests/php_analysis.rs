//! End-to-end tests for PHP structural integrity analysis.
//!
//! These spawn a real PHP interpreter, because the analysis runs `token_get_all`
//! over an embedded tool and unit tests over hand-written summaries cannot prove
//! the tool works. They are `#[ignore]`d so a plain `cargo test` stays fast and
//! independent of whether PHP is installed.
//!
//! The PHPUnit tests run against a real PHPUnit when one is installed under
//! `WITDIFF_PHPUNIT`, because the point of ADR-0023 is that the classifier was
//! measured against real output rather than remembered output.

use std::{fs, path::Path, process::Command};

use witdiff_core::framework::TestFramework;
use witdiff_core::model::FailureKind;
use witdiff_core::phpanalysis::{analyze_php_test_change, PhpToolchain};

fn toolchain() -> Option<PhpToolchain> {
    match Command::new("php").arg("-v").output() {
        Ok(output) if output.status.success() => {
            PhpToolchain::from_test_command(&["phpunit".to_owned()], Path::new("."))
        }
        _ => None,
    }
}

fn analyze(base: Option<&str>, head: &str) -> Vec<witdiff_core::IntegrityFinding> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("CalcTest.php");
    fs::write(&path, head).expect("write head");
    let toolchain = toolchain().expect("php");
    analyze_php_test_change("CalcTest.php", &toolchain, &path, base)
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

fn phpunit_class(body: &str) -> String {
    format!(
        "<?php\nuse PHPUnit\\Framework\\TestCase;\nclass CalcTest extends TestCase {{\n{body}\n}}\n"
    )
}

/// Locate a real PHPUnit, if one has been made available.
fn phpunit_binary() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("WITDIFF_PHPUNIT") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Run a real PHPUnit over `files`, returning (success, output, exit code).
fn run_phpunit(files: &[(&str, String)]) -> Option<(bool, String, i32)> {
    let phpunit = phpunit_binary()?;
    let dir = tempfile::TempDir::new().expect("temp dir");
    fs::create_dir_all(dir.path().join("tests")).expect("mkdir tests");
    for (name, contents) in files {
        fs::write(dir.path().join("tests").join(name), contents).expect("write test file");
    }
    fs::write(
        dir.path().join("phpunit.xml"),
        "<?xml version=\"1.0\"?>\n<phpunit colors=\"false\">\n  <testsuites><testsuite name=\"all\"><directory>tests</directory></testsuite></testsuites>\n</phpunit>\n",
    )
    .expect("write config");

    let output = Command::new("php")
        .arg(&phpunit)
        .arg("--configuration")
        .arg("phpunit.xml")
        .current_dir(dir.path())
        .output()
        .expect("run phpunit");
    Some((
        output.status.success(),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        output.status.code().unwrap_or(-1),
    ))
}

#[test]
fn identical_revisions_produce_no_findings() {
    let source = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }",
    );
    let findings = analyze(Some(&source), &source);
    assert!(
        findings.is_empty(),
        "an unchanged file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// The argument-order normalization that would otherwise fail silently:
/// PHPUnit names the expectation first, so without swapping, every changed
/// expectation reads as a removal plus an addition rather than as a change.
#[test]
fn changing_an_expectation_is_reported() {
    let base = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }",
    );
    let head = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(6, $this->add(2, 3));\n    }",
    );
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "expected a changed-expectation finding, got {:?}",
        rules(&findings)
    );
}

#[test]
fn a_trivialized_assertion_is_reported() {
    let base = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }",
    );
    let head = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertTrue(true);\n    }",
    );
    let findings = analyze(Some(&base), &head);
    assert!(
        !findings.is_empty(),
        "replacing a real expectation with `assertTrue(true)` must be reported"
    );
}

#[test]
fn removing_a_test_is_reported() {
    let base = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }\n\n    public function testSub(): void {\n        $this->assertEquals(4, $this->sub(6, 2));\n    }",
    );
    let head = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }",
    );
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"removed_test"),
        "expected a removed-test finding, got {:?}",
        rules(&findings)
    );
}

/// A new file has no base, so there is nothing to compare and nothing to
/// report. Reporting here would fire on every new test file.
#[test]
fn a_new_file_produces_no_comparative_findings() {
    let head = phpunit_class(
        "    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }",
    );
    let findings = analyze(None, &head);
    assert!(
        findings.is_empty(),
        "a new file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// Pest writes its examples as calls, not declarations, and its expectations
/// as a chain. Both shapes must be read, or every Pest project is analyzed as
/// having no tests at all.
#[test]
fn pest_expectations_are_normalized() {
    let source = "<?php\ntest('adds', function () {\n    expect($this->add(2, 3))->toBe(5);\n});\n";
    let findings = analyze(Some(source), source);
    assert!(
        findings.is_empty(),
        "an unchanged Pest file must produce no findings, got {:?}",
        rules(&findings)
    );
}

#[test]
fn a_weakened_pest_expectation_is_reported() {
    let base = "<?php\ntest('adds', function () {\n    expect($this->add(2, 3))->toBe(5);\n});\n";
    let head = "<?php\ntest('adds', function () {\n    expect(true)->toBeTrue();\n});\n";
    let findings = analyze(Some(base), head);
    assert!(
        !findings.is_empty(),
        "weakening a Pest expectation must be reported, got {:?}",
        rules(&findings)
    );
}

/// The whole point of the adapter: a vacuous test must not read as a
/// regression guard. `assertTrue(true)` passes under any implementation.
#[test]
fn a_vacuous_assertion_is_reported_as_trivial() {
    let head = phpunit_class(
        "    public function testNothing(): void {\n        $this->assertTrue(true);\n    }",
    );
    let findings = analyze(None, &head);
    let _ = findings;
    // A vacuous assertion is only comparable against a base; with no base
    // there is nothing to report. Assert the tool still READ the assertion,
    // which the change-detection tests above depend on.
    let base = phpunit_class(
        "    public function testNothing(): void {\n        $this->assertEquals(1, 1);\n    }",
    );
    let head = phpunit_class(
        "    public function testNothing(): void {\n        $this->assertTrue(true);\n    }",
    );
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"trivial_assertion")
            || rules(&findings).contains(&"weakened_assertion"),
        "a self-satisfying assertion must be reported, got {:?}",
        rules(&findings)
    );
}

/// A live passing run must not be read as a failure by the classifier.
#[test]
fn a_live_passing_phpunit_run_is_not_a_failure() {
    let Some((success, output, code)) = run_phpunit(&[(
        "CalcTest.php",
        "<?php\nuse PHPUnit\\Framework\\TestCase;\nclass CalcTest extends TestCase {\n    public function testPasses(): void {\n        $this->assertTrue(true);\n    }\n}\n".to_owned(),
    )]) else {
        eprintln!("skipping: set WITDIFF_PHPUNIT to a real phpunit binary");
        return;
    };
    assert!(success, "the fixture should pass, got:\n{output}");
    assert_ne!(
        TestFramework::Php.classify(&output, "", Some(code)),
        FailureKind::TestFailure,
        "a passing run must not classify as a failure:\n{output}"
    );
}

/// A live failing run must classify as a behavioural failure, which is the
/// only classification that can produce a red/green proof.
#[test]
fn a_live_failing_phpunit_run_is_a_test_failure() {
    let Some((success, output, code)) = run_phpunit(&[(
        "CalcTest.php",
        "<?php\nuse PHPUnit\\Framework\\TestCase;\nclass CalcTest extends TestCase {\n    public function testFails(): void {\n        $this->assertEquals(6, 5);\n    }\n}\n".to_owned(),
    )]) else {
        eprintln!("skipping: set WITDIFF_PHPUNIT to a real phpunit binary");
        return;
    };
    assert!(!success, "the fixture should fail, got:\n{output}");
    assert_eq!(
        TestFramework::Php.classify(&output, "", Some(code)),
        FailureKind::TestFailure,
        "a failing run must classify as a test failure:\n{output}"
    );
}

/// A file PHPUnit cannot parse must be a compile failure. Classifying it as a
/// behavioural failure would let a base that never ran stand in for a real
/// regression — the exact substitution invariant 2 forbids.
#[test]
fn a_live_unparsable_phpunit_run_is_a_compile_failure() {
    let Some((success, output, code)) = run_phpunit(&[(
        "CalcTest.php",
        "<?php\nclass CalcTest extends {\n  public function testX( {\n}\n".to_owned(),
    )]) else {
        eprintln!("skipping: set WITDIFF_PHPUNIT to a real phpunit binary");
        return;
    };
    assert!(
        !success,
        "the fixture should not run cleanly, got:\n{output}"
    );
    assert_eq!(
        TestFramework::Php.classify(&output, "", Some(code)),
        FailureKind::CompileError,
        "an unparsable file must classify as a compile failure:\n{output}"
    );
}

/// A WordPress-style test whose only check is `expectException`. Measured
/// before this test existed: the tool summarized it with ZERO assertions, so
/// deleting the guard changed nothing a comparison could see and the test went
/// from asserting to asserting nothing with no finding — while the red/green
/// proof still passed, because the experiment genuinely did distinguish two
/// revisions. The test had stopped constraining anything.
#[test]
fn an_exception_expectation_counts_as_an_assertion() {
    let source = "<?php\nclass LoaderTest extends TestCase {\n    public function testMissing(): void {\n        $this->expectException(\\RuntimeException::class);\n        (new Loader())->load('/nope');\n    }\n}\n";
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("LoaderTest.php");
    fs::write(&path, source).expect("write");
    let toolchain = toolchain().expect("php");
    let summary = toolchain.summarize(&path).expect("summarize");

    let function = summary
        .functions
        .iter()
        .find(|function| function.name == "testMissing")
        .expect("testMissing");
    assert_eq!(
        function.assertions.len(),
        1,
        "an expectException call is the test's only check and must count"
    );
}

/// Deleting that guard must be reported as a removal. This is the weakening the
/// zero-assertion summary hid.
#[test]
fn deleting_an_exception_expectation_is_reported() {
    let base = "<?php\nclass LoaderTest extends TestCase {\n    public function testMissing(): void {\n        $this->expectException(\\RuntimeException::class);\n        (new Loader())->load('/nope');\n    }\n}\n";
    let head = "<?php\nclass LoaderTest extends TestCase {\n    public function testMissing(): void {\n        (new Loader())->load('/nope');\n    }\n}\n";

    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("LoaderTest.php");
    fs::write(&path, head).expect("write head");
    let toolchain = toolchain().expect("php");
    let findings = analyze_php_test_change("LoaderTest.php", &toolchain, &path, Some(base));
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "deleting the only check must be reported, got {:?}",
        rules(&findings)
    );
}
