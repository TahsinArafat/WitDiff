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
    /// Which results count as a pass, committed once per repository.
    ///
    /// Lives in its own section rather than under `verification` because it
    /// answers a different question: `verification` describes how to gather
    /// evidence, this describes what evidence will be accepted.
    pub gate: crate::model::GatePolicy,
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
    /// How many times to run a command that **fails**, to tell a real failure
    /// from a flaky one.
    ///
    /// One means a single sample, which is the previous behaviour and the
    /// default. Above one, a failing run is repeated until it passes or the
    /// bound is reached, and disagreement is reported as an unstable suite
    /// rather than as evidence in either direction.
    ///
    /// Only failures are repeated. A passing run has nothing to distinguish,
    /// and repeating it would double the cost of the common case to learn that
    /// it still passes.
    #[serde(default = "one")]
    pub flake_repeats: usize,
    /// Dependency directories to make available to the base worktree.
    ///
    /// The base experiment runs in a `git worktree`, which contains only
    /// committed files. A project whose dependencies are gitignored —
    /// Composer's `vendor/`, npm's `node_modules/`, a Python `venv/` — therefore
    /// cannot start its test command on the base revision at all: the run fails
    /// with something like `Could not open input file: vendor/bin/phpunit`, and
    /// the receipt reports `base control: FAIL`, which reads as "the base is
    /// broken" rather than "the base could not start".
    ///
    /// Each directory is **symlinked** from the workspace into the worktree
    /// when the workspace has it and the worktree does not. A symlink is used
    /// rather than a copy because `node_modules` can be gigabytes, and rather
    /// than `cp -al` because a hardlink tree would share inodes that a test run
    /// could then mutate.
    ///
    /// **Only when the lockfile is unchanged.** Sharing the workspace's
    /// dependencies with the base revision is sound exactly when the two
    /// revisions resolve to the same dependency set. If `composer.lock`,
    /// `package-lock.json`, `Cargo.lock` or `go.sum` changed between base and
    /// head, the workspace's installed dependencies are *not* what the base
    /// revision would have installed, and linking them would run the base
    /// against a dependency set it never declared. That direction can invent a
    /// failure or hide a regression, so in that case nothing is linked and the
    /// receipt says so.
    ///
    /// Opt-in and explicit: this changes what the base experiment executes,
    /// which is a semantic decision about evidence rather than a convenience.
    #[serde(default)]
    pub base_dependency_dirs: Vec<String>,
    /// Run the test command inside a container instead of on the host.
    ///
    /// When set to an image name, the command is rewritten to
    /// `docker run --rm --name <unique> --workdir <cwd> --volume <cwd>:<cwd>
    /// <image> <command>`. The workspace is mounted at its own absolute path so
    /// that paths in the command's output still resolve.
    ///
    /// This is opt-in because the container must contain whatever the test
    /// command needs — the toolchain, and the dependency cache. `cargo test` in
    /// a bare Rust image downloads the workspace's crates; a project with no
    /// network and no preloaded registry will fail. Isolation is therefore a
    /// property the image has to provide, not something WitDiff can promise
    /// from the image's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_image: Option<String>,
    /// Host environment variable names to forward into the container.
    ///
    /// An explicit allow-list rather than forwarding everything: a sandboxed
    /// run should not inherit the operator's shell by default. Empty means
    /// nothing is forwarded beyond the `WITDIFF=1` marker.
    #[serde(default)]
    pub sandbox_env: Vec<String>,
    /// Whether the container may reach the network.
    ///
    /// Defaults to true so that a project which fetches dependencies keeps
    /// working. Set false to run with `--network none`, which is the stronger
    /// isolation and the one that prevents a candidate's test command from
    /// exfiltrating anything.
    #[serde(default = "default_true")]
    pub sandbox_network: bool,
    /// Whether to mutate the changed production code and observe the tests.
    ///
    /// Off by default: each mutant costs a full test run (ADR-0011). Mutation is
    /// supplementary evidence and never changes `status`.
    #[serde(default)]
    pub mutation: bool,
    /// Measure how much of the changed production code the tests execute.
    ///
    /// Supplementary evidence: it never changes `status`, for the same reason
    /// mutation does not (ADR-0011). It re-runs the suite with instrumentation,
    /// so it is a second full test run and is off by default.
    ///
    /// Only implemented for a `cargo test` command; anything else is reported
    /// as not available rather than approximated.
    #[serde(default)]
    pub coverage: bool,
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
    /// Path to an Ed25519 private key used to sign the receipt.
    ///
    /// When set, the receipt is signed over its `verification_digest`. WitDiff
    /// reads the key and never stores, copies or logs it (ADR-0022).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
    /// The exit code when a receipt could not be signed.
    ///
    /// Defaults to 1, a tool error. Setting it to 2 makes an unsigned receipt
    /// fail a CI gate, which is what an operator who asked for signatures and did
    /// not get one wants.
    #[serde(default)]
    pub signing_failure_exit_code: u8,
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

