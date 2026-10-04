//! A content digest over the material facts of a verification.
//!
//! See ADR-0015. This exists because a signature is only as meaningful as what
//! it covers, and the receipt as originally shaped had nothing that identified
//! the verified inputs.
//!
//! ## Why the workspace fingerprint is not enough
//!
//! [`crate::git::GitRepo::workspace_fingerprint`] hashes the diff against the
//! base revision, the working-tree status, and the contents of untracked files.
//! On a clean tree all three are empty, so the fingerprint is exactly SHA-256 of
//! the empty string. Measured: two repositories, one containing `f() -> 1` and
//! the other `f() -> 999`, produced the identical fingerprint.
//!
//! That is correct for what the fingerprint is *for* — detecting movement during
//! a run — but it cannot distinguish one clean tree from another. A signature
//! over it would attest that a run happened, not which code was verified.
//!
//! ## What this digest covers
//!
//! The inputs that determine the outcome:
//!
//! - the base and head revisions;
//! - the exact test command that ran;
//! - the content of every changed test file at the head revision;
//! - the content of the production source the base worktree received, which is
//!   what makes the base transplant reproducible.
//!
//! The digest is over *content*, not over metadata, so two checkouts of the same
//! revision in different directories produce the same digest and the same
//! revision in two directories does not.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::git::GitRepo;

/// The digest of one verification's inputs.
///
/// Hex-encoded, so it is stable in a receipt and comparable across machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationDigest(String);

impl VerificationDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Build a digest from already-collected material.
    ///
    /// Exposed for tests, which need to construct a digest without a repository.
    pub fn from_parts(parts: &DigestParts) -> Self {
        let mut hasher = Sha256::new();
        // A domain separator, so this digest can never collide with a
        // fingerprint or any other hash WitDiff computes.
        hasher.update(b"witdiff.verification-digest.v1\0");
        hasher.update(parts.base.as_bytes());
        hasher.update(b"\0");
        hasher.update(parts.head_commit.as_bytes());
        hasher.update(b"\0");
        for argument in &parts.test_command {
            hasher.update(argument.as_bytes());
            hasher.update(b"\0");
        }
        // Sorted, so the digest does not depend on iteration order.
        for (path, content_digest) in &parts.inputs {
            hasher.update(path.as_bytes());
            hasher.update(b"\0");
            hasher.update(content_digest.as_bytes());
            hasher.update(b"\0");
        }
        Self(hex::encode(hasher.finalize()))
    }
}

/// The material a [`VerificationDigest`] covers.
#[derive(Debug, Clone, Default)]
pub struct DigestParts {
    pub base: String,
    pub head_commit: String,
    pub test_command: Vec<String>,
    /// Repository-relative path to a digest of that path's content.
    pub inputs: BTreeMap<String, String>,
}

/// Hash one piece of content.
fn content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Collect the digest inputs for a verification.
///
/// `test_paths` are the changed test files, read from the working tree because
/// that is the revision being verified. `production_paths` are read from the
/// base revision, because those are what the base worktree received.
pub fn collect(
    repo: &GitRepo,
    base: &str,
    head_commit: &str,
    test_command: &[String],
    test_paths: &[String],
    production_paths: &[String],
) -> Result<VerificationDigest> {
    let mut parts = DigestParts {
        base: base.to_owned(),
        head_commit: head_commit.to_owned(),
        test_command: test_command.to_vec(),
        inputs: BTreeMap::new(),
    };

    for path in test_paths {
        // The head revision is the working tree: the change under verification
        // is normally uncommitted.
        if let Some(source) = repo.worktree_source(path)? {
            parts
                .inputs
                .insert(format!("head:{path}"), content_digest(&source));
        }
    }

    for path in production_paths {
        // Production source comes from the base revision, since that is what
        // the base worktree was given and therefore what the proof rests on.
        if let Some(source) = repo.show_file_at(base, path)? {
            parts
                .inputs
                .insert(format!("base:{path}"), content_digest(&source));
        }
    }

    Ok(VerificationDigest::from_parts(&parts))
}

