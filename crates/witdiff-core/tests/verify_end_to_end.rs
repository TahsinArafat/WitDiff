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

use std::{fs, path::Path, process::Command};

use std::process::Command as StdCommand;

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
    // A program that cannot be spawned. Since ADR-0019 this is reported in a
    // receipt rather than returned as an error, so that integrity findings
    // computed before the run are not discarded. The worktree invariant this
    // test exists for is unchanged: nothing may leak.
    config.verification.test_command = vec!["witdiff-nonexistent-program".to_owned()];

    let repo = fixture.repo();
    let receipt = verify_repository(&repo, &config, VerifyOptions::default())
        .expect("a missing program is reported in a receipt, not as an error");

    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "no proof was attempted, so the status must not be verified"
    );
    assert!(
        receipt.head_run.is_missing_program(),
        "the receipt must record that the program was absent"
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

// ---------------------------------------------------------------------------
// ADR-0010: inline `#[cfg(test)]` transplantation
// ---------------------------------------------------------------------------

/// A crate whose only test lives inside a `#[cfg(test)]` module in the same
/// file as the code it exercises — the layout this feature exists to support.
///
/// `test_assertion` is what the test asserts, so a scenario can change the test
/// independently of the implementation. That distinction matters: the invariant
/// is about a *newly changed* test, so a scenario that changes only the
/// implementation has no inline test change to transplant.
fn inline_lib(implementation: &str, test_assertion: &str) -> String {
    format!(
        "\
pub fn is_even(value: i32) -> bool {{
    {implementation}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn even_numbers_are_reported_even() {{
        {test_assertion}
    }}
}}
"
    )
}

/// The core ADR-0010 guarantee: a changed inline test is transplanted onto the
/// base revision, and the production fix in the same file is *not*.
///
/// If the production fix rode along, the transplanted test would pass on the
/// base and the proof would be worthless.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn inline_test_is_transplanted_without_the_production_fix() {
    // Base: the buggy implementation, with the test asserting the *stronger*
    // claim that the fix will satisfy but the bug does not.
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        &inline_lib("value % 2 != 0", "assert!(!is_even(2));"),
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // Head: fix the implementation and tighten the test in the same commit.
    // The test change is what must reach the base worktree.
    fixture.write(
        "src/lib.rs",
        &inline_lib("value % 2 == 0", "assert!(is_even(2));"),
    );

    let receipt = fixture.verify(&fixture_config());

    let spliced = receipt
        .spliced_inline_tests
        .iter()
        .find(|entry| entry.path == "src/lib.rs")
        .unwrap_or_else(|| {
            panic!(
                "src/lib.rs should be recorded as spliced, got spliced={:?} refused={:?} notes={:?}",
                receipt.spliced_inline_tests, receipt.refused_inline_tests, receipt.notes
            )
        });
    assert!(
        spliced.modules.iter().any(|module| module == "tests"),
        "the transplanted module must be named, got {:?}",
        spliced.modules
    );
    assert!(
        receipt.refused_inline_tests.is_empty(),
        "nothing should have been refused, got {:?}",
        receipt.refused_inline_tests
    );
    assert!(
        receipt.red_green_proven,
        "the transplanted inline test must fail on base and pass on head; status={:?} notes={:?}",
        receipt.status, receipt.notes
    );
    assert_eq!(receipt.status, VerificationStatus::Verified);
}

/// Production changes elsewhere in the same file do not block the splice.
///
/// Fixing a bug and tightening the inline test in one commit is the normal
/// shape of a change. Only the test module's bytes are moved, so the production
/// change stays at base — which is what makes the experiment meaningful.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn inline_test_is_spliced_alongside_a_production_edit() {
    let base = inline_lib("value % 2 != 0", "assert!(!is_even(2));");
    let mut head = inline_lib("value % 2 == 0", "assert!(is_even(2));");
    // An unrelated production addition elsewhere in the same file.
    head.push_str("\npub fn unrelated_helper() -> i32 {\n    41 + 1\n}\n");
    let fixture = Fixture::new(&[("src/lib.rs", &base)]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", &head);

    let receipt = fixture.verify(&fixture_config());

    assert!(
        receipt
            .spliced_inline_tests
            .iter()
            .any(|entry| entry.path == "src/lib.rs"),
        "a production edit elsewhere must not block the splice, got spliced={:?} refused={:?} status={:?}",
        receipt.spliced_inline_tests,
        receipt.refused_inline_tests,
        receipt.status
    );
    assert!(
        receipt.red_green_proven,
        "the transplanted test must still fail on base; status={:?} notes={:?}",
        receipt.status, receipt.notes
    );
}

