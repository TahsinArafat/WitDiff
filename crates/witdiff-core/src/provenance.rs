//! An append-only chain of receipts, so a sequence of verifications can be
//! checked as a sequence rather than one at a time.
//!
//! A single receipt proves that *one* run happened. It says nothing about the
//! runs around it: an operator who has been verifying this repository for a
//! year can be shown a receipt for any one commit, but nothing in the receipt
//! says whether that run is the one that follows the last, or a run somebody
//! produced after deleting the inconvenient ones in between.
//!
//! ## What the chain buys
//!
//! Each entry records the digest of a receipt and the hash of the entry
//! before it, and its own hash covers both:
//!
//! ```text
//! hash_i = SHA-256(prev_hash_{i-1} || receipt_digest_i)
//! ```
//!
//! Editing, dropping, or reordering an entry breaks every link that follows,
//! because the previous hash is carried forward. This is the standard
//! construction and its limit is the usual one: it shows a chain was not
//! modified, not *who* wrote it. Like ADR-0022, this is tamper-evidence.
//!
//! ## What it deliberately does not do
//!
//! It does not replace the receipt signature, and it is not signed. A signature
//! makes a single receipt unforgeable; the chain makes a sequence unbreakable.
//! A consumer who wants both can have both, and neither depends on the other.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The domain separator for a chain entry, so a chain hash can never collide
/// with a verification digest or a workspace fingerprint.
const DOMAIN: &[u8] = b"witdiff.receipt-provenance.v1\0";

/// How many receipts the chain retains.
///
/// Bounded so the file cannot grow without limit on a repository that verifies
/// often. Entries are dropped from the front, which breaks verification of the
/// dropped ones by design — the chain attests to the retained window, and a
/// longer history belongs in the repository's own storage.
const MAX_ENTRIES: usize = 64;

/// One link in the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainEntry {
    /// Zero-based position when this receipt was appended.
    pub sequence: u64,
    /// The `verification_digest` of the receipt this entry records.
    pub receipt_digest: String,
    /// The hash of the entry before this one, or `None` for the first.
    pub previous: Option<String>,
    /// `SHA-256(DOMAIN || previous || receipt_digest)`.
    pub hash: String,
}

impl ChainEntry {
    fn compute(previous: Option<&str>, receipt_digest: &str, sequence: u64) -> String {
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN);
        hasher.update(sequence.to_le_bytes());
        hasher.update(previous.unwrap_or("").as_bytes());
        hasher.update(b"\0");
        hasher.update(receipt_digest.as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// The stored chain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub entries: Vec<ChainEntry>,
}

impl Chain {
    /// Where the chain is kept for a repository.
    pub fn path_for(repo_root: &Path) -> PathBuf {
        repo_root.join(".witdiff").join("provenance.json")
    }

    /// Load the chain, or an empty one when the repository has none.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "the provenance chain at {} is not valid JSON",
                path.display()
            )
        })
    }

    /// Whether every entry hashes correctly and links to the one before it.
    ///
    /// Returns the reason for the first broken link, or `None` when the chain
    /// is intact. Verification is of the *chain*, not of the receipts it names:
    /// the receipt digests are the values being protected.
    pub fn defect(&self) -> Option<String> {
        // Verification starts from the first retained entry's recorded
        // predecessor, not from nothing: entries are dropped from the front to
        // bound the file, and whatever preceded the window is no longer ours to
        // check. Every retained entry after the first must still link to its
        // own predecessor, so any edit inside the window is caught.
        let mut previous = self
            .entries
            .first()
            .and_then(|entry| entry.previous.as_deref());

        for (index, entry) in self.entries.iter().enumerate() {
            if entry.sequence as usize != index + self.first_sequence_offset() {
                return Some(format!(
                    "entry {} claims sequence {}, so an entry was removed or reordered",
                    index, entry.sequence
                ));
            }
            if index > 0 && entry.previous.as_deref() != previous {
                return Some(format!(
                    "entry {} does not link to entry {}; an entry was replaced or removed",
                    index,
                    index - 1
                ));
            }
            let expected = ChainEntry::compute(previous, &entry.receipt_digest, entry.sequence);
            if entry.hash != expected {
                return Some(format!(
                    "entry {} has been modified since it was recorded",
                    index
                ));
            }
            previous = Some(&entry.hash);
        }
        None
    }

    /// How many entries have been trimmed from the front, if any.
    ///
    /// Derived from the first retained entry so a trimmed chain still checks
    /// its own internal links while making no claim about what was dropped.
    fn first_sequence_offset(&self) -> usize {
        self.entries
            .first()
            .map(|entry| entry.sequence as usize)
            .unwrap_or(0)
    }

    /// The hash the next entry will link to, or `None` for an empty chain.
    pub fn tip(&self) -> Option<&str> {
        self.entries.last().map(|entry| entry.hash.as_str())
    }
}

