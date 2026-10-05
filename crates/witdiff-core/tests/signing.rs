//! Signing and verification of receipts (ADR-0022).
//!
//! These run entirely in Rust. The backend was Node's built-in `crypto` for as
//! long as a dependency could not be fetched; ADR-0022 recorded that as an
//! environment workaround rather than a design choice, and this is that change.
//!
//! The properties asserted here are the ones that must survive the swap, and
//! each exists because its absence was a real defect:
//!
//! - the domain separator is part of the signed bytes;
//! - **the status is part of the signed bytes**, because signing the digest
//!   alone let a receipt forged from `not_verified` to `verified` still verify
//!   as VALID;
//! - a signature made for one status does not verify against another.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use witdiff_core::model::ReceiptSignature;
use witdiff_core::signing::{sign_digest, signed_bytes, verify_signature, DOMAIN_SEPARATOR};

/// A deterministic key pair, generated in Rust so the tests need no external
/// tool and a failure is reproducible.
fn test_keys() -> (SigningKey, VerifyingKey) {
    let seed: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    let signing = SigningKey::from_bytes(&seed);
    let verifying = signing.verifying_key();
    (signing, verifying)
}

/// A signing key file (the 32-byte seed) and its matching public key file.
///
/// They are separate files on purpose: verifying must not require private key
/// material.
fn write_key_pair(directory: &std::path::Path) -> (PathBuf, PathBuf) {
    let (signing, verifying) = test_keys();
    let private = directory.join("signing.key");
    let public = directory.join("verifying.key");
    std::fs::write(&private, signing.to_bytes()).expect("write seed");
    std::fs::write(&public, verifying.to_bytes()).expect("write public key");
    (private, public)
}

fn write_public_key(directory: &std::path::Path, name: &str, seed: &[u8; 32]) -> PathBuf {
    let (signing, _) = test_keys_from(seed);
    let path = directory.join(name);
    std::fs::write(&path, signing.verifying_key().to_bytes()).expect("write key");
    path
}

fn test_keys_from(seed: &[u8; 32]) -> (SigningKey, VerifyingKey) {
    let signing = SigningKey::from_bytes(seed);
    let verifying = signing.verifying_key();
    (signing, verifying)
}

#[test]
fn a_signature_round_trips() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (key_path, public_key) = write_key_pair(dir.path());

    let signature = sign_digest("verified", "abc123", &key_path).expect("sign");
    assert_eq!(signature.algorithm, "ed25519");
    assert_eq!(signature.digest, "abc123");

    assert!(
        verify_signature("verified", "abc123", &signature, &public_key).expect("verify"),
        "a signature over the same status and digest must verify"
    );

    // Real Ed25519, produced and checked with no helper process: the detached
    // signature verifies against the documented bytes using only dalek.
    let (_, verifying) = test_keys();
    let raw = STANDARD.decode(&signature.value).expect("base64");
    assert_eq!(raw.len(), 64, "an Ed25519 signature is 64 bytes");
    let parsed = Signature::from_slice(&raw).expect("a 64-byte signature parses");
    assert!(
        verifying
            .verify(&signed_bytes("verified", "abc123"), &parsed)
            .is_ok(),
        "the signature must verify as plain Ed25519 over the documented bytes"
    );
}

#[test]
fn the_domain_separator_is_the_documented_one() {
    assert_eq!(DOMAIN_SEPARATOR, b"witdiff.receipt-signature.v1\0");
}

/// The defect ADR-0022 exists because of: a receipt whose `status` is edited
/// from `not_verified` to `verified` must NOT still verify.
#[test]
fn a_forged_status_does_not_verify() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (key_path, public_key) = write_key_pair(dir.path());

    let digest = "9f2c";
    let honest = sign_digest("not_verified", digest, &key_path).expect("sign");

    assert!(
        verify_signature("not_verified", digest, &honest, &public_key).expect("verify"),
        "the honest receipt must verify"
    );
    assert!(
        !verify_signature("verified", digest, &honest, &public_key).expect("verify"),
        "editing the status to `verified` must invalidate the signature; a \
         signature that survives it lends borrowed authority to the forgery"
    );
}

