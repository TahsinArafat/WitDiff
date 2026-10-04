//! Structural analysis of Ruby test changes.
//!
//! See ADR-0020. The Ruby counterpart to [`crate::javaanalysis`], sharing the
//! same rule engine ([`crate::testshape`]) so no language can disagree about
//! what a weakening is.
//!
//! ## How it works
//!
//! `ripper` ships in Ruby's standard library, so a small embedded tool
//! ([`crate::rubyscript::SUMMARY_SCRIPT`]) parses the file and emits a
//! normalized structural summary. No gem is needed, and the analysis works in
//! any project that can run its tests.
//!
//! ## Minitest and RSpec also put the expectation first
//!
//! `assert_equal expected, actual` reverses the order, like JUnit and unlike
//! Python and Go. The tool swaps them so the shared engine compares like with
//! like. This is the detail that would otherwise fail silently: every
//! expectation change would be reported as a removal plus an addition rather
//! than as a changed expectation.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::model::{IntegrityFinding, Severity};
use crate::rubyscript::{SUMMARY_FORMAT, SUMMARY_SCRIPT};
use crate::testshape::{Assertion, FileSummary, Operators, TestFunction};

/// The Java operator vocabulary.
///
/// Includes `Eq` and `NotEq` because the summary tool normalizes
/// `assertEquals(expected, actual)` into the canonical subject-first form
/// `actual Eq expected`, exactly as the Python analyzer does. The vocabulary
/// must list the names the tool actually *emits*, not the Java spellings: an
/// earlier version listed only `" == "`, which never matched `Eq`, so every
/// expectation change was misreported as a removed assertion and then an
/// addition. Found by running the analyzer, not by reading it.
const OPERATORS: Operators = Operators {
    comparisons: &[
        " Eq ", " NotEq ", " == ", " != ", " < ", " <= ", " > ", " >= ", " && ", " || ", " Is ",
        " IsNot ",
    ],
    unary: &["!", "-", "+", "~"],
    trivials: &["true", "false", "1", "0", "nil"],
};

/// Where the JDK is, and where to run it.
#[derive(Debug, Clone)]
pub struct RubyToolchain {
    program: String,
    working_directory: PathBuf,
}

impl RubyToolchain {
    /// Derive the toolchain from a configured test command.
    ///
    /// Recognizes `java`, `mvn`, `gradle` and `./gradlew`, because those are
    /// the commands a Java project actually configures. Returns `None` for
    /// anything else, so a non-Java project is never probed.
    pub fn from_test_command(command: &[String], working_directory: &Path) -> Option<Self> {
        let program = command.first()?;
        let base = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let is_ruby = base == "ruby"
            || base == "rake"
            || base == "rspec"
            || base == "bundle"
            || base == "minitest";
        if !is_ruby {
            return None;
        }
        Some(Self {
            program: "ruby".to_owned(),
            working_directory: working_directory.to_owned(),
        })
    }

    /// Summarize one Ruby file.
    ///
    /// `ruby summarize.rb <path>` needs no compilation step, so this is the
    /// cheapest of the language adapters.
    fn summarize(&self, path: &Path) -> Result<FileSummary> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        self.summarize_source(&source)
    }

    fn summarize_source(&self, source: &str) -> Result<FileSummary> {
        let directory = tempfile::Builder::new()
            .prefix("witdiff-rubysummary-")
            .tempdir()
            .context("failed creating a directory for the Ruby summary")?;

        let tool = directory.path().join("summarize.rb");
        std::fs::write(&tool, SUMMARY_SCRIPT).context("failed writing the Ruby summary tool")?;
        let target = directory.path().join("target.rb");
        std::fs::write(&target, source).context("failed writing the Ruby target file")?;

        let output = Command::new(&self.program)
            .arg("summarize.rb")
            .arg("target.rb")
            .current_dir(directory.path())
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze Ruby source; is Ruby on PATH?",
                    self.program
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing Ruby source: {}",
                self.program,
                output.status,
                stderr.trim()
            );
        }

        // The tool writes JSON on the last stdout line; `java` may precede it
        // with diagnostics, so the last line is taken rather than the whole
        // stream.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let payload = stdout
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .unwrap_or_default();
        let wire: WireSummary = serde_json::from_str(payload.trim())
            .with_context(|| format!("the Java summary was not valid JSON: {}", payload.trim()))?;

        if wire.format != SUMMARY_FORMAT {
            anyhow::bail!(
                "the Ruby summary used format {}, but this build expects {}",
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
                    body_is_empty: function.body_is_empty,
                    guards: function.guards,
                    bindings: function.bindings.into_iter().collect(),
                })
                .collect(),
        })
    }

    /// Expose the working directory for diagnostics.
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
    #[serde(default)]
    guards: Vec<String>,
    #[serde(default)]
    body_is_empty: bool,
}

#[derive(Debug, serde::Deserialize)]
struct WireAssertion {
    #[serde(default)]
    line: usize,
    #[serde(default)]
    test: String,
}

/// Analyze a Java test file by comparing its base and head revisions.
pub fn analyze_ruby_test_change(
    path: &str,
    toolchain: &RubyToolchain,
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
                    "this Ruby test file could not be analyzed, so no structural comparison was performed: {error}"
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
            format!("this Ruby test file could not be parsed: {error}"),
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
                        "the base revision of this Ruby test file could not be parsed, so removed assertions and changed expectations were not compared: {}",
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
                        "the base revision of this Ruby test file could not be analyzed, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    findings.extend(crate::testshape::analyze(
        path,
        "Ruby",
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
    fn toolchain_is_discovered_from_ruby_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["ruby".to_string(), "-Itest".into(), "test/a_test.rb".into()],
            vec!["rake".into(), "test".into()],
            vec!["bundle".into(), "exec".into(), "rspec".into()],
            vec!["rspec".into()],
        ] {
            assert!(
                RubyToolchain::from_test_command(&command, cwd).is_some(),
                "{command:?} should be recognized as a Ruby project"
            );
        }
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["go".into(), "test".into()],
            vec!["python3".into(), "-m".into(), "pytest".into()],
            vec!["mvn".into(), "test".into()],
        ] {
            assert!(
                RubyToolchain::from_test_command(&command, cwd).is_none(),
                "{command:?} is not Ruby and must not be treated as such"
            );
        }
    }

    /// The vocabulary must list the names the tool *emits*, which for
    /// `assert_equal` is the canonical `Eq`, not the Ruby spelling `==`.
    #[test]
    fn the_ruby_operator_vocabulary_matches_the_tool() {
        assert!(OPERATORS.comparisons.contains(&" Eq "));
        assert!(OPERATORS.comparisons.contains(&" NotEq "));
        assert!(OPERATORS.comparisons.contains(&" > "));
        assert!(OPERATORS.trivials.contains(&"nil"));
    }

    /// The canonical form the tool produces for `assertEquals` must be
    /// classified as an exact comparison, or the subject cannot be extracted
    /// and every expectation change is misreported as a removal.
    #[test]
    fn the_canonical_equality_form_is_recognized() {
        use crate::testshape::{strength_of, subject_of, Strength};
        assert_eq!(
            strength_of("Compute.call(1, 1) Eq 2", &OPERATORS),
            Strength::Exact
        );
        assert_eq!(
            subject_of("Compute.call(1, 1) Eq 2", &OPERATORS).as_deref(),
            Some("Compute.call(1, 1)")
        );
        // And it pairs with the predicate form over the same subject.
        assert_eq!(
            subject_of("Compute.call(1, 1)", &OPERATORS).as_deref(),
            Some("Compute.call(1, 1)")
        );
    }
}
