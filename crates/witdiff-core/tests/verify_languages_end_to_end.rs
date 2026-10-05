//! End-to-end red/green verification against real non-Rust repositories.
//!
//! `verify_end_to_end.rs` proves the core invariant for Rust. This file proves
//! it for the other five supported languages by running `verify_repository`
//! against real temporary Git repositories containing real projects, and
//! asserting the whole chain: HEAD green, pristine base control green, and
//! base-plus-transplanted-test red **for a recognized test reason**.
//!
//! The reason this file exists: verifying an *analyzer* against a real parser or
//! runner is not the same as proving the *verification* works. Running the
//! Python, Go, Java, Ruby and JavaScript analyzers against real toolchains found
//! nine bugs, including two classifier bugs where a red suite was silently
//! reported as an unrecognized command failure — which cannot produce a proof, so
//! those languages could never verify anything. Those bugs were reachable from
//! the analyzer tests alone.
//!
//! Tests are `#[ignore]`d because they spawn real toolchains:
//!
//! ```text
//! cargo test --workspace --all-features -- --ignored --test-threads=1
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as StdCommand,
};

use tempfile::TempDir;
use witdiff_core::{
    verify::verify_repository, Config, FailureKind, GitRepo, VerificationStatus, VerifyOptions,
};

/// A temporary Git repository holding a real project in some language.
struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = TempDir::new().expect("temporary directory");
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).expect("fixture root");

        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "witdiff@example.invalid"]);
        git(&root, &["config", "user.name", "WitDiff Test"]);
        git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);

        for (relative, contents) in files {
            write(&root, relative, contents);
        }
        Self { _tmp: tmp, root }
    }

    fn write(&self, relative: &str, contents: &str) {
        write(&self.root, relative, contents);
    }

    /// Commit the current tree as the base revision.
    ///
    /// Any ignore file the fixture ships must already be in place, because
    /// WitDiff's workspace fingerprint deliberately includes `git status` and
    /// untracked files: a first build that creates an untracked byproduct is a
    /// genuine change, and the fingerprint correctly reports stale evidence.
    fn commit_base(&self, message: &str) {
        git(&self.root, &["add", "-A"]);
        git(&self.root, &["commit", "-q", "-m", message]);
    }

    fn verify(&self, config: &Config) -> witdiff_core::Receipt {
        let repo = GitRepo::discover(&self.root).expect("discover fixture repository");
        verify_repository(&repo, config, VerifyOptions::default())
            .expect("verification should complete")
    }

    /// Verify with extra environment for the test command.
    ///
    /// The fixture's own script needs to know where the JUnit jars are, and
    /// WitDiff deliberately does not offer a way to inject environment into a
    /// configured command — that would be a shell-interpolation surface. The
    /// variable is set for the test process, which the child inherits.
    fn verify_with_env(&self, config: &Config, env: &[(&str, &str)]) -> witdiff_core::Receipt {
        let repo = GitRepo::discover(&self.root).expect("discover fixture repository");
        // The guard is process-wide because `verify_repository` spawns the test
        // command as a child, which inherits this process's environment.
        let _guard = EnvGuard::set(env);
        verify_repository(&repo, config, VerifyOptions::default())
            .expect("verification should complete")
    }
}

