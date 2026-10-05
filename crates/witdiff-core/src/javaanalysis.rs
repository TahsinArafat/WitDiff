//! Structural analysis of Java test changes.
//!
//! See ADR-0018. The Java counterpart to [`crate::goanalysis`], sharing the
//! same rule engine ([`crate::testshape`]) so no language can disagree about
//! what a weakening is.
//!
//! ## How it works
//!
//! The JDK ships a Java parser in `com.sun.source`, reachable through
//! `JavacTask`. A small embedded tool ([`crate::javasummary::SUMMARY_SCRIPT`])
//! uses only the public API and emits a normalized structural summary, so no
//! dependency is needed and no `--add-exports` flag is required.
//!
//! ## JUnit reverses the argument order
//!
//! `assertEquals(expected, actual)` puts the expectation **first**, unlike
//! Python's `assert actual == expected` and Go's `if actual != want`. The tool
//! swaps them so the shared engine, which expects the subject first, compares
//! like with like. Getting this wrong would invert every expectation rather
//! than fail loudly — the worst kind of mistake for a tool whose value is that
//! it does not cry wolf.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::javasummary::{SUMMARY_CLASS, SUMMARY_FORMAT, SUMMARY_SCRIPT};
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
        " Eq ", " NotEq ", " == ", " != ", " < ", " <= ", " > ", " >= ", " && ", " || ", " Is ",
        " IsNot ",
    ],
    unary: &["! ", "-", "+", "~"],
    trivials: &["true", "false", "1", "0", "null"],
};

/// Where the JDK is, and where to run it.
#[derive(Debug, Clone)]
pub struct JavaToolchain {
    program: String,
    working_directory: PathBuf,
}

impl JavaToolchain {
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

        let is_java = base == "java"
            || base == "mvn"
            || base == "mvnw"
            || base == "gradle"
            || base == "gradlew"
            || base == "java.exe";
        if !is_java {
            return None;
        }
        Some(Self::new(working_directory))
    }

    /// A Java project identified by its test files rather than by its command.
    ///
    /// A repository whose tests are `src/test/**/*Test.java` is a Java project
    /// however it runs them, and a command that runs a committed wrapper script
    /// names no JDK at all. Deriving the toolchain only from the command meant
    /// such a project lost structural analysis while its red/green proof still
    /// held, which is why Java reached `VerifiedWithWarnings` and never
    /// `Verified`.
    ///
    /// The analyzer itself needs only `java` — it compiles the summary tool in
    /// memory with single-file source launch — so no build tool is required to
    /// be present for analysis to run.
    pub fn new(working_directory: &Path) -> Self {
        Self {
            program: "java".to_owned(),
            working_directory: working_directory.to_owned(),
        }
    }

    /// Whether a set of changed test files proves this is a Java project.
    pub fn is_java_test_file(path: &str) -> bool {
        path.ends_with(".java")
    }

    /// Summarize one Java file.
    ///
    /// `java Summarize.java <target>` uses single-file source launch, which
    /// compiles in memory. It requires a JDK rather than a bare JRE, which the
    /// tool reports explicitly rather than mistaking for a syntax error.
    fn summarize(&self, path: &Path) -> Result<FileSummary> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        self.summarize_source(&source)
    }

    fn summarize_source(&self, source: &str) -> Result<FileSummary> {
        let directory = tempfile::Builder::new()
            .prefix("witdiff-javasummary-")
            .tempdir()
            .context("failed creating a directory for the Java summary")?;

        // The public class must match the filename, so the name is fixed.
        let tool = directory.path().join(format!("{SUMMARY_CLASS}.java"));
        std::fs::write(&tool, SUMMARY_SCRIPT).context("failed writing the Java summary tool")?;
        let target = directory.path().join("Target.java");
        std::fs::write(&target, source).context("failed writing the Java target file")?;

        let output = Command::new(&self.program)
            .arg(format!("{SUMMARY_CLASS}.java"))
            .arg("Target.java")
            .current_dir(directory.path())
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze Java source; is a JDK on PATH?",
                    self.program
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing Java source: {}",
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
                "the Java summary used format {}, but this build expects {}",
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
pub fn analyze_java_test_change(
    path: &str,
    toolchain: &JavaToolchain,
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
                    "this Java test file could not be analyzed, so no structural comparison was performed: {error}"
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
            format!("this Java test file could not be parsed: {error}"),
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
                        "the base revision of this Java test file could not be parsed, so removed assertions and changed expectations were not compared: {}",
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
                        "the base revision of this Java test file could not be analyzed, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    findings.extend(crate::testshape::analyze(
        path,
        "Java",
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
    fn toolchain_is_discovered_from_java_build_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["java".to_string(), "-jar".into(), "junit.jar".into()],
            vec!["mvn".into(), "test".into()],
            vec!["./mvnw".into(), "test".into()],
            vec!["gradle".into(), "test".into()],
            vec!["./gradlew".into(), "test".into()],
        ] {
            assert!(
                JavaToolchain::from_test_command(&command, cwd).is_some(),
                "{command:?} should be recognized as a Java project"
            );
        }
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["go".into(), "test".into()],
            vec!["python3".into(), "-m".into(), "pytest".into()],
            vec!["npm".into(), "test".into()],
        ] {
            assert!(
                JavaToolchain::from_test_command(&command, cwd).is_none(),
                "{command:?} is not Java and must not be treated as such"
            );
        }
    }

    /// The vocabulary must list the names the tool *emits*, which for
    /// `assertEquals` is the canonical `Eq`, not the Java spelling `==`.
    #[test]
    fn the_java_operator_vocabulary_matches_the_tool() {
        assert!(OPERATORS.comparisons.contains(&" Eq "));
        assert!(OPERATORS.comparisons.contains(&" NotEq "));
        assert!(OPERATORS.comparisons.contains(&" > "));
        assert!(OPERATORS.unary.contains(&"! "));
        assert!(OPERATORS.trivials.contains(&"true"));
    }

    /// The canonical form the tool produces for `assertEquals` must be
    /// classified as an exact comparison, or the subject cannot be extracted
    /// and every expectation change is misreported as a removal.
    #[test]
    fn the_canonical_equality_form_is_recognized() {
        use crate::testshape::{strength_of, subject_of, Strength};
        assert_eq!(strength_of("Add(1, 1) Eq 2", &OPERATORS), Strength::Exact);
        assert_eq!(
            subject_of("Add(1, 1) Eq 2", &OPERATORS).as_deref(),
            Some("Add(1, 1)")
        );
        // And it pairs with the predicate form over the same subject.
        assert_eq!(
            subject_of("Add(1, 1) > 0", &OPERATORS).as_deref(),
            Some("Add(1, 1)")
        );
    }
}
