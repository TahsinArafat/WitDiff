# ADR-0015: Signed receipts — what a signature can and cannot attest

Status: proposed

This ADR is a design document. It specifies a problem and a boundary, and
deliberately does **not** commit to an implementation, because the remaining
questions are key-management and deployment decisions rather than engineering
ones. PG-503 requires this ordering.

## Context

A receipt is a JSON document the tool writes into the verified repository. It is
currently trustworthy only to the extent that the reader trusts whoever controls
that file. Three specific weaknesses were demonstrated before designing
anything.

### 1. A receipt is trivially forgeable

Editing `.witdiff/receipt.json` to change `status` from `no_changed_tests` to
`verified` and set `red_green_proven: true` is accepted verbatim:

```text
$ # edit .witdiff/receipt.json
$ witdiff receipt --json
status: verified | red_green_proven: True
```

Nothing in the document is bound to the tool that produced it.

### 2. The workspace fingerprint does not identify the verified code

This is the finding that constrains the whole design. `workspace_fingerprint`
hashes `git diff --binary <base>`, `git status --porcelain`, and the contents of
untracked files. On a clean tree all three are empty, so the fingerprint is
exactly SHA-256 of the empty string:

```text
clean tree fingerprint: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
sha256("")            : e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

Two different repositories, one containing `f() -> 1` and the other
`f() -> 999`, produced the *identical* fingerprint:

```text
clean1  head=1ee7ba0  fingerprint: e3b0c44298fc1c149afbf4c8
clean2  head=f1468b2  fingerprint: e3b0c44298fc1c149afbf4c8
```

That is correct for what the field is for: it is a **staleness check**, not a
content hash. It answers "did anything move while verification ran". It does not
answer "which code was verified".

So a signature over the receipt as currently shaped would attest that *some*
WitDiff run emitted this document. It would not attest to the code, which is the
thing a reader actually cares about.

### 3. A receipt is not bound to the code it describes

Nothing prevents code changing after verification. Confirmed by committing a
breaking change after a run: the receipt continued to claim the old
`head_commit`, and `witdiff receipt` reported it without complaint. The receipt
becomes a claim about a revision that is no longer checked out, and the reader
has to notice that themselves.

## Decision

Signed receipts are **deferred**, with the design constrained as follows. The
first two items are prerequisites for any signature work; implementing a
signature without them would produce an attestation that does not attest to
anything useful.

### Prerequisite A: a content digest over the verified inputs

Add a digest over the material facts of the verification, so a signature has
something meaningful to cover. It must include at minimum:

- the base and head revisions;
- the exact test command that ran (`effective_test_command`);
- the contents of the changed test files at head, and of any transplanted
  inline test module;
- the production source that the base worktree received.

This is distinct from `workspace_fingerprint` and must not replace it. They
answer different questions: the fingerprint detects movement during a run, the
digest identifies the inputs. Conflating them is what made finding 2 surprising.

### Prerequisite B: a binding from receipt to revision

A receipt must state the revision it describes and be checkable against the
working state. The existing `head_commit` is necessary but not sufficient: it is
absent for uncommitted work, which is the normal case. The digest from A
provides the checkable binding.

### Then, and only then, a signature

The signature covers the digest and the status. It says: *the holder of key K
attests that a WitDiff run with these inputs produced this status*. It does
**not** say the code is correct, and the schema field name must not suggest that
it does.

Key management is explicitly out of scope for this ADR and must be decided
separately before implementation. The open questions are real ones:

- Is the key per-developer, per-repository, or per-CI-runner?
- Where does the private key live, and what happens when it leaks?
- Is the goal non-repudiation, or only tamper-evidence?

Those have different answers and lead to different designs. Choosing a scheme
now would be guessing.

### Replay semantics

A signature must not be reusable for different code. Since the signature covers
a digest over the verified inputs, replaying it against a different revision
fails the digest check. This is why Prerequisite A comes first: without a
content digest, a signature is indefinitely replayable across any tree that
happens to produce the same receipt.

Verification of a signature must also be local and offline. Invariant 4 applies:
checking a receipt must not require a network call or a key server.

## Consequences

- No signed-receipt implementation ships until Prerequisites A and B exist. The
  roadmap item stays open and unstarted, which is the honest state.
- Findings 2 and 3 are latent defects in the *current* receipt regardless of
  signing, and are worth fixing for their own sake. A reader comparing a receipt
  to the code today has only `head_commit` to go on, and nothing tells them when
  it has gone stale.
- Adding a digest is an additive change to `witdiff.receipt.v1`, consistent with
  invariant 8. A signature field would likewise be additive.
- The receipt schema will need a field whose name cannot be misread as asserting
  correctness. `attestation` over `verified_by`, since the latter invites exactly
  the misreading this project exists to prevent.

## Alternatives considered

- **Sign the receipt document as it stands today.** Rejected: per finding 2 it
  would attest that a run happened, not what was run, so a reader could not use
  it to distinguish verified code from different verified-looking code.
- **Treat `workspace_fingerprint` as the content binding.** Rejected on
  measurement: it is the empty-string hash on every clean tree, so it
  distinguishes nothing between them.
- **Adopt sigstore or a similar hosted transparency service now.** Rejected as
  premature: it answers the key-distribution question without having decided
  whether non-repudiation is even the goal, and it would put a network
  dependency near the verification path.
- **Use a Git commit signature as the attestation.** Rejected: a signed commit
  attests authorship of a change, not that a verification was performed against
  it. The two claims are different, and conflating them would be precisely the
  kind of borrowed authority this project avoids.
- **Ship a signature with a documented caveat instead.** Rejected: a receipt
  that carries an "attested" mark but does not bind to the code would be read as
  stronger evidence than it is, and misuse of a verification tool is worse than
  its absence.