/// Sets environment variables for the duration of a scope.
///
/// WitDiff has no supported way to inject environment into a configured test
/// command — doing so would be a shell-interpolation surface, which the
/// process boundary deliberately avoids. Setting a variable on the test
/// process works because the spawned command inherits it, and the guard
/// restores the previous value so fixtures cannot leak into one another.
struct EnvGuard {
    restore: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(variables: &[(&str, &str)]) -> Self {
        let mut restore = Vec::new();
        for (key, value) in variables {
            let key = std::ffi::OsString::from(key);
            let previous = std::env::var_os(&key);
            std::env::set_var(&key, value);
            restore.push((key, previous));
        }
        Self { restore }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in self.restore.drain(..) {
            match previous {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&path, contents).expect("write fixture file");
}

fn git(repo: &Path, args: &[&str]) {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Whether a program is on PATH and runs.
fn program_available(program: &str, probe: &[&str]) -> bool {
    StdCommand::new(program)
        .args(probe)
        .output()
        .is_ok_and(|output| output.status.success() || program == "go")
}

/// The single assertion every test in this file makes.
///
/// The v0.1 invariant, stated once so each language proves the same thing:
///
/// > A newly changed dedicated regression test is credible only when the
/// > configured test command passes on the current workspace and fails for a
/// > recognized test reason when the test-only change is transplanted onto the
/// > base revision.
fn assert_red_green_proven(receipt: &witdiff_core::Receipt, language: &str) {
    assert_eq!(
        receipt.status,
        VerificationStatus::Verified,
        "{language}: expected Verified, got {:?}; notes={:?}; findings={:?}",
        receipt.status,
        receipt.notes,
        receipt.integrity_findings
    );
    assert!(
        receipt.red_green_proven,
        "{language}: red/green must be recorded as proven"
    );
    assert!(
        receipt.head_run.success,
        "{language}: HEAD run must pass; output={:?}",
        receipt.head_run.stdout
    );

    let control = receipt
        .base_control_run
        .as_ref()
        .expect("the pristine base control must have run");
    assert!(
        control.success,
        "{language}: the pristine base must pass the control run, otherwise a \
         later failure cannot be attributed to the changed tests; output={:?}",
        control.stdout
    );

    let base = receipt
        .base_run
        .as_ref()
        .expect("the base experiment must have run");
    assert!(
        !base.success,
        "{language}: base + the transplanted test must fail"
    );
    assert_eq!(
        base.failure_kind,
        Some(FailureKind::TestFailure),
        "{language}: the failure must be classified as a behavioural test \
         failure. A different kind means the experiment proved nothing; \
         output={:?}",
        base.stdout
    );
    assert!(
        receipt.evidence_fresh,
        "{language}: evidence must be fresh; the workspace shifted mid-run"
    );
    assert!(
        receipt.integrity_findings.is_empty(),
        "{language}: a credible change produces no integrity findings, got {:?}",
        receipt
            .integrity_findings
            .iter()
            .map(|f| (f.rule.as_str(), f.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// A config carrying only what a language needs, with a finite deadline.
fn config_for(
    framework: &str,
    test_command: &[&str],
    test_globs: &[&str],
    extra_test_paths: &[&str],
) -> Config {
    let mut config = Config::default();
    config.project.language = framework.to_owned();
    config.verification.framework = framework.to_owned();
    config.verification.test_command = test_command.iter().map(|s| (*s).to_owned()).collect();
    config.verification.test_globs = test_globs.iter().map(|s| (*s).to_owned()).collect();
    config.verification.extra_test_paths =
        extra_test_paths.iter().map(|s| (*s).to_owned()).collect();
    config.verification.max_output_bytes = 32_768;
    config.verification.timeout_secs = Some(600);
    config
}

// ---------------------------------------------------------------------------
// Python / pytest
// ---------------------------------------------------------------------------

fn python_config() -> Config {
    config_for(
        "pytest",
        &["python3", "-m", "pytest", "-q"],
        &["tests/test_*.py", "test_*.py", "**/test_*.py"],
        &["tests"],
    )
}

/// `is_even` is buggy at the base revision and fixed at head, and a new test
/// catches it. This is the same shape as the Rust fixture, in Python.
const PY_BUGGY: &str = "def is_even(value):\n    return value % 2 == 1\n";
const PY_FIXED: &str = "def is_even(value):\n    return value % 2 == 0\n";
const PY_CREDIBLE: &str =
    "from calc import is_even\n\n\ndef test_even_is_even():\n    assert is_even(2)\n    assert not is_even(3)\n";
const PY_VACUOUS: &str = "def test_arithmetic_is_unchanged():\n    assert 1 + 1 == 2\n";

#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn pytest_reaches_verified_end_to_end() {
    if !program_available("python3", &["--version"]) {
        eprintln!("python3 absent; skipping");
        return;
    }
    let project = Project::new(&[
        ("calc.py", PY_BUGGY),
        ("tests/test_existing.py", PY_VACUOUS),
    ]);
    project.commit_base("buggy base");

    project.write("calc.py", PY_FIXED);
    project.write("tests/test_regression.py", PY_CREDIBLE);

    let receipt = project.verify(&python_config());
    assert_red_green_proven(&receipt, "pytest");
}

/// A regression test that passes on both revisions proves nothing, and must be
/// reported as `not_verified` rather than as a proof.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_pytest_that_also_passes_on_base_is_not_verified() {
    if !program_available("python3", &["--version"]) {
        return;
    }
    let project = Project::new(&[
        ("calc.py", PY_BUGGY),
        ("tests/test_existing.py", PY_VACUOUS),
    ]);
    project.commit_base("buggy base");

    project.write("calc.py", PY_FIXED);
    project.write(
        "tests/test_vacuous.py",
        "from calc import is_even\n\n\ndef test_vacuous():\n    # True at both revisions, so it cannot demonstrate the fix.\n    assert is_even(0) or not is_even(0)\n",
    );

    let receipt = project.verify(&python_config());
    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "a test that passes on base cannot be a regression proof; notes={:?}",
        receipt.notes
    );
    assert!(!receipt.red_green_proven, "red/green must not be claimed");
}

/// A test weakened to `assert True` alongside a real fix must be caught, not
/// silently accepted. This is the case the Python analyzer was written for.
#[test]
#[ignore = "end-to-end: spawns the Python interpreter; run with -- --ignored"]
fn a_weakened_pytest_blocks_verified() {
    if !program_available("python3", &["--version"]) {
        return;
    }
    let project = Project::new(&[
        ("calc.py", PY_BUGGY),
        ("tests/test_existing.py", PY_VACUOUS),
    ]);
    project.commit_base("buggy base");

    project.write("calc.py", PY_FIXED);
    project.write(
        "tests/test_regression.py",
        "from calc import is_even\n\n\ndef test_even_is_even():\n    assert True\n",
    );

    let receipt = project.verify(&python_config());
    assert_ne!(
        receipt.status,
        VerificationStatus::Verified,
        "a gutted assertion must not reach Verified; findings={:?}",
        receipt
            .integrity_findings
            .iter()
            .map(|f| f.rule.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !receipt.integrity_findings.is_empty(),
        "the weakening must be reported as a finding"
    );
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn go_config() -> Config {
    config_for("go", &["go", "test", "./..."], &["*_test.go"], &[])
}

const GO_BUGGY: &str = "package calc\n\nfunc IsEven(value int) bool {\n\treturn value%2 == 1\n}\n";
const GO_FIXED: &str = "package calc\n\nfunc IsEven(value int) bool {\n\treturn value%2 == 0\n}\n";
const GO_EXISTING: &str =
    "package calc\n\nimport \"testing\"\n\nfunc TestArithmetic(t *testing.T) {\n\tif 1+1 != 2 {\n\t\tt.Errorf(\"arithmetic broken\")\n\t}\n}\n";
const GO_CREDIBLE: &str = "package calc\n\nimport \"testing\"\n\nfunc TestIsEven(t *testing.T) {\n\tif !IsEven(2) {\n\t\tt.Errorf(\"got %v\", IsEven(2))\n\t}\n\tif IsEven(3) {\n\t\tt.Errorf(\"got %v\", IsEven(3))\n\t}\n}\n";

#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn go_reaches_verified_end_to_end() {
    if !program_available("go", &["version"]) {
        eprintln!("go absent; skipping");
        return;
    }
    let project = Project::new(&[
        ("calc.go", GO_BUGGY),
        ("calc_test.go", GO_EXISTING),
        ("go.mod", "module calc\n\ngo 1.21\n"),
    ]);
    project.commit_base("buggy base");

    project.write("calc.go", GO_FIXED);
    project.write("regression_test.go", GO_CREDIBLE);

    let receipt = project.verify(&go_config());
    assert_red_green_proven(&receipt, "go");
}

/// Rewording a `t.Errorf` message must not be a finding: the message is not the
/// assertion, and the condition guarding the call is.
#[test]
#[ignore = "end-to-end: spawns the Go toolchain; run with -- --ignored"]
fn reworded_go_error_messages_are_not_findings() {
    if !program_available("go", &["version"]) {
        return;
    }
    let project = Project::new(&[
        ("calc.go", GO_FIXED),
        ("calc_test.go", GO_EXISTING),
        ("go.mod", "module calc\n\ngo 1.21\n"),
    ]);
    project.commit_base("base");

    project.write(
        "calc_test.go",
        "package calc\n\nimport \"testing\"\n\nfunc TestArithmetic(t *testing.T) {\n\tif 1+1 != 2 {\n\t\tt.Errorf(\"addition is wrong: %d\", 1+1)\n\t}\n}\n",
    );

    let receipt = project.verify(&go_config());
    assert!(
        receipt.integrity_findings.is_empty(),
        "rewording a message must not be reported; got {:?}",
        receipt
            .integrity_findings
            .iter()
            .map(|f| (f.rule.as_str(), f.message.clone()))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Java / JUnit
// ---------------------------------------------------------------------------

/// A JUnit 5 classpath, discovered from an existing installation.
///
/// Neither Maven nor Gradle is installed here, and the JUnit console launcher
/// is not vendored, so the jars are borrowed from whatever JUnit 5 runtime the
/// machine already has. Returning `None` means the test skips rather than
/// pretending to have run: a fixture that silently substituted a stub would
/// make the proof meaningless, which is the failure this project exists to
/// prevent.
fn junit_classpath() -> Option<String> {
    // A JUnit 5 runtime is identifiable by five artifacts that must all be
    // present and must agree on their version. Any one alone is not runnable.
    //
    // Two facts about real installations shape this:
    //
    // 1. **Version separator.** An IDE extension ships
    //    `junit-platform-engine_1.10.2.jar`; a Maven Central download is
    //    `junit-platform-engine-1.10.2.jar`. Matching only one made the other
    //    invisible, and the test then *skipped* in CI while appearing to pass.
    // 2. **Version coherence.** Mixing a `1.14.4` platform with a `6.0.1`
    //    Jupiter engine compiles the tests, runs them, prints `0 tests found`
    //    and exits 0 — a green run that executed nothing. So one coherent set is
    //    chosen and every member is required to agree on its major version.
    //
    // Both were found by running the real launcher, not by reading file names.
    const ARTIFACTS: [&str; 5] = [
        "junit-platform-engine",
        "junit-platform-commons",
        "junit-platform-launcher",
        "junit-jupiter-engine",
        "junit-jupiter-api",
    ];

    /// Split a jar name into its artifact and its leading version number.
    ///
    /// The version is the trailing run of digits, not whatever follows the
    /// first separator. Artifact names are themselves hyphenated
    /// (`junit-platform-engine`), so splitting on `-` gave the stem `junit`
    /// and the "version" `platform`, and nothing ever matched.
    fn split(jar: &Path) -> (String, String) {
        let name = jar.file_name().unwrap_or_default().to_string_lossy();
        let stem = name.strip_suffix(".jar").unwrap_or(&name);
        // Walk backwards to the start of the version.
        let bytes = stem.as_bytes();
        let mut cut = stem.len();
        while cut > 0 && bytes[cut - 1].is_ascii_digit() || (cut > 1 && bytes[cut - 1] == b'.') {
            cut -= 1;
        }
        let artifact = stem[..cut].trim_end_matches(['_', '-']).to_owned();
        let version = &stem[cut..];
        let major = version.split('.').next().unwrap_or_default().to_owned();
        (artifact, major)
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("WITDIFF_JUNIT_JARS") {
        roots.push(PathBuf::from(explicit));
    } else if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(&home).join(".vscode/extensions"));
        roots.push(PathBuf::from(&home).join(".m2/repository"));
    }

    for root in roots {
        // The jars may sit directly in `root`, or under a `server`/`lib`
        // subdirectory of a parent listed inside it.
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&root) {
            // Each entry is either a directory of jars itself or a parent
            // holding one under `server`/`lib`. Both contribute; the parent
            // form is how an IDE extension ships its runtime.
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jar") {
                    candidates.push(path);
                    continue;
                }
                for sub in ["server", "lib", "jars"] {
                    if let Ok(files) = std::fs::read_dir(path.join(sub)) {
                        for file in files.flatten() {
                            if file.path().extension().is_some_and(|e| e == "jar") {
                                candidates.push(file.path());
                            }
                        }
                    }
                }
            }
        }

        let jars: Vec<PathBuf> = candidates
            .into_iter()
            .filter(|p| {
                let (artifact, _) = split(p);
                artifact.starts_with("junit-platform-")
                    || artifact.starts_with("junit-jupiter-")
                    || artifact.contains("opentest4j")
                    || artifact.contains("apiguardian")
            })
            .collect();

        let platform_majors: Vec<String> = jars
            .iter()
            .filter(|p| split(p).0 == "junit-platform-engine")
            .map(|p| split(p).1)
            .collect();
        let jupiter_majors: Vec<String> = jars
            .iter()
            .filter(|p| split(p).0 == "junit-jupiter-engine")
            .map(|p| split(p).1)
            .collect();

        for platform_major in &platform_majors {
            for jupiter_major in &jupiter_majors {
                let mut chosen: Vec<PathBuf> = Vec::new();
                let mut complete = true;
                for artifact in ARTIFACTS {
                    let family: &str = if artifact.starts_with("junit-jupiter-") {
                        jupiter_major
                    } else {
                        platform_major
                    };
                    match jars.iter().find(|p| {
                        let (name, major) = split(p);
                        name == artifact && major == family
                    }) {
                        Some(jar) => chosen.push(jar.clone()),
                        None => {
                            complete = false;
                            break;
                        }
                    }
                }
                if complete {
                    let support: Vec<&PathBuf> = jars
                        .iter()
                        .filter(|p| {
                            let (name, _) = split(p);
                            name.contains("opentest4j") || name.contains("apiguardian")
                        })
                        .collect();
                    let path = support
                        .into_iter()
                        .chain(chosen.iter())
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(":");
                    return Some(path);
                }
            }
        }
    }
    None
}

fn java_config() -> Config {
    config_for(
        "java",
        &["./run_tests.sh"],
        // The same globs `init` writes for a Java project.
        &["src/test/**/*.java", "**/src/test/**/*.java"],
        &["src/test"],
    )
}

const JAVA_BUGGY: &str =
    "public class Calc {\n    public static int add(int a, int b) { return a - b; }\n}\n";
const JAVA_FIXED: &str =
    "public class Calc {\n    public static int add(int a, int b) { return a + b; }\n}\n";

/// A real JUnit test. `assertEquals` puts the expectation first, which is the
/// argument order the Java analyzer swaps at its boundary (ADR-0018).
const JAVA_TEST: &str = "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\npublic class RegressionTest {\n    @Test\n    public void addsTwoNumbers() {\n        assertEquals(4, Calc.add(2, 2), \"two and two\");\n    }\n}\n";

/// A test that passes at the base revision, so the control run is green.
/// `add(2, 0)` is the only shape that survives `a - b`.
const JAVA_BASE_TEST: &str = "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\npublic class CalcTest {\n    @Test\n    public void addsTwoAndZero() {\n        assertEquals(2, Calc.add(2, 0), \"two and zero\");\n    }\n}\n";

/// A launcher built on the real JUnit Platform API.
///
/// It prints **only** what JUnit itself emits. An earlier version of this
/// fixture appended a hand-written `Tests run: N, Failures: N` line so that
/// WitDiff's Surefire matcher would fire — which would have made the test prove
/// the stub rather than JUnit. JUnit's own failure text carries
/// `org.opentest4j.AssertionFailedError`, which the Java classifier matches, so
/// no synthetic summary is needed.
const JAVA_LAUNCHER: &str = r#"import org.junit.platform.launcher.Launcher;
import org.junit.platform.launcher.core.LauncherFactory;
import org.junit.platform.launcher.listeners.SummaryGeneratingListener;
import org.junit.platform.launcher.listeners.TestExecutionSummary;

import java.io.PrintWriter;

import static org.junit.platform.engine.discovery.DiscoverySelectors.selectPackage;
import static org.junit.platform.launcher.core.LauncherDiscoveryRequestBuilder.request;

public class RunTests {
    public static void main(String[] args) {
        Launcher launcher = LauncherFactory.create();
        SummaryGeneratingListener listener = new SummaryGeneratingListener();
        launcher.execute(request().selectors(selectPackage("")).build(), listener);

        TestExecutionSummary summary = listener.getSummary();
        PrintWriter out = new PrintWriter(System.out);
        summary.printTo(out);
        if (summary.getTotalFailureCount() > 0) {
            summary.printFailuresTo(out);
        }
        out.flush();
        System.exit(summary.getTotalFailureCount() > 0 ? 1 : 0);
    }
}
"#;

/// Compiles and runs the fixture with a real JDK and a real JUnit.
const JAVA_RUNNER: &str = r#"#!/bin/sh
# `javac` exit status is propagated: a compile failure must be visible as such
# rather than reported as a green run with no tests.
rm -rf out
mkdir -p out
javac -cp "$WITDIFF_JUNIT_CP" -d out src/*.java src/test/*.java || exit 2
java -cp "out:$WITDIFF_JUNIT_CP" RunTests
"#;

#[test]
#[ignore = "end-to-end: spawns the JDK and a real JUnit; run with -- --ignored"]
fn java_reaches_verified_end_to_end() {
    if !program_available("javac", &["-version"]) || !program_available("java", &["-version"]) {
        eprintln!("JDK absent; skipping");
        return;
    }
    let Some(classpath) = junit_classpath() else {
        let message = "no JUnit 5 runtime found; set WITDIFF_JUNIT_JARS to a \
             directory of JUnit platform jars, or install a JDK with a JUnit dependency";
        // A skipped test and a passing one look identical to the runner. That
        // ambiguity is how this fixture already reported success while running
        // nothing, so an explicitly required JUnit fails the job instead.
        assert!(
            std::env::var_os("WITDIFF_JUNIT_JARS").is_none(),
            "{message}"
        );
        eprintln!("{message}");
        return;
    };

    let project = Project::new(&[
        // `javac` writes into `out/`, and WitDiff's workspace fingerprint
        // deliberately includes untracked files, so build output must be
        // ignored. Without this the run reports stale evidence — correctly:
        // the first build really did change the workspace.
        (".gitignore", "out/\n"),
        ("src/Calc.java", JAVA_BUGGY),
        ("src/test/CalcTest.java", JAVA_BASE_TEST),
        ("src/test/RunTests.java", JAVA_LAUNCHER),
        ("run_tests.sh", JAVA_RUNNER),
    ]);
    fs::set_permissions(
        project.root.join("run_tests.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("make runner executable");
    project.commit_base("buggy base");

    project.write("src/Calc.java", JAVA_FIXED);
    project.write("src/test/RegressionTest.java", JAVA_TEST);

    let receipt =
        project.verify_with_env(&java_config(), &[("WITDIFF_JUNIT_CP", classpath.as_str())]);
    // The red/green proof is established. The status is
    // `VerifiedWithWarnings` rather than `Verified` because the configured
    // command is a script, so the analyzer cannot derive a Java toolchain and
    // reports that structural analysis was not performed. That is the correct
    // conservative outcome: fewer findings, never wrong ones.
    assert_eq!(
        receipt.status,
        VerificationStatus::VerifiedWithWarnings,
        "java: expected VerifiedWithWarnings, got {:?}; notes={:?}",
        receipt.status,
        receipt.notes
    );
    assert!(receipt.red_green_proven, "java: red/green must be proven");
    let base = receipt.base_run.as_ref().expect("the base experiment ran");
    assert_eq!(
        base.failure_kind,
        Some(FailureKind::TestFailure),
        "java: the base failure must be a behavioural test failure, not a \
         compile error; stdout={:?} stderr={:?}",
        base.stdout,
        base.stderr
    );

    // A wrapper script rather than `java`/`mvn`/`gradle` means the analyzer
    // cannot derive a toolchain, so structural analysis is unavailable. That is
    // reported rather than passed over, which is the conservative direction,
    // but it is a real limitation and the receipt says so.
    let unparsed = receipt
        .integrity_findings
        .iter()
        .find(|f| f.rule == "test_source_unparsable")
        .expect("a script command loses structural analysis, and must say so");
    assert!(
        unparsed.message.contains("could not be determined"),
        "the note should name the cause, got {}",
        unparsed.message
    );
}

// ---------------------------------------------------------------------------
// Ruby / RSpec
// ---------------------------------------------------------------------------

fn rspec_config() -> Config {
    config_for(
        "rspec",
        &["rspec", "--no-color"],
        &["spec/**/*_spec.rb"],
        &["spec"],
    )
}

const RUBY_BUGGY: &str = "module Calc\n  def self.add(a, b)\n    a - b\n  end\nend\n";
const RUBY_FIXED: &str = "module Calc\n  def self.add(a, b)\n    a + b\n  end\nend\n";

/// The existing spec must pass at the **base** revision, or the control run is
/// legitimately red and no proof is possible. So it asserts something the bug
/// does not affect; the new regression spec is what exposes the defect.
const RUBY_EXISTING: &str =
    "RSpec.describe \"Calc\" do\n  it \"adds zero\" do\n    expect(Calc.add(1, 0)).to eq(1)\n  end\nend\n";

#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn rspec_reaches_verified_end_to_end() {
    if !program_available("rspec", &["--version"]) {
        eprintln!("rspec absent; skipping");
        return;
    }
    let project = Project::new(&[
        ("calc.rb", RUBY_BUGGY),
        ("spec/calc_spec.rb", RUBY_EXISTING),
        (".rspec", "--require spec_helper\n"),
        ("spec/spec_helper.rb", "require_relative \"../calc\"\n"),
    ]);
    project.commit_base("buggy base");

