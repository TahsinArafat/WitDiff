//! Toolchain installation, kept in the CLI so the core gains no network
//! capability (ADR-0019).
//!
//! Scope is deliberately narrow: WitDiff runs the *project's own* installer
//! when one is committed, and never chooses a version itself.
//!
//! - A Java project's `mvnw`/`gradlew` downloads an exactly pinned
//!   distribution described by files the repository already commits, and Gradle
//!   supports a `distributionSha256Sum`. That is the same command a human
//!   developer would run.
//! - Package managers that execute arbitrary project scripts (`npm install`,
//!   `pip install`, `go install`) are **not** run. Whether to execute
//!   project-controlled install scripts is the developer's decision, not a
//!   verification tool's.
//! - Nothing is installed silently: every attempt is reported with the exact
//!   command and its outcome.

use anyhow::Context;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A toolchain WitDiff can try to obtain.
pub struct Installer {
    /// The program that was missing.
    pub program: String,
    /// The command to run, if WitDiff is willing to run one.
    pub command: Option<(PathBuf, Vec<String>)>,
    /// Human-readable description of what running it does.
    pub description: String,
}

/// Look for a project-committed installer for a missing program.
///
/// Returns `None` when the project commits no installer, which is the signal to
/// report the manual step instead of guessing.
pub fn find_installer(root: &Path, program: &str) -> Option<Installer> {
    let base = Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    match base.as_str() {
        // The Maven wrapper is a shell script on Unix and a .cmd on Windows.
        "mvn" => {
            for candidate in ["mvnw", "mvnw.cmd"] {
                let path = root.join(candidate);
                if path.is_file() {
                    return Some(Installer {
                        program: program.to_owned(),
                        command: Some((path, vec!["test".to_owned()])),
                        description:
                            "the repository's Maven wrapper, which downloads the version pinned in .mvn/wrapper/maven-wrapper.properties"
                                .to_owned(),
                    });
                }
            }
            None
        }
        "gradle" => {
            for candidate in ["gradlew", "gradlew.bat"] {
                let path = root.join(candidate);
                if path.is_file() {
                    return Some(Installer {
                        program: program.to_owned(),
                        command: Some((path, vec!["test".to_owned()])),
                        description:
                            "the repository's Gradle wrapper, which downloads the version pinned in gradle/wrapper/gradle-wrapper.properties"
                                .to_owned(),
                    });
                }
            }
            None
        }
        _ => None,
    }
}

impl Installer {
    /// The command line to use in place of the missing program.
    ///
    /// A wrapper puts its toolchain on `PATH` only for its own invocation, so
    /// the configured command would still fail to start. Substituting the
    /// wrapper is what makes the installation useful, and it is recorded in the
    /// receipt's `effective_test_command` rather than applied silently.
    pub fn command_line(&self) -> Option<Vec<String>> {
        let (program, args) = self.command.as_ref()?;
        let mut parts = vec![program.display().to_string()];
        parts.extend(args.iter().cloned());
        Some(parts)
    }
}

/// Run an installer, capturing its output.
pub fn run_installer(installer: &Installer) -> anyhow::Result<()> {
    let Some((program, args)) = &installer.command else {
        anyhow::bail!("no installer is available for `{}`", installer.program);
    };
    let output = Command::new(program)
        .args(args)
        .current_dir(
            program
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )
        .output()
        .with_context(|| format!("failed to run {}", program.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "{} exited with {}: {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_with_no_wrapper_yields_no_installer() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        assert!(find_installer(dir.path(), "mvn").is_none());
        assert!(find_installer(dir.path(), "gradle").is_none());
    }

    #[test]
    fn a_committed_maven_wrapper_is_found() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("mvnw"), "#!/bin/sh\n").expect("write");
        let installer = find_installer(dir.path(), "mvn").expect("wrapper should be found");
        assert!(installer.command.is_some());
        assert!(
            installer.description.contains("pinned"),
            "the description should say the version comes from the project"
        );
    }

    /// Windows wrappers are `.cmd`/`.bat`, not shell scripts.
    #[test]
    fn a_windows_maven_wrapper_is_found() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("mvnw.cmd"), "@echo off\n").expect("write");
        assert!(find_installer(dir.path(), "mvn").is_some());
    }

    #[test]
    fn a_committed_gradle_wrapper_is_found() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("gradlew"), "#!/bin/sh\n").expect("write");
        assert!(find_installer(dir.path(), "gradle").is_some());
    }

    /// The dangerous direction: an installer must never be invented for a
    /// program the project has not pinned, because that would mean WitDiff
    /// choosing a version.
    #[test]
    fn no_installer_is_invented_for_unpinned_programs() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        // A wrapper exists, but for a different program.
        std::fs::write(dir.path().join("mvnw"), "#!/bin/sh\n").expect("write");
        assert!(
            find_installer(dir.path(), "cargo").is_none(),
            "a Maven wrapper must not be offered for a missing cargo"
        );
        assert!(
            find_installer(dir.path(), "npm").is_none(),
            "package managers that run project scripts are not run"
        );
        assert!(find_installer(dir.path(), "python3").is_none());
    }
}
