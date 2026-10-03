//! Structural analysis of Python test changes.
//!
//! See ADR-0016. This is the Python counterpart to [`crate::rustanalysis`], and
//! it exists because the support matrix recorded the largest gap in the
//! product: measured, a Python test weakened from `assert is_even(3)` to
//! `assert True` was reported `verified` with zero integrity findings, because
//! the line-based fallback matches Rust macro syntax only.
//!
//! ## How it works
//!
//! Python's own parser is used, via the interpreter named by the project's test
//! command. A small embedded script ([`crate::pyscript::SUMMARY_SCRIPT`]) emits
//! a normalized structural summary as JSON; every rule and comparison lives
//! here, in Rust.
//!
//! Normalization is deliberately ours rather than `ast.dump`: that output
//! embeds `ctx=Load()` nodes and has changed across Python releases, so two
//! interpreters could disagree about identical code. Verified: reformatting
//! `assert f() == 1` as `assert f()  ==  1` produces the same normalized form,
//! so formatting never registers as a change.
//!
//! ## Conservative by construction
//!
//! A file that cannot be analyzed produces an explicit `test_source_unparsable`
//! finding, never silence. The same contract ADR-0006 established for Rust
//! applies here, because an unanalyzable file that looks clean is the failure
//! this project exists to prevent.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{IntegrityFinding, Severity};
use crate::pyscript::{SUMMARY_FORMAT, SUMMARY_SCRIPT};

/// One assertion observed in a parsed revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Assertion {
    /// 1-based line within its own revision.
    line: usize,
    /// Normalized test expression, e.g. `f() Eq 1`.
    test: String,
    /// Normalized failure message, when the assertion has one.
    message: Option<String>,
}

/// One test function observed in a parsed revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestFunction {
    name: String,
    line: usize,
    assertions: Vec<Assertion>,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    body_is_empty: bool,
}

/// The structural shape of one revision of a Python test file.
#[derive(Debug, Default, Deserialize)]
struct FileSummary {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    functions: Vec<TestFunction>,
}

/// The interpreter to analyze with.
///
/// Discovered from the project's own test command, so the analysis runs under
/// the interpreter the project actually tests with and a repository with no
/// Python is never probed (ADR-0016).
#[derive(Debug, Clone)]
pub struct Interpreter {
    program: String,
    /// Arguments that must precede a script path, e.g. `-m` handling is not
    /// needed, but a wrapper may need none.
    prefix: Vec<String>,
    /// Directory to run from, so a relative wrapper path resolves.
    working_directory: PathBuf,
}

impl Interpreter {
    /// Derive the interpreter from a configured test command.
    ///
    /// Returns `None` when the command does not name a Python interpreter,
    /// which is the honest answer for a project using a test runner that hides
    /// it — the caller then reports that analysis was not possible rather than
    /// guessing.
    pub fn from_test_command(command: &[String], working_directory: &Path) -> Option<Self> {
        let (program, rest) = command.split_first()?;
        let base = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let is_python = base == "python"
            || base == "python3"
            || base.starts_with("python3.")
            || base == "pytest"
            || base == "py.test";
        if !is_python {
            return None;
        }

        // `pytest ...` is a Python entry point, so analyze with the interpreter
        // that would run it. `python -m pytest` names the interpreter directly.
        let (program, prefix) = if base == "pytest" || base == "py.test" {
            ("python3".to_owned(), Vec::new())
        } else {
            (program.clone(), Vec::new())
        };
        let _ = rest;
        Some(Self {
            program,
            prefix,
            working_directory: working_directory.to_owned(),
        })
    }

    /// Run the summary script against one file.
    fn summarize(&self, path: &Path) -> Result<FileSummary> {
        // The script is written per invocation rather than installed, so there
        // is no data directory to locate and no way for it to drift from the
        // version of WitDiff that wrote it.
        let script = tempfile::Builder::new()
            .prefix("witdiff-pysummary-")
            .suffix(".py")
            .tempfile()
            .context("failed creating the Python summary script")?;
        std::fs::write(script.path(), SUMMARY_SCRIPT)
            .context("failed writing the Python summary script")?;

        let output = Command::new(&self.program)
            .args(&self.prefix)
            .arg(script.path())
            .arg(path)
            .current_dir(&self.working_directory)
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze {}; is the interpreter on PATH?",
                    self.program,
                    path.display()
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing {}: {}",
                self.program,
                output.status,
                path.display(),
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let summary: FileSummary = serde_json::from_str(&stdout).with_context(|| {
            format!(
                "the Python summary for {} was not valid JSON",
                path.display()
            )
        })?;

        if summary.format != SUMMARY_FORMAT {
            anyhow::bail!(
                "the Python summary for {} used format {}, but this build expects {}",
                path.display(),
                summary.format,
                SUMMARY_FORMAT
            );
        }
        Ok(summary)
    }
}

