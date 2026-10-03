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

/// Iterator over NUL-delimited records from a `-z` Git plumbing command.
///
/// Git's `-z` output is a flat sequence of `field\0` records, not lines, so a
/// path containing a newline, a quote, or a backslash is delivered verbatim.
/// Records are decoded up front so each field is an owned `String`.
///
/// Non-UTF-8 paths cannot be represented exactly in a JSON receipt, so they are
/// lossily decoded. The loss is recorded per field at decode time — it cannot
/// be recovered afterwards, because the lossy form is itself valid UTF-8 — and
/// surfaced as `ChangedFile::path_is_lossy`.
struct NulRecords {
    fields: std::vec::IntoIter<PathField>,
}

impl NulRecords {
    fn new(bytes: &[u8]) -> Self {
        let mut fields = Vec::new();
        for raw in bytes.split(|byte| *byte == 0) {
            if raw.is_empty() {
                continue;
            }
            fields.push(match std::str::from_utf8(raw) {
                Ok(text) => PathField {
                    text: text.to_owned(),
                    lossy: false,
                },
                Err(_) => PathField {
                    text: String::from_utf8_lossy(raw).into_owned(),
                    lossy: true,
                },
            });
        }
        Self {
            fields: fields.into_iter(),
        }
    }

    fn next_field(&mut self) -> Option<PathField> {
        self.fields.next()
    }
}

/// One NUL-delimited Git record, decoded.
#[derive(Debug, Clone)]
struct PathField {
    text: String,
    lossy: bool,
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
        let mut files = Vec::new();
        let mut seen = HashSet::new();

