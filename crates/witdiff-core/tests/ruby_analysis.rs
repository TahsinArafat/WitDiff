//! End-to-end tests for Ruby structural integrity analysis.
//!
//! These spawn a real Ruby interpreter, because the analysis runs `ripper` over
//! an embedded tool and unit tests over hand-written summaries cannot prove the
//! tool works. They are `#[ignore]`d so a plain `cargo test` stays fast and
//! independent of whether Ruby is installed.

use std::{fs, path::Path, process::Command};

use witdiff_core::rubyanalysis::{analyze_ruby_test_change, RubyToolchain};

fn toolchain() -> Option<RubyToolchain> {
    match Command::new("ruby").arg("--version").output() {
        Ok(output) if output.status.success() => RubyToolchain::from_test_command(
            &["rake".to_owned(), "test".to_owned()],
            Path::new("."),
        ),
        _ => None,
    }
}

fn analyze(base: Option<&str>, head: &str) -> Vec<witdiff_core::IntegrityFinding> {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("calc_test.rb");
    fs::write(&path, head).expect("write head");
    let toolchain = toolchain().expect("ruby");
    analyze_ruby_test_change("calc_test.rb", &toolchain, &path, base)
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

/// A Minitest file wrapping one assertion.
fn ruby_test(body: &str) -> String {
    format!(
        "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_adds\n    {body}\n  end\nend\n"
    )
}

#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn identical_revisions_produce_no_findings() {
    let source = ruby_test("assert_equal 2, add(1, 1)");
    let findings = analyze(Some(&source), &source);
    assert!(
        findings.is_empty(),
        "an unchanged file must produce no findings, got {:?}",
        rules(&findings)
    );
}

/// Minitest puts the expectation first, unlike Python and Go. Without the swap
/// every expectation change would look like a removal plus an addition.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn the_minitest_argument_order_is_normalized() {
    let source = ruby_test("assert_equal 2, add(1, 1)");
    let findings = analyze(Some(&source), &source);
    assert!(
        findings.is_empty(),
        "the swapped form must compare equal to itself, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn changing_an_expectation_is_reported() {
    let base = ruby_test("assert_equal 2, add(1, 1)");
    let head = ruby_test("assert_equal 3, add(1, 1)");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "an inverted expectation must be reported, got {:?}",
        rules(&findings)
    );
}

/// The case measured before Ruby analysis existed: a gutted assertion.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn a_trivialized_assertion_is_reported() {
    let base = ruby_test("assert_equal 2, add(1, 1)");
    let head = ruby_test("assert true");
    let findings = analyze(Some(&base), &head);
    assert!(
        rules(&findings).contains(&"trivial_assertion"),
        "an assertion that cannot fail must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn removing_a_test_is_reported() {
    let base = "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_a\n    assert_equal 2, add(1, 1)\n  end\n\n  def test_b\n    assert_equal 4, add(2, 2)\n  end\nend\n";
    let head = ruby_test("assert_equal 2, add(1, 1)");
    let findings = analyze(Some(base), &head);
    assert!(
        rules(&findings).contains(&"removed_test"),
        "a deleted test must be reported, got {:?}",
        rules(&findings)
    );
}

/// A deleted `raise` guard fails the test without an assertion macro, so the
/// assertion rules never see its removal.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn a_deleted_failure_guard_is_reported() {
    let base = "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_adds\n    r = add(1, 1)\n    if r.nil?\n      raise \"expected a value\"\n    end\n    assert_equal 2, r\n  end\nend\n";
    let head = "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_adds\n    r = add(1, 1)\n    assert_equal 2, r\n  end\nend\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"removed_error_check"),
        "a deleted guard removes a check and must be reported, got {:?}",
        rules(&findings)
    );
}

#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn a_new_file_produces_no_comparative_findings() {
    let head = ruby_test("assert_equal 2, add(1, 1)");
    let findings = analyze(None, &head);
    assert!(
        !rules(&findings).contains(&"removed_test"),
        "a brand-new file has nothing to remove, got {:?}",
        rules(&findings)
    );
}
