//! End-to-end red/green verification tests (backlog PG-001).
//!
//! These tests exercise `verify_repository` against real temporary Git
//! repositories containing real Rust crates, because the v0.1 invariant is
//! about observable process behavior, not about an internal function's return
//! value:
//!
//! > A newly changed dedicated regression test is credible only when the
//! > configured test command passes on the current workspace and fails for a
//! > recognized test reason when the test-only change is transplanted onto the
//! > base revision.
//!
//! A fake test command cannot validate this. The tests below therefore run the
//! real `cargo` binary. They are ignored by default so that a plain
//! `cargo test` stays offline-capable and fast; run them with:
//!
//! ```text
//! cargo test --workspace --all-features -- --ignored
//! ```
//!
//! Design note on fixture isolation: every fixture writes its own
//! `[build] target-dir` into a checked-in `.cargo/config.toml`. The base
//! revision and the developer workspace are then two distinct directories that
//! still share one target directory, which mirrors a developer's real
//! incremental cache instead of forcing a full rebuild per revision. Without
//! this, the base worktree would fall back to `~/.cargo`'s shared registry but
//! a private target dir, and the two runs would each pay full compile cost.

use std::{fs, path::Path, process::Command as StdCommand};

use tempfile::TempDir;
use witdiff_core::{
    verify::verify_repository, Config, GitRepo, VerificationStatus, VerifyOptions, WorktreeGuard,
};

/// Absolute path to the `cargo` binary running the test harness.
const CARGO: &str = env!("CARGO");

// ---------------------------------------------------------------------------
// Fixture harness
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: TempDir,
    root: std::path::PathBuf,
}

impl Fixture {
    /// Create an initialized Git repository with the given initial files.
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = TempDir::new().expect("temporary directory");
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).expect("fixture root");

        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "witdiff@example.invalid"]);
        git(&root, &["config", "user.name", "WitDiff Test"]);
        // Make the default branch deterministic across Git versions.
        git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);

        // A dedicated target directory keeps fixtures from contending with each
        // other and lets the base worktree reuse the workspace build cache.
        //
        // The target directory MUST be git-ignored and `Cargo.lock` MUST be
        // committed, otherwise the very first build mutates the workspace
        // fingerprint and WitDiff correctly reports stale evidence. That is
        // not a false positive: `workspace_fingerprint` deliberately includes
        // `git status` and untracked files so that build byproducts are visible.
        // A real repository must resolve this the same way, by versioning its
        // lockfile and ignoring `target/`.
        let target_dir = root.join("target");
        let cargo_dir = root.join(".cargo");
        fs::create_dir_all(&cargo_dir).expect("cargo dir");
        fs::write(
            cargo_dir.join("config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", target_dir.display()),
        )
        .expect("place cargo config");
        fs::write(root.join(".gitignore"), "target/\n").expect("gitignore");

        // Dependency-free crate. `edition = "2021"` keeps the fixture buildable
        // by the minimum supported toolchain.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"witdiff_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
        )
        .expect("Cargo.toml");

        for (relative, contents) in files {
            write(&root, relative, contents);
        }

        Self { _tmp: tmp, root }
    }

    fn write(&self, relative: &str, contents: &str) {
        write(&self.root, relative, contents);
    }

    /// Commit the current working tree as the base revision.
    ///
    /// `Cargo.lock` is committed here (not regenerated later) so that the
    /// workspace fingerprint is already in its steady state before the first
    /// build. See the note in `Fixture::new`.
    fn commit_base(&self, message: &str) {
        git(&self.root, &["add", "-A"]);
        // Force-add the lockfile: the fixture ignores build output, not the lock.
        if self.root.join("Cargo.lock").exists() {
            git(&self.root, &["add", "-f", "Cargo.lock"]);
        }
        git(&self.root, &["commit", "-q", "-m", message]);
    }

    /// Resolve the fixture's dependency graph once, then commit the resulting
    /// lockfile so later builds cannot change the fingerprint.
    fn warm_lockfile(&self) {
        let output = StdCommand::new(CARGO)
            .arg("generate-lockfile")
            .current_dir(&self.root)
            .output()
            .expect("cargo generate-lockfile should run");
        assert!(
            output.status.success(),
            "cargo generate-lockfile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo(&self) -> GitRepo {
        GitRepo::discover(&self.root).expect("discover fixture repository")
    }

    /// Verify against the committed base revision, using a lower running-cost
    /// command than the workspace default so fixtures stay quick.
    fn verify(&self, config: &Config) -> witdiff_core::Receipt {
        verify_repository(&self.repo(), config, VerifyOptions::default())
            .expect("verification should complete")
    }

    /// Registered worktrees, used to assert cleanup actually happened.
    fn worktree_list(&self) -> String {
        String::from_utf8(
            StdCommand::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["worktree", "list", "--porcelain"])
                .output()
                .expect("git worktree list")
                .stdout,
        )
        .expect("utf8")
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

fn git_allow_failure(repo: &Path, args: &[&str]) -> bool {
    StdCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run")
        .status
        .success()
}

/// Config that runs the fixture suite in one test thread.
///
/// `--test-threads=1` keeps cargo's output shape stable and avoids interleaved
/// harness output, so failure classification and integrity findings are
/// deterministic across machines.
fn fixture_config() -> Config {
    let mut config = Config::default();
    config.verification.test_command = vec![
        CARGO.to_owned(),
        "test".to_owned(),
        "--quiet".to_owned(),
        "--".to_owned(),
        "--test-threads=1".to_owned(),
    ];
    config.verification.test_globs = vec!["tests/*.rs".to_owned(), "tests/**/*.rs".to_owned()];
    // Keep every fixture run bounded and deterministic.
    config.verification.max_output_bytes = 32_768;
    // Fixtures build from scratch, so keep the deadline generous but finite.
    config.verification.timeout_secs = Some(600);
    config
}

/// Baseline crate contents: `is_even` is buggy, and one unrelated test exists.
fn buggy_lib() -> &'static str {
    "pub fn is_even(value: i32) -> bool {\n    value % 2 == 1\n}\n"
}