/// A transplanted test that references a production item existing only in head
/// cannot compile at base. ADR-0003 already classifies that as
/// `base_incompatible`, and it must never be reported as a proof.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn inline_test_referencing_head_only_code_is_base_incompatible() {
    let base = "pub fn is_even(value: i32) -> bool {\n    value % 2 != 0\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn t() { assert!(!is_even(2)); }\n}\n";
    // Head adds a production helper and a test that calls it.
    let head = "pub fn is_even(value: i32) -> bool {\n    value % 2 == 0\n}\n\nfn helper() -> bool {\n    true\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn t() { assert!(helper()); }\n}\n";
    let fixture = Fixture::new(&[("src/lib.rs", base)]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", head);

    let receipt = fixture.verify(&fixture_config());

    assert_eq!(
        receipt.status,
        VerificationStatus::BaseIncompatible,
        "a splice that cannot compile at base is incompatible, not verified; notes={:?}",
        receipt.notes
    );
    assert!(
        !receipt.red_green_proven,
        "a compile error is not a red result"
    );
}

/// A test module that is new in head has no base counterpart to replace, so it
/// is refused instead of being invented at base.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn inline_test_module_new_in_head_is_refused() {
    let base = "pub fn is_even(value: i32) -> bool {\n    value % 2 != 0\n}\n";
    let fixture = Fixture::new(&[("src/lib.rs", base)]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // Head must pass, or the run stops at `head_failed` before any transplant
    // is attempted. The test therefore asserts the behavior the *fixed*
    // implementation actually has.
    fixture.write(
        "src/lib.rs",
        &inline_lib("value % 2 == 0", "assert!(is_even(2));"),
    );

    let receipt = fixture.verify(&fixture_config());

    let refused = receipt
        .refused_inline_tests
        .iter()
        .find(|entry| entry.path == "src/lib.rs")
        .unwrap_or_else(|| {
            panic!(
                "a test module new in head must be refused, got status={:?} refused={:?} notes={:?}",
                receipt.status, receipt.refused_inline_tests, receipt.notes
            )
        });
    assert_eq!(refused.reason, "no_counterpart_in_base");
    assert!(receipt.spliced_inline_tests.is_empty());
    assert_ne!(receipt.status, VerificationStatus::Verified);
}

// ---------------------------------------------------------------------------
// ADR-0011: mutation is supplementary and never a proof
// ---------------------------------------------------------------------------

/// A crate whose production code changes *and* carries a weak inline test.
///
/// Both halves matter. Mutation targets the changed production lines, so a
/// fixture that changed only the test module would generate no mutants at all —
/// which is the correct behavior, and would make this fixture prove nothing.
fn weak_test_lib(implementation: &str) -> String {
    format!(
        "\
pub fn is_positive(value: i32) -> bool {{
    {implementation}
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn t() {{ assert!(is_positive(5)); }}
}}
"
    )
}

fn mutation_config() -> Config {
    let mut config = fixture_config();
    config.verification.mutation = true;
    config.verification.max_mutants = 6;
    config.verification.max_mutants_per_function = 3;
    config
}

