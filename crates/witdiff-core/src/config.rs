use std::{fs, path::Path};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::framework::TestFramework;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub project: ProjectConfig,
    pub verification: VerificationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VerificationConfig {
    pub base: Option<String>,
    pub test_command: Vec<String>,
    pub test_globs: Vec<String>,
    pub extra_test_paths: Vec<String>,
    pub block_on_integrity_findings: bool,
    pub max_output_bytes: usize,
    /// When true, narrow the test command to the cargo targets of the changed
    /// dedicated tests instead of running the whole suite.
    ///
    /// Narrowing is opportunistic and never silently substitutes evidence: it
    /// is applied only when the configured command can be narrowed without
    /// changing what actually runs (see [`crate::selection`]). When narrowing is
    /// not possible the full suite runs and a note records why. A narrowed run
    /// is a strictly smaller claim, so a failure is attributed to the changed
    /// tests only when those tests are the ones that ran.
    pub targeted_test_selection: bool,
    /// Wall-clock deadline in seconds for each individual test command run.
    ///
    /// A run that exceeds it is killed and recorded as `timed_out`, which can
    /// never satisfy a proof. A hung suite is a fact about the candidate, not a
    /// WitDiff malfunction, so it is reported rather than allowed to block.
    pub timeout_secs: Option<u64>,
    /// Whether to mutate the changed production code and observe the tests.
    ///
    /// Off by default: each mutant costs a full test run (ADR-0011). Mutation is
    /// supplementary evidence and never changes `status`.
    #[serde(default)]
    pub mutation: bool,
    /// Maximum mutants attempted per run. Bounds are mandatory because mutation
    /// cost is otherwise unbounded; the truncation is deterministic.
    #[serde(default = "default_max_mutants")]
    pub max_mutants: usize,
    /// Maximum mutants attempted per changed function.
    #[serde(default = "default_max_mutants_per_function")]
    pub max_mutants_per_function: usize,
    /// Which test framework's output the run should be classified against.
    ///
    /// Named explicitly rather than sniffed from output, because inferring it
    /// from text the candidate controls could turn a broken invocation into a
    /// test failure (ADR-0012). Defaults to `cargo`; an unrecognized value is
    /// an error rather than a silent fallback to the Rust classifier.
    #[serde(default = "default_framework")]
    pub framework: String,
}

fn default_framework() -> String {
    TestFramework::Cargo.as_str().to_owned()
}

fn default_max_mutants() -> usize {
    25
}

fn default_max_mutants_per_function() -> usize {
    5
}

impl VerificationConfig {
    /// Resolve the configured framework name.
    ///
    /// An unrecognized name is an error: silently classifying with the Rust
    /// matcher would produce exactly the wrong-conservative answer ADR-0012
    /// exists to fix, and the operator would have no way to notice.
    pub fn framework(&self) -> anyhow::Result<TestFramework> {
        TestFramework::parse(&self.framework).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown test framework `{}`; supported values are {}",
                self.framework,
                TestFramework::all()
                    .iter()
                    .map(|framework| framework.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            language: "rust".into(),
        }
    }
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            base: None,
            test_command: vec![
                "cargo".into(),
                "test".into(),
                "--all-targets".into(),
                "--all-features".into(),
            ],
            test_globs: vec![
                "tests/*.rs".into(),
                "tests/**/*.rs".into(),
                "**/tests/*.rs".into(),
                "**/tests/**/*.rs".into(),
                "**/*_test.rs".into(),
                "**/test_*.rs".into(),
            ],
            extra_test_paths: Vec::new(),
            block_on_integrity_findings: true,
            // Narrowing makes verification faster but yields a strictly
            // smaller claim, so it stays opt-in for v1. Enable it per
            // repository once the full-suite path is understood.
            targeted_test_selection: false,
            max_output_bytes: 16_384,
            // Generous by default so a cold CI build is not mistaken for a hang,
            // while still bounding a truly stuck suite.
            timeout_secs: Some(900),
            mutation: false,
            max_mutants: default_max_mutants(),
            max_mutants_per_function: default_max_mutants_per_function(),
            framework: default_framework(),
        }
    }
}

impl Config {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join("witdiff.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn render_default() -> Result<String> {
        toml::to_string_pretty(&Self::default()).context("failed to serialize default config")
    }

    pub fn test_matcher(&self) -> Result<TestMatcher> {
        TestMatcher::new(
            &self.verification.test_globs,
            &self.verification.extra_test_paths,
        )
    }
}

#[derive(Debug, Clone)]
pub struct TestMatcher {
    globs: GlobSet,
    extra_prefixes: Vec<String>,
}

impl TestMatcher {
    pub fn new(patterns: &[String], extra_test_paths: &[String]) -> Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(
                Glob::new(pattern)
                    .with_context(|| format!("invalid test glob in witdiff.toml: {pattern}"))?,
            );
        }
        let globs = builder
            .build()
            .context("failed to build test glob matcher")?;
        let extra_prefixes = extra_test_paths
            .iter()
            .map(|p| normalize(p).trim_end_matches('/').to_owned())
            .collect();
        Ok(Self {
            globs,
            extra_prefixes,
        })
    }

    pub fn is_test_path(&self, path: &str) -> bool {
        let path = normalize(path);
        if self.globs.is_match(&path) {
            return true;
        }
        self.extra_prefixes
            .iter()
            .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults that ADR-0011 and the README promise. Mutation is opt-in
    /// because each mutant costs a full test run, and the bounds exist so an
    /// unbounded mutant population cannot run away.
    ///
    /// This test exists because a surviving mutant found it missing: flipping
    /// `mutation: false` to `true` in the default broke no test, even though
    /// the documentation states the opposite.
    #[test]
    fn defaults_keep_optional_and_costly_features_off() {
        let config = VerificationConfig::default();
        assert!(
            !config.mutation,
            "mutation must be off by default: it costs a test run per mutant"
        );
        assert!(
            !config.targeted_test_selection,
            "targeted selection must be off by default"
        );
        assert!(
            config.max_mutants > 0 && config.max_mutants_per_function > 0,
            "mutation bounds must be positive, or no mutant could ever run"
        );
        assert!(
            config.block_on_integrity_findings,
            "high-severity integrity findings must block verification by default"
        );
    }

    #[test]
    fn default_matcher_finds_rust_integration_tests() {
        let matcher = Config::default().test_matcher().unwrap();
        assert!(matcher.is_test_path("tests/auth.rs"));
        assert!(matcher.is_test_path("crates/api/tests/login.rs"));
        assert!(matcher.is_test_path("src/parser_test.rs"));
        assert!(!matcher.is_test_path("src/parser.rs"));
    }
}
