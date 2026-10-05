//! End-to-end tests for Ruby structural integrity analysis.
//!
//! These spawn a real Ruby interpreter, because the analysis runs `ripper` over
//! an embedded tool and unit tests over hand-written summaries cannot prove the
//! tool works. They are `#[ignore]`d so a plain `cargo test` stays fast and
//! independent of whether Ruby is installed.
//!
//! The RSpec tests run a **real RSpec**, not a fixture of its output. ADR-0020
//! recorded that the RSpec path had never been exercised against a live runner,
//! because `gem install` was blocked. It is available here, and running it
//! found output shapes the hand-written cases did not cover.

use std::{fs, path::Path, process::Command};

use witdiff_core::framework::TestFramework;
use witdiff_core::model::FailureKind;
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

/// Whether a real `rspec` is on PATH.
fn rspec_available() -> bool {
    Command::new("rspec")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Run a real RSpec over a real `spec/` project and return its output.
///
/// The file name must end in `_spec.rb`: RSpec's default pattern is
/// `**/*_spec.rb`, so `calc.rb` in a `spec/` directory runs zero examples and
/// exits successfully. That is a silent pass, and it is exactly what a
/// hand-written output fixture would never reveal.
fn run_rspec(files: &[(&str, String)]) -> Option<(bool, String, i32)> {
    if !rspec_available() {
        return None;
    }
    let dir = tempfile::TempDir::new().ok()?;
    let spec = dir.path().join("spec");
    fs::create_dir_all(&spec).ok()?;
    for (name, body) in files {
        fs::write(spec.join(name), body).ok()?;
    }
    let output = Command::new("rspec")
        .arg("--no-color")
        .current_dir(dir.path())
        .output()
        .ok()?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((
        output.status.success(),
        combined,
        output.status.code().unwrap_or(-1),
    ))
}

/// A real `add`, so an expectation mismatch is the only thing under test. A
/// fixture that calls an undefined method fails with `NoMethodError`, which
/// measures the fixture rather than the classifier.
const ADD_HELPER: &str = "def add(a, b)\n  a + b\nend\n\n";

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

/// RSpec examples are `it "..." do` blocks, not `def`, and normalize to the
/// same canonical subject-first form as Minitest. Verified against
/// `Ripper.sexp`: the node is a `[:method_add_block, [:command, [:@ident, "it"],
/// ...]]`, so the example name and the matcher both had to be read from the
/// tree rather than assumed.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn rspec_expectations_are_normalized() {
    let base = "RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(2)\n  end\nend\n";
    let head = "RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(3)\n  end\nend\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "an RSpec expectation change must be reported, got {:?}",
        rules(&findings)
    );
}

/// The two assertion styles must produce comparable canonical forms, or a
/// project migrating from one to the other would look like it removed every
/// check.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn minitest_and_rspec_normalize_to_the_same_form() {
    let minitest = "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_adds\n    assert_equal 2, add(1, 1)\n  end\nend\n";
    let rspec = "RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(2)\n  end\nend\n";
    let from_minitest = analyze(Some(minitest), minitest);
    let from_rspec = analyze(Some(rspec), rspec);

    assert!(
        from_minitest.is_empty() && from_rspec.is_empty(),
        "neither style should report on itself; minitest={:?} rspec={:?}",
        rules(&from_minitest),
        rules(&from_rspec)
    );

    // The same expectation expressed in RSpec must be seen as unchanged when
    // a Minitest file keeps its own: each is compared only against itself.
    let modified = rspec.replace("eq(2)", "eq(4)");
    let changed = analyze(Some(rspec), &modified);
    assert!(
        rules(&changed).contains(&"changed_expected_value"),
        "a changed RSpec expectation must be reported, got {:?}",
        rules(&changed)
    );
}

/// `not_to eq` is an inequality expectation, and must normalize as such rather
/// than as a bare predicate.
#[test]
#[ignore = "end-to-end: spawns the Ruby interpreter; run with -- --ignored"]
fn a_negated_rspec_expectation_is_an_inequality() {
    let base =
        "RSpec.describe \"Calc\" do\n  it \"rejects\" do\n    expect(x).not_to eq(3)\n  end\nend\n";
    let head =
        "RSpec.describe \"Calc\" do\n  it \"rejects\" do\n    expect(x).not_to eq(4)\n  end\nend\n";
    let findings = analyze(Some(base), head);
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a changed negated expectation must be reported, got {:?}",
        rules(&findings)
    );
}

// ---------------------------------------------------------------------------
// Verified against a live RSpec.
//
// The tests above exercise the analyzer over RSpec *sources*. These run RSpec
// itself, because the classifier has to recognize a real runner's output and a
// hand-written fixture can only assert what somebody already thought to write
// down. ADR-0020 recorded this as unverified because `gem install` was blocked.
// ---------------------------------------------------------------------------