        // `-z` is mandatory, not cosmetic. Without it Git C-quotes any path
        // containing non-ASCII or special bytes, so `tests/café.rs` arrives as
        // the literal text `"tests/caf\303\251.rs"` including the quotes and
        // backslash escapes. That string matches no glob and names no file on
        // disk, so such a change would be silently misclassified as production
        // code and never transplanted. `-z` also NUL-delimits, so a filename
        // containing a newline cannot desynchronize the record stream the way
        // line-splitting does.
        let output =
            self.git_bytes(["diff", "--name-status", "--find-renames", "-z", base, "--"])?;
        let mut records = NulRecords::new(&output);
        while let Some(status) = records.next_field() {
            let code = status.text.chars().next().unwrap_or('?');
            let (previous, path) = if matches!(code, 'R' | 'C') {
                match (records.next_field(), records.next_field()) {
                    (Some(previous), Some(path)) => (Some(previous), path),
                    _ => continue,
                }
            } else {
                match records.next_field() {
                    Some(path) => (None, path),
                    None => continue,
                }
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
            let is_lossy = path.lossy || previous.as_ref().is_some_and(|p| p.lossy);
            // A rename is only transplantable as a test-only change when the
            // path it came from was also a test. Renaming production code into a
            // test directory must not let the production file's diff ride along
            // in a "test-only" transplant.
            let previous_is_test = previous
                .as_ref()
                .is_some_and(|p| matcher.is_test_path(&p.text));
            seen.insert(path.text.clone());
            files.push(ChangedFile {
                is_test: matcher.is_test_path(&path.text),
                path: path.text,
                previous_path: previous.map(|p| p.text),
                previous_is_test,
                kind,
                tracked: true,
                path_is_lossy: is_lossy,
            });
        }

        let untracked = self.git_bytes(["ls-files", "--others", "--exclude-standard", "-z"])?;
        let mut untracked_records = NulRecords::new(&untracked);
        while let Some(path) = untracked_records.next_field() {
            if seen.contains(&path.text) {
                continue;
            }
            files.push(ChangedFile {
                is_test: matcher.is_test_path(&path.text),
                path: path.text,
                previous_path: None,
                previous_is_test: false,
                kind: ChangeKind::Untracked,
                tracked: false,
                path_is_lossy: path.lossy,
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

    /// Read a path's contents at a revision, for syntax-aware comparison.
    ///
    /// Returns `None` when the path did not exist at that revision, which is
    /// the ordinary case for a newly added test file.
    pub fn show_file_at(&self, reference: &str, path: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .arg("show")
            .arg(format!("{reference}:{path}"))
            .output()
            .with_context(|| format!("failed to read {path} at {reference}"))?;
        if output.status.success() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A path absent from a revision is a normal outcome, not a failure.
        // Anything else stays loud rather than being mistaken for "new file".
        if stderr.contains("does not exist") || stderr.contains("exists on disk") {
            return Ok(None);
        }
        bail!("git show {reference}:{path} failed: {}", stderr.trim());
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

    /// Write `contents` over a path inside a worktree, creating parents.
    ///
    /// Used by the inline-test splice (ADR-0010), which produces a file that
    /// exists in neither revision and therefore cannot be copied or patched.
    pub fn write_into_worktree(
        &self,
        worktree: &Path,
        relative: &str,
        contents: &str,
    ) -> Result<()> {
        let destination = worktree.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }
        fs::write(&destination, contents).with_context(|| {
            format!(
                "failed writing spliced inline tests to base worktree: {}",
                destination.display()
            )
        })
    }

    /// Head-revision line numbers touched by a path's diff, 1-based.
    ///
    /// Used by mutation to restrict candidates to the changed lines, so cost
    /// tracks the change rather than the file size (ADR-0011). Only added and
    /// context lines have head positions; a removed line exists only in base.
    pub fn changed_head_lines(&self, base: &str, path: &str) -> Result<Vec<usize>> {
        let diff = self.diff_text_for_path(base, path)?;
        Ok(parse_changed_head_lines(&diff))
    }

    /// Read a repository-relative path from the working tree.
    ///
    /// Returns `None` when the path is absent or is not valid UTF-8, because a
    /// source file that cannot be decoded as text cannot be parsed or spliced.
    pub fn worktree_source(&self, relative: &str) -> Result<Option<String>> {
        match fs::read(self.root.join(relative)) {
            Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("failed reading working-tree source for {relative}")),
        }
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

    /// Production `.rs` files whose change may involve an inline test module.
    ///
    /// The earlier version of this check looked for marker substrings
    /// (`#[test]`, `assert!(`, …) on added or removed lines. That misses the
    /// common cases it exists to catch: a changed assertion body
    /// (`is_even(3)` becoming `is_even(4)`) contains no marker at all, and a
    /// file that merely *has* a test module while its production code changes
    /// contains markers on no changed line.
    ///
    /// The check is structural instead. A file is a candidate when it parses and
    /// contains a `#[cfg(test)]` module, and something in it changed. Whether
    /// that change is confined to the module — and can therefore be spliced —
    /// is decided later, by the code that has both revisions to compare
    /// (ADR-0010).
    pub fn inline_test_hints(&self, base: &str, changed: &[ChangedFile]) -> Result<Vec<String>> {
        let mut hints = Vec::new();
        for file in changed.iter().filter(|f| {
            f.path.ends_with(".rs") && !f.is_test && !matches!(f.kind, ChangeKind::Deleted)
        }) {
            // The head revision is what is on disk; the base revision is read
            // from Git. Either may fail to parse, in which case the file is not
            // a candidate for a structural splice.
            let head_source = fs::read_to_string(self.root.join(&file.path)).unwrap_or_default();
            if crate::inline::test_module_identities(&head_source).is_empty() {
                continue;
            }
            if !self.diff_text_for_path(base, &file.path)?.trim().is_empty() {
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

/// Head-revision line numbers that a unified diff adds or carries as context.
///
/// Hunk headers are `@@ -base_start,base_len +head_start,head_len @@`. Walking
/// the body lets each line be attributed to its head position: context lines
/// (` `) and added lines (`+`) advance the head counter, removed lines (`-`) do
/// not, because they exist only in the base revision.
fn parse_changed_head_lines(diff: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut head_line = 0usize;
    let mut in_hunk = false;

    for raw in diff.lines() {
        if let Some(rest) = raw.strip_prefix("@@ ") {
            let head = rest
                .split_whitespace()
                .find(|token| token.starts_with('+'))
                .and_then(|token| {
                    let start = token.trim_start_matches('+');
                    start.split(',').next().map(str::to_owned)
                })
                .and_then(|start| start.parse::<usize>().ok());
            match head {
                Some(start) => {
                    head_line = start;
                    in_hunk = true;
                }
                None => in_hunk = false,
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        match raw.as_bytes().first() {
            Some(b'+') => {
                lines.push(head_line);
                head_line += 1;
            }
            Some(b' ') => {
                lines.push(head_line);
                head_line += 1;
            }
            Some(b'-') => {}
            _ => {}
        }
    }

    lines.sort_unstable();
    lines.dedup();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_records_split_on_nul_not_newline() {
        // A filename containing a newline must survive as one record. Splitting
        // the output on line breaks would shred it into two bogus paths.
        let bytes = b"M\0tests/we\nird.rs\0A\0tests/plain.rs\0";
        let mut records = NulRecords::new(bytes);
        assert_eq!(records.next_field().unwrap().text, "M");
        assert_eq!(records.next_field().unwrap().text, "tests/we\nird.rs");
        assert_eq!(records.next_field().unwrap().text, "A");
        assert_eq!(records.next_field().unwrap().text, "tests/plain.rs");
        assert!(records.next_field().is_none());
    }

    #[test]
    fn nul_records_decode_non_ascii_paths_verbatim() {
        // Without `-z`, Git renders this as "tests/caf\303\251.rs".
        let bytes = "M\0tests/café_ünïcode.rs\0".as_bytes();
        let mut records = NulRecords::new(bytes);
        assert_eq!(records.next_field().unwrap().text, "M");
        let field = records.next_field().unwrap();
        assert_eq!(field.text, "tests/café_ünïcode.rs");
        assert!(!field.lossy, "valid UTF-8 must not be flagged lossy");
    }

    #[test]
    fn nul_records_flag_undecodable_paths() {
        // Lossiness has to be detected here, at decode time. After lossy
        // replacement the string is itself valid UTF-8, so the flag can never
        // be recovered downstream.
        let bytes = b"M\0tests/invalid_\xff.rs\0";
        let mut records = NulRecords::new(bytes);
        assert_eq!(records.next_field().unwrap().text, "M");
        let field = records
            .next_field()
            .expect("record should still be present");
        assert!(field.lossy, "undecodable path must be flagged lossy");
        assert!(field.text.contains('\u{fffd}'));
    }

    #[test]
    fn nul_records_tolerate_a_missing_trailing_nul() {
        // A truncated read should not discard a real final field.
        let bytes = b"M\0tests/plain.rs";
        let mut records = NulRecords::new(bytes);
        assert_eq!(records.next_field().unwrap().text, "M");
        assert_eq!(records.next_field().unwrap().text, "tests/plain.rs");
    }

    #[test]
    fn changed_head_lines_attributes_added_and_context_lines() {
        // A raw string, because a `\`-continued string literal strips the
        // leading space of each line — which would silently delete the context
        // markers this test exists to check.
        let diff = r"diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,4 +1,4 @@
 ctx1
-removed
+added3
 ctx3
 ctx4
@@ -9,4 +10,5 @@ fn thing()
 ctx10
 ctx11
+added12
 ctx12
 ctx13
";
        let lines = parse_changed_head_lines(diff);
        // Head positions: ctx1=1, added3=2, ctx3=3, ctx4=4. Removed lines
        // consume no head position.
        assert_eq!(lines, vec![1, 2, 3, 4, 10, 11, 12, 13, 14]);
    }

    #[test]
    fn changed_head_lines_ignores_file_headers() {
        let diff = "--- a/f.rs\n+++ b/f.rs\n";
        assert!(parse_changed_head_lines(diff).is_empty());
    }

    #[test]
    fn changed_head_lines_handles_a_single_line_hunk_header() {
        // Git omits `,1` for one-line ranges: `@@ -7 +7 @@`.
        let diff = "@@ -7 +7 @@\n-old\n+new\n";
        assert_eq!(parse_changed_head_lines(diff), vec![7]);
    }

    #[test]
    fn nul_records_skip_empty_records() {
        // `--name-status -z` can emit empty fields; they are not paths.
        let bytes = b"M\0\0tests/plain.rs\0";
        let mut records = NulRecords::new(bytes);
        assert_eq!(records.next_field().unwrap().text, "M");
        assert_eq!(records.next_field().unwrap().text, "tests/plain.rs");
        assert!(records.next_field().is_none());
    }
}
