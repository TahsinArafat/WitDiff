//! Structural analysis of Go test changes.
//!
//! See ADR-0017. The Go counterpart to [`crate::pyanalysis`], sharing the same
//! rule engine ([`crate::testshape`]) so the two cannot disagree about what a
//! weakening is.
//!
//! ## How it works
//!
//! Go's parser is in the standard library, so `go run` executes a small
//! embedded script ([`crate::goscript::SUMMARY_SCRIPT`]) that emits a
//! normalized structural summary. No dependency is needed on either side.
//!
//! ## What counts as an assertion in Go
//!
//! Go has no assertion keyword. A test fails by calling `t.Error`, `t.Errorf`,
//! `t.Fatal` or `t.Fatalf`, and the meaningful part is the *condition that
//! guarded the call*:
//!
//! ```go
//! if got != want {
//!     t.Errorf("got %d, want %d", got, want)
//! }
//! ```
//!
//! The assertion is `got != want`, not the message. Comparing messages instead
//! would report a reworded message as a changed expectation and miss an
//! inverted condition entirely, so the script walks up to the enclosing `if`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::goscript::{SUMMARY_FORMAT, SUMMARY_SCRIPT};
use crate::model::{IntegrityFinding, Severity};
use crate::testshape::{Assertion, FileSummary, Operators, TestFunction};

/// The Go operator vocabulary.
///
/// Go renders operators as themselves (`!=`, `<=`), unlike Python's `NotEq`,
/// so the comparison markers are spelled with surrounding spaces to avoid
/// matching inside identifiers.
const OPERATORS: Operators = Operators {
    comparisons: &[" != ", " == ", " < ", " <= ", " > ", " >= ", " && ", " || "],
    unary: &["! ", "-", "+", "~"],
    trivials: &["true", "false", "1", "0", "nil", "len(nil) == 0"],
};

/// Where the Go toolchain is, and where to run it.
#[derive(Debug, Clone)]
pub struct GoToolchain {
    /// The `go` program, as named by the project's test command.
    program: String,
    working_directory: PathBuf,
}

impl GoToolchain {
    /// Derive the toolchain from a configured test command.
    ///
    /// Returns `None` when the command does not name `go`, which is the honest
    /// answer for a project using a wrapper — the caller then reports that
    /// analysis was not possible rather than guessing.
    pub fn from_test_command(command: &[String], working_directory: &Path) -> Option<Self> {
        let program = command.first()?;
        let base = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if base != "go" {
            return None;
        }
        Some(Self {
            program: program.clone(),
            working_directory: working_directory.to_owned(),
        })
    }

    /// Summarize one Go file.
    ///
    /// `go run` requires every named file to share a directory, so the script
    /// and the target are written side by side in a temporary directory. The
    /// target is copied under a `.txt` name because `go run` refuses to run a
    /// `_test.go` file, and `go/parser` reads any filename.
    fn summarize(&self, path: &Path) -> Result<FileSummary> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        self.summarize_source(&source)
    }

    fn summarize_source(&self, source: &str) -> Result<FileSummary> {
        let directory = tempfile::Builder::new()
            .prefix("witdiff-gosummary-")
            .tempdir()
            .context("failed creating a directory for the Go summary")?;

        std::fs::write(
            directory.path().join("go.mod"),
            "module witdiffsummary\n\ngo 1.21\n",
        )
        .context("failed writing a go.mod for the summary")?;
        std::fs::write(directory.path().join("summarize.go"), SUMMARY_SCRIPT)
            .context("failed writing the Go summary script")?;
        // `go run` refuses `_test.go`, so the copy avoids that suffix.
        let target = directory.path().join("target.txt");
        std::fs::write(&target, source).context("failed writing the Go target file")?;

        let output = Command::new(&self.program)
            .arg("run")
            .arg("summarize.go")
            .arg("target.txt")
            .current_dir(directory.path())
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze Go source; is the toolchain on PATH?",
                    self.program
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing Go source: {}",
                self.program,
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let wire: WireSummary = serde_json::from_str(stdout.trim())
            .with_context(|| "the Go summary was not valid JSON".to_owned())?;

        if wire.format != SUMMARY_FORMAT {
            anyhow::bail!(
                "the Go summary used format {}, but this build expects {}",
                wire.format,
                SUMMARY_FORMAT
            );
        }

        Ok(FileSummary {
            format: wire.format,
            error: wire.error,
            functions: wire
                .functions
                .into_iter()
                .map(|function| TestFunction {
                    name: function.name,
                    line: function.line,
                    assertions: function
                        .assertions
                        .into_iter()
                        .map(|assertion| Assertion {
                            line: assertion.line,
                            test: assertion.test,
                            message: None,
                        })
                        .collect(),
                    skipped: function.skipped,
                    body_is_empty: function.empty,
                    bindings: function.bindings.into_iter().collect(),
                })
                .collect(),
        })
    }

    /// Expose the raw working directory for diagnostics.
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
}