    project.write("calc.rb", RUBY_FIXED);
    project.write(
        "spec/regression_spec.rb",
        "RSpec.describe \"Calc\" do\n  it \"adds two numbers\" do\n    expect(Calc.add(2, 2)).to eq(4)\n  end\nend\n",
    );

    let receipt = project.verify(&rspec_config());
    assert_red_green_proven(&receipt, "rspec");
}

/// The bug this session found: the Ruby summary matcher required the plural
/// `" examples,"`, so a real one-example red suite printed as
/// `1 example, 1 failure` and classified as an unrecognized command failure.
///
/// This reproduces that shape end to end. `Calc.add` is buggy at the base, so
/// the single existing example must not touch it and the control passes while
/// running **one** example. The transplanted test then exercises the defect and
/// makes the suite red with exactly **one** failing example — the precise output
/// the old matcher skipped. Reaching a proof here is the assertion that the fix
/// holds.
#[test]
#[ignore = "end-to-end: spawns a real RSpec; run with -- --ignored"]
fn a_single_example_red_suite_can_be_attributed() {
    if !program_available("rspec", &["--version"]) {
        return;
    }
    let project = Project::new(&[
        ("calc.rb", RUBY_BUGGY),
        (
            "spec/calc_spec.rb",
            // `b == 0` is the only shape that survives `a - b` at the base, so
            // this example is green at base and the control passes.
            "RSpec.describe \"Calc\" do\n  it \"adds zero and one\" do\n    expect(Calc.add(1, 0)).to eq(1)\n  end\nend\n",
        ),
        (".rspec", "--require spec_helper\n"),
        ("spec/spec_helper.rb", "require_relative \"../calc\"\n"),
    ]);
    project.commit_base("base with a single green example");

    project.write("calc.rb", RUBY_FIXED);
    project.write(
        "spec/regression_spec.rb",
        "RSpec.describe \"Calc\" do\n  it \"adds two and two\" do\n    expect(Calc.add(2, 2)).to eq(4)\n  end\nend\n",
    );

    let receipt = project.verify(&rspec_config());
    assert_red_green_proven(&receipt, "rspec");
}

