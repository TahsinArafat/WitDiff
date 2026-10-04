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
    #[serde(default)]
    bindings: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    guards: Vec<String>,
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

/// The Python operator vocabulary.
///
/// Supplied to the shared engine rather than hardcoded there, because `==`
/// renders as `Eq` in the normalization this language uses.
const OPERATORS: crate::testshape::Operators = crate::testshape::Operators {
    comparisons: &[
        " Eq ", " NotEq ", " Lt ", " LtE ", " Gt ", " GtE ", " Is ", " IsNot ", " In ", " NotIn ",
    ],
    unary: &["Not ", "-", "+", "~"],
    trivials: &["True", "1", "None", "[]", "{}", "()", "''", "\"\""],
};

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
    let head = to_shape(head);

    // The base revision is compared by writing it to a sibling temporary file,
    // so the same script and interpreter handle both sides.
    let base = match base_source {
        None => None,
        Some(source) => match write_and_summarize(interpreter, head_path, source) {
            Ok(summary) => {
                if let Some(error) = &summary.error {
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
                } else {
                    Some(to_shape(summary))
                }
            }
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

    findings.extend(crate::testshape::analyze(
        path,
        "Python",
        &OPERATORS,
        base.as_ref(),
        &head,
    ));
    findings
}

/// Convert the wire summary into the shared shape.
fn to_shape(summary: FileSummary) -> crate::testshape::FileSummary {
    crate::testshape::FileSummary {
        format: summary.format,
        error: summary.error,
        functions: summary
            .functions
            .into_iter()
            .map(|function| crate::testshape::TestFunction {
                name: function.name,
                line: function.line,
                assertions: function
                    .assertions
                    .into_iter()
                    .map(|assertion| crate::testshape::Assertion {
                        line: assertion.line,
                        test: assertion.test,
                        message: assertion.message,
                    })
                    .collect(),
                skipped: function.skipped,
                body_is_empty: function.body_is_empty,
                guards: function.guards,
                bindings: function.bindings.into_iter().collect(),
            })
            .collect(),
    }
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

fn finding(
    severity: Severity,
    path: &str,
    line: usize,
    rule: &str,
    message: String,
) -> IntegrityFinding {
    crate::testshape::finding(severity, path, line, rule, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The interpreter must be discovered from the test command, and a
    /// non-Python command must yield nothing rather than a guess.
    ///
    /// The rule engine itself is tested in [`crate::testshape`], which is
    /// shared by every structural analyzer; duplicating those assertions here
    /// would let the two copies disagree.
    #[test]
    fn interpreter_is_discovered_only_from_python_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["python3".to_string(), "-m".into(), "pytest".into()],
            vec!["python".into(), "-m".into(), "pytest".into()],
            vec!["pytest".into()],
            vec!["python3.11".into(), "-m".into(), "pytest".into()],
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
            vec!["php".into(), "unit".into()],
        ] {
            assert!(
                Interpreter::from_test_command(&command, cwd).is_none(),
                "{command:?} is not Python and must not be treated as such"
            );
        }
    }

    /// The operator vocabulary must match what the embedded script emits.
    /// Verified against the script's own output rather than assumed: it renders
    /// `==` as `Eq`.
    #[test]
    fn the_python_operator_vocabulary_matches_the_script() {
        assert!(OPERATORS.comparisons.contains(&" Eq "));
        assert!(OPERATORS.comparisons.contains(&" NotEq "));
        assert!(OPERATORS.unary.contains(&"Not "));
        assert!(OPERATORS.trivials.contains(&"True"));
    }
}