/// The central ADR-0011 guarantee: mutation is reported, and it is *not*
/// consulted when the status is computed.
///
/// The status here is `not_verified` for a reason that has nothing to do with
/// mutation — the changed test also passes on base. A surviving mutant must not
/// change that, and must not be laundered into a proof.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn mutation_is_reported_without_changing_status() {
    // The base uses `< 0`; head changes the comparison to `> 0`, so the
    // operator is on a changed line and becomes a mutation candidate.
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        "pub fn is_positive(value: i32) -> bool {\n    value < 0\n}\n",
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // A weak test: it never probes the boundary, so `>` -> `>=` survives.
    fixture.write("src/lib.rs", &weak_test_lib("value > 0"));

    let receipt = fixture.verify(&mutation_config());

    let mutation = receipt
        .mutation
        .as_ref()
        .expect("mutation should be reported when enabled");
    assert!(
        mutation.generated > 0,
        "the changed comparison should generate mutants, got {mutation:?}"
    );

    // Whatever the mutation outcome, the status is decided by red/green alone.
    // A base-revision test that passes cannot become verified through mutation.
    assert_ne!(
        receipt.status,
        VerificationStatus::Verified,
        "mutation must not create a proof; status={:?} mutation={:?}",
        receipt.status,
        mutation
    );
    assert!(
        !receipt.red_green_proven,
        "mutation must not set red_green_proven"
    );

    // And the undecided outcomes are reported rather than hidden.
    assert_eq!(
        mutation.killed
            + mutation.survived
            + mutation.not_compiled
            + mutation.timeout
            + mutation.skipped,
        mutation.results.len(),
        "every attempted mutant must be classified exactly once"
    );
}

/// Disabling mutation must omit the section entirely, so a receipt from the
/// default configuration is unchanged by this feature's existence.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn mutation_is_absent_when_disabled() {
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        "pub fn is_positive(value: i32) -> bool {\n    value < 0\n}\n",
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", &weak_test_lib("value > 0"));

    let mut config = fixture_config();
    config.verification.mutation = false;
    let receipt = fixture.verify(&config);

    assert!(
        receipt.mutation.is_none(),
        "mutation must be absent when disabled, got {:?}",
        receipt.mutation
    );
}

