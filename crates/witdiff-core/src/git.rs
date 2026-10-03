//! Git object and worktree access.
//!
//! The base experiment runs inside a detached worktree that must never outlive
//! the verification attempt, on any `Result` path. `WorktreeGuard` owns that
//! lifetime.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::{
    config::TestMatcher,
    model::{ChangeKind, ChangedFile},
};

/// RAII owner of a temporary detached Git worktree.
///
/// Cleanup is performed on `Drop` so that an early `?` — a failed patch
/// transplant, a failed test spawn, or a panic — cannot leak a worktree
/// registration into the developer's `git worktree list`. A leaked registration
/// pointing at a deleted temporary directory is not merely untidy: it makes
/// later `git worktree add` calls fail and corrupts the environment for
/// subsequent verification runs.
///
/// Cleanup is idempotent and best-effort during unwinding: `Drop` cannot report
/// an error, so failures are swallowed deliberately. The one thing `Drop` must
/// never do is panic, because a panic inside `Drop` while already unwinding
/// aborts the process.
pub struct WorktreeGuard<'repo> {
    repo: &'repo GitRepo,
    path: PathBuf,
    /// Set once cleanup has run, so `remove_worktree` and `Drop` never fight and
    /// so a deliberate retain is not silently undone.
    finished: bool,
}

