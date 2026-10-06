# Receipt contract

The current schema ID is `witdiff.receipt.v1`.

The receipt records:

- generation time;
- base and head identities;
- exact workspace fingerprints before/after;
- changed files and changed tests;
- integrity findings;
- which test command variant produced the evidence, and the exact command used;
- HEAD command/evidence;
- pristine BASE control command/evidence;
- BASE+changed-tests command/evidence;
- red/green decision;
- final conservative status;
- a machine-readable `reason` naming *why* that status was reached;
- human-readable notes for unsupported/ambiguous cases.

Consumers should prefer enum/status fields over parsing notes or stdout.

## `reason`: why, not just what

`status` says what happened. `reason` says why, as one stable token, because
`not_verified` alone covers several unrelated causes with different remedies:

| `reason` | What happened | What to do |
| --- | --- | --- |
| `proven` | The changed tests distinguish the two revisions | Nothing |
| `proven_with_integrity_warnings` | Proven, but the change weakened its own tests | Read the findings |
| `base_also_passes` | The tests pass on the base too, so they pin nothing | Strengthen the assertion |
| `base_control_failed` | The pristine base does not pass the command | Fix or re-target the base |
| `base_control_timed_out` | The base control exceeded its timeout | Raise `timeout_secs` or fix the hang |
| `test_did_not_compile_on_base` | The transplanted test needs an API this change adds | Split the change |
| `experiment_unstable` | A run disagreed with itself | Find the flake |
| `blocked_by_integrity_finding` | Behaviour was proven but a `High` finding blocks it | Address the finding |
| `partial_transplant` | Some changed tests could not be transplanted | Read `notes` for which |
| `base_experiment_timed_out` | The base+test run did not terminate | Fix the hang |
| `unrecognized_failure_kind` | The failure is not one the framework's classifier knows | Check `framework`, or the runner's output vocabulary |
| `head_tests_failed` | The command already fails on this workspace | Fix that first |
| `test_command_unavailable` | The command could not be started | Install the toolchain, or configure `base_dependency_dirs` |
| `no_dedicated_tests_changed` | No test file changed, so nothing was attempted | Not a failure; gate with `--fail-on-no-changed-tests` |
| `unrecorded` | A receipt written before this field existed | Nothing; the field claims nothing |

An agent should branch on this token rather than matching the prose in `notes`:
the notes are for humans and their wording is not part of the contract. The MCP
`verify` tool returns `reason`, `reason_is_actionable_by_author` and
`remediation` directly, so a client needs no parsing at all.

`unrecorded` is the default for a receipt written before the field existed. It
deliberately claims nothing, rather than guessing at a reason from the status.

See `schemas/witdiff.receipt.v1.schema.json`.

Changes made after the first release are additive and optional within
`witdiff.receipt.v1`. There is no v2 and no breaking change: a receipt written
before a field existed still deserializes, and each new field defaults to the
value that matches how such receipts were actually produced.

## The verification digest

`verification_digest` identifies *which* code was verified. It covers the base
and head revisions, the effective test command, and the content of the changed
test files and the base production source.

It is not the same as `workspace_fingerprint`, and the difference matters:

| | `workspace_fingerprint` | `verification_digest` |
| --- | --- | --- |
| Answers | did anything move during the run? | which inputs were verified? |
| On a clean tree | SHA-256 of the empty string | differs per content |
| Changes when a test is edited | only if uncommitted changes exist | always |

Measured: two repositories, one containing `f() -> 1` and the other
`f() -> 999`, produce the identical fingerprint `e3b0c44298fc1c14` and different
digests. So the fingerprint cannot distinguish one clean tree from another, and
the digest can.

Two receipts with the same `verification_digest` describe the same verified
inputs. That is checkable without any key material.

## Environment

The receipt records the environment that produced it: the configured test
program and its version, the toolchain that participates in it, and a digest of
each dependency manifest present (`Cargo.lock`, `package-lock.json`, `go.sum`,
`Gemfile.lock`, and so on).

This answers a question the digest deliberately does not. The digest says *what*
was verified; the environment says *where*. A proof that holds under Python 3.9
and pytest 8.4 is a different claim than the same revision under Python 3.12 and
pytest 9, and until this field existed the receipt could not tell them apart.

Two deliberate boundaries:

- **It is not folded into the verification digest.** That digest is recomputed by
  `witdiff receipt` against a stored receipt, and folding the environment in
  would make every older receipt report that its inputs changed the moment
  Python was upgraded — a false staleness warning on entirely unchanged
  evidence. The two answer different questions and are recorded separately.
- **It is not signed**, for the same reason the other receipt fields are not.
  ADR-0022 keeps signed bytes to the status and the digest so that adding a
  field cannot invalidate an existing signature. The environment is descriptive
  evidence for a reader and carries no authority on its own.

Manifests are digested rather than copied, so a consumer can answer "did the
dependencies change?" without receiving anyone's dependency list.

## Signatures

A receipt can carry a detached Ed25519 signature. It proves two things: that the
receipt has not been edited since the run, and that it describes the code the
digest names. It does **not** prove who produced it — that is tamper-evidence,
not non-repudiation (ADR-0022).

```toml
[verification]
signing_key = "/run/secrets/witdiff-signing-key.pem"
```