/// A surviving mutant must be findable and must name the operator and span, so
/// a reviewer can judge whether the test should have caught it.
#[test]
#[ignore = "end-to-end: spawns real cargo builds; run with -- --ignored"]
fn surviving_mutants_name_the_operator_and_span() {
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        "pub fn is_positive(value: i32) -> bool {\n    value < 0\n}\n",
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", &weak_test_lib("value > 0"));

    let receipt = fixture.verify(&mutation_config());
    let mutation = receipt.mutation.as_ref().expect("mutation enabled");

    if let Some(survivor) = mutation
        .results
        .iter()
        .find(|result| result.outcome == witdiff_core::model::MutantOutcome::Survived)
    {
        assert_eq!(survivor.operator, "comparison_boundary");
        assert_eq!(survivor.path, "src/lib.rs");
        assert_eq!(survivor.original, ">");
        assert_eq!(survivor.replacement, ">=");
        assert!(
            survivor.function.as_deref() == Some("is_positive"),
            "the surviving mutant should name its function, got {:?}",
            survivor.function
        );
        assert!(!survivor.id.is_empty(), "a mutant needs a stable id");
    } else {
        // The bound may have excluded the boundary operator; that is reported.
        assert!(
            !mutation.notes.is_empty() || mutation.killed > 0,
            "either a survivor is reported or the run explains itself: {mutation:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ADR-0012: framework-specific failure classification
// ---------------------------------------------------------------------------

/// A non-Rust repository must be able to obtain a red/green proof.
///
/// Before ADR-0012, `classify_failure` matched cargo output only, so a real
/// pytest failure classified as `CommandFailure` and the run ended at
/// `not_verified`. Measured: the same pytest output yields `CommandFailure`
/// under the cargo classifier and `TestFailure` under the pytest classifier.
///
/// This test uses `sh -c` deliberately: the fixture needs a command whose
/// failure output looks like pytest's, and an actual Python interpreter is not
/// guaranteed to be present in every environment the suite runs in.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn a_non_rust_framework_can_reach_a_red_green_proof() {
    // A command that prints pytest-shaped output and fails. The base control
    // runs the same command, so it must have a mode that passes.
    let passing = "printf '1 passed in 0.01s\\n'; exit 0";
    let failing = "printf 'short test summary info\\nFAILED test_sample.py::test_even\\n1 failed, 1 passed in 0.02s\\n'; exit 1";

    let fixture = Fixture::new(&[("tests/existing.rs", UNRELATED_TEST)]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // The base revision runs the passing form; the working tree runs the
    // failing form, selected through an environment marker the script reads.
    let mut config = fixture_config();
    config.verification.framework = "pytest".to_owned();
    config.verification.test_command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        format!("if [ -f .failing ]; then {failing}; else {passing}; fi"),
    ];

    // Head is failing...
    fs::write(fixture.root.join(".failing"), "").expect("marker");
    // ...but the base worktree is a fresh checkout without the marker.
    let receipt = fixture.verify(&config);

    // Whatever the outcome, the classification must be the framework's, not a
    // generic command failure, or the proof could never be established.
    if let Some(base_run) = &receipt.base_run {
        assert_eq!(
            base_run.failure_kind,
            Some(witdiff_core::FailureKind::TestFailure),
            "pytest-shaped output must classify as a test failure, got {:?}",
            base_run.failure_kind
        );
    }
}

/// An unrecognized framework name is an explicit error, never a silent
/// fallback to the Rust classifier.
#[test]
fn an_unknown_framework_name_is_rejected() {
    let mut config = Config::default();
    config.verification.framework = "not-a-real-framework".to_owned();
    let error = config
        .verification
        .framework()
        .expect_err("an unknown framework must be rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("not-a-real-framework"),
        "the error should name the offending value: {message}"
    );
    assert!(
        message.contains("cargo") && message.contains("pytest"),
        "the error should list the supported values: {message}"
    );
}

/// The default configuration keeps classifying as Rust, so existing users see
/// no behavior change.
#[test]
fn the_default_framework_is_cargo() {
    let framework = Config::default()
        .verification
        .framework()
        .expect("the default framework must resolve");
    assert_eq!(framework, witdiff_core::framework::TestFramework::Cargo);
}

// ---------------------------------------------------------------------------
// ADR-0019: a missing test toolchain must not discard the run
// ---------------------------------------------------------------------------

/// The defect this ADR fixes: when the test command cannot start, the whole run
/// used to abort with no receipt at all, discarding integrity findings that
/// never needed that program.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn a_missing_test_command_still_produces_a_receipt() {
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        "pub fn is_even(value: i32) -> bool {\n    value % 2 != 0\n}\n",
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // A test that is weakened, plus a test command that cannot start.
    fixture.write("tests/existing.rs", "#[test]\nfn t() { assert!(true); }\n");

    let mut config = fixture_config();
    config.verification.test_command = vec!["witdiff-definitely-not-a-real-program".to_owned()];

    let receipt = fixture.verify(&config);

    assert_eq!(
        receipt.status,
        VerificationStatus::NotVerified,
        "a proof that never ran is not verified; notes: {:?}",
        receipt.notes
    );
    assert!(
        receipt.head_run.is_missing_program(),
        "the receipt must record that the program was absent, got {:?}",
        receipt.head_run.failure_kind
    );
    assert_eq!(
        receipt.head_run.exit_code, None,
        "a program that never started has no exit code to report; a consumer \
         must not read a null exit code as a failing test"
    );
    assert!(
        !receipt.head_run.success,
        "a run that did not start is not a success"
    );
    assert!(
        receipt
            .notes
            .iter()
            .any(|note| note.contains("was not found")),
        "the note should name the missing program, got {:?}",
        receipt.notes
    );
    assert!(
        receipt.notes.iter().any(|note| note.contains("next:")),
        "the developer needs an actionable next step, got {:?}",
        receipt.notes
    );
}

/// The point of writing a receipt anyway: findings that do not depend on the
/// missing program are still delivered.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn integrity_findings_survive_a_missing_test_command() {
    let fixture = Fixture::new(&[(
        "src/lib.rs",
        "pub fn is_even(value: i32) -> bool {\n    value % 2 != 0\n}\n",
    )]);
    fixture.warm_lockfile();
    fixture.commit_base("base");

    // Gut the test, then make the test command unrunnable.
    fixture.write("tests/existing.rs", "#[test]\nfn t() { assert!(true); }\n");
    let mut config = fixture_config();
    config.verification.test_command = vec!["witdiff-definitely-not-a-real-program".to_owned()];

    let receipt = fixture.verify(&config);

    assert!(
        !receipt.integrity_findings.is_empty(),
        "findings computed before the run do not depend on the test command \
         and must still be reported; notes: {:?}",
        receipt.notes
    );
}

