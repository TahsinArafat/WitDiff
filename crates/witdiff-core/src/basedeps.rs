//! Making gitignored dependencies available to the base worktree.
//!
//! The base experiment runs in a `git worktree`, which contains only committed
//! files. A project that gitignores its dependency directory — Composer's
//! `vendor/`, npm's `node_modules/`, a Python `venv/` — therefore cannot start
//! its test command there at all. Measured on a PHP project:
//!
//! ```text
//! base control : FAIL
//! stderr       : Could not open input file: vendor/bin/phpunit
//! ```
//!
//! The receipt says "the pristine base revision does not pass the configured
//! test command", which blames the base for a failure that is really WitDiff
//! failing to give the base the dependencies it needs.
//!
//! ## The guard is the whole point
//!
//! Linking the workspace's `vendor/` into the base worktree is sound **exactly
//! when the two revisions resolve to the same dependency set**. If the
//! lockfile changed between base and head, the workspace's installed
//! dependencies are not what the base revision would have installed, and the
//! base would run against a set it never declared.
//!
//! That direction is not merely imprecise, it is unsound in both directions:
//! a base that would genuinely have failed can pass (hiding a regression), and
//! a base that would genuinely have passed can fail (inventing one). So when a
//! lockfile changed, nothing is linked and the receipt says why.
//!
//! ## Why a copy, and not a symlink
//!
//! The first implementation symlinked the workspace's `vendor/` into the
//! worktree. It appeared to work — the base control moved from `FAIL` to
//! `PASS` — and it was **unsound**.
//!
//! Composer's generated autoloader computes `$baseDir = dirname($vendorDir)`
//! and maps `App\` to `$baseDir . '/src'`. Through a symlink that resolves back
//! to the *workspace*, so the base worktree loaded the head revision's source
//! files. Measured on a fixture whose base computed `a - b` and whose head
//! computed `a + b`, the base experiment reported:
//!
//! ```text
//! OK (2 tests, 2 assertions)
//! ```
//!
//! for a test asserting `add(2, 3) == 5`, which the base cannot satisfy. The
//! base had run head's code. A verdict drawn from that run is meaningless, and
//! the direction is the dangerous one: a genuine regression is exactly what
//! such a run hides.
//!
//! A dependency directory is not self-contained; it may contain generated files
//! holding paths that point outside it. A copy is therefore the only sound
//! option, and `node_modules` is the price of it.
//!
//! ## Why the guard is still the whole point

use std::path::Path;

use anyhow::{Context, Result};

/// Lockfiles that pin a dependency set. If any of these differs between base
/// and head, the workspace's installed dependencies are not the base's.
const LOCKFILES: [&str; 12] = [
    "composer.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "go.sum",
    "Gemfile.lock",
    "requirements.txt",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
];

/// What was copied, and what was refused.
#[derive(Debug, Default)]
pub struct Linking {
    /// Directory names that were copied into the worktree.
    pub linked: Vec<String>,
    /// Why a configured directory was not copied.
    pub refused: Vec<String>,
}

impl Linking {
    /// The receipt note, or `None` when nothing was configured or applicable.
    pub fn note(&self) -> Option<String> {
        if self.linked.is_empty() && self.refused.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.linked.is_empty() {
            parts.push(format!(
                "the base experiment reused this workspace's installed dependencies for {} \
                 (copied into the temporary worktree)",
                self.linked.join(", ")
            ));
        }
        for refusal in &self.refused {
            parts.push(refusal.clone());
        }
        Some(parts.join("; "))
    }
}