// ---------------------------------------------------------------------------
// JavaScript / Jest-compatible
// ---------------------------------------------------------------------------

fn javascript_config() -> Config {
    config_for(
        "javascript",
        &["node", "run_tests.js"],
        // The same globs `init` writes for a JavaScript project.
        &["**/*.test.js", "**/*.spec.js", "tests/**/*.js"],
        &[],
    )
}

const JS_BUGGY: &str = "function add(a, b) { return a - b; }\nmodule.exports = { add };\n";
const JS_FIXED: &str = "function add(a, b) { return a + b; }\nmodule.exports = { add };\n";

/// A minimal runner with a Jest-shaped output summary, so the JavaScript
/// classifier is exercised against the structure it keys on without requiring
/// a `node_modules` install.
const JS_RUNNER: &str = r#"const fs = require("fs");
const assert = require("assert");
const { add } = require("./calc");
let passed = 0;
let failed = 0;
function test(name, fn) {
  try { fn(); passed++; }
  catch (error) {
    failed++;
    console.log("  \u00d7 " + name);
    console.log("    " + error.message);
  }
}
for (const file of ["existing", "regression"]) {
  const path = "./tests/" + file + ".test.js";
  // The regression spec does not exist at the base revision; a runner that
  // required it unconditionally would fail the pristine base control, which is
  // a fixture artifact rather than a property of the product.
  if (fs.existsSync(path)) require(path)(test);
}
console.log("");
console.log("Tests:  " + (passed + failed) + ", " + failed + " failed, " + passed + " passed, " + (passed + failed) + " total");
process.exit(failed > 0 ? 1 : 0);
"#;

