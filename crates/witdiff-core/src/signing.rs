//! Ed25519 signing over the verification digest (ADR-0022).
//!
//! Ed25519 comes from `ed25519-dalek`. An earlier revision reached Ed25519
//! through Node's built-in `crypto`, recorded in ADR-0022 as an environment
//! workaround rather than a design choice: the cargo cache could not be written
//! in the development environment, so the crate could not be fetched or
//! exercised. It could be added once that was resolved, and Node is no longer
//! required to sign or verify.
//!
//! What the backend must not change, because it is what the signature means:
//!
//! - the domain separator;
//! - the status being inside the signed bytes, not merely the digest.
//!
//! Both are covered by tests in `tests/signing.rs`.

use std::path::Path;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::model::ReceiptSignature;

/// The domain separator mixed into the signed bytes.
///
/// Without it a signature over a digest could be replayed against any other
/// artifact that happens to share the same hash.
pub const DOMAIN_SEPARATOR: &[u8] = b"witdiff.receipt-signature.v1\0";

/// The exact bytes a receipt signature covers:
/// `witdiff.receipt-signature.v1\0<status>\0<digest>`.
///
/// The status is part of what is signed, not an afterthought. Signing the
/// digest alone left the receipt's headline claim editable while the signature
/// still verified, so a receipt forged from `not_verified` to `verified` was
/// accepted as VALID — a signature that lends borrowed authority to exactly the
/// edit an attacker wants.
pub fn signed_bytes(status: &str, digest: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(DOMAIN_SEPARATOR.len() + status.len() + digest.len() + 2);
    payload.extend_from_slice(DOMAIN_SEPARATOR);
    payload.extend_from_slice(status.as_bytes());
    payload.push(0);
    payload.extend_from_slice(digest.as_bytes());
    payload
}

/// Read a 32-byte Ed25519 seed from an operator-supplied key file.
///
/// WitDiff never creates, stores or logs a key; this reads and discards.
fn read_seed(path: &Path) -> Result<SigningKey> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read the signing key {}", path.display()))?;
    let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        anyhow::anyhow!(
            "the signing key {} must be exactly 32 bytes of Ed25519 seed material, got {}",
            path.display(),
            bytes.len()
        )
    })?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The public half of a key file: 32 bytes of Ed25519 public key material.
fn public_key_from(path: &Path) -> Result<VerifyingKey> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read the verifying key {}", path.display()))?;
    if bytes.len() != 32 {
        anyhow::bail!(
            "the verifying key {} must be 32 bytes of Ed25519 public key material, got {}",
            path.display(),
            bytes.len()
        );
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    VerifyingKey::from_bytes(&key)
        .with_context(|| format!("{} is not a valid Ed25519 public key", path.display()))
}

/// Sign a digest and the status it belongs to, with an operator-supplied key.
///
/// The status is part of what is signed (ADR-0022). The key is read, used, and
/// not stored or logged.
pub fn sign_digest(status: &str, digest: &str, key_path: &Path) -> Result<ReceiptSignature> {
    if !key_path.is_file() {
        anyhow::bail!(
            "the signing key {} does not exist; WitDiff never creates one",
            key_path.display()
        );
    }
    let signing = read_seed(key_path)?;
    let signature: Signature = signing.sign(&signed_bytes(status, digest));
    Ok(ReceiptSignature {
        algorithm: "ed25519".to_owned(),
        value: STANDARD.encode(signature.to_bytes()),
        digest: digest.to_owned(),
    })
}

/// Check a detached signature against a digest and the status it claims.
///
/// The key file holds the **public** half. Verifying must not require private
/// key material, and accepting a seed here would hide a missing public key
/// behind a file that happens to hold one.
pub fn verify_signature(
    status: &str,
    digest: &str,
    signature: &ReceiptSignature,
    key_path: &Path,
) -> Result<bool> {
    if signature.algorithm != "ed25519" {
        anyhow::bail!(
            "unsupported signature algorithm `{}`; expected ed25519",
            signature.algorithm
        );
    }
    if !key_path.is_file() {
        anyhow::bail!(
            "the verifying key {} does not exist; WitDiff never creates one",
            key_path.display()
        );
    }

    let raw = STANDARD
        .decode(signature.value.as_bytes())
        .with_context(|| format!("the signature is not valid base64: {}", signature.value))?;
    let parsed =
        Signature::from_slice(&raw).context("the signature is not a 64-byte Ed25519 signature")?;

    let verifying = public_key_from(key_path)?;
    Ok(verifying
        .verify(&signed_bytes(status, digest), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> [u8; 32] {
        [42u8; 32]
    }

    #[test]
    fn the_payload_is_the_documented_byte_sequence() {
        let payload = signed_bytes("verified", "abc");
        assert_eq!(
            payload,
            b"witdiff.receipt-signature.v1\0verified\0abc".to_vec()
        );
    }

    /// The separator keeps a signature over a digest from being replayed
    /// against any other artifact hashing to the same value.
    #[test]
    fn a_signature_over_one_artifact_does_not_verify_against_another() {
        let signing = SigningKey::from_bytes(&seed());
        let signature: Signature = signing.sign(&signed_bytes("verified", "abc"));
        // Same digest, different bytes entirely.
        let other: Signature = signing.sign(b"some other artifact");
        let verifying = signing.verifying_key();
        assert!(verifying
            .verify(&signed_bytes("verified", "abc"), &signature)
            .is_ok());
        assert!(verifying.verify(b"some other artifact", &other).is_ok());
        assert!(verifying
            .verify(b"witdiff.receipt-signature.v1\0verified\0abc", &other)
            .is_err());
    }

    /// The status and digest are separated by a NUL, so neither can be moved
    /// across the boundary and re-read as the other.
    #[test]
    fn the_status_and_digest_cannot_be_swapped() {
        let signing = SigningKey::from_bytes(&seed());
        let signature: Signature = signing.sign(&signed_bytes("verified", "abc"));
        let verifying = signing.verifying_key();
        // A crafted pairing that would be identical if the separator were
        // missing from the middle.
        assert!(verifying
            .verify(b"witdiff.receipt-signature.v1\0verifiedabc\0", &signature)
            .is_err());
    }

    #[test]
    fn a_missing_key_is_refused_rather_than_created() {
        let missing = Path::new("/nonexistent/witdiff-signing-key.pem");
        let error = sign_digest("verified", "abc", missing).expect_err("must fail");
        assert!(format!("{error:#}").contains("never creates one"));
    }

    #[test]
    fn an_unusable_key_names_the_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("bad.key");
        std::fs::write(&path, b"short").expect("write");
        let error = sign_digest("verified", "abc", &path).expect_err("must fail");
        let message = format!("{error:#}");
        assert!(message.contains("bad.key"), "got {message}");
        assert!(message.contains("32 bytes"), "got {message}");
    }
}