fn fixed_lib() -> &'static str {
    "pub fn is_even(value: i32) -> bool {\n    value % 2 == 0\n}\n"
}

const UNRELATED_TEST: &str =
    "#[test]\nfn arithmetic_still_works() {\n    assert_eq!(2 + 2, 4);\n}\n";

/// A regression test that fails against `buggy_lib()` and passes against `fixed_lib()`.
const CREDIBLE_REGRESSION_TEST: &str = "\
#[test]
fn even_numbers_are_reported_even() {
    assert!(witdiff_fixture::is_even(2));
    assert!(!witdiff_fixture::is_even(3));
}
";

/// A test that passes on both revisions and therefore proves nothing.
const VACUOUS_REGRESSION_TEST: &str = "\
#[test]
fn arithmetic_is_unchanged() {
    assert_eq!(1 + 1, 2);
}
";

// ---------------------------------------------------------------------------
// PG-001 acceptance criteria
// ---------------------------------------------------------------------------

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn buggy_base_with_fixed_workspace_and_new_test_is_verified() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    // Fix the defect, then add a dedicated regression test.
    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let receipt = fixture.verify(&fixture_config());

    assert_eq!(
        receipt.status,
        VerificationStatus::Verified,
        "expected Verified, got {:?}; notes={:?}",
        receipt.status,
        receipt.notes
    );
    assert!(
        receipt.red_green_proven,
        "red/green must be recorded as proven"
    );

    // HEAD is green.
    assert!(receipt.head_run.success, "HEAD run must pass");

    // The pristine base control passes, which is what licenses attribution.
    let control = receipt
        .base_control_run
        .as_ref()
        .expect("base control must have run");
    assert!(control.success, "pristine base must pass the control run");

    // The base + transplanted test run fails for a test reason.
    let base_run = receipt.base_run.as_ref().expect("base experiment must run");
    assert!(!base_run.success, "base + changed test must fail");
    assert_eq!(
        base_run.failure_kind,
        Some(witdiff_core::FailureKind::TestFailure),
        "failure must be classified as a behavioral test failure, not a compile error"
    );

    assert!(receipt.evidence_fresh, "workspace must not have shifted");
    assert!(
        receipt.integrity_findings.is_empty(),
        "no integrity findings expected: {:?}",
        receipt.integrity_findings
    );
    assert_eq!(receipt.schema_version, "witdiff.receipt.v1");

    // Cleanup really happened.
    let worktrees = fixture.worktree_list();
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1,
        "only the primary worktree should remain, got:\n{worktrees}"
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn test_that_also_passes_on_base_is_not_verified() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    // Passes on both revisions: it does not demonstrate the behavioral change.
    fixture.write("tests/regression.rs", VACUOUS_REGRESSION_TEST);

    let receipt = fixture.verify(&fixture_config());

    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "a test that passes on base proves nothing; notes={:?}",
        receipt.notes
    );
    assert!(!receipt.red_green_proven, "red/green must not be claimed");

    let base_run = receipt.base_run.as_ref().expect("base experiment must run");
    assert!(base_run.success, "the transplanted test passes on base");
    assert!(
        receipt
            .notes
            .iter()
            .any(|n| n.contains("also pass on the base revision")),
        "expected an explanatory note, got {:?}",
        receipt.notes
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn failing_pristine_base_is_not_verified() {
    let fixture = Fixture::new(&[
        (
            "src/lib.rs",
            "pub fn is_even(value: i32) -> bool {\n    value % 2 == 1\n}\n",
        ),
        (
            "tests/existing.rs",
            "#[test]\nfn broken_on_base() {\n    assert_eq!(1, 2);\n}\n",
        ),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base with a failing suite");

    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    // The control fails, but HEAD must still be green for the control to be the
    // binding reason. A `HeadFailed` status would short-circuit before the base
    // experiment ever ran, which would not test the control logic at all.
    fixture.write("tests/existing.rs", UNRELATED_TEST);
    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let receipt = fixture.verify(&fixture_config());

    // A non-passing control means a later failure cannot be attributed to the
    // changed tests. WitDiff must refuse to claim verification.
    assert!(
        receipt.head_run.success,
        "HEAD must be green so the failing control is the binding constraint; notes={:?}",
        receipt.notes
    );
    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "broken base must not yield Verified; notes={:?}",
        receipt.notes
    );
    assert!(!receipt.red_green_proven);
    let control = receipt
        .base_control_run
        .as_ref()
        .expect("control must be recorded even when it fails");
    assert!(
        !control.success,
        "control is expected to fail in this fixture"
    );
    // The experiment must not have been attempted without a passing control.
    assert!(
        receipt.base_run.is_none(),
        "base+test run must be skipped when the control fails"
    );
    assert!(
        receipt
            .notes
            .iter()
            .any(|n| n.contains("does not pass the configured test command")),
        "expected a conservative explanatory note, got {:?}",
        receipt.notes
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn test_requiring_new_api_reports_base_incompatible() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    // Add a genuinely new API plus a test that depends on it.
    fixture.write(
        "src/lib.rs",
        "pub fn is_even(value: i32) -> bool {\n    value % 2 == 0\n}\n\npub fn parity_label(value: i32) -> &'static str {\n    if is_even(value) { \"even\" } else { \"odd\" }\n}\n",
    );
    fixture.write(
        "tests/regression.rs",
        "#[test]\nfn labels_even_numbers() {\n    assert_eq!(witdiff_fixture::parity_label(2), \"even\");\n}\n",
    );

    let receipt = fixture.verify(&fixture_config());

    assert_eq!(
        receipt.status,
        VerificationStatus::BaseIncompatible,
        "a test depending on a new API cannot compile against base; notes={:?}",
        receipt.notes
    );
    // Compile failure must never be upgraded to behavioral proof.
    assert!(
        !receipt.red_green_proven,
        "compile failure is not red/green proof"
    );
    let base_run = receipt.base_run.as_ref().expect("base experiment must run");
    assert!(!base_run.success);
    assert_eq!(
        base_run.failure_kind,
        Some(witdiff_core::FailureKind::CompileError),
        "failure must be classified as a compile error"
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn high_severity_integrity_finding_blocks_verified() {
    // Two things must hold simultaneously for this test to exercise the
    // integrity gate rather than some earlier conservative branch:
    //
    //   1. the pristine base control must PASS, and
    //   2. the transplant must FAIL for a test reason (red/green observed).
    //
    // A test that is merely `#[ignore]`d satisfies neither, because a skipped
    // test passes. So the fixture keeps the base test passing on the buggy code,
    // and the workspace edit both strengthens the assertions and adds a second
    // ignored test. The transplant therefore fails on base while an ignored_test
    // finding fires, which is precisely the branch under test.
    let base_regression = "\
#[test]
fn even_numbers_are_reported_even() {
    assert!(!witdiff_fixture::is_even(2));
}
";
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
        ("tests/regression.rs", base_regression),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    fixture.write(
        "tests/regression.rs",
        "\
#[test]
fn even_numbers_are_reported_even() {
    assert!(witdiff_fixture::is_even(2));
    assert!(!witdiff_fixture::is_even(3));
}

#[test]
#[ignore]
fn even_numbers_are_reported_even_for_large_values() {
    assert!(witdiff_fixture::is_even(1_000_002));
}
",
    );

    let config = fixture_config();
    assert!(
        config.verification.block_on_integrity_findings,
        "fixture relies on the default blocking policy"
    );

    let receipt = fixture.verify(&config);

    let ignored_finding = receipt
        .integrity_findings
        .iter()
        .find(|f| f.rule == "ignored_test")
        .expect("ignoring the changed test must be detected");
    assert_eq!(
        ignored_finding.severity,
        witdiff_core::Severity::High,
        "ignoring a regression test is a high-severity weakening"
    );

    // Red/green was genuinely observed: the strengthened assertions fail against
    // the buggy base. The integrity finding is what withholds the VERIFIED claim,
    // so the observation must still be recorded for the reviewer.
    assert!(
        receipt.red_green_proven,
        "the behavioral red/green observation must still be recorded"
    );
    let base_run = receipt.base_run.as_ref().expect("base experiment must run");
    assert!(
        !base_run.success,
        "the strengthened assertions must fail against the buggy base"
    );
    assert_eq!(
        base_run.failure_kind,
        Some(witdiff_core::FailureKind::TestFailure),
        "the transplant failure must be behavioral, not a compile error"
    );

    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "a high-severity integrity finding must block Verified; notes={:?}",
        receipt.notes
    );
    assert!(
        receipt
            .notes
            .iter()
            .any(|n| n.contains("integrity findings block verification")),
        "expected a blocking note, got {:?}",
        receipt.notes
    );

    assert!(
        receipt.status != VerificationStatus::Verified,
        "an ignored regression test must never be certified"
    );
}

// ---------------------------------------------------------------------------
// PG-002 acceptance: cleanup on failure paths
// ---------------------------------------------------------------------------

/// PG-002: a failed transplant must not leak a worktree registration.
///
/// This exercises the guard directly rather than through `verify_repository`.
/// WitDiff always applies the test patch to a *pristine* base worktree, and
/// `git apply` cannot fail there for a genuine modification hunk: Git re-anchors
/// hunks by context and only rejects a patch when the target file has already
/// diverged. Verified empirically — the same patch that fails on a dirty
/// worktree applies cleanly on a fresh one. So the reachable failure paths are
/// the ones asserted here: an explicit `apply_patch` error, and the early `?`
/// returns that follow it.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn patch_apply_failure_does_not_leak_worktree() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
        ("tests/regression.rs", "// placeholder committed at base\n"),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    // The workspace must actually differ from base, otherwise the transplant
    // patch is empty and there is nothing to fail applying.
    fixture.write("tests/regression.rs", "// modified in the workspace\n");

    let repo = fixture.repo();
    let tmp = TempDir::new().expect("temp");
    let worktree = tmp.path().join("base");

    // A guard is live, then a transplant fails part-way through — exactly the
    // shape of the real `verify_repository` body.
    let result = (|| -> witdiff_core::Result<()> {
        let _guard = WorktreeGuard::create(&repo, &worktree, "main")?;
        // Make the worktree diverge, then try to apply a patch computed against
        // the pristine base. Git must reject it.
        fs::write(worktree.join("tests/regression.rs"), "// diverged\n").expect("diverging write");
        let patch = repo.diff_for_paths("main", &["tests/regression.rs".to_owned()], 3)?;
        // An empty patch means the fixture did not produce a modification hunk;
        // assert loudly rather than passing vacuously.
        assert!(
            !patch.is_empty(),
            "fixture must produce a non-empty transplant patch"
        );
        repo.apply_patch(&worktree, &patch)?;
        Ok(())
    })();

    assert!(
        result.is_err(),
        "a transplant onto a diverged worktree must fail"
    );
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("transplant"),
        "error should name the transplant failure, got: {message}"
    );

    let worktrees = fixture.worktree_list();
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1,
        "the guard must remove the worktree when the transplant fails, got:\n{worktrees}"
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn missing_test_program_does_not_leak_worktree() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let mut config = fixture_config();
    // A program that cannot be spawned: `run` returns Err from the HEAD run, so
    // no worktree is created yet -- then verify against a config whose HEAD
    // passes but whose base run cannot spawn.
    config.verification.test_command = vec!["witdiff-nonexistent-program".to_owned()];

    let repo = fixture.repo();
    let result = verify_repository(&repo, &config, VerifyOptions::default());
    assert!(
        result.is_err(),
        "a missing test program must be an explicit error"
    );

    let worktrees = fixture.worktree_list();
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1,
        "no worktree may leak when the runner cannot spawn, got:\n{worktrees}"
    );
}

#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn keep_worktree_retains_the_base_tree_under_the_temp_directory() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let receipt = verify_repository(
        &fixture.repo(),
        &fixture_config(),
        VerifyOptions {
            keep_worktree: true,
            ..VerifyOptions::default()
        },
    )
    .expect("verification should complete");

    assert_eq!(receipt.status, VerificationStatus::Verified);
    assert!(
        receipt
            .notes
            .iter()
            .any(|n| n.contains("base worktree retained at")),
        "retention must be reported with its exact path, got {:?}",
        receipt.notes
    );
}