/// The status must not be separable from the digest: an attacker who can move
/// bytes must not be able to re-interpret a signature over one pairing as a
/// signature over another.
#[test]
fn a_signature_cannot_be_moved_to_another_digest() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (key_path, public_key) = write_key_pair(dir.path());

    let signature = sign_digest("verified", "digest-a", &key_path).expect("sign");
    assert!(
        !verify_signature("verified", "digest-b", &signature, &public_key).expect("verify"),
        "a signature must not verify against a different digest"
    );
}

/// The signed bytes must actually be the documented layout. If the separator
/// were dropped, a signature over some other artifact hashing to the same
/// digest would be accepted.
#[test]
fn the_signed_bytes_carry_the_separator_the_status_and_the_digest() {
    let payload = signed_bytes("verified", "abc123");
    assert!(
        payload.starts_with(DOMAIN_SEPARATOR),
        "the payload must begin with the domain separator"
    );
    assert!(
        payload.ends_with(b"verified\0abc123"),
        "the status and digest must follow the separator, got {:?}",
        String::from_utf8_lossy(&payload)
    );

    // Tie the layout above to what the verifier actually uses.
    let (signing, verifying) = test_keys();
    let signature = signing.sign(&payload);
    assert!(verifying.verify(&payload, &signature).is_ok());
}

/// A different key must not verify. Otherwise any key would do.
#[test]
fn another_key_does_not_verify() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (key_path, public_key) = write_key_pair(dir.path());

    let signature = sign_digest("verified", "abc123", &key_path).expect("sign");

    let other_path = write_public_key(dir.path(), "other.key", &[7u8; 32]);

    assert!(
        verify_signature("verified", "abc123", &signature, &public_key).expect("verify"),
        "the matching public key must verify, otherwise the next assertion is vacuous"
    );
    assert!(
        !verify_signature("verified", "abc123", &signature, &other_path).expect("verify"),
        "a signature must not verify under an unrelated key"
    );
}

/// Tampering with the signature must invalidate it.
#[test]
fn a_tampered_signature_does_not_verify() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (key_path, public_key) = write_key_pair(dir.path());

    let signature = sign_digest("verified", "abc123", &key_path).expect("sign");
    let mut raw = STANDARD.decode(&signature.value).expect("base64");
    raw[0] ^= 0x01;
    let tampered = ReceiptSignature {
        algorithm: "ed25519".to_owned(),
        value: STANDARD.encode(&raw),
        digest: "abc123".to_owned(),
    };
    assert!(
        !verify_signature("verified", "abc123", &tampered, &public_key).expect("verify"),
        "a modified signature must not verify"
    );
}

/// Signing must never happen implicitly, and a missing key must be refused
/// rather than created.
#[test]
fn a_missing_key_is_refused_rather_than_created() {
    let missing = std::path::Path::new("/nonexistent/witdiff-signing-key.pem");
    let error = sign_digest("verified", "abc", missing).expect_err("a missing key must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("never creates one"),
        "the error should say WitDiff does not create keys, got {message}"
    );
}

/// An algorithm WitDiff does not implement must be refused, not ignored:
/// accepting it would suggest a verification that never happened.
#[test]
fn an_unknown_algorithm_is_refused() {
    let signature = ReceiptSignature {
        algorithm: "rsa-sha256".to_owned(),
        value: "AAAA".to_owned(),
        digest: "abc".to_owned(),
    };
    let error = verify_signature(
        "verified",
        "abc",
        &signature,
        std::path::Path::new("/nonexistent"),
    )
    .expect_err("an unknown algorithm must fail");
    assert!(format!("{error:#}").contains("unsupported signature algorithm"));
}

/// A key file that is not a key must be reported with the path, so an operator
/// can tell which file is wrong.
#[test]
fn an_unusable_key_is_reported_with_its_path() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("not-a-key.pem");
    std::fs::write(&path, b"this is not a key").expect("write");
    let error = sign_digest("verified", "abc", &path).expect_err("a bad key must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("not-a-key.pem"),
        "the error should name the key file, got {message}"
    );
}