// ---------------------------------------------------------------------------
// ADR-0022: signed receipts
// ---------------------------------------------------------------------------

/// A receipt is signed when a key is configured, and the signature covers the
/// status as well as the digest.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn a_configured_key_signs_the_receipt() {
    let Some(key) = write_ed25519_key() else {
        return; // No Node in this environment; nothing to assert.
    };
    let fixture = Fixture::new(&[
        ("src/lib.rs", fixed_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let mut config = fixture_config();
    config.verification.signing_key = Some(key.display().to_string());

    let receipt = fixture.verify(&config);
    let signature = receipt
        .signature
        .as_ref()
        .expect("a configured key must sign");

    assert_eq!(signature.algorithm, "ed25519");
    assert_eq!(
        signature.digest,
        receipt.verification_digest.as_deref().unwrap_or_default(),
        "the signature must cover the digest in the same receipt"
    );
}

/// Without a key the receipt is simply unsigned — not an error, because
/// signing is opt-in.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn an_unconfigured_key_leaves_the_receipt_unsigned() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", fixed_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let receipt = fixture.verify(&fixture_config());
    assert!(receipt.signature.is_none(), "signing is opt-in");
}

/// A key path that does not exist must not silently yield an unsigned receipt:
/// the operator asked for a signature and did not get one, so it is reported.
#[test]
#[ignore = "end-to-end: spawns real commands; run with -- --ignored"]
fn a_missing_key_is_reported_rather_than_ignored() {
    let fixture = Fixture::new(&[
        ("src/lib.rs", fixed_lib()),
        ("tests/existing.rs", UNRELATED_TEST),
    ]);
    fixture.warm_lockfile();
    fixture.commit_base("base");
    fixture.write("src/lib.rs", fixed_lib());
    fixture.write("tests/regression.rs", CREDIBLE_REGRESSION_TEST);

    let mut config = fixture_config();
    config.verification.signing_key = Some("/nonexistent/witdiff-signing-key.pem".to_owned());

    let receipt = fixture.verify(&config);
    assert!(receipt.signature.is_none());
    assert!(
        receipt
            .notes
            .iter()
            .any(|note| note.contains("could not be signed")),
        "the operator must be told the receipt is unsigned, got {:?}",
        receipt.notes
    );
}

/// Write a throwaway Ed25519 key, or return `None` when Node is unavailable.
fn write_ed25519_key() -> Option<std::path::PathBuf> {
    if !Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return None;
    }
    let path = std::env::temp_dir().join(format!("witdiff-test-key-{}.pem", std::process::id()));
    let script = r#"
const c = require("crypto"), fs = require("fs");
const { privateKey } = c.generateKeyPairSync("ed25519");
// argv[2], not argv[1]: argv[1] is this script's own path, and writing the
// key there overwrote the script before it could run.
fs.writeFileSync(process.argv[2], privateKey.export({ type: "pkcs8", format: "pem" }));
"#;
    let script_path =
        std::env::temp_dir().join(format!("witdiff-keygen-{}.js", std::process::id()));
    fs::write(&script_path, script).expect("write keygen");
    let status = Command::new("node")
        .arg(&script_path)
        .arg(&path)
        .status()
        .expect("run keygen");
    let _ = fs::remove_file(&script_path);
    assert!(status.success(), "key generation should succeed");
    Some(path)
}