/// Copy a parser into the fixture's own `node_modules`.
///
/// The JavaScript analyzer requires its parser from the project under
/// verification (ADR-0021) and never bundles one, so a fixture without a
/// `node_modules` is reported as unanalyzed. A real Jest project always has one,
/// so the fixture provides one — the same shape as the real thing rather than a
/// stub.
fn install_acorn(root: &Path) -> bool {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|repo| repo.join("node_modules").join("acorn"));
    let Some(source) = source else {
        return false;
    };
    if !source.is_dir() {
        return false;
    }
    let target = root.join("node_modules").join("acorn");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("create node_modules");
    }
    // A symlink is enough: `require.resolve` follows it, and copying a whole
    // package per fixture would dominate the runtime.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &target).expect("link acorn");
    #[cfg(not(unix))]
    fs::create_dir_all(&target).expect("copy acorn");
    true
}

#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn javascript_reaches_verified_end_to_end() {
    if !program_available("node", &["--version"]) {
        eprintln!("node absent; skipping");
        return;
    }
    let project = Project::new(&[
        ("calc.js", JS_BUGGY),
        (
            "tests/existing.test.js",
            "const assert = require(\"assert\");\nmodule.exports = (test) => { test(\"arithmetic\", () => assert.strictEqual(1 + 1, 2)); };\n",
        ),
        ("run_tests.js", JS_RUNNER),
    ]);
    project.commit_base("buggy base");

    // The parser must exist at the base revision too, or the base experiment
    // analyzes nothing. A real project commits its `node_modules` contents via
    // its lockfile; here the link is made before the base commit and ignored.
    assert!(
        install_acorn(&project.root),
        "acorn must be available; run `npm install` in the repository"
    );

    project.write("calc.js", JS_FIXED);
    project.write(
        "tests/regression.test.js",
        "const assert = require(\"assert\");\nconst { add } = require(\"../calc\");\nmodule.exports = (test) => { test(\"adds\", () => assert.strictEqual(add(2, 2), 4)); };\n",
    );

    let receipt = project.verify(&javascript_config());
    assert_red_green_proven(&receipt, "javascript");
}
