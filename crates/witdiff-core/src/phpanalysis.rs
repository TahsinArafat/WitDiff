//! Structural analysis of PHP test changes.
//!
//! See ADR-0023. The PHP counterpart to [`crate::rubyanalysis`], sharing the
//! same rule engine ([`crate::testshape`]) so no language can disagree about
//! what a weakening is.
//!
//! ## How it works
//!
//! `token_get_all` ships with every PHP installation, so a small embedded tool
//! ([`crate::phpscript::SUMMARY_SCRIPT`]) tokenizes the file and emits a
//! normalized structural summary. No Composer dependency is needed, and the
//! analysis works in any project that can run its tests — which matters for
//! WordPress plugins, where the test runner is often a plain `phpunit` call
//! rather than a Composer script.
//!
//! ## PHPUnit and Pest both put the expectation first
//!
//! `assertEquals(expected, actual)` and `expect($x)->toBe($y)` both name the
//! expected value before the subject, the reverse of Python and Go. The tool
//! swaps them so the shared engine compares like with like. This is the detail
//! that would otherwise fail silently: every expectation change would be
//! reported as a removal plus an addition rather than as a changed expectation.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::model::{IntegrityFinding, Severity};
use crate::phpscript::{SUMMARY_FORMAT, SUMMARY_SCRIPT};
use crate::testshape::{Assertion, FileSummary, Operators, TestFunction};

/// The PHP operator vocabulary.
///
/// Lists the names the summary tool actually *emits*, not the PHP spellings.
/// `Eq` and `NotEq` are both here because the tool normalizes
/// `assertEquals(expected, actual)` and `expect($x)->toBe($y)` into the
/// canonical subject-first form. An earlier version of a sibling analyzer
/// listed only `" == "`, which never matched `Eq`, so every expectation change
/// was misreported as a removed assertion followed by an addition.
const OPERATORS: Operators = Operators {
    comparisons: &[
        " Eq ", " NotEq ", " == ", " != ", " === ", " !== ", " < ", " <= ", " > ", " >= ", " && ",
        " || ",
    ],
    unary: &["!", "-", "+", "~"],
    trivials: &["true", "false", "1", "0", "null", "nil"],
};

/// Where PHP is, and where to run it.
#[derive(Debug, Clone)]
pub struct PhpToolchain {
    program: String,
    working_directory: PathBuf,
}

impl PhpToolchain {
    /// Derive the toolchain from a configured test command.
    ///
    /// Recognizes `php`, `phpunit`, `pest`, `composer` and `./vendor/bin/pest`,
    /// because those are the commands a PHP project actually configures.
    /// Returns `None` for anything else, so a non-PHP project is never probed.
    pub fn from_test_command(command: &[String], working_directory: &Path) -> Option<Self> {
        let program = command.first()?;
        let base = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let is_php = base == "php"
            || base == "phpunit"
            || base == "pest"
            || base == "composer"
            || base.ends_with(".phar");
        if !is_php {
            return None;
        }
        Some(Self {
            program: "php".to_owned(),
            working_directory: working_directory.to_owned(),
        })
    }