/// Whether a path is inside a directory that should not contribute to a digest.
///
/// Build output changes for reasons unrelated to the change and would make the
/// digest unstable between runs on the same source.
pub fn is_ignorable(path: &Path) -> bool {
    const IGNORED: [&str; 6] = [
        "target",
        "node_modules",
        ".venv",
        "__pycache__",
        ".git",
        ".witdiff",
    ];
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| IGNORED.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> DigestParts {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "head:tests/a.rs".to_owned(),
            content_digest("assert_eq!(x, 1);"),
        );
        DigestParts {
            base: "origin/main".to_owned(),
            head_commit: "abc123".to_owned(),
            test_command: vec!["cargo".to_owned(), "test".to_owned()],
            inputs,
        }
    }

    #[test]
    fn the_same_inputs_produce_the_same_digest() {
        assert_eq!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&parts())
        );
    }

    /// The property the whole design rests on: different code must produce a
    /// different digest, or a signature would be replayable across revisions.
    #[test]
    fn different_test_content_changes_the_digest() {
        let mut changed = parts();
        changed.inputs.insert(
            "head:tests/a.rs".to_owned(),
            content_digest("assert_eq!(x, 2);"),
        );
        assert_ne!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&changed),
            "a different assertion must change the digest"
        );
    }

    #[test]
    fn a_different_revision_changes_the_digest() {
        let mut changed = parts();
        changed.head_commit = "def456".to_owned();
        assert_ne!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&changed)
        );
    }

    /// A different command can produce a different result, so it is material.
    #[test]
    fn a_different_test_command_changes_the_digest() {
        let mut changed = parts();
        changed.test_command = vec![
            "cargo".to_owned(),
            "test".to_owned(),
            "--release".to_owned(),
        ];
        assert_ne!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&changed)
        );
    }

    #[test]
    fn a_different_base_changes_the_digest() {
        let mut changed = parts();
        changed.base = "HEAD~1".to_owned();
        assert_ne!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&changed)
        );
    }

    /// Adding a path changes the digest even if the existing ones are unchanged.
    #[test]
    fn an_added_input_changes_the_digest() {
        let mut changed = parts();
        changed
            .inputs
            .insert("base:src/lib.rs".to_owned(), content_digest("fn f() {}"));
        assert_ne!(
            VerificationDigest::from_parts(&parts()),
            VerificationDigest::from_parts(&changed)
        );
    }

    /// The digest is a domain-separated SHA-256, so it must not equal the hash
    /// of any single input, which is what makes it distinguishable from the
    /// workspace fingerprint.
    #[test]
    fn the_digest_is_not_a_bare_content_hash() {
        let digest = VerificationDigest::from_parts(&parts());
        assert_eq!(digest.as_str().len(), 64, "SHA-256 in hex");
        assert_ne!(digest.as_str(), content_digest(""));
        assert_ne!(digest.as_str(), content_digest("abc123"));
    }

    /// Field boundaries must be unambiguous, or two different inputs could
    /// concatenate to the same bytes.
    #[test]
    fn field_boundaries_are_unambiguous() {
        let mut left = parts();
        left.base = "ab".to_owned();
        left.head_commit = "c".to_owned();

        let mut right = parts();
        right.base = "a".to_owned();
        right.head_commit = "bc".to_owned();

        assert_ne!(
            VerificationDigest::from_parts(&left),
            VerificationDigest::from_parts(&right),
            "without separators, `ab`+`c` and `a`+`bc` would collide"
        );
    }

    #[test]
    fn build_output_is_ignorable() {
        assert!(is_ignorable(Path::new("target/debug/x")));
        assert!(is_ignorable(Path::new("node_modules/a/index.js")));
        assert!(is_ignorable(Path::new(".witdiff/receipt.json")));
        assert!(!is_ignorable(Path::new("src/lib.rs")));
        assert!(!is_ignorable(Path::new("tests/a.rs")));
        // A name that merely contains an ignored one must not match.
        assert!(!is_ignorable(Path::new("src/targeting.rs")));
    }
}
