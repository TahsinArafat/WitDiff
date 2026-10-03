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

## Test-only transplant boundary

The transplant must contain test changes and nothing else. A changed dedicated
test is ineligible for the transplant when:

- it was renamed or copied from a path that was not itself a test, because
  carrying the old path would pull that file's production diff onto the base
  revision and make the experiment prove nothing about the test;
- its path did not survive a UTF-8 round trip, because the lossy text names no
  file that exists on disk.

Git path output is read NUL-delimited, so a path containing non-ASCII
characters is classified as the path it actually is instead of as Git's
quoted rendering of it.

An ineligible test is excluded from the transplant, recorded by name in `notes`,
and prevents `verified` or `verified_with_warnings`. A partial transplant is
evidence about the files it contained, not about the change as a whole.

## Test integrity

Rust test files are compared structurally. WitDiff parses the base and head
revision of each changed dedicated test file and compares which functions carry
`#[test]`, which attributes they carry, and which assertions each body
contains. A finding is therefore a statement about code rather than about the
lines a diff happened to mark, and it names the line it was observed at.

Structural rules:

- `removed_assertion`: an assertion present at base and absent at head;
- `changed_expected_value`: the same subject expression with a different
  expected value;
- `weakened_assertion`: an exact assertion replaced by a weaker predicate;
- `removed_test`: a `#[test]` function that no longer exists;
- `ignored_test`: a test carrying `#[ignore]`;
- `trivial_assertion`: an assertion that cannot fail;
- `added_should_panic`: `#[should_panic]` on a test, as a warning;
- `test_source_unparsable`: the file could not be parsed as Rust; a warning
  when the head revision is unreadable, informational when only the base
  revision is, because the base is only the comparison point;
- `unignored_test` and `empty_test_body`: informational.

Non-Rust test files, and Rust files the parser cannot read, fall back to the
line-oriented rules. Those rules flag removed assertions, removed `#[test]`
attributes, newly added `#[ignore]`, newly added `#[should_panic]`, and
trivially true assertions by scanning added and removed diff lines. They see
only what the diff marks, so a moved or reformatted assertion appears to them
as one removal plus one addition.

These checks are deliberately conservative. They report what changed between two
revisions; they do not decide whether a test is correct.

### Normalization and bindings

Assertion text is normalized to be whitespace-insensitive, so reformatting or
reordering a test produces no finding.

WitDiff does not resolve `let` bindings. An assertion rewritten from
`assert_eq!(compute(), 4)` to `assert_eq!(v, 4)` is therefore reported as
`removed_assertion`, because the analyzer cannot prove that the assertion merely
moved to a different subject. This is the conservative direction: a reported
removal is reviewed by a human or agent, while a missed removal would be a false
pass.

A test file that cannot be parsed yields `test_source_unparsable` and falls back
to the line-oriented rules. It is never reported as clean.

## Targeted test selection

Opt-in through `verification.targeted_test_selection`, default `false`. When
enabled, WitDiff narrows the cargo test command to the `--test` targets of the
changed dedicated tests. The decision is made once, before any evidence is
collected, so the HEAD run, the base control, and the base+changed-tests run all
use the same command.

Narrowing is applied only when it cannot change what actually runs. WitDiff
refuses to narrow, runs the full suite, and records a note explaining why when:

- the configured command is not a `cargo test` invocation;
- it carries `--all-targets`, or the workspace-wide `--all`; cargo lets
  `--all-targets` override `--test`, so the full suite would run while the
  receipt named a narrow selection;
- it already selects tests explicitly with `--test` or `--tests`;
- any changed test file is not a direct child of a `tests/` directory, because
  cargo compiles such a file into its parent target rather than into a
  selectable one, or because there are no changed tests at all.

A narrowed run is a strictly smaller claim than a full-suite run, so the receipt
records which one occurred (`test_selection`) together with the exact argument
vector that ran (`effective_test_command`). Selection never silently
substitutes evidence: when narrowing did not happen, `test_selection` is
`full_suite` and the reason is in `notes`.

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
- every changed test was transplanted;
- no integrity findings are present.

### `verified_with_warnings`

Red/green proof exists and evidence is fresh, but integrity warnings/findings require human or agent attention.

A partial transplant never reaches this status; it is downgraded to
`not_verified`.

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
- the fix rode into the transplant on a rename from a production path;
- a test file with a non-ASCII name was misread as production code and skipped;
- agent ran tests before a final edit;
- compile failure was mistaken for a behavioral regression.

It does not yet protect against malicious test binaries, compromised compilers, malicious build scripts, or fully adversarial repositories. Sandboxing is a future layer.