/// Append a receipt digest, returning the entry as recorded.
///
/// Never fails a verification on its own: a chain that cannot be written is a
/// loss of continuity, not a reason to reject evidence that was already
/// gathered. The caller reports it as a note instead.
pub fn append(repo_root: &Path, receipt_digest: &str) -> Result<ChainEntry> {
    let path = Chain::path_for(repo_root);
    let mut chain = Chain::load(&path)?;

    // If the chain already ends with this receipt, re-report the existing
    // entry rather than growing: running verification twice is not two
    // provenance records.
    if let Some(last) = chain.entries.last() {
        if last.receipt_digest == receipt_digest {
            return Ok(last.clone());
        }
    }

    // Not `entries.len()`: entries are dropped from the front, so the next
    // sequence is one past the last *recorded*, not the size of the retained
    // window. Deriving it from the length restarted numbering after the first
    // trim and made every later entry look reordered.
    let sequence = match chain.entries.last() {
        Some(last) => last.sequence + 1,
        None => 0,
    };
    let entry = ChainEntry {
        sequence,
        receipt_digest: receipt_digest.to_owned(),
        previous: chain.tip().map(str::to_owned),
        hash: ChainEntry::compute(chain.tip(), receipt_digest, sequence),
    };
    chain.entries.push(entry.clone());
    if chain.entries.len() > MAX_ENTRIES {
        let excess = chain.entries.len() - MAX_ENTRIES;
        chain.entries.drain(..excess);
        // Retained entries must keep claiming their original sequence numbers,
        // or a dropped head would make every retained entry look reordered.
        // Positions therefore stay fixed and only the window moves.
    }

    let parent = path
        .parent()
        .expect("the chain path has a parent directory");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed creating {}", parent.display()))?;
    let bytes =
        serde_json::to_vec_pretty(&chain).context("failed serializing the provenance chain")?;
    std::fs::write(&path, bytes).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(entry)
}

