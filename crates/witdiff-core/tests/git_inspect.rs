use std::{fs, path::Path, process::Command};

use tempfile::TempDir;
use witdiff_core::{inspect_repository, Config, GitRepo};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git should run");
    assert!(status.success(), "git command failed: {args:?}");
}

#[test]
fn inspect_classifies_changed_tests_and_production_files() {
    let tmp = TempDir::new().unwrap();
    let repo_path = tmp.path().join("repo");
    fs::create_dir_all(repo_path.join("src")).unwrap();
    fs::create_dir_all(repo_path.join("tests")).unwrap();

    git(tmp.path(), &["init", "repo"]);
    git(
        &repo_path,
        &["config", "user.email", "witdiff@example.invalid"],
    );
    git(&repo_path, &["config", "user.name", "WitDiff Test"]);

    fs::write(
        repo_path.join("src/lib.rs"),
        "pub fn answer() -> u8 { 41 }\n",
    )
    .unwrap();
    fs::write(
        repo_path.join("tests/base.rs"),
        "#[test]\nfn base() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();
    git(&repo_path, &["add", "."]);
    git(&repo_path, &["commit", "-m", "base"]);

    fs::write(
        repo_path.join("src/lib.rs"),
        "pub fn answer() -> u8 { 42 }\n",
    )
    .unwrap();
    fs::write(
        repo_path.join("tests/regression.rs"),
        "#[test]\nfn regression() { assert_eq!(40 + 2, 42); }\n",
    )
    .unwrap();

    let repo = GitRepo::discover(&repo_path).unwrap();
    let report = inspect_repository(&repo, &Config::default(), Some("HEAD")).unwrap();

    assert!(report
        .changed_production_files
        .contains(&"src/lib.rs".to_owned()));
    assert!(report
        .changed_test_files
        .contains(&"tests/regression.rs".to_owned()));
    assert!(report.workspace_dirty);
}

/// Scaffold a repository with one production file and one test file, both
/// committed, so each test below only has to express its own change.
struct Repo {
    _tmp: TempDir,
    path: std::path::PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("repo");
        fs::create_dir_all(path.join("src")).unwrap();
        fs::create_dir_all(path.join("tests")).unwrap();
        git(tmp.path(), &["init", "repo"]);
        git(&path, &["config", "user.email", "witdiff@example.invalid"]);
        git(&path, &["config", "user.name", "WitDiff Test"]);
        fs::write(path.join("src/lib.rs"), "pub fn answer() -> u8 { 41 }\n").unwrap();
        fs::write(
            path.join("tests/base.rs"),
            "#[test]\nfn base() { assert_eq!(2 + 2, 4); }\n",
        )
        .unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        Self { _tmp: tmp, path }
    }

    fn inspect(&self) -> witdiff_core::InspectReport {
        let repo = GitRepo::discover(&self.path).unwrap();
        inspect_repository(&repo, &Config::default(), Some("HEAD")).unwrap()
    }
}

/// Git C-quotes any path containing non-ASCII or special bytes unless the
/// caller asks for NUL-delimited output. Reading paths with the default
/// encoding yields `"tests/caf\303\251.rs"`, a string that matches no glob and
/// names no file, so the change would be silently misclassified.
#[test]
fn non_ascii_paths_are_reported_verbatim_not_c_quoted() {
    let repo = Repo::new();
    fs::write(
        repo.path.join("tests/café_ünïcode.rs"),
        "#[test]\nfn unicode_name() { assert!(true); }\n",
    )
    .unwrap();
    fs::write(
        repo.path.join("tests/with space.rs"),
        "#[test]\nfn spaced() { assert!(true); }\n",
    )
    .unwrap();

    let report = repo.inspect();

    assert!(
        report
            .changed_test_files
            .contains(&"tests/café_ünïcode.rs".to_owned()),
        "non-ASCII test path was mangled; got {:?}",
        report.changed_test_files
    );
    assert!(
        report
            .changed_test_files
            .contains(&"tests/with space.rs".to_owned()),
        "space-containing test path was mangled; got {:?}",
        report.changed_test_files
    );
    // A C-quoted path would still be "classified", but as production code,
    // because the quotes and escapes defeat every test glob.
    assert!(report.changed_production_files.is_empty());
}

/// A rename out of production code must not be classified as a test-only
/// change, or the production file's diff would ride along in the transplant.
#[test]
fn rename_from_production_into_tests_is_not_a_test_only_change() {
    let repo = Repo::new();
    fs::rename(repo.path.join("src/lib.rs"), repo.path.join("tests/lib.rs")).unwrap();
    git(&repo.path, &["add", "-A"]);

    let report = repo.inspect();
    let renamed = report
        .changed_files
        .iter()
        .find(|file| file.path == "tests/lib.rs")
        .expect("the renamed file should appear in the changed set");

    assert_eq!(renamed.previous_path.as_deref(), Some("src/lib.rs"));
    assert!(
        !renamed.previous_is_test,
        "a rename sourced from production code must not report previous_is_test"
    );
}

/// A genuine test-to-test rename is the supported case and must keep its
/// test-only transplant eligibility.
#[test]
fn rename_between_test_paths_remains_test_only() {
    let repo = Repo::new();
    fs::rename(
        repo.path.join("tests/base.rs"),
        repo.path.join("tests/moved.rs"),
    )
    .unwrap();
    git(&repo.path, &["add", "-A"]);

    let report = repo.inspect();
    let renamed = report
        .changed_files
        .iter()
        .find(|file| file.path == "tests/moved.rs")
        .expect("the renamed file should appear in the changed set");

    assert!(renamed.is_test);
    assert!(renamed.previous_is_test);
}

/// A path whose bytes are not valid UTF-8 cannot be named in a JSON receipt.
///
/// The detection itself is unit-tested in `git.rs` against raw `-z` bytes,
/// because creating such a filename is not permitted on every platform and
/// under every sandbox. This test only asserts the end-to-end invariant that
/// no reported path is silently mangled: whatever `git ls-files -z` produced
/// must be reported as-is.
#[test]
#[cfg(unix)]
fn reported_paths_are_never_silently_mangled() {
    let repo = Repo::new();
    // A valid-but-tricky name: Unicode, a space, and a quote. None of these
    // may be escaped, truncated, or split when reported.
    fs::write(
        repo.path.join("tests/quote\"and_ünïcode name.rs"),
        "#[test]\nfn tricky() { assert!(true); }\n",
    )
    .unwrap();

    let report = repo.inspect();

    let tricky = report
        .changed_files
        .iter()
        .find(|file| file.path.contains("quote\""))
        .expect("the tricky path should be reported");
    assert_eq!(tricky.path, "tests/quote\"and_ünïcode name.rs");
    assert!(
        !tricky.path_is_lossy,
        "a valid UTF-8 path must not be flagged lossy"
    );
    assert!(
        report
            .changed_test_files
            .contains(&"tests/quote\"and_ünïcode name.rs".to_owned()),
        "a tricky but valid path must still classify as a test; got {:?}",
        report.changed_test_files
    );
}