#[derive(Debug, serde::Deserialize)]
struct WireSummary {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    functions: Vec<WireFunction>,
}

#[derive(Debug, serde::Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    line: usize,
    #[serde(default)]
    assertions: Vec<WireAssertion>,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    bindings: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "body_is_empty")]
    empty: bool,
}

#[derive(Debug, serde::Deserialize)]
struct WireAssertion {
    #[serde(default)]
    line: usize,
    #[serde(default)]
    test: String,
}

/// Analyze a Go test file by comparing its base and head revisions.
pub fn analyze_go_test_change(
    path: &str,
    toolchain: &GoToolchain,
    head_path: &Path,
    base_source: Option<&str>,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    let head = match toolchain.summarize(head_path) {
        Ok(summary) => summary,
        Err(error) => {
            findings.push(crate::testshape::finding(
                Severity::Warning,
                path,
                0,
                "test_source_unparsable",
                format!(
                    "this Go test file could not be analyzed, so no structural comparison was performed: {error}"
                ),
            ));
            return findings;
        }
    };

    if let Some(error) = &head.error {
        findings.push(crate::testshape::finding(
            Severity::Warning,
            path,
            0,
            "test_source_unparsable",
            format!("this Go test file could not be parsed: {error}"),
        ));
        return findings;
    }

    let base = match base_source {
        None => None,
        Some(source) => match toolchain.summarize_source(source) {
            Ok(summary) if summary.error.is_none() => Some(summary),
            Ok(summary) => {
                findings.push(crate::testshape::finding(
                    Severity::Info,
                    path,
                    0,
                    "test_source_unparsable",
                    format!(
                        "the base revision of this Go test file could not be parsed, so removed assertions and changed expectations were not compared: {}",
                        summary.error.unwrap_or_default()
                    ),
                ));
                None
            }
            Err(error) => {
                findings.push(crate::testshape::finding(
                    Severity::Info,
                    path,
                    0,
                    "test_source_unparsable",
                    format!(
                        "the base revision of this Go test file could not be analyzed, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    findings.extend(crate::testshape::analyze(
        path,
        "Go",
        &OPERATORS,
        base.as_ref(),
        &head,
    ));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_is_discovered_only_from_go_commands() {
        let cwd = Path::new("/tmp");
        assert!(GoToolchain::from_test_command(
            &["go".to_string(), "test".into(), "./...".into()],
            cwd
        )
        .is_some());
        assert!(GoToolchain::from_test_command(
            &["/usr/local/go/bin/go".to_string(), "test".into()],
            cwd
        )
        .is_some());
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["python3".into(), "-m".into(), "pytest".into()],
            vec!["npm".into(), "test".into()],
        ] {
            assert!(
                GoToolchain::from_test_command(&command, cwd).is_none(),
                "{command:?} is not Go and must not be treated as such"
            );
        }
    }

    /// The vocabulary must match what the script emits: Go renders operators
    /// as themselves, unlike Python's `NotEq`.
    #[test]
    fn the_go_operator_vocabulary_matches_the_script() {
        assert!(OPERATORS.comparisons.contains(&" != "));
        assert!(OPERATORS.comparisons.contains(&" == "));
        assert!(OPERATORS.unary.contains(&"! "));
        assert!(OPERATORS.trivials.contains(&"true"));
    }
}
