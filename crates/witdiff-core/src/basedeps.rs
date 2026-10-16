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

    // `base_dependency_dirs` entries must be repo-relative paths that resolve
    // inside the worktree. This contract was undocumented and unenforced: an
    // absolute path was joined anyway — `worktree.join("/abs")` discards the
    // worktree entirely — and an escaping `..` walked out of it, after which
    // the copy failed with a permission error that named a path the user
    // recognised and a root they did not. Reject both before anything is
    // written, naming the contract and the config key.
    for name in configured {
        let path = Path::new(name);
        let escapes = path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir));
        if escapes {
            return Err(anyhow::anyhow!(
                "`verification.base_dependency_dirs` entries must be relative paths inside the \
                 repository; `{name}` is {} and would resolve outside the base worktree. Use a \
                 path like `node_modules` (relative to the repository root), not an absolute path \
                 and not one containing `..`.",
                if path.is_absolute() {
                    "absolute"
                } else {
                    "parent-directory-escaping"
                }
            ));
        }
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
    // A preserved symlink tree can carry links whose targets were never in the
    // copied directory. Measured: a plain pnpm farm (relative links into
    // `.pnpm/`) survives a copy and resolves — but a layout that links outside
    // `node_modules` (workspace `link:` entries, a virtual store placed
    // elsewhere, links recorded absolute) copies faithfully and then dies in
    // the base run with ERR_MODULE_NOT_FOUND, while the note still says the
    // dependencies were reused. Checking after the copy converts that silent
    // dead copy into a note that names the dangling target.
    for name in &outcome.linked {
        let target = worktree.join(name);
        for dangling in dangling_links(&target, 8) {
            outcome.refused.push(format!(
                "`{name}` was copied into the base worktree, but {dangling} does not resolve \
                 there, so the base control run may fail with ERR_MODULE_NOT_FOUND. The copy \
                 preserves symlinks as symlinks, so a tree that links outside itself (a \
                 workspace `link:` entry or a virtual store in another directory) points at \
                 what it always pointed at. Add the directory it targets to \
                 `verification.base_dependency_dirs`, or install a flat tree (for pnpm: \
                 `pnpm install --node-linker=hoisted`)"
            ));
        }
    }
    Ok(outcome)
}

/// The first few symlinks under `root` whose targets do not resolve.
///
/// Bounded twice: at most `max` findings (a broken tree can have thousands),
/// and the walk stops once that many are found. Files are never followed, only
/// links inspected, so the cost is the number of symlinks rather than the size
/// of the store beside them.
fn dangling_links(root: &Path, max: usize) -> Vec<String> {
    let mut found = Vec::new();
    if max == 0 || !root.is_dir() {
        return found;
    }
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if found.len() >= max {
                return found;
            }
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                if std::fs::metadata(&path).is_err() {
                    let link = std::fs::read_link(&path).unwrap_or_default();
                    found.push(format!("`{}` -> `{}`", path.display(), link.display()));
                }
            } else if file_type.is_dir() {
                stack.push(path);
            }
        }
    }
    found
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

    /// An absolute or escaping entry failed with a permission error naming a
    /// path the user recognised and a root they did not. The contract — entries
    /// are repo-relative and must resolve inside the worktree — was neither
    /// documented nor enforced, so the error read as a filesystem problem
    /// rather than a configuration one.
    #[test]
    fn an_absolute_entry_is_rejected_with_the_contract() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let error = link_dependencies(
            workspace.path(),
            worktree.path(),
            &["/tmp/somewhere/node_modules".to_owned()],
            true,
        )
        .expect_err("an absolute path must not be attempted");
        let text = error.to_string();
        assert!(
            text.contains("relative") && text.contains("base_dependency_dirs"),
            "the error must name the contract and the config key, got {text:?}"
        );
        assert!(!text.contains("Permission denied"), "got {text:?}");
    }

    #[test]
    fn an_escaping_entry_is_rejected() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let error = link_dependencies(
            workspace.path(),
            worktree.path(),
            &["../../elsewhere/node_modules".to_owned()],
            true,
        )
        .expect_err("`..` must not be walked out of the worktree");
        assert!(error.to_string().contains("relative"));
    }

    /// Relative entries still link, so the validation does not reject the form
    /// the docs actually recommend.
    /// A copied symlink tree can carry links that point outside it. The copy
    /// preserves them faithfully — which is correct for a pnpm farm whose links
    /// are relative into `.pnpm/`, and that case resolves after the copy — but a
    /// tree that links outside itself copies "successfully" and then dies in the
    /// base run with ERR_MODULE_NOT_FOUND while the receipt says the
    /// dependencies were reused. The dead links must be named in a note.
    #[test]
    fn a_copy_with_dangling_links_is_reported_not_silent() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let deps = workspace.path().join("deps");
        std::fs::create_dir_all(deps.join("pkg")).unwrap();
        std::fs::write(deps.join("pkg/index.js"), "1").unwrap();
        // A link that pointed somewhere real in the workspace but has no
        // counterpart in the worktree after the copy.
        std::os::unix::fs::symlink("../../../outside", deps.join("pkg/dead")).unwrap();

        let outcome = link_dependencies(
            workspace.path(),
            worktree.path(),
            &["deps".to_owned()],
            true,
        )
        .expect("the copy itself succeeds");

        assert_eq!(outcome.linked, vec!["deps".to_owned()], "the copy happened");
        assert!(
            !outcome.refused.is_empty(),
            "a dead link must be reported, not swallowed: {:?}",
            outcome.refused
        );
        let note = outcome.note().expect("a note describing the dead link");
        assert!(
            note.contains("base_dependency_dirs") && note.contains("pnpm"),
            "the note must name the config key and the actionable fix, got {note:?}"
        );
    }

    /// The common pnpm layout — relative links into `.pnpm/` inside the copied
    /// tree — must NOT be reported as broken, or every pnpm user gets a false
    /// alarm on a copy that works. Measured: that farm resolves after copy.
    #[test]
    fn a_farm_with_relative_links_is_not_reported_as_broken() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        let farm = workspace.path().join("node_modules");
        std::fs::create_dir_all(farm.join(".pnpm/lodash@1/node_modules/lodash")).unwrap();
        std::fs::write(
            farm.join(".pnpm/lodash@1/node_modules/lodash/index.js"),
            "1",
        )
        .unwrap();
        std::os::unix::fs::symlink(".pnpm/lodash@1/node_modules/lodash", farm.join("lodash"))
            .unwrap();

        let outcome = link_dependencies(
            workspace.path(),
            worktree.path(),
            &["node_modules".to_owned()],
            true,
        )
        .expect("the copy succeeds");
        assert!(outcome.refused.is_empty(), "{:?}", outcome.refused);
    }

    #[test]
    fn a_relative_entry_still_links() {
        let workspace = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workspace.path().join("vendor")).unwrap();
        std::fs::write(workspace.path().join("vendor/pkg.json"), "x").unwrap();
        let outcome = link_dependencies(
            workspace.path(),
            worktree.path(),
            &["vendor".to_owned()],
            true,
        )
        .expect("a relative entry must link");
        assert_eq!(outcome.linked, vec!["vendor".to_owned()]);
        assert!(worktree.path().join("vendor/pkg.json").is_file());
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