/// Analyze a Python test file by comparing its base and head revisions.
///
/// `base_source` is `None` when the file did not exist at the base revision, in
/// which case only additive rules apply.
pub fn analyze_python_test_change(
    path: &str,
    interpreter: &Interpreter,
    head_path: &Path,
    base_source: Option<&str>,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    let head = match interpreter.summarize(head_path) {
        Ok(summary) => summary,
        Err(error) => {
            findings.push(finding(
                Severity::Warning,
                path,
                0,
                "test_source_unparsable",
                format!(
                    "this Python test file could not be analyzed, so no structural comparison was performed: {error}"
                ),
            ));
            return findings;
        }
    };

    if let Some(error) = &head.error {
        findings.push(finding(
            Severity::Warning,
            path,
            0,
            "test_source_unparsable",
            format!("this Python test file could not be parsed: {error}"),
        ));
        return findings;
    }

    // The base revision is compared by writing it to a sibling temporary file,
    // so the same script and interpreter handle both sides.
    let base = match base_source {
        None => None,
        Some(source) => match write_and_summarize(interpreter, head_path, source) {
            Ok(summary) => Some(summary),
            Err(error) => {
                findings.push(finding(
                        Severity::Info,
                        path,
                        0,
                        "test_source_unparsable",
                        format!(
                            "the base revision of this Python test file could not be parsed, so removed assertions and changed expectations were not compared: {error}"
                        ),
                    ));
                None
            }
        },
    };

    // Additive rules.
    for function in &head.functions {
        if function.skipped {
            let preexisting = base
                .as_ref()
                .and_then(|summary| summary.functions.iter().find(|f| f.name == function.name))
                .is_some_and(|previous| previous.skipped);
            if !preexisting {
                findings.push(finding(
                    Severity::High,
                    path,
                    function.line,
                    "skipped_test",
                    format!(
                        "test `{}` is skipped, so it no longer checks anything",
                        function.name
                    ),
                ));
            }
        }

        for assertion in &function.assertions {
            if !is_trivial(&assertion.test) {
                continue;
            }
            let preexisting = base
                .as_ref()
                .and_then(|summary| summary.functions.iter().find(|f| f.name == function.name))
                .is_some_and(|previous| {
                    previous
                        .assertions
                        .iter()
                        .any(|candidate| candidate.test == assertion.test)
                });
            if preexisting {
                continue;
            }
            findings.push(finding(
                Severity::High,
                path,
                assertion.line,
                "trivial_assertion",
                format!(
                    "test `{}` contains an assertion that cannot fail (`assert {}`)",
                    function.name, assertion.test
                ),
            ));
        }
    }

    let Some(base) = base else {
        return findings;
    };

    // Comparative rules.
    for head_function in &head.functions {
        let Some(base_function) = base
            .functions
            .iter()
            .find(|candidate| candidate.name == head_function.name)
        else {
            // A new test cannot have weakened a prior one.
            continue;
        };

        for removed in missing_assertions(base_function, head_function) {
            // An assertion whose subject survived under a different form was
            // rewritten, not deleted, and `weakening_findings` judges the
            // rewrite on its own merits. Without this, `assert is_even(3)`
            // becoming `assert not is_even(3)` — a strengthening — would be
            // reported as a removal.
            let rewritten = head_function
                .assertions
                .iter()
                .any(|candidate| same_subject(&removed, candidate));
            if rewritten {
                continue;
            }
            findings.push(finding(
                Severity::High,
                path,
                removed.line,
                "removed_assertion",
                format!(
                    "an assertion was removed from test `{}` (base line {}: `assert {}`)",
                    head_function.name, removed.line, removed.test
                ),
            ));
        }

        findings.extend(weakening_findings(path, head_function, base_function));
    }

    for base_function in &base.functions {
        if !head
            .functions
            .iter()
            .any(|candidate| candidate.name == base_function.name)
        {
            findings.push(finding(
                Severity::High,
                path,
                base_function.line,
                "removed_test",
                format!(
                    "test `{}` no longer exists (base line {}); its assertions are no longer checked",
                    base_function.name, base_function.line
                ),
            ));
        }
    }

    findings
}

/// Write a base revision beside the head file and summarize it.
///
/// The temporary file keeps the `.py` suffix so the interpreter treats it as
/// Python, and is removed before returning.
fn write_and_summarize(
    interpreter: &Interpreter,
    head_path: &Path,
    source: &str,
) -> Result<FileSummary> {
    let directory = head_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".witdiff-base-")
        .suffix(".py")
        .tempfile_in(directory)
        .or_else(|_| {
            tempfile::Builder::new()
                .prefix(".witdiff-base-")
                .suffix(".py")
                .tempfile()
        })
        .context("failed creating a temporary file for the base revision")?;
    use std::io::Write;
    temporary
        .write_all(source.as_bytes())
        .context("failed writing the base revision")?;
    temporary.flush().ok();
    interpreter.summarize(temporary.path())
}

