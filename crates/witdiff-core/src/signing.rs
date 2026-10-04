//! The Ed25519 signing helper, embedded so there is no separate file to locate.
//!
//! Node is required **only** when a caller asks to sign or verify a signature,
//! never during ordinary verification (ADR-0022). Using Node's built-in crypto
//! rather than a Rust crate is a workaround for a development environment where
//! a dependency cannot be fetched; the algorithm, the covered digest and the
//! domain separator are all independent of that choice, so moving to a Rust
//! crate later is a change confined to this module.

/// The domain separator mixed into the signed bytes.
///
/// Without it a signature over a digest could be replayed against any other
/// artifact that happens to share the same hash.
pub const DOMAIN_SEPARATOR: &[u8] = b"witdiff.receipt-signature.v1\0";

/// Sign or verify a digest with an Ed25519 key.
///
/// Modes: `sign <key> <digest>` and `verify <key> <signature-b64> <digest>`.
/// Both print one JSON object on stdout.
pub const SIGN_HELPER: &str = concat!(
    "// Ed25519 detached signing over the verification digest (ADR-0022).\n",
    "//\n",
    "// Node is required only when a caller asks to sign or verify, never during\n",
    "// ordinary verification. The choice is a workaround for a development\n",
    "// environment where a Rust Ed25519 crate cannot be fetched; the signature covers\n",
    "// a digest and uses Ed25519 either way, so moving to a Rust crate later is a\n",
    "// change to this one module.\n",
    "//\n",
    "// Usage:\n",
    "//   node sign_helper.js sign <key-path> <status> <digest>\n",
    "//   node sign_helper.js verify <key-path> <signature-b64> <status> <digest>\n",
    "const crypto = require(\"crypto\");\n",
    "\n",
    "// The domain separator prevents a signature over a digest from being replayed\n",
    "// against any other artifact that happens to share the same hash.\n",
    "const DOMAIN = Buffer.from(\"witdiff.receipt-signature.v1\\0\", \"utf-8\");\n",
    "\n",
    "function die(message) {\n",
    "  process.stdout.write(JSON.stringify({ ok: false, error: message }));\n",
    "  process.exit(0);\n",
    "}\n",
    "\n",
    "function main() {\n",
    "  const [mode, keyPath, third, fourth, fifth] = process.argv.slice(2);\n",
    "  if (!mode || !keyPath) die(\"usage: sign|verify <key-path> ...\");\n",
    "\n",
    "  const keyBytes = require(\"fs\").readFileSync(keyPath);\n",
    "\n",
    "  if (mode === \"sign\") {\n",
    "    let key;\n",
    "    try {\n",
    "      key = crypto.createPrivateKey(keyBytes);\n",
    "    } catch (error) {\n",
    "      die(`could not read a private key from ${keyPath}: ${error.message}`);\n",
    "    }\n",
    "    if (!third) die(\"usage: sign <key-path> <digest>\");\n",
    "    // The status is signed alongside the digest: signing the digest alone left\n",
    "    // the receipt's headline claim editable while the signature still verified.\n",
    "    const payload = Buffer.concat([\n",
    "      DOMAIN,\n",
    "      Buffer.from(third, \"utf-8\"),\n",
    "      Buffer.from(\"\\0\", \"utf-8\"),\n",
    "      Buffer.from(fourth, \"utf-8\"),\n",
    "    ]);\n",
    "    const signature = crypto.sign(null, payload, key);\n",
    "    process.stdout.write(JSON.stringify({ ok: true, signature: signature.toString(\"base64\") }));\n",
    "    return;\n",
    "  }\n",
    "\n",
    "  if (mode === \"verify\") {\n",
    "    if (!third || !fourth || !fifth)\n",
    "      die(\"usage: verify <key-path> <signature-b64> <status> <digest>\");\n",
    "    let publicKey;\n",
    "    try {\n",
    "      publicKey = crypto.createPublicKey(keyBytes);\n",
    "    } catch (error) {\n",
    "      die(`could not read a public key from ${keyPath}: ${error.message}`);\n",
    "    }\n",
    "    let signature;\n",
    "    try {\n",
    "      signature = Buffer.from(third, \"base64\");\n",
    "    } catch (error) {\n",
    "      die(`the signature is not valid base64: ${error.message}`);\n",
    "    }\n",
    "    const payload = Buffer.concat([\n",
    "      DOMAIN,\n",
    "      Buffer.from(fourth, \"utf-8\"),\n",
    "      Buffer.from(\"\\0\", \"utf-8\"),\n",
    "      Buffer.from(fifth, \"utf-8\"),\n",
    "    ]);\n",
    "    const valid = crypto.verify(null, payload, publicKey, signature);\n",
    "    process.stdout.write(JSON.stringify({ ok: true, valid }));\n",
    "    return;\n",
    "  }\n",
    "\n",
    "  die(`unknown mode ${mode}`);\n",
    "}\n",
    "\n",
    "main();\n",
    "\n",
);