/// Copy the configured dependency directories into `worktree`.
///
/// `lockfiles_unchanged_between_base_and_head` is the guard. When it is false
/// nothing is copied at all, even if the workspace has the directory.
///
/// A **copy** and never a symlink: see the module comment. A dependency
/// directory can contain generated files whose paths resolve outside it, and
/// through a symlink those resolve into the workspace, which makes the base
/// experiment run head's code.
pub fn link_dependencies(
    workspace: &Path,
    worktree: &Path,
    configured: &[String],
    lockfiles_unchanged: bool,
) -> Result<Linking> {
    let mut outcome = Linking::default();
    if configured.is_empty() {
        return Ok(outcome);
    }

    if !lockfiles_unchanged {
        outcome.refused.push(
            "the base experiment did not reuse this workspace's installed dependencies: a \
             dependency lockfile changed between the base revision and the current one, so \
             the installed set is not the one the base would have resolved. If the base \
             control fails to start, that is why."
                .to_owned(),
        );
        return Ok(outcome);
    }

    for name in configured {
        let source = workspace.join(name);
        let target = worktree.join(name);
        // Never overwrite something the worktree actually has: if the base
        // committed the directory, that is the base's own content and wins.
        if !source.exists() {
            outcome.refused.push(format!(
                "`{name}` is configured for base dependency reuse but does not exist in this \
                 workspace"
            ));
            continue;
        }
        if target.exists() {
            continue;
        }
        copy_tree(&source, &target).with_context(|| {
            format!(
                "failed copying {} into the base worktree; the base control run will not be \
                 able to start",
                source.display()
            )
        })?;
        outcome.linked.push(name.clone());
    }
    Ok(outcome)
}

/// Whether any dependency lockfile is identical in both revisions.
///
/// Compares content, not presence: a lockfile added on head is a change, and a
/// lockfile removed on head is too.
pub fn lockfiles_unchanged(repo: &crate::git::GitRepo, base: &str, head: &str) -> bool {
    for name in LOCKFILES {
        let at_base = repo.show_file_at(base, name).ok().flatten();
        let at_head = repo.show_file_at(head, name).ok().flatten();
        match (at_base, at_head) {
            (Some(a), Some(b)) if a == b => {}
            // Absent in both, or one absent: the lockfile did not change. A
            // project with no lockfile has declared nothing to pin.
            (None, None) => {}
            _ => return false,
        }
    }
    true
}

/// Copy a directory tree, preserving symlinks as symlinks.
///
/// An inner symlink is copied rather than followed, so a dependency directory
/// that links to something outside itself keeps pointing where it pointed. This
/// is the difference from a top-level symlink: the *tree* is real, so a
/// generated file computing `dirname(__DIR__)` from inside it resolves inside
/// the worktree, while a link the package itself placed still resolves as the
/// package intended.
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)
        .with_context(|| format!("failed creating {}", target.display()))?;
    let entries = std::fs::read_dir(source)
        .with_context(|| format!("failed reading {}", source.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed reading {}", source.display()))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let kind = entry
            .file_type()
            .with_context(|| format!("failed stat {}", from.display()))?;
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(&from)
                .with_context(|| format!("failed reading link {}", from.display()))?;
            symlink(&link, &to)
                .with_context(|| format!("failed copying link {}", from.display()))?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("failed copying {}", from.display()))?;
        }
    }
    Ok(())
}