WitDiff reads the key, signs, and forgets it. It never creates, stores, copies or
logs a key, and it never writes one into the repository. **The key path is a
secret location, not a secret value** — pointing at a key inside the repository is
publishing it.

The signature covers the domain separator, the status, and the digest:

```text
witdiff.receipt-signature.v1\0<status>\0<digest>
```

The status is included because signing the digest alone left the receipt's
headline claim outside the signature: a receipt edited from `not_verified` to
`verified` still verified, because the digest was unchanged. That is exactly the
borrowed authority this project exists to refuse.

Verifying is a separate, key-free concern in the common case: `witdiff receipt`
reports whether a receipt's signature covers its own digest, and cryptography
requires the public key, which is the operator's to supply.

If signing fails — an unreadable key, no digest — the receipt is still written,
unsigned, and a note says so. An unsigned receipt is a fact, not a failure. Set
`signing_failure_exit_code = 2` to make an unsigned receipt fail a CI gate.

Signing and verification are pure Rust (`ed25519-dalek`), so no JavaScript
runtime is required for either. Keys are raw 32-byte files: a seed to sign, a
public key to verify. WitDiff never creates one.

## Provenance

A receipt proves that one run happened. It says nothing about the runs around it:
an operator who has been verifying this repository for a year can be shown a
receipt for any one commit, but nothing in that receipt says whether it follows
the last one, or whether somebody produced it after removing the inconvenient
ones in between.

`witdiff verify` therefore appends the receipt's digest to
`.witdiff/provenance.json`, an append-only chain of up to 64 entries:

```text
hash_i = SHA-256(witdiff.receipt-provenance.v1 || sequence_i || previous_hash || receipt_digest)
```

Each entry carries the hash of the one before it, so editing, dropping, or
reordering an entry breaks every link that follows. `witdiff receipt` reports
whether the chain is intact, and `witdiff verify` reports it too — otherwise a CI
run could pass while the recorded history had been altered.

Like the signature, this is **tamper-evidence, not non-repudiation**: it shows
the recorded sequence was not modified, not who wrote it. It is separate from the
signature and neither depends on the other. The signature makes one receipt
unforgeable; the chain makes a sequence unbreakable.

Entries are bounded to 64 and trimmed from the front, so the chain attests to a
retained window rather than to an unbounded history — a longer one belongs in the
repository's own storage. Whatever preceded the window is not checked, and the
first retained entry's predecessor is taken as given.

## Staleness

A receipt is a claim about a revision. Nothing stops the code changing
afterwards, so `witdiff receipt` checks whether the stored receipt still
describes the working state and says so when it does not:

```text
  stale            : this receipt is stale and no longer describes the working state:
                     the receipt was written for 3a82c8dadd76 but HEAD is now df6af6db5326;
                     the workspace has changed since the receipt was written
  next             : re-run `witdiff verify` to produce evidence for the current state.
```

Two independent checks, because either alone misses a real change:

- **the head commit**, which catches a new commit;
- **the workspace fingerprint**, which catches uncommitted edits the head commit
  cannot see.

The uncommitted case is the common one: a receipt written for a change, then the
change edited, keeps the same `head_commit` while describing code that no longer
exists.

`--json` adds two top-level fields alongside the receipt document rather than
changing it, so `witdiff.receipt.v1` is unaffected:

```json
{ "receipt_current": false, "receipt_warning": "this receipt is stale ..." }
```

When the check cannot be performed — a missing Git binary, an unresolvable base
ref — the result is reported as unknown rather than current, because a receipt
that was not checked must not be presented as though it had been.


## Test selection

- `test_selection`: `full_suite` or `targeted`, naming which variant produced
  `head_run` and `base_run`. It defaults to `full_suite`, which is what receipts
  written before this field existed recorded.
- `effective_test_command`: the exact argument vector used for the proof runs,
  after any narrowing. It is absent only when no command was run.

A `targeted` receipt makes a smaller claim than a `full_suite` receipt, so
consumers should read this field rather than infer scope from the configured
command. When narrowing was requested but declined, `test_selection` stays
`full_suite` and the reason appears in `notes`; narrowing never silently
substitutes evidence.

## Changed files

Each `changed_files` entry carries two optional fields:

- `path_is_lossy`: true when the path could not be decoded as UTF-8 and was
  lossily replaced. The lossy text names no file on disk, so WitDiff does not
  transplant it. Omitted when false.
- `previous_is_test`: for a rename or copy, whether the source path was itself a
  test. `false` means the file was moved from production code and must not be
  part of a test-only transplant. Omitted when true; absent means false.

A changed test excluded by either condition is named in `notes` and prevents
`verified` and `verified_with_warnings`, because a partial transplant is not
evidence about the change as a whole.

## Run results

Each recorded run (`head_run`, `base_control_run`, `base_run`) carries:

- `command`, `cwd`, `success`, `exit_code`, `duration_ms`;
- `stdout` / `stderr`, truncated to `verification.max_output_bytes`;
- `failure_kind`: `test_failure`, `compile_error`, `command_failure`,
  `spawn_failure`, `timeout`, `unknown`, or `null` on success;
- `timed_out`: true when the run was killed for exceeding
  `verification.timeout_secs`.

`failure_kind = "timeout"` is never accepted by a proof path. See
`docs/verification-model.md` for the status each timeout produces.