// ---------------------------------------------------------------------------
// Guard-level unit coverage (no cargo build required)
// ---------------------------------------------------------------------------

#[test]
fn worktree_guard_removes_registration_on_drop() {
    let fixture = Fixture::new(&[("src/lib.rs", buggy_lib())]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    let repo = fixture.repo();
    let tmp = TempDir::new().expect("temp");
    let path = tmp.path().join("base");

    {
        let _guard = WorktreeGuard::create(&repo, &path, "HEAD").expect("worktree creation");
        assert!(
            path.join("src/lib.rs").is_file(),
            "worktree should be checked out"
        );
    }

    let worktrees = fixture.worktree_list();
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1,
        "drop must unregister the worktree, got:\n{worktrees}"
    );
}

#[test]
fn worktree_guard_retain_leaves_the_registration_in_place() {
    let fixture = Fixture::new(&[("src/lib.rs", buggy_lib())]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    let repo = fixture.repo();
    let tmp = TempDir::new().expect("temp");
    let path = tmp.path().join("base");

    let guard = WorktreeGuard::create(&repo, &path, "HEAD").expect("worktree creation");
    let retained = guard.retain();
    assert_eq!(retained, path);

    let worktrees = fixture.worktree_list();
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        2,
        "retain must keep the worktree registered, got:\n{worktrees}"
    );
    assert!(path.is_dir(), "retained worktree must still exist on disk");

    // Deliberate teardown so the TempDir can be cleaned up.
    assert!(
        git_allow_failure(
            repo.root(),
            &[
                "worktree",
                "remove",
                "--force",
                path.to_str().expect("utf8 path")
            ]
        ),
        "deliberate teardown of the retained worktree should succeed"
    );
}