fn default_true() -> bool {
    true
}

fn one() -> usize {
    1
}

impl VerificationConfig {
    /// Resolve the configured framework name.
    ///
    /// An unrecognized name is an error: silently classifying with the Rust
    /// matcher would produce exactly the wrong-conservative answer ADR-0012
    /// exists to fix, and the operator would have no way to notice.
    /// Resolve the configured framework.
    ///
    /// An unrecognized name fails loudly rather than falling back: silently
    /// classifying another framework's output would turn a broken configuration
    /// into a wrong verdict. The message therefore has to be actionable rather
    /// than merely correct — a tester reported the previous wording as "fighting
    /// with the unknown-framework error", because it listed valid values without
    /// saying what the choice was *for* or where to look.
    pub fn framework(&self) -> anyhow::Result<TestFramework> {
        TestFramework::parse(&self.framework).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown test framework `{}`.\n\n\
                 `verification.framework` selects whose output WitDiff classifies when the \
                 base experiment fails. WitDiff implements one classifier per framework, and \
                 only these: {}.\n\n\
                 If yours is not listed, red/green proof is unavailable too, because WitDiff \
                 cannot tell a test failure from a build failure in output it has never seen — \
                 guessing there is exactly what would produce a false proof. Set \
                 `verification.flake_repeats` and the analysis settings aside; they depend on \
                 this classifier.\n\n\
                 Structural test-integrity analysis needs a parser for the language. Supported \
                 today: Rust, Python, Go, Java, Ruby, JavaScript/TypeScript. See \
                 docs/support-matrix.md for what is proven per language, and what adding a \
                 framework would cost.",
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
            flake_repeats: 1,
            // Empty by default: linking a dependency directory into the base
            // worktree changes what the base experiment executes, so it is
            // opted into per repository rather than guessed.
            base_dependency_dirs: Vec::new(),
            sandbox_image: None,
            sandbox_env: Vec::new(),
            sandbox_network: true,
            coverage: false,
            max_output_bytes: 16_384,
            // Generous by default so a cold CI build is not mistaken for a hang,
            // while still bounding a truly stuck suite.
            timeout_secs: Some(900),
            mutation: false,
            max_mutants: default_max_mutants(),
            max_mutants_per_function: default_max_mutants_per_function(),
            framework: default_framework(),
            signing_key: None,
            signing_failure_exit_code: 1,
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

    /// A starting configuration inferred from the files in a repository.
    ///
    /// `init` writes Rust defaults unconditionally otherwise, which is actively
    /// misleading in a Python or Go repository: the developer's first
    /// verification would run `cargo test` in a project that has no Cargo.toml.
    ///
    /// Detection is by the presence of a manifest or a conventional test file.
    /// It is a starting point for the developer to edit, not a heuristic that
    /// runs at verification time, so being wrong is cheap and visible.
    pub fn inferred_for(root: &Path) -> Self {
        let mut config = Self::default();

        let exists = |name: &str| root.join(name).exists();
        let any_exists = |names: &[&str]| names.iter().any(|name| exists(name));

        if any_exists(&["Cargo.toml"]) {
            // The default already describes a Rust project.
            return config;
        }

        if any_exists(&[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
            "tox.ini",
        ]) {
            config.project.language = "python".into();
            config.verification.framework = "pytest".into();
            config.verification.test_command = vec!["python3".into(), "-m".into(), "pytest".into()];
            config.verification.test_globs = vec![
                "tests/*.py".into(),
                "tests/**/*.py".into(),
                "test_*.py".into(),
                "**/test_*.py".into(),
                "*_test.py".into(),
                "**/*_test.py".into(),
            ];
            config.verification.extra_test_paths = vec!["tests".into()];
            return config;
        }

        if any_exists(&["go.mod"]) {
            config.project.language = "go".into();
            config.verification.framework = "go".into();
            config.verification.test_command = vec!["go".into(), "test".into(), "./...".into()];
            config.verification.test_globs = vec!["*_test.go".into(), "**/*_test.go".into()];
            return config;
        }

        // Java before JavaScript: a project can have a package.json for
        // front-end tooling while being a Maven or Gradle project, and the
        // build file is the stronger signal.
        if any_exists(&[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
        ]) || exists("mvnw")
            || exists("gradlew")
        {
            config.project.language = "java".into();
            config.verification.framework = "java".into();
            // Prefer the committed wrapper when there is one: it downloads the
            // version the repository pins, and it is what a developer runs.
            let uses_gradle = any_exists(&["build.gradle", "build.gradle.kts", "settings.gradle"])
                || exists("gradlew");
            // Use the committed wrapper when there is one, because it downloads
            // the version the repository pins. Without a wrapper, name the plain
            // tool: writing `./mvnw test` in a project that has no `mvnw` would
            // hand the developer a command that cannot start.
            let has_wrapper =
                exists("gradlew") || exists("gradlew.bat") || exists("mvnw") || exists("mvnw.cmd");
            config.verification.test_command = match (has_wrapper, uses_gradle) {
                (true, true) => vec!["./gradlew".into(), "test".into()],
                (true, false) => vec!["./mvnw".into(), "test".into()],
                (false, true) => vec!["gradle".into(), "test".into()],
                (false, false) => vec!["mvn".into(), "test".into()],
            };
            config.verification.test_globs =
                vec!["src/test/**/*.java".into(), "**/src/test/**/*.java".into()];
            config.verification.extra_test_paths = vec!["src/test".into()];
            return config;
        }

        if any_exists(&["Gemfile", "Rakefile", ".rspec", "spec"]) {
            config.project.language = "ruby".into();
            // `rake test` is the conventional entry point; a project with an
            // .rspec file is using RSpec.
            let uses_rspec = exists(".rspec") || exists("spec");
            config.verification.framework = if uses_rspec { "rspec" } else { "minitest" }.into();
            config.verification.test_command = if uses_rspec {
                vec!["bundle".into(), "exec".into(), "rspec".into()]
            } else {
                vec!["rake".into(), "test".into()]
            };
            config.verification.test_globs =
                vec!["test/**/*_test.rb".into(), "spec/**/*_spec.rb".into()];
            config.verification.extra_test_paths = vec!["test".into(), "spec".into()];
            return config;
        }

        // PHP before JavaScript: a WordPress plugin commonly ships a
        // package.json for its build tooling while its tests are PHPUnit or
        // Pest, so package.json is the weaker signal.
        if any_exists(&[
            "composer.json",
            "phpunit.xml",
            "phpunit.xml.dist",
            "pest.php",
        ]) || exists("phpunit")
            || exists("vendor/bin/phpunit")
        {
            config.project.language = "php".into();
            let uses_pest = exists("pest.php") || exists("tests/Pest.php");
            config.verification.framework = if uses_pest { "pest" } else { "phpunit" }.into();
            // Use a committed binary when one exists; otherwise name the
            // installed tool, which is what a developer without Composer's
            // vendor directory would run.
            let local_phpunit = exists("vendor/bin/phpunit") || exists("phpunit");
            config.verification.test_command = if local_phpunit {
                vec!["php".into(), "vendor/bin/phpunit".into()]
            } else {
                vec!["phpunit".into()]
            };
            config.verification.test_globs = vec![
                "tests/**/*Test.php".into(),
                "tests/**/*.php".into(),
                "**/*Test.php".into(),
                "**/tests/**/*.php".into(),
            ];
            config.verification.extra_test_paths = vec!["tests".into()];
            return config;
        }

        if any_exists(&["package.json"]) {
            config.project.language = "javascript".into();
            config.verification.framework = "javascript".into();
            // `npm test` is the conventional entry point and defers to whatever
            // the project configured, which is more likely correct than naming
            // a runner directly.
            config.verification.test_command = vec!["npm".into(), "test".into()];
            config.verification.test_globs = vec![
                "**/*.test.ts".into(),
                "**/*.test.js".into(),
                "**/*.spec.ts".into(),
                "**/*.spec.js".into(),
                "tests/**/*.ts".into(),
                "tests/**/*.js".into(),
            ];
            config.verification.extra_test_paths = vec!["tests".into(), "__tests__".into()];
            return config;
        }

        // Nothing recognized: keep the Rust defaults, which is also what a
        // greenfield project is most likely to want from this tool today.
        config
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
        assert!(
            config.base_dependency_dirs.is_empty(),
            "base dependency reuse must be off by default: it changes what the \
             base experiment executes"
        );
    }

    /// Legacy configuration must keep parsing. A field added without a default
    /// makes every existing `witdiff.toml` fail to load, which is a breaking
    /// change to a file the user owns.
    #[test]
    fn a_config_without_base_dependency_dirs_still_parses() {
        let parsed: Config =
            toml::from_str("[verification]\ntest_command = [\"cargo\", \"test\"]\n")
                .expect("a config written before this field existed must still load");
        assert!(parsed.verification.base_dependency_dirs.is_empty());
    }

    #[test]
    fn base_dependency_dirs_round_trip() {
        let source = "[verification]\nbase_dependency_dirs = [\"vendor\", \"node_modules\"]\n";
        let parsed: Config = toml::from_str(source).expect("parse");
        assert_eq!(
            parsed.verification.base_dependency_dirs,
            vec!["vendor".to_string(), "node_modules".to_string()]
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

#[cfg(test)]
mod inference_tests {
    use super::*;

    fn infer(files: &[&str]) -> Config {
        let dir = tempfile::TempDir::new().expect("temp dir");
        for name in files {
            std::fs::write(dir.path().join(name), "").expect("write marker");
        }
        Config::inferred_for(dir.path())
    }

    /// Writing Rust defaults into a Python repository would make the first
    /// verification run `cargo test` in a project with no Cargo.toml.
    #[test]
    fn python_projects_get_pytest() {
        let config = infer(&["pyproject.toml"]);
        assert_eq!(config.project.language, "python");
        assert_eq!(config.verification.framework, "pytest");
        assert_eq!(config.verification.test_command[0], "python3");
        assert!(
            config
                .verification
                .test_globs
                .iter()
                .any(|g| g.contains("test_")),
            "pytest's conventional naming should be covered: {:?}",
            config.verification.test_globs
        );
    }

    #[test]
    fn go_projects_get_go_test() {
        let config = infer(&["go.mod"]);
        assert_eq!(config.project.language, "go");
        assert_eq!(config.verification.framework, "go");
        assert_eq!(
            config.verification.test_command,
            vec!["go", "test", "./..."]
        );
        assert!(config
            .verification
            .test_globs
            .iter()
            .any(|g| g.contains("_test.go")));
    }

    #[test]
    fn javascript_projects_get_npm_test() {
        let config = infer(&["package.json"]);
        assert_eq!(config.project.language, "javascript");
        assert_eq!(config.verification.framework, "javascript");
        assert_eq!(config.verification.test_command, vec!["npm", "test"]);
    }

    /// Rust stays the default, and a Rust repository must not be misdetected.
    #[test]
    fn cargo_projects_keep_the_rust_defaults() {
        let config = infer(&["Cargo.toml"]);
        assert_eq!(config.project.language, "rust");
        assert_eq!(config.verification.test_command[0], "cargo");
    }

    /// A repository with no recognizable manifest gets the defaults, which is
    /// what `init` has always done.
    #[test]
    fn an_unrecognized_project_falls_back_to_defaults() {
        let default = Config::default();
        let inferred = infer(&["README.md"]);
        assert_eq!(inferred.project.language, default.project.language);
        assert_eq!(
            inferred.verification.test_command,
            default.verification.test_command
        );
    }

    /// A Java project must not be told to run `cargo test`, which is what
    /// happened before this was added: every language except Rust, Python, Go
    /// and JavaScript fell through to the Rust defaults.
    #[test]
    fn java_projects_get_the_java_command() {
        let maven = infer(&["pom.xml"]);
        assert_eq!(maven.project.language, "java");
        assert_eq!(maven.verification.framework, "java");
        assert_eq!(maven.verification.test_command, vec!["mvn", "test"]);

        let gradle = infer(&["build.gradle"]);
        assert_eq!(gradle.project.language, "java");
        assert_eq!(gradle.verification.test_command, vec!["gradle", "test"]);
    }

    /// A committed wrapper is preferred because it downloads the version the
    /// repository pins, but naming it when it does not exist would hand the
    /// developer a command that cannot start.
    #[test]
    fn a_committed_wrapper_is_preferred_over_the_plain_tool() {
        let with_wrapper = infer(&["pom.xml", "mvnw"]);
        assert_eq!(
            with_wrapper.verification.test_command,
            vec!["./mvnw", "test"],
            "a committed wrapper is what a developer runs and is version-pinned"
        );

        let without = infer(&["pom.xml"]);
        assert_eq!(
            without.verification.test_command,
            vec!["mvn", "test"],
            "without a wrapper, ./mvnw would not exist"
        );
    }

    /// A project can carry a package.json for front-end tooling while being a
    /// JVM project; the build file is the stronger signal.
    #[test]
    fn a_jvm_build_file_wins_over_package_json() {
        let config = infer(&["pom.xml", "package.json"]);
        assert_eq!(
            config.project.language, "java",
            "a Maven project with front-end tooling is still a Maven project"
        );
    }

    #[test]
    fn ruby_projects_get_a_ruby_command() {
        let rspec = infer(&[".rspec", "Gemfile"]);
        assert_eq!(rspec.project.language, "ruby");
        assert_eq!(rspec.verification.framework, "rspec");

        let minitest = infer(&["Gemfile"]);
        assert_eq!(minitest.project.language, "ruby");
        assert_eq!(minitest.verification.framework, "minitest");
    }

    /// Every inferred configuration must be usable as-is, or `init` would
    /// hand the developer something that fails immediately.
    ///
    /// The framework check is the important one: `init` previously wrote a
    /// framework name the parser did not know, which would make every
    /// subsequent command fail.
    #[test]
    fn every_inferred_configuration_is_valid() {
        for markers in [
            vec!["Cargo.toml"],
            vec!["pyproject.toml"],
            vec!["go.mod"],
            vec!["package.json"],
            vec!["pom.xml"],
            vec!["build.gradle"],
            vec!["Gemfile"],
            vec![".rspec"],
            vec!["README.md"],
        ] {
            let config = infer(&markers);
            assert!(
                config.verification.framework().is_ok(),
                "{markers:?} produced an unparseable framework"
            );
            assert!(
                !config.verification.test_command.is_empty(),
                "{markers:?} produced no test command"
            );
            assert!(
                config.test_matcher().is_ok(),
                "{markers:?} produced invalid test globs"
            );
        }
    }

    /// The `[gate]` section must load, and an absent section must default.
    /// Otherwise an operator could set `strict = true` and silently leave the
    /// other two requirements at whatever they happened to be.
    #[test]
    fn a_gate_section_loads_and_absent_fields_default() {
        let loaded: Config = toml::from_str("[gate]\nstrict = true\nrequire_signature = true\n")
            .expect("a gate section must parse");
        assert!(loaded.gate.strict);
        assert!(loaded.gate.require_signature);
        assert!(
            !loaded.gate.fail_on_no_changed_tests,
            "an unset field must take its default rather than a neighbouring value"
        );

        let absent: Config = toml::from_str("[project]\nlanguage = \"rust\"\n")
            .expect("a config without a gate section must parse");
        assert!(
            absent.gate.is_default(),
            "an absent [gate] section must mean the defaults, got {:?}",
            absent.gate
        );
    }

    /// The shipped example is what an operator reads first. It must parse, or
    /// the one document meant to teach the policy is unusable.
    #[test]
    fn the_shipped_example_config_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|workspace| workspace.parent())
            .map(|repo| repo.join("examples").join("witdiff.toml"));
        let Some(path) = path else {
            return;
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return;
        };
        let parsed: Config =
            toml::from_str(&text).expect("examples/witdiff.toml must remain valid TOML");
        assert!(
            !parsed.verification.test_command.is_empty(),
            "the example must still configure a test command"
        );
        assert!(
            parsed.gate.is_default(),
            "the example documents defaults rather than unusual demands"
        );
    }

    /// The repository's own configuration must parse too: a broken one would
    /// mean the project cannot verify itself.
    #[test]
    fn this_repository_s_own_config_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|workspace| workspace.parent())
            .map(|repo| repo.join("witdiff.toml"));
        let Some(path) = path else {
            return;
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return;
        };
        toml::from_str::<Config>(&text).expect("witdiff.toml must remain valid TOML");
    }
}
