# ADR-0022: Signed receipts — Ed25519 over the verification digest

Status: accepted

Supersedes the open questions in ADR-0015, which deferred signing until key
management was decided. This ADR decides them.

## Context

ADR-0015 established that a signature over the receipt as originally shaped would
attest that a run happened, not which code was verified: on a clean tree the
workspace fingerprint is exactly SHA-256 of the empty string. It also recorded
two prerequisites — a content digest and a binding from receipt to revision — and
both are now implemented as `verification_digest` (ADR-0019 measurement).

Three questions were left open: what the signature proves, where the key lives,
and which algorithm. They are answered here.

## Decision

1. **Tamper-evidence, not non-repudiation.** The signature proves that a receipt
   describes a particular verification and has not been edited since. It does not
   prove *who* produced it.

   Non-repudiation would require private-key custody, usually a key server or a
   hardware-backed key, and a public identity registry. None of that is WitDiff's
   job, and a signature that invites identity claims WitDiff cannot support is
   worse than one that does not.

   Practical consequence: the same key may sign many repositories, and a verifier
   learns only "this key signed this digest". Identity is the operator's business.

2. **Keys are operator-supplied and never stored by WitDiff.** Configuration
   names a path. WitDiff reads the key, signs, and forgets it. Custody stays
   entirely with the operator: a local file, a CI secret, a mounted hardware key.

   WitDiff never generates a key. A key managed by the tool is a key nobody
   reasons about, and CI would have to persist one deliberately.

3. **Ed25519.** Small, fast, and independently verifiable with standard tooling.
   No key-size or parameter choices to get wrong.

4. **Ed25519 is reached through Node's built-in `crypto`, and only when signing.**
   This is a workaround for the development environment, recorded as such.

   Measured, in order:

   - `ed25519-dalek` resolves against the workspace's `rust-version = 1.78` but
     pulls 68 packages and **cannot be downloaded or compiled here** — the cargo
     cache is outside the sandbox's writable area. Shipping signing code that was
     never compiled or exercised is the exact failure this project exists to
     catch, so it was rejected.
   - `openssl` is present on macOS, but as **LibreSSL**, which does not implement
     Ed25519 at all: `openssl genpkey -algorithm ed25519` reports "Algorithm
     ed25519 not found". The option would work on Linux CI and fail for many
     developers.
   - Node's built-in `crypto` performs Ed25519 sign and verify with **no
     dependency**, verified directly.

   The cost is that Node must be present to sign. That is bounded deliberately:
   Node is required **only** when a caller asks to sign or verify a signature,
   never during ordinary verification. A Rust, Python or Go repository with no
   signature never touches it, which keeps ADR-0021's principle intact — the
   project's own resources are used, and nothing extra is imposed.

   This is an implementation choice, not a design one. The signature covers a
   digest and uses Ed25519 either way, so moving to a Rust crate later is a
   contained change to one module. The constraint that forced it should not
   outlive it.

5. **What is signed.** A detached signature over the domain separator followed
   by the **status and the digest**:
   `witdiff.receipt-signature.v1\0<status>\0<digest>`.

   This corrects a flaw found by measuring the first implementation. Signing the
   digest alone left the receipt's central claim outside the signature: a forged
   receipt whose `status` was edited from `not_verified` to `verified` still
   verified as VALID, because the digest was unchanged. A signature that can be
   carried onto a forged result is worse than none — it lends borrowed authority
   to exactly the edit an attacker wants.

   Including the status is cheap and makes the receipt's headline claim part of
   what the key attests to. The remaining receipt fields are deliberately *not*
   covered: paths, captured output and notes carry no authority, and covering
   them would mean every new field invalidated an existing signature.

6. **Replay.** Because the digest covers the base and head revisions, the
   effective test command, and the content of the changed test files and base
   production source, a signature is only valid for the code it was made over. A
   verifier recomputes the digest and fails on any difference. This is why the
   digest precedes the signature.

## Consequences

- A signed receipt can be checked offline by anyone with the public key and the
  digest, with no network and no key server (invariant 4 preserved).
- The receipt gains an optional `signature` field, additive in v1. A receipt
  without one still parses and behaves exactly as before.
- Signing is opt-in per configuration. Nothing signs implicitly, and no key is
  written anywhere.
- **The key path is a secret location, not a secret.** WitDiff does not read the
  key into a receipt, log it, or copy it. An operator who points at a key inside
  the repository is choosing to publish it, and the docs say so.
- The signature is over the *digest*, not the whole receipt document. A receipt
  can gain fields without invalidating an existing signature, and two receipts
  with the same digest describe the same verification.
- Verification of a signature is separate from `witdiff receipt`: ordinary
  staleness reporting stays key-free and Node-free.
- Node becomes an optional dependency of signing only. On a machine without it,
  signing fails with an explicit message and ordinary verification is unaffected.

## Alternatives considered

- **ed25519-dalek in Rust.** The natural choice, rejected only because it cannot
  be built or tested here. Recorded as the preferred implementation wherever
  dependencies can be fetched.
- **openssl.** Rejected on measurement: LibreSSL on macOS has no Ed25519, so the
  feature would work in CI and fail on developer machines.
- **Ship the key to a key server (sigstore and similar).** Rejected: it answers
  key distribution before the goal is settled, and puts a network dependency near
  the verification path, which invariant 4 forbids.
- **Non-repudiation.** Rejected above; it requires infrastructure WitDiff should
  not operate.
- **Let WitDiff generate keys.** Rejected: key custody stays with the operator.
- **Sign the whole receipt document.** Rejected: adding a field would invalidate
  an existing signature, and the document carries material — paths, captured
  output — that has no reason to be covered.