/// Check that a stored chain is intact.
///
/// `current_digest`, when given, must be the digest of the receipt currently on
/// disk, and must appear as the chain's last entry. That is what ties the chain
/// to the receipt a consumer is holding rather than to history in general.
pub fn verify(path: &Path, current_digest: Option<&str>) -> Result<Vec<String>> {
    let chain = Chain::load(path)?;
    let mut problems = Vec::new();

    if let Some(reason) = chain.defect() {
        problems.push(reason);
    }
    if let (Some(digest), Some(last)) = (current_digest, chain.entries.last()) {
        if last.receipt_digest != digest {
            problems.push(
                "the receipt on disk is not the last entry in the chain; a newer \
                 receipt was replaced or an older one was restored"
                    .to_owned(),
            );
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digests(n: u64) -> Vec<String> {
        (0..n).map(|i| format!("digest-{i}")).collect()
    }

    fn build(repo_root: &Path, digests: &[String]) -> Vec<ChainEntry> {
        digests
            .iter()
            .map(|digest| append(repo_root, digest).expect("append"))
            .collect()
    }

    #[test]
    fn an_intact_chain_verifies() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        build(dir.path(), &digests(3));
        let problems = verify(&Chain::path_for(dir.path()), Some("digest-2")).expect("verify");
        assert!(
            problems.is_empty(),
            "an intact chain must verify: {problems:?}"
        );
    }

    /// The property the construction exists for: altering a recorded digest
    /// breaks the link that follows it.
    #[test]
    fn modifying_an_entry_breaks_the_chain() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = Chain::path_for(dir.path());
        build(dir.path(), &digests(4));

        let mut chain = Chain::load(&path).expect("load");
        // Forge the middle: claim a different receipt was recorded.
        chain.entries[1].receipt_digest = "forged".to_owned();
        std::fs::write(&path, serde_json::to_vec(&chain).expect("serialize")).expect("write");

        let defect = Chain::load(&path).expect("load").defect();
        assert!(
            defect.is_some(),
            "a modified entry must break the chain, but it verified"
        );
    }

    #[test]
    fn dropping_an_entry_breaks_the_chain() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = Chain::path_for(dir.path());
        build(dir.path(), &digests(4));

        let mut chain = Chain::load(&path).expect("load");
        chain.entries.remove(1);
        std::fs::write(&path, serde_json::to_vec(&chain).expect("serialize")).expect("write");

        let defect = Chain::load(&path).expect("load").defect();
        assert!(defect.is_some(), "removing an entry must break the chain");
    }

    #[test]
    fn a_new_receipt_must_be_the_last_entry() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = Chain::path_for(dir.path());
        build(dir.path(), &digests(3));

        let problems = verify(&path, Some("some-older-digest")).expect("verify");
        assert!(
            !problems.is_empty(),
            "a receipt that is not the newest must be reported, got {problems:?}"
        );
    }

    #[test]
    fn a_missing_chain_is_not_a_problems() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let problems = verify(&Chain::path_for(dir.path()), Some("digest")).expect("verify");
        assert!(
            problems.is_empty(),
            "no chain yet is not a defect: {problems:?}"
        );
    }

    /// Re-running verification over an unchanged receipt must not grow the
    /// chain, or a repository that verifies twice per day would accumulate
    /// meaningless links.
    #[test]
    fn re_recording_the_same_receipt_does_not_grow_the_chain() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let first = append(dir.path(), "digest-1").expect("append");
        let again = append(dir.path(), "digest-1").expect("append");
        assert_eq!(first, again);
        let chain = Chain::load(&Chain::path_for(dir.path())).expect("load");
        assert_eq!(chain.entries.len(), 1);
    }

    /// The first entry has no predecessor and must still hash consistently.
    #[test]
    fn an_empty_previous_is_accepted_for_the_first_entry() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        append(dir.path(), "digest-0").expect("append");
        let chain = Chain::load(&Chain::path_for(dir.path())).expect("load");
        assert_eq!(chain.entries[0].previous, None);
        assert_eq!(chain.defect(), None);
    }

    /// A tampered *hash* must be caught as well as a tampered payload.
    #[test]
    fn a_tampered_hash_is_caught() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = Chain::path_for(dir.path());
        build(dir.path(), &digests(3));

        let mut chain = Chain::load(&path).expect("load");
        chain.entries[2].hash = "0".repeat(64);
        std::fs::write(&path, serde_json::to_vec(&chain).expect("serialize")).expect("write");

        let defect = Chain::load(&path).expect("load").defect();
        assert!(defect.is_some(), "a rewritten hash must break the chain");
    }

    #[test]
    fn the_chain_is_bounded_so_it_cannot_grow_forever() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let many: Vec<String> = (0..(MAX_ENTRIES + 10))
            .map(|i| format!("digest-{i}"))
            .collect();
        build(dir.path(), &many);
        let chain = Chain::load(&Chain::path_for(dir.path())).expect("load");
        assert!(
            chain.entries.len() <= MAX_ENTRIES,
            "the chain must stay bounded, got {}",
            chain.entries.len()
        );
    }

    /// Sequence numbers must not restart after trimming, or a retained entry
    /// would look reordered.
    #[test]
    fn sequence_numbers_survive_trimming() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let many: Vec<String> = (0..(MAX_ENTRIES + 5))
            .map(|i| format!("digest-{i}"))
            .collect();
        build(dir.path(), &many);
        let chain = Chain::load(&Chain::path_for(dir.path())).expect("load");
        assert_eq!(chain.defect(), None, "a trimmed chain must still verify");
    }
}