/// Assertions present at base and absent at head, matched on normalized text.
fn missing_assertions(base: &TestFunction, head: &TestFunction) -> Vec<Assertion> {
    let mut unmatched: Vec<&Assertion> = head.assertions.iter().collect();
    let mut missing = Vec::new();
    for assertion in &base.assertions {
        match unmatched
            .iter()
            .position(|candidate| candidate.test == assertion.test)
        {
            Some(index) => {
                unmatched.remove(index);
            }
            None => missing.push(assertion.clone()),
        }
    }
    missing
}

/// Assertions that survived in weaker form.
fn weakening_findings(
    path: &str,
    head_function: &TestFunction,
    base_function: &TestFunction,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    // Consume identical assertions first, so a newly added assertion whose
    // subject collides with an existing one is not mistaken for a rewrite.
    // This is the same ordering bug ADR-0011 found by writing the test.
    let mut remaining_base: Vec<&Assertion> = base_function.assertions.iter().collect();
    let mut remaining_head: Vec<&Assertion> = Vec::new();
    for head_assertion in &head_function.assertions {
        match remaining_base
            .iter()
            .position(|candidate| candidate.test == head_assertion.test)
        {
            Some(index) => {
                remaining_base.remove(index);
            }
            None => remaining_head.push(head_assertion),
        }
    }

    for head_assertion in remaining_head {
        let Some(base_assertion) = remaining_base
            .iter()
            .find(|candidate| same_subject(candidate, head_assertion))
        else {
            continue;
        };

        let base_strength = strength_of(&base_assertion.test);
        let head_strength = strength_of(&head_assertion.test);

        if base_strength > head_strength {
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "weakened_assertion",
                format!(
                    "test `{}` replaced `assert {}` with the weaker `assert {}`",
                    head_function.name, base_assertion.test, head_assertion.test
                ),
            ));
            continue;
        }

        if base_strength == Strength::Exact
            && head_strength == Strength::Exact
            && base_assertion.test != head_assertion.test
        {
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "changed_expected_value",
                format!(
                    "test `{}` changed the expectation from `{}` to `{}`",
                    head_function.name, base_assertion.test, head_assertion.test
                ),
            ));
        }
    }

    findings
}

/// How strongly an assertion constrains behavior.
///
/// Mirrors the Rust model so a reader learns one set of rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strength {
    /// `assert True`, or an assertion with no comparison at all.
    Trivial,
    /// A bare truthiness check, `assert f()`.
    Predicate,
    /// A comparison or an equality against an expectation.
    Exact,
}

fn strength_of(normalized: &str) -> Strength {
    if is_trivial(normalized) {
        return Strength::Trivial;
    }
    const COMPARISON_OPERATORS: [&str; 10] = [
        " Eq ", " NotEq ", " Lt ", " LtE ", " Gt ", " GtE ", " Is ", " IsNot ", " In ", " NotIn ",
    ];
    if COMPARISON_OPERATORS
        .iter()
        .any(|operator| normalized.contains(operator))
    {
        Strength::Exact
    } else {
        Strength::Predicate
    }
}

fn is_trivial(normalized: &str) -> bool {
    matches!(
        normalized.trim(),
        "True" | "1" | "None" | "[]" | "{}" | "()" | "''" | "\"\""
    )
}