// ---------------------------------------------------------------------------
// Bounded execution: a hung suite must never be certified
// ---------------------------------------------------------------------------

/// A suite that never terminates must be bounded and must never be verified.
///
/// This drives `verify_repository` with a deliberately hanging test command, so
/// it needs no Rust fixture build and stays fast and deterministic. Without a
/// deadline the very first `run` call would block forever and WitDiff would
/// never produce a receipt at all.
#[test]
fn hung_test_command_is_bounded_and_never_verified() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let mut config = fixture_config();
    // `sleep` stands in for a suite blocked on a deadlock or on I/O. Nothing is
    // compiled, so the deadline is the only thing under test.
    config.verification.test_command = vec!["sleep".to_owned(), "3600".to_owned()];
    config.verification.timeout_secs = Some(1);

    let started = std::time::Instant::now();
    let receipt = fixture.verify(&config);
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(60),
        "the deadline must bound verification, took {elapsed:?}"
    );
    assert!(
        receipt.head_run.timed_out,
        "the hung head run must be reported as timed out"
    );
    assert!(
        !receipt.head_run.success,
        "a timed-out run is never a success"
    );
    assert_eq!(
        receipt.head_run.failure_kind,
        Some(witdiff_core::FailureKind::Timeout),
        "a timeout must be its own failure kind, never a test failure"
    );

    // A suite that hangs has demonstrated nothing, so no proof may be claimed.
    assert_eq!(
        receipt.status,
        VerificationStatus::HeadFailed,
        "a hung head run cannot be verified; notes={:?}",
        receipt.notes
    );
    assert!(!receipt.red_green_proven);
    assert!(
        receipt
            .notes
            .iter()
            .any(|n| n.contains("exceeded the configured timeout")),
        "expected an explanatory timeout note, got {:?}",
        receipt.notes
    );
    assert!(receipt.evidence_fresh);

    // The receipt must still be serializable and carry the flag.
    let encoded = serde_json::to_string(&receipt).expect("receipt must serialize");
    assert!(
        encoded.contains("\"timeout\""),
        "receipt must carry the timeout kind"
    );
    assert!(encoded.contains("\"timed_out\":true"));
}