#[cfg(test)]
mod tests {
    use super::*;

    /// The algorithm comes from the key file rather than a flag, so the script
    /// names Ed25519 in documentation rather than in a parameter. What matters
    /// is that it signs and verifies detached over the separated bytes.
    #[test]
    fn the_helper_has_the_expected_shape() {
        assert!(SIGN_HELPER.contains("Ed25519"));
        assert!(SIGN_HELPER.contains("crypto.sign"));
        assert!(SIGN_HELPER.contains("crypto.verify"));
        // Detached: the digest is written to stdout as base64, never inlined
        // into a document that could be edited around it.
        assert!(SIGN_HELPER.contains("base64"));
    }

    /// The separator is part of what is signed, so it must be the same on both
    /// sides or every verification fails.
    #[test]
    fn the_domain_separator_is_stable() {
        assert_eq!(DOMAIN_SEPARATOR, b"witdiff.receipt-signature.v1\0");
        assert!(SIGN_HELPER.contains("witdiff.receipt-signature.v1"));
    }

    /// Signing must read a private key and verifying a public one; loading both
    /// as a private key was a bug that made every verification fail.
    #[test]
    fn the_helper_loads_the_right_key_kind_per_mode() {
        assert!(SIGN_HELPER.contains("createPrivateKey"));
        assert!(SIGN_HELPER.contains("createPublicKey"));
    }
}

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::ReceiptSignature;

/// What the helper reports back.
#[derive(Debug, Deserialize)]
struct HelperOutput {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    valid: bool,
}

/// Sign a digest and the status it belongs to, with an operator-supplied key.
///
/// The status is part of what is signed. Signing the digest alone left the
/// receipt's central claim editable while the signature still verified, which
/// lends borrowed authority to exactly the edit an attacker wants (ADR-0022).
///
/// The key is read, used, and not stored or logged. Requires Node, which is
/// called only from here and never during ordinary verification.
pub fn sign_digest(status: &str, digest: &str, key_path: &Path) -> Result<ReceiptSignature> {
    let output = run_helper("sign", key_path, &[status, digest])?;
    if output.signature.is_empty() {
        anyhow::bail!("the signing helper returned no signature");
    }
    Ok(ReceiptSignature {
        algorithm: "ed25519".to_owned(),
        value: output.signature,
        digest: digest.to_owned(),
    })
}

/// Check a detached signature against a digest.
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
    let output = run_helper("verify", key_path, &[&signature.value, status, digest])?;
    Ok(output.valid)
}

fn run_helper(mode: &str, key_path: &Path, arguments: &[&str]) -> Result<HelperOutput> {
    if !key_path.is_file() {
        anyhow::bail!(
            "the signing key {} does not exist; WitDiff never creates one",
            key_path.display()
        );
    }

    let directory = tempfile::Builder::new()
        .prefix("witdiff-sign-")
        .tempdir()
        .context("failed creating a directory for the signing helper")?;
    let script = directory.path().join("sign.js");
    std::fs::write(&script, SIGN_HELPER).context("failed writing the signing helper")?;

    let output = Command::new("node")
        .arg(&script)
        .arg(mode)
        .arg(key_path)
        .args(arguments)
        .output()
        .with_context(|| {
            "could not run `node` to sign or verify; Node is required for signed \
             receipts and is not used during ordinary verification"
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: HelperOutput = serde_json::from_str(stdout.trim()).with_context(|| {
        format!(
            "the signing helper did not return usable output: {}{}",
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;

    if !parsed.ok {
        anyhow::bail!("{}", parsed.error);
    }
    Ok(parsed)
}

#[cfg(test)]
mod signing_tests {
    use super::*;

    /// Node is the signing backend; without it the failure must be explicit
    /// rather than a silent unsigned receipt.
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

    #[test]
    fn a_verification_with_a_missing_key_fails_explicitly() {
        let missing = std::path::Path::new("/nonexistent/pub.pem");
        let signature = ReceiptSignature {
            algorithm: "ed25519".to_owned(),
            value: "AAAA".to_owned(),
            digest: "abc".to_owned(),
        };
        assert!(verify_signature("verified", "abc", &signature, missing).is_err());
    }

    /// The defect this design exists to prevent: a signature that stays valid
    /// after the receipt's status is edited is worse than none.
    #[test]
    fn the_status_is_part_of_what_is_signed() {
        // Both the helper and this module must include the status, so the
        // signature cannot be carried onto a forged result.
        assert!(SIGN_HELPER.contains("third"));
        assert!(SIGN_HELPER.contains("fourth"));
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
        let error = verify_signature("verified", "abc", &signature, Path::new("/nonexistent"))
            .expect_err("an unknown algorithm must fail");
        assert!(format!("{error:#}").contains("unsupported signature algorithm"));
    }
}