/// Create a symlink, on every platform Rust can build for.
///
/// `std::os::unix::fs::symlink` does not exist on Windows, so calling it
/// unconditionally breaks the Windows build — which is exactly what happened:
/// this module compiled and its tests passed on macOS, while the release
/// workflow failed on `x86_64-pc-windows-msvc` with
/// `error[E0433]: failed to resolve: could not find 'unix' in 'os'`.
///
/// The dependency-visible behavior is to preserve an inner link as a link. On
/// Windows the platform decides whether that needs a file or directory hint,
/// so the target is inspected rather than guessed.
#[allow(unused_variables)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        // A Windows symlink must declare whether it points at a file or a
        // directory, and the target may not exist yet, so an existing target's
        // type is used and a missing one is assumed to be a file.
        let resolved = link.parent().map(|dir| dir.join(target));
        let is_dir = resolved.as_deref().is_some_and(|path| path.is_dir());
        if is_dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks are not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_linked_when_nothing_is_configured() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let outcome = link_dependencies(workspace.path(), worktree.path(), &[], true).unwrap();
        assert!(outcome.linked.is_empty());
        assert!(outcome.note().is_none());
    }

    #[test]
    fn an_existing_directory_is_linked_into_the_worktree() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(workspace.path().join("vendor")).unwrap();
        std::fs::write(workspace.path().join("vendor/autoload.php"), "<?php").unwrap();

        let outcome =
            link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();
        assert_eq!(outcome.linked, vec!["vendor".to_string()]);
        // The target must resolve to the workspace's real file, which is the
        // whole point: a copy would not.
        let resolved = worktree.path().join("vendor/autoload.php");
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "<?php");
    }

    /// The unsound case. A base that ran against head's dependencies is not a
    /// measurement of the base, so nothing may be linked.
    #[test]
    fn nothing_is_linked_when_a_lockfile_changed() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(workspace.path().join("vendor")).unwrap();

        let outcome =
            link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], false)
                .unwrap();
        assert!(
            outcome.linked.is_empty(),
            "must not link across a lockfile change"
        );
        assert!(
            !worktree.path().join("vendor").exists(),
            "the worktree must be left untouched"
        );
        assert!(outcome.note().unwrap().contains("lockfile changed"));
    }

    /// If the base committed its dependencies, that content is the base's own
    /// and must not be replaced by a symlink to the workspace's.
    #[test]
    fn a_directory_the_worktree_already_has_is_left_alone() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(workspace.path().join("vendor")).unwrap();
        std::fs::create_dir(worktree.path().join("vendor")).unwrap();
        std::fs::write(worktree.path().join("vendor/base-only.txt"), "base").unwrap();

        let outcome =
            link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();
        assert!(outcome.linked.is_empty());
        assert!(worktree.path().join("vendor/base-only.txt").exists());
    }

    #[test]
    fn a_missing_directory_is_reported_rather_than_silently_skipped() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let outcome =
            link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();
        assert!(outcome.linked.is_empty());
        let note = outcome.note().expect("a refusal must be reported");
        assert!(note.contains("does not exist"), "{note}");
    }

    /// The bug the first implementation shipped. Composer's autoloader computes
    /// `$baseDir = dirname($vendorDir)`, so a *symlinked* vendor resolves back
    /// to the workspace and the base worktree loads head's source. Measured:
    /// the base experiment reported `OK (2 tests)` for a test asserting
    /// `add(2, 3) == 5` against a base that computed `a - b`.
    ///
    /// The assertion is on the resolution, not on the file's bytes: after the
    /// copy, `dirname(vendor)` must be the worktree, so a path computed that
    /// way cannot reach outside it.
    #[test]
    fn the_copied_directory_resolves_inside_the_worktree() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(workspace.path().join("vendor")).unwrap();
        std::fs::write(workspace.path().join("vendor/autoload.php"), "<?php").unwrap();

        link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();

        let copied = worktree.path().join("vendor");
        assert!(
            !copied.symlink_metadata().unwrap().file_type().is_symlink(),
            "a symlinked vendor makes the base worktree resolve paths outside itself"
        );
        // dirname(vendor) inside the worktree must be the worktree.
        let base_dir = copied.parent().unwrap();
        assert_eq!(
            std::fs::canonicalize(base_dir).unwrap(),
            std::fs::canonicalize(worktree.path()).unwrap(),
        );
        // And it must NOT be the workspace, which is what a symlink produced.
        assert_ne!(
            std::fs::canonicalize(base_dir).unwrap(),
            std::fs::canonicalize(workspace.path()).unwrap(),
        );
    }

    /// A directory the package itself placed as a link must keep working: the
    /// tree is copied, not flattened, so inner links survive as links.
    #[test]
    fn an_inner_symlink_is_copied_as_a_link() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workspace.path().join("vendor/bin")).unwrap();
        std::fs::write(workspace.path().join("real-tool"), "#!/bin/sh\n").unwrap();
        // The platform-aware helper, not `std::os::unix`: this test failed to
        // compile on Windows for the same reason the module did.
        symlink(
            Path::new("../../real-tool"),
            &workspace.path().join("vendor/bin/tool"),
        )
        .unwrap();

        link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();

        let link = worktree.path().join("vendor/bin/tool");
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "an inner link must survive as a link"
        );
        assert_eq!(
            std::fs::read_link(&link).unwrap().to_string_lossy(),
            "../../real-tool"
        );
    }

    /// A base whose committed contents differ from the workspace must not be
    /// overwritten: the copy only fills a directory the worktree lacks.
    #[test]
    fn copying_does_not_overwrite_the_bases_own_dependencies() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(workspace.path().join("vendor")).unwrap();
        std::fs::write(workspace.path().join("vendor/head.txt"), "head").unwrap();
        std::fs::create_dir(worktree.path().join("vendor")).unwrap();
        std::fs::write(worktree.path().join("vendor/base.txt"), "base").unwrap();

        link_dependencies(workspace.path(), worktree.path(), &["vendor".into()], true).unwrap();

        assert!(worktree.path().join("vendor/base.txt").exists());
        assert!(!worktree.path().join("vendor/head.txt").exists());
    }
}
