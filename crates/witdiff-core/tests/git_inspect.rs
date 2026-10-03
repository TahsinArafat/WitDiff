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