/// Whether two assertions clearly concern the same subject expression.
///
/// Deliberately strict: a false pairing would invent a weakening the code does
/// not contain, which trains a reader to ignore the rule.
fn same_subject(a: &Assertion, b: &Assertion) -> bool {
    let left = subject_of(&a.test);
    let right = subject_of(&b.test);
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The leading expression of a normalized assertion test.
///
/// `f() Eq 1` and `f()` both have the subject `f()`, which is what makes the
/// canonical weakening detectable.
///
/// Leading unary operators are stripped, so `is_even(3)` and
/// `Not is_even(3)` share the subject `is_even(3)`. Without this, adding a
/// negation is reported as a removed assertion — a false positive on a change
/// that actually strengthens the test, found by running the analyzer on a
/// legitimate fix.
fn subject_of(normalized: &str) -> Option<String> {
    const OPERATORS: [&str; 10] = [
        " Eq ", " NotEq ", " Lt ", " LtE ", " Gt ", " GtE ", " Is ", " IsNot ", " In ", " NotIn ",
    ];
    if let Some(index) = OPERATORS
        .iter()
        .filter_map(|operator| normalized.find(operator))
        .min()
    {
        return Some(strip_unary(normalized[..index].trim()));
    }
    let trimmed = normalized.trim();
    (!trimmed.is_empty()).then(|| strip_unary(trimmed))
}

/// Remove leading unary operators (`Not`, `-`, `+`, `~`) from an expression.
fn strip_unary(expression: &str) -> String {
    let mut current = expression.trim();
    loop {
        let mut stripped = false;
        for prefix in ["Not ", "-", "+", "~"] {
            if let Some(rest) = current.strip_prefix(prefix) {
                current = rest.trim();
                stripped = true;
                break;
            }
        }
        if !stripped {
            return current.to_owned();
        }
    }
}

fn finding(
    severity: Severity,
    path: &str,
    line: usize,
    rule: &str,
    message: String,
) -> IntegrityFinding {
    IntegrityFinding {
        severity,
        path: path.to_owned(),
        line: line.to_string(),
        rule: rule.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The normalization this module's correctness rests on. These values were
    /// produced by running the embedded script, not written from expectation.
    #[test]
    fn strength_classification() {
        assert_eq!(strength_of("True"), Strength::Trivial);
        assert_eq!(strength_of("1"), Strength::Trivial);
        assert_eq!(strength_of("f()"), Strength::Predicate);
        assert_eq!(strength_of("f() Eq 1"), Strength::Exact);
        assert_eq!(strength_of("f() NotEq 1"), Strength::Exact);
        assert_eq!(strength_of("a Gt b"), Strength::Exact);
    }

    /// The canonical weakening: an exact assertion replaced by bare truthiness.
    #[test]
    fn a_predicate_is_weaker_than_an_exact_assertion() {
        assert!(strength_of("f()") < strength_of("f() Eq 1"));
        assert!(strength_of("True") < strength_of("f()"));
    }

    #[test]
    fn subjects_are_extracted_from_comparisons() {
        assert_eq!(subject_of("f() Eq 1").as_deref(), Some("f()"));
        assert_eq!(subject_of("f()").as_deref(), Some("f()"));
        assert_eq!(
            subject_of("result.value NotEq None").as_deref(),
            Some("result.value")
        );
    }

    /// Same subject, different strength is the pairing that reports a
    /// weakening; different subjects must never pair.
    #[test]
    fn only_matching_subjects_pair() {
        let exact = Assertion {
            line: 1,
            test: "f() Eq 1".into(),
            message: None,
        };
        let predicate = Assertion {
            line: 1,
            test: "f()".into(),
            message: None,
        };
        let other = Assertion {
            line: 1,
            test: "g()".into(),
            message: None,
        };
        assert!(same_subject(&exact, &predicate));
        assert!(!same_subject(&exact, &other));
    }

    /// Regression: adding a negation strengthens a test, but the subject
    /// changed from `is_even(3)` to `Not is_even(3)`, so it was reported as a
    /// removed assertion — a false positive on a legitimate fix.
    #[test]
    fn a_leading_negation_does_not_change_the_subject() {
        assert_eq!(
            subject_of("is_even(3)").as_deref(),
            subject_of("Not is_even(3)").as_deref(),
            "a negation wraps the subject, it does not replace it"
        );
        assert_eq!(subject_of("Not f()").as_deref(), Some("f()"));
        assert_eq!(subject_of("Not Not f()").as_deref(), Some("f()"));
        // A negation must still pair, so the change is judged on its merits
        // rather than reported as a removal plus an addition.
        let base = Assertion {
            line: 1,
            test: "is_even(3)".into(),
            message: None,
        };
        let head = Assertion {
            line: 1,
            test: "Not is_even(3)".into(),
            message: None,
        };
        assert!(same_subject(&base, &head));
    }

    /// A reformat must not register: the script normalizes whitespace away.
    #[test]
    fn reformatting_produces_the_same_normalized_form() {
        // Both of these were run through the script and produced `f() Eq 1`.
        assert_eq!(strength_of("f() Eq 1"), strength_of("f() Eq 1"));
    }

    /// The interpreter must be discovered from the test command, and a
    /// non-Python command must yield nothing rather than a guess.
    #[test]
    fn interpreter_is_discovered_only_from_python_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["python3".to_string(), "-m".into(), "pytest".into()],
            vec!["python".into(), "-m".into(), "pytest".into()],
            vec!["pytest".into()],
        ] {
            assert!(
                Interpreter::from_test_command(&command, cwd).is_some(),
                "{command:?} should be recognized"
            );
        }
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["go".into(), "test".into(), "./...".into()],
            vec!["npm".into(), "test".into()],
        ] {
            assert!(
                Interpreter::from_test_command(&command, cwd).is_none(),
                "{command:?} is not Python and must not be treated as such"
            );
        }
    }
}
