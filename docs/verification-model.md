# Verification model

WitDiff intentionally distinguishes **green**, **red**, **fresh**, and **integrity** evidence.

## HEAD green

The configured command must exit successfully in the current workspace.

A failing HEAD can never be verified.

## BASE control

Before applying changed tests, WitDiff runs the configured command on the untouched base worktree. The control must pass. If it does not, v0.1 cannot attribute a later full-suite failure to the changed tests and returns a conservative non-verified result.

## BASE red

After the green control, WitDiff applies only the changed dedicated tests. The same command must fail with a recognized test-failure signature.

For Rust/cargo v0.1:

- `test result: FAILED` / recognizable failure summary -> behavioral red evidence;
- `could not compile` -> base incompatible, not verified;
- generic process failure -> not verified.

## Test integrity

Current syntax-light rules flag:

- removed assertions;
- removed `#[test]` attributes;
- newly added `#[ignore]`;
- newly added `#[should_panic]` as a warning;
- trivially true assertions.

These checks are deliberately conservative and are not a substitute for syntax-aware analysis.

## Bounded execution

Each individual test command run is subject to `verification.timeout_secs`. On
expiry the process is killed and the run is recorded with `timed_out: true` and
`failure_kind: "timeout"`.

A timeout is a fact about the candidate's suite, not a WitDiff malfunction, so
it is reported as a result rather than raised as a tool error. It is never
accepted as evidence:

- a timed-out HEAD run yields `head_failed`;
- a timed-out control run yields `not_verified`, because attribution is lost;
- a timed-out base+changed-tests run yields `not_verified`, because a suite that
  does not terminate has not demonstrated a behavioral regression.

`Timeout` is deliberately distinct from `test_failure`. A killed run is not a
red observation, and treating it as one would manufacture proof from a hang.

## Evidence freshness

WitDiff hashes the diff from base plus untracked changed-file content before and after verification. If the fingerprint changes, evidence is stale and a previously verified result is downgraded.

## Status semantics

### `verified`

- HEAD command passes;
- BASE+changed-tests command fails with a recognized test failure;
- evidence is fresh;
- no integrity findings are present.

### `verified_with_warnings`

Red/green proof exists and evidence is fresh, but integrity warnings/findings require human or agent attention.

Strict CLI mode does **not** accept this status.

### `not_verified`

The changed tests pass on base, evidence became stale, or base failed for a non-test reason that WitDiff cannot accept.

### `no_changed_tests`

The current proof mode has no changed dedicated test files to transplant.

### `head_failed`

Current workspace tests fail.

### `base_incompatible`

The transplanted tests do not compile against base.

## Threat model

WitDiff v0.1 protects against accidental or agent-produced false verification such as:

- “regression” test passes before the fix;
- test was marked ignored;
- assertion was removed;
- trivial assertion was added;
- agent ran tests before a final edit;
- compile failure was mistaken for a behavioral regression.

It does not yet protect against malicious test binaries, compromised compilers, malicious build scripts, or fully adversarial repositories. Sandboxing is a future layer.