// ---------------------------------------------------------------------------
// M1 hardening: transplant must stay test-only
// ---------------------------------------------------------------------------

/// Renaming production code into `tests/` must not smuggle the production diff
/// into a "test-only" transplant.
///
/// If the old production path were included in the transplant pathspec, the
/// base worktree would receive the production change too, the failure on base
/// would no longer be attributable to the test, and WitDiff would report a
/// proof it never performed.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn rename_from_production_into_tests_does_not_smuggle_production_code() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", buggy_lib()),
        // A production helper module that the library does not import. Moving
        // it into `tests/` makes its new path match the `tests/*.rs` glob while
        // leaving the crate buildable, which is exactly the shape that could
        // smuggle a production diff into a test-only transplant.
        ("src/helper.rs", "pub fn offset() -> i32 {\n    0\n}\n"),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("buggy base");

    fixture.write("src/lib.rs", fixed_lib());
    git(&fixture.root, &["mv", "src/helper.rs", "tests/helper.rs"]);

    let mut config = fixture_config();
    config.verification.targeted_test_selection = false;

    let receipt = verify_repository(&fixture.repo(), &config, VerifyOptions::default())
        .expect("verification should complete");

    let moved = receipt
        .changed_files
        .iter()
        .find(|file| file.path == "tests/helper.rs")
        .expect("the renamed file should be reported");
    assert!(
        !moved.previous_is_test,
        "a rename sourced from src/ must not be treated as test-to-test"
    );

    assert!(
        receipt
            .notes
            .iter()
            .any(|note| note.contains("tests/helper.rs") && note.contains("not transplanted")),
        "the receipt must explain that the file was excluded, got {:?}",
        receipt.notes
    );

    // Excluding a changed test means the red/green proof is partial, so it can
    // never be reported as complete.
    assert!(
        !matches!(
            receipt.status,
            VerificationStatus::Verified | VerificationStatus::VerifiedWithWarnings
        ),
        "a partial transplant must not yield a verified status, got {:?}",
        receipt.status
    );
}

