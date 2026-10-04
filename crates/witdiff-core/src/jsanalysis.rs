//! Structural analysis of JavaScript and TypeScript test changes.
//!
//! See ADR-0021. The JavaScript counterpart to [`crate::javaanalysis`], sharing
//! the same rule engine ([`crate::testshape`]) so no language can disagree about
//! what a weakening is.
//!
//! ## How it works, and how it differs from the other languages
//!
//! Every other language's parser ships with its runtime. Node ships none, so
//! this one is required from the **project under verification** — `@babel/
//! parser`, `acorn`, or `typescript`, tried in that order. All three produce an
//! ESTree-compatible tree, so one traversal serves them all.
//!
//! That makes the working directory load-bearing: the tool is run from the
//! project so the parser resolves, and a project with no parser is reported as
//! not-analyzed rather than analyzed more weakly.
//!
//! ## Jest and Vitest
//!
//! `expect(x).toBe(y)` is a member-call chain. `expect(x).toBeTruthy()` and the
//! other predicate matchers constrain the subject alone, so they normalize to
//! the subject; equality matchers normalize to the subject-first form the
//! shared engine compares.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::jsscript::{SUMMARY_FORMAT, SUMMARY_SCRIPT};
use crate::model::{IntegrityFinding, Severity};
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
        " Eq ", " NotEq ", " == ", " != ", " < ", " <= ", " > ", " >= ", " && ", " || ",
    ],
    unary: &["!", "-", "+", "~"],
    trivials: &["true", "false", "1", "0", "null", "undefined"],
};

/// Where the JDK is, and where to run it.
#[derive(Debug, Clone)]
pub struct JsToolchain {
    program: String,
    working_directory: PathBuf,
}

impl JsToolchain {
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

        let is_javascript = base == "node"
            || base == "npm"
            || base == "npx"
            || base == "yarn"
            || base == "pnpm"
            || base == "vitest"
            || base == "jest";
        if !is_javascript {
            return None;
        }
        Some(Self {
            program: "node".to_owned(),
            working_directory: working_directory.to_owned(),
        })
    }

    /// Summarize one JavaScript or TypeScript file.
    ///
    /// Run from the project so its parser resolves. A project with no parser is
    /// reported rather than analyzed more weakly.
    fn summarize(&self, path: &Path) -> Result<FileSummary> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        self.summarize_source(&source)
    }

    fn summarize_source(&self, source: &str) -> Result<FileSummary> {
        let directory = tempfile::Builder::new()
            .prefix("witdiff-jssummary-")
            .tempdir()
            .context("failed creating a directory for the JavaScript summary")?;

        let tool = directory.path().join("summarizer.js");
        std::fs::write(&tool, SUMMARY_SCRIPT)
            .context("failed writing the JavaScript summary tool")?;
        let target = directory.path().join("input.js");
        std::fs::write(&target, source).context("failed writing the JavaScript target file")?;

        // The working directory is the *project*, not the temporary directory:
        // the parser is required from the project, so resolving it from
        // elsewhere would report every file as unanalyzable (ADR-0021).
        let output = Command::new(&self.program)
            .arg(tool)
            .arg(target)
            .arg(&self.working_directory)
            .current_dir(&self.working_directory)
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze JavaScript source; is Node on PATH?",
                    self.program
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing JavaScript source: {}",
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
                "the JavaScript summary used format {}, but this build expects {}",
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
pub fn analyze_javascript_test_change(
    path: &str,
    toolchain: &JsToolchain,
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
                    "this JavaScript test file could not be analyzed, so no structural comparison was performed: {error}"
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
            format!("this JavaScript test file could not be parsed: {error}"),
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
                        "the base revision of this JavaScript test file could not be parsed, so removed assertions and changed expectations were not compared: {}",
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
                        "the base revision of this JavaScript test file could not be analyzed, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    findings.extend(crate::testshape::analyze(
        path,
        "JavaScript",
        &OPERATORS,
        base.as_ref(),
        &head,
    ));
    findings
}

/// The operator vocabulary this analyzer compares with.
///
/// Exposed so the embedded tool's rendering and the rule engine's expectations
/// can be asserted against each other, which is what caught the `.not` nesting
/// bug.
#[doc(hidden)]
pub fn operators_for_tests() -> Operators {
    OPERATORS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_is_discovered_from_javascript_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["npm".to_owned(), "test".into()],
            vec!["node".to_owned(), "node_modules/.bin/jest".into()],
            vec!["yarn".into(), "test".into()],
            vec!["vitest".into(), "run".into()],
        ] {
            assert!(
                JsToolchain::from_test_command(&command, cwd).is_some(),
                "{command:?} should be recognized as a JavaScript project"
            );
        }
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["python3".into(), "-m".into(), "pytest".into()],
            vec!["mvn".into(), "test".into()],
            vec!["ruby".into(), "test.rb".into()],
        ] {
            assert!(
                JsToolchain::from_test_command(&command, cwd).is_none(),
                "{command:?} is not JavaScript and must not be treated as such"
            );
        }
    }

    #[test]
    fn the_javascript_operator_vocabulary_matches_the_tool() {
        assert!(OPERATORS.comparisons.contains(&" Eq "));
        assert!(OPERATORS.comparisons.contains(&" NotEq "));
        assert!(OPERATORS.unary.contains(&"!"));
        assert!(OPERATORS.trivials.contains(&"undefined"));
    }

    /// The canonical equality form the tool emits must be classified exact and
    /// yield a subject, or every expectation change is misreported as a removal.
    #[test]
    fn the_canonical_equality_form_is_recognized() {
        use crate::testshape::{strength_of, subject_of, Strength};
        assert_eq!(strength_of("add(2, 3) Eq 5", &OPERATORS), Strength::Exact);
        assert_eq!(
            subject_of("add(2, 3) Eq 5", &OPERATORS).as_deref(),
            Some("add(2, 3)")
        );
        assert_eq!(
            subject_of("add(2, 3)", &OPERATORS).as_deref(),
            Some("add(2, 3)")
        );
    }
}