    /// Summarize one PHP file.
    ///
    /// `php summarize.php <path>` needs no compilation step, so this is the
    /// cheapest of the language adapters.
    ///
    /// Public because a structural summary is what makes these claims checkable
    /// from outside: a test that asserts "this file yields one assertion" is
    /// only meaningful if a caller can read what the tool actually saw.
    pub fn summarize(&self, path: &Path) -> Result<FileSummary> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        self.summarize_source(&source)
    }

    /// Summarize PHP source held in memory.
    pub fn summarize_source(&self, source: &str) -> Result<FileSummary> {
        let directory = tempfile::Builder::new()
            .prefix("witdiff-phpsummary-")
            .tempdir()
            .context("failed creating a directory for the PHP summary")?;

        let tool = directory.path().join("summarize.php");
        std::fs::write(&tool, SUMMARY_SCRIPT).context("failed writing the PHP summary tool")?;
        let target = directory.path().join("target.php");
        std::fs::write(&target, source).context("failed writing the PHP target file")?;

        let output = Command::new(&self.program)
            .arg("summarize.php")
            .arg("target.php")
            .current_dir(directory.path())
            .output()
            .with_context(|| {
                format!(
                    "failed to run `{}` to analyze PHP source; is PHP on PATH?",
                    self.program
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`{}` exited with {} while analyzing PHP source: {}",
                self.program,
                output.status,
                stderr.trim()
            );
        }

        // The tool writes JSON on the last stdout line; `php` may precede it
        // with diagnostics, so the last line is taken rather than the whole
        // stream.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let payload = stdout
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .unwrap_or_default();
        let wire: WireSummary = serde_json::from_str(payload.trim())
            .with_context(|| format!("the PHP summary was not valid JSON: {}", payload.trim()))?;

        if wire.format != SUMMARY_FORMAT {
            anyhow::bail!(
                "the PHP summary used format {}, but this build expects {}",
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

/// Analyze a PHP test file by comparing its base and head revisions.
pub fn analyze_php_test_change(
    path: &str,
    toolchain: &PhpToolchain,
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
                    "this PHP test file could not be analyzed, so no structural comparison was performed: {error}"
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
            format!("this PHP test file could not be parsed: {error}"),
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
                        "the base revision of this PHP test file could not be parsed, so removed assertions and changed expectations were not compared: {}",
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
                        "the base revision of this PHP test file could not be analyzed, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    findings.extend(crate::testshape::analyze(
        path,
        "PHP",
        &OPERATORS,
        base.as_ref(),
        &head,
    ));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolchain() -> PhpToolchain {
        PhpToolchain {
            program: "php".to_owned(),
            working_directory: PathBuf::from("/tmp"),
        }
    }

    fn php_available() -> bool {
        Command::new("php")
            .arg("-r")
            .arg("echo 1;")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn toolchain_is_discovered_from_php_commands() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["php".to_string(), "vendor/bin/phpunit".into()],
            vec!["phpunit".to_string()],
            vec!["./vendor/bin/pest".to_string()],
            vec!["phpunit.phar".to_string()],
        ] {
            assert!(
                PhpToolchain::from_test_command(&command, cwd).is_some(),
                "expected a PHP toolchain for {command:?}"
            );
        }
    }

    #[test]
    fn toolchain_is_not_derived_from_other_languages() {
        let cwd = Path::new("/tmp");
        for command in [
            vec!["cargo".to_string(), "test".into()],
            vec!["pytest".to_string()],
            vec!["go".to_string(), "test".into()],
            vec!["bundle".to_string(), "exec".into(), "rspec".into()],
        ] {
            assert!(
                PhpToolchain::from_test_command(&command, cwd).is_none(),
                "a non-PHP command must not yield a PHP toolchain: {command:?}"
            );
        }
    }

    /// The tool must be readable by the interpreter it will be run with. A
    /// script that only compiles under a developer's own PHP would fail inside
    /// a user's project and read as a product defect.
    #[test]
    fn embedded_tool_is_valid_php() {
        if !php_available() {
            eprintln!("skipping: php is not on PATH");
            return;
        }
        let directory = tempfile::Builder::new()
            .prefix("witdiff-phpcheck-")
            .tempdir()
            .expect("temp dir");
        let tool = directory.path().join("summarize.php");
        std::fs::write(&tool, SUMMARY_SCRIPT).expect("write tool");

        let output = Command::new("php")
            .arg("-l")
            .arg(&tool)
            .output()
            .expect("run php -l");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "the embedded PHP summary tool does not parse: {}",
            stderr.trim()
        );
    }

    /// A Pest example is a call, not a declaration. This is the shape most
    /// likely to regress silently, because nothing about it looks like a
    /// function until the token stream is read.
    #[test]
    fn pest_examples_are_collected_with_their_assertions() {
        if !php_available() {
            eprintln!("skipping: php is not on PATH");
            return;
        }
        let source = "<?php\ntest('adds', function () {\n    expect((new Calc(1, 2))->add())->toBe(3);\n});\n";
        let summary = toolchain()
            .summarize_source(source)
            .expect("summarize Pest source");
        assert_eq!(summary.functions.len(), 1, "expected one Pest example");
        let example = &summary.functions[0];
        assert_eq!(example.name, "adds");
        assert_eq!(example.assertions.len(), 1);
        // The subject's own `->add()` must not be mistaken for the matcher.
        assert_eq!(
            example.assertions[0].test, "(new Calc(1, 2))->add() Eq 3",
            "the expectation must be read from the chain, not the subject"
        );
    }

    /// PHPUnit puts the expectation first; the tool swaps it so the shared
    /// engine compares subject-first. Without this, every expectation change
    /// looks like a removal plus an addition.
    #[test]
    fn phpunit_expectations_are_normalized_subject_first() {
        if !php_available() {
            eprintln!("skipping: php is not on PATH");
            return;
        }
        let source =
            "<?php\nclass CalcTest extends TestCase {\n    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }\n}\n";
        let summary = toolchain().summarize_source(source).expect("summarize");
        let function = summary
            .functions
            .iter()
            .find(|function| function.name == "testAdd")
            .expect("testAdd");
        assert_eq!(function.assertions[0].test, "$this->add(2, 3) Eq 5");
    }

    /// A weakened test is the finding that makes this language worth
    /// supporting. Assert it end to end through the shared engine, so PHP is
    /// held to the same standard as the other five languages.
    #[test]
    fn a_weakened_phpunit_expectation_is_reported() {
        if !php_available() {
            eprintln!("skipping: php is not on PATH");
            return;
        }
        let base = "<?php\nclass CalcTest extends TestCase {\n    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }\n}\n";
        let head = "<?php\nclass CalcTest extends TestCase {\n    public function testAdd(): void {\n        $this->assertEquals(99, $this->add(2, 3));\n    }\n}\n";

        let directory = tempfile::Builder::new()
            .prefix("witdiff-phphead-")
            .tempdir()
            .expect("temp dir");
        let head_path = directory.path().join("CalcTest.php");
        std::fs::write(&head_path, head).expect("write head");

        let findings =
            analyze_php_test_change("tests/CalcTest.php", &toolchain(), &head_path, Some(base));
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "changed_expected_value"),
            "a changed expectation must be reported, got {:?}",
            findings
                .iter()
                .map(|finding| &finding.rule)
                .collect::<Vec<_>>()
        );
    }

    /// Removing an assertion outright is a weakening the engine must catch.
    #[test]
    fn a_removed_phpunit_assertion_is_reported() {
        if !php_available() {
            eprintln!("skipping: php is not on PATH");
            return;
        }
        let base = "<?php\nclass CalcTest extends TestCase {\n    public function testAdd(): void {\n        $this->assertEquals(5, $this->add(2, 3));\n    }\n}\n";
        let head = "<?php\nclass CalcTest extends TestCase {\n    public function testAdd(): void {\n        $this->assertTrue(true);\n    }\n}\n";

        let directory = tempfile::Builder::new()
            .prefix("witdiff-phphead-")
            .tempdir()
            .expect("temp dir");
        let head_path = directory.path().join("CalcTest.php");
        std::fs::write(&head_path, head).expect("write head");

        let findings =
            analyze_php_test_change("tests/CalcTest.php", &toolchain(), &head_path, Some(base));
        assert!(
            !findings.is_empty(),
            "removing a real assertion for `assertTrue(true)` must be reported"
        );
    }
}