// ---------------------------------------------------------------------------
// M2: syntax-aware integrity analysis
// ---------------------------------------------------------------------------

/// An assertion that is merely reformatted must not be reported as removed.
///
/// This is the case the line-oriented analyzer could not handle: every line of
/// the assertion changes in the diff, but the assertion itself is unchanged.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn reformatting_a_test_is_not_an_integrity_finding() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", fixed_lib()),
        (
            "tests/existing.rs",
            "#[test]\nfn even_numbers_are_reported_even() {\n    assert_eq!(witdiff_fixture::is_even(2), true);\n}\n",
        ),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // Reformat the assertion across several lines without changing meaning.
    fixture.write(
        "tests/existing.rs",
        "#[test]\nfn even_numbers_are_reported_even() {\n    assert_eq!(\n        witdiff_fixture::is_even(2),\n        true,\n    );\n}\n",
    );

    let receipt = fixture.verify(&fixture_config());

    assert!(
        !receipt
            .integrity_findings
            .iter()
            .any(|finding| finding.rule == "removed_assertion"),
        "reformatting must not register as a removed assertion, got {:?}",
        receipt.integrity_findings
    );
}

/// Changing only the expected value is detected through the syntax-aware path.
///
/// The subject expression stays identical, so the line-oriented analyzer sees
/// one changed line with no signal about what changed. The structural analyzer
/// can say precisely that the expectation moved from `true` to `false`.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn changed_expected_value_is_reported_against_a_real_repository() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", fixed_lib()),
        (
            "tests/existing.rs",
            "#[test]\nfn even_numbers_are_reported_even() {\n    assert_eq!(witdiff_fixture::is_even(2), true);\n}\n",
        ),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // Invert the expectation. The test still passes, but it now asserts the
    // opposite of what it checked before.
    fixture.write(
        "tests/existing.rs",
        "#[test]\nfn even_numbers_are_reported_even() {\n    assert_eq!(witdiff_fixture::is_even(2), false);\n}\n",
    );

    let receipt = fixture.verify(&fixture_config());

    let changed = receipt
        .integrity_findings
        .iter()
        .find(|finding| finding.rule == "changed_expected_value")
        .unwrap_or_else(|| {
            panic!(
                "a changed expected value must be reported, got {:?}",
                receipt.integrity_findings
            )
        });
    assert_eq!(changed.severity.as_str(), "high");
    assert!(
        changed.message.contains("true") && changed.message.contains("false"),
        "the finding should name both expectations, got {}",
        changed.message
    );
}