impl<'repo> WorktreeGuard<'repo> {
    /// Create a detached worktree at `path` checked out at `reference`.
    ///
    /// On failure the temporary directory has already been removed by the
    /// caller's `TempDir`; Git registers a fresh worktree only after a
    /// successful checkout, so no cleanup is owed here.
    pub fn create(repo: &'repo GitRepo, path: &Path, reference: &str) -> Result<Self> {
        repo.add_worktree(path, reference)?;
        Ok(Self {
            repo,
            path: path.to_owned(),
            finished: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the worktree now and report the outcome.
    ///
    /// Callers that care about a cleanup failure should use this instead of
    /// relying on `Drop`, which cannot surface errors.
    pub fn remove(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        // Mark finished before attempting removal: if the explicit removal
        // fails we still must not double-remove during drop, and the operator
        // needs `git worktree prune` rather than a second silent attempt.
        self.finished = true;
        self.repo.remove_worktree(&self.path)
    }

    /// Relinquish cleanup and hand ownership of the worktree to the caller.
    ///
    /// Used by `--keep-worktree`, where retaining the worktree is the requested
    /// behavior rather than a leak.
    pub fn retain(mut self) -> PathBuf {
        self.finished = true;
        self.path.clone()
    }
}

impl Drop for WorktreeGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        // Best-effort: `Drop` runs during unwinding and cannot propagate.
        if let Err(error) = self.repo.remove_worktree(&self.path) {
            eprintln!(
                "witdiff: warning: could not remove temporary base worktree {}: {error:#}",
                self.path.display()
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    pub fn discover(start: &Path) -> Result<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(start)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("failed to launch git")?;
        if !output.status.success() {
            bail!(
                "{} is not inside a Git repository: {}",
                start.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head_commit(&self) -> Result<String> {
        Ok(self.git_text(["rev-parse", "HEAD"])?.trim().to_owned())
    }

    pub fn is_dirty(&self) -> Result<bool> {
        Ok(!self.git_text(["status", "--porcelain"])?.trim().is_empty())
    }

    pub fn ref_exists(&self, reference: &str) -> bool {
        let commitish = format!("{reference}^{{commit}}");
        self.git_status(["rev-parse", "--verify", "--quiet", &commitish])
            .unwrap_or(false)
    }

    pub fn choose_base(&self, requested: Option<&str>) -> Result<String> {
        if let Some(reference) = requested {
            if !self.ref_exists(reference) {
                bail!("base reference does not resolve to a commit: {reference}");
            }
            return Ok(reference.to_owned());
        }

        for candidate in ["origin/main", "main", "origin/master", "master", "HEAD~1"] {
            if self.ref_exists(candidate) {
                return Ok(candidate.to_owned());
            }
        }
        bail!("could not auto-detect a base ref; pass --base <ref> or set verification.base")
    }

    pub fn changed_files(&self, base: &str, matcher: &TestMatcher) -> Result<Vec<ChangedFile>> {
        let output = self.git_text(["diff", "--name-status", "--find-renames", base, "--"])?;
        let mut files = Vec::new();
        let mut seen = HashSet::new();

        for line in output.lines().filter(|line| !line.trim().is_empty()) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let status = parts[0];
            let code = status.chars().next().unwrap_or('?');
            let (previous_path, path) = if matches!(code, 'R' | 'C') && parts.len() >= 3 {
                (Some(parts[1].to_owned()), parts[2].to_owned())
            } else {
                (None, parts[1].to_owned())
            };
            let kind = match code {
                'A' => ChangeKind::Added,
                'M' => ChangeKind::Modified,
                'D' => ChangeKind::Deleted,
                'R' => ChangeKind::Renamed,
                'C' => ChangeKind::Copied,
                'T' => ChangeKind::TypeChanged,
                'U' => ChangeKind::Unmerged,
                _ => ChangeKind::Unknown,
            };
            seen.insert(path.clone());
            files.push(ChangedFile {
                is_test: matcher.is_test_path(&path),
                path,
                previous_path,
                kind,
                tracked: true,
            });
        }

        let untracked = self.git_text(["ls-files", "--others", "--exclude-standard"])?;
        for path in untracked.lines().filter(|line| !line.trim().is_empty()) {
            if seen.contains(path) {
                continue;
            }
            files.push(ChangedFile {
                is_test: matcher.is_test_path(path),
                path: path.to_owned(),
                previous_path: None,
                kind: ChangeKind::Untracked,
                tracked: false,
            });
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    pub fn diff_for_paths(&self, base: &str, paths: &[String], unified: usize) -> Result<Vec<u8>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.root)
            .arg("diff")
            .arg(format!("--unified={unified}"))
            .arg("--binary")
            .arg(base)
            .arg("--");
        for path in paths {
            cmd.arg(path);
        }
        let output = cmd.output().context("failed to run git diff")?;
        if !output.status.success() {
            bail!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }

    pub fn diff_text_for_path(&self, base: &str, path: &str) -> Result<String> {
        let bytes = self.diff_for_paths(base, &[path.to_owned()], 0)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn add_worktree(&self, path: &Path, base: &str) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["worktree", "add", "--detach", "--force"])
            .arg(path)
            .arg(base)
            .output()
            .context("failed to create base worktree")?;
        if !output.status.success() {
            bail!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["worktree", "remove", "--force"])
            .arg(path)
            .output()
            .context("failed to remove base worktree")?;
        if !output.status.success() {
            return Err(anyhow!(
                "git worktree remove failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let _ = self.git_status(["worktree", "prune"]);
        Ok(())
    }

    pub fn apply_patch(&self, worktree: &Path, patch: &[u8]) -> Result<()> {
        if patch.is_empty() {
            return Ok(());
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["apply", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to launch git apply")?;
        child
            .stdin
            .as_mut()
            .context("git apply stdin unavailable")?
            .write_all(patch)
            .context("failed writing patch to git apply")?;
        let output = child
            .wait_with_output()
            .context("failed waiting for git apply")?;
        if !output.status.success() {
            bail!(
                "could not transplant changed tests onto base revision: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn copy_untracked_files(&self, worktree: &Path, files: &[String]) -> Result<()> {
        for relative in files {
            let source = self.root.join(relative);
            let destination = worktree.join(relative);
            if !source.is_file() {
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed creating {}", parent.display()))?;
            }
            fs::copy(&source, &destination).with_context(|| {
                format!(
                    "failed copying untracked test {} to base worktree",
                    source.display()
                )
            })?;
        }
        Ok(())
    }

    pub fn workspace_fingerprint(&self, base: &str) -> Result<String> {
        let mut hasher = Sha256::new();
        let diff = self.git_bytes(["diff", "--binary", base, "--"])?;
        hasher.update(&diff);

        // Include status so newly created/deleted/untracked paths during verification
        // invalidate evidence even if they were not in the initial changed-file set.
        let status = self.git_bytes(["status", "--porcelain=v1", "-z"])?;
        hasher.update(&status);

        let untracked = self.git_text(["ls-files", "--others", "--exclude-standard"])?;
        for relative in untracked.lines().filter(|line| !line.trim().is_empty()) {
            hasher.update(relative.as_bytes());
            let path = self.root.join(relative);
            if path.is_file() {
                hasher.update(fs::read(&path).with_context(|| {
                    format!(
                        "failed reading untracked file for fingerprint: {}",
                        path.display()
                    )
                })?);
            }
        }
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn inline_test_hints(&self, base: &str, changed: &[ChangedFile]) -> Result<Vec<String>> {
        let mut hints = Vec::new();
        for file in changed.iter().filter(|f| {
            f.path.ends_with(".rs") && !f.is_test && !matches!(f.kind, ChangeKind::Deleted)
        }) {
            let diff = self.diff_text_for_path(base, &file.path)?;
            if diff.lines().any(|line| {
                (line.starts_with('+') || line.starts_with('-'))
                    && !line.starts_with("+++")
                    && !line.starts_with("---")
                    && (line.contains("#[test]")
                        || line.contains("#[cfg(test)]")
                        || line.contains("mod tests")
                        || line.contains("assert!(")
                        || line.contains("assert_eq!(")
                        || line.contains("assert_ne!("))
            }) {
                hints.push(file.path.clone());
            }
        }
        Ok(hints)
    }

    fn git_text<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git_bytes(args)?).into_owned())
    }

    fn git_bytes<const N: usize>(&self, args: [&str; N]) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .context("failed to launch git")?;
        if !output.status.success() {
            bail!(
                "git command failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    fn git_status<const N: usize>(&self, args: [&str; N]) -> Result<bool> {
        let status = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .status()
            .context("failed to launch git")?;
        Ok(status.success())
    }
}