/// A passing RSpec run must not be classified as a failure. This is the
/// dangerous direction: a green suite reported as red would manufacture a
/// proof.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_live_passing_rspec_run_is_not_a_failure() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc_spec.rb",
        format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(2)\n  end\nend\n"),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(success, "the suite should pass, got:\n{output}");
    assert_eq!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::CommandFailure,
        "a green suite must not classify as a test failure, got:\n{output}"
    );
}

/// A failing RSpec run must classify as a test failure, which is what allows a
/// red/green proof at all.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_live_failing_rspec_run_is_a_test_failure() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc_spec.rb",
        format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(3)\n  end\n\n  it \"also adds\" do\n    expect(add(2, 2)).to eq(4)\n  end\nend\n"),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(!success, "the suite should fail, got:\n{output}");
    assert_eq!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::TestFailure,
        "a red suite must classify as a test failure, got:\n{output}"
    );
}

/// An example that raises is an **error**, and RSpec reports it differently
/// from a failed expectation. Measured against real RSpec: the summary is
/// `1 example, 1 failure` and the detail line reads
/// `An error occurred while loading ./spec/calc_spec.rb`, not `Failure/Error:`.
/// A classifier keyed only on the failure count calls this a test failure,
/// which is the wrong reason and would attribute a broken load to a
/// behavioural regression.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_live_rspec_error_is_distinguished_from_a_failure() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc_spec.rb",
        format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    raise \"boom\"\n  end\nend\n"),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(!success, "the suite should fail, got:\n{output}");
    let kind = TestFramework::Ruby.classify(&output, "", Some(code));
    assert_ne!(
        kind,
        FailureKind::CommandFailure,
        "an errored example must still be a test failure rather than an \
         unrecognized command failure, got:\n{output}"
    );
}

/// A **pending** example is skipped, not passed, and RSpec exits successfully.
/// Classifying that green run as a pass is correct — but the suite checked
/// nothing, which is the same situation as an ignored test elsewhere.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_live_pending_rspec_run_exits_successfully() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc_spec.rb",
        format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    skip \"not ready\"\n  end\nend\n"),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(
        success,
        "a pending example is not a failure, got:\n{output}"
    );
    assert_ne!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::TestFailure,
        "a pending example must not be reported as a failure, got:\n{output}"
    );
}

/// RSpec's default file pattern is `**/*_spec.rb`. A spec file that does not
/// match it runs **zero** examples and exits **successfully** — a silent pass.
///
/// This is recorded because it is invisible to any hand-written output
/// fixture: there is no output to copy. A project whose spec files are named
/// `calc.rb` would have its entire suite pass vacuously, and WitDiff would
/// report that as green evidence for the red/green proof.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_spec_file_rspec_does_not_collect_runs_nothing_and_still_passes() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc.rb",
        format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(3)\n  end\nend\n"),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(
        success && output.contains("0 examples"),
        "a spec file not matching RSpec's pattern is expected to run nothing \
         and exit successfully; measured output was:\n{output}"
    );
    assert_eq!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::CommandFailure,
        "a suite that ran nothing is green, and must not be reported as a \
         test failure"
    );
}

/// The same source, named so RSpec collects it, must fail. Paired with the
/// test above, this pins the difference down: it is the *name*, not the
/// content, that decides whether the suite runs.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn the_same_source_named_correctly_does_fail() {
    let source = format!("{ADD_HELPER}RSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(add(1, 1)).to eq(3)\n  end\nend\n");
    let Some((success, output, code)) = run_rspec(&[("calc_spec.rb", source.clone())]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(!success, "the suite should fail, got:\n{output}");
    assert_eq!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::TestFailure,
        "got:\n{output}"
    );
}

/// A Ruby-level `LoadError` is a compile failure, not a behavioural one, and
/// the analyzer must classify a real one as such.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_live_rspec_load_error_is_a_compile_failure() {
    let Some((success, output, code)) = run_rspec(&[(
        "calc_spec.rb",
        "require \"no_such_library_at_all\"\n\nRSpec.describe \"Calc\" do\n  it \"adds\" do\n    expect(1).to eq(1)\n  end\nend\n".to_owned(),
    )]) else {
        eprintln!("rspec not on PATH; skipping");
        return;
    };
    assert!(!success, "the suite should fail to load, got:\n{output}");
    assert_eq!(
        TestFramework::Ruby.classify(&output, "", Some(code)),
        FailureKind::CompileError,
        "a LoadError is a compile failure, got:\n{output}"
    );
}
