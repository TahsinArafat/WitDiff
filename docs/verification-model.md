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
- `removed_error_check`: a failure guard present at base and absent at head.
  A guard is a conditional that fails the test without an assertion macro, such
  as `if r.is_err() { panic!("...") }`. Measured before this rule existed:
  deleting one produced **zero** findings, and the test still compiled and
  passed, so red/green could not see it either;
- `removed_match_arm`: a `match` arm present at base and absent at head
  on the same scrutinee; a guard is part of the arm's identity, and
  collapsing specific arms into a wildcard is reported because the
  specific arms are then absent;
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
reordering a test produces no finding. `match` arms are compared as a
multiset of `(pattern, guard)` pairs grouped by scrutinee, so reordering
arms or splitting one `match` into two on the same scrutinee also produces
no finding. A `match` replaced by an `if`/`else` chain is not reported
either: its assertions are still compared, and the structural removal is a
common refactor.

Simple bindings are resolved. An assertion rewritten from
`assert_eq!(compute(), 4)` to `let v = compute(); assert_eq!(v, 4)` is
recognized as the same assertion, because it asserts the same thing.

Resolution is deliberately narrow, and the narrowness is the conservative
direction:

- only a single name bound to a single expression is recorded;
- a name assigned more than once is not resolved, since the later value is what
  the assertion sees;
- destructuring (`let (v,) = ...`) is not resolved;
- resolution is scoped to one test, so a binding cannot make an unrelated
  assertion in another test look equivalent.

Anything that cannot be resolved is reported rather than assumed equal: a
reported removal is reviewed by a human or agent, while a missed removal would
be a false pass.

A test file that cannot be parsed yields `test_source_unparsable` and falls back
to the line-oriented rules. It is never reported as clean.

## Inline `#[cfg(test)]` transplantation

Tests inside a `#[cfg(test)] mod tests` block share a file with the code they
exercise, so transplanting them means transplanting part of a file. WitDiff does
this by span splicing (ADR-0010): `syn` locates the module spans in the head
revision and those spans replace the corresponding base spans. Only the test
module's bytes move, so production changes elsewhere in the file stay at the
base revision — which is what makes the experiment meaningful.

A file is refused, rather than partially spliced, when any precondition fails:

- `unparsable` — the base or head revision is not valid Rust;
- `no_test_module_in_head` — there is no inline test module to transplant;
- `no_counterpart_in_base` — the module is new in this revision, so there is no
  base span to replace;
- `test_outside_test_module` — a changed `#[test]` lies outside every module.

A partial splice would produce a worktree that exists in neither revision, and
the resulting red or green would be attributed to a code state that never
existed. A transplanted test that references production code introduced in head
fails to compile at base, which is classified `base_incompatible` and never
`verified`.

Production changes elsewhere in the same file do **not** block the splice. A
commit that fixes a bug and tightens its inline test is the normal shape of a
change, and the production change is simply not transplanted.

## Signatures

A receipt may carry a detached Ed25519 signature over the domain separator, the
status and the `verification_digest`.

It is **tamper-evidence, not non-repudiation**: it shows the receipt has not been
edited since the run and describes the code the digest names, but says nothing
about *who* produced it. Keys are operator-supplied; WitDiff never creates,
stores or logs one.

The status is included in the signed bytes because signing the digest alone left
the receipt's headline claim editable while the signature still verified — a
signature that can be carried onto a forged result is worse than none.

Signing is opt-in and additive: a receipt without a signature parses and behaves
exactly as before, and an unsigned receipt is a fact rather than a failure
(ADR-0022).

## Gating in CI

The CLI's exit code is a three-way signal, and CI treats statuses in three
classes rather than two (ADR-0013):

- `0` — the claim was proven, or there was nothing to prove;
- `1` — WitDiff itself failed to run, which is a tool error rather than a
  verification result;
- `2` — verification did not pass.

Exit code 2 is a failed gate, never a crash to be retried.

Which statuses pass is deliberately not "everything except `verified`":

- `verified` passes, and `verified_with_warnings` passes unless `--strict`;
- `no_changed_tests` passes and is reported as *nothing to prove*, so a pass is
  never mistaken for a proof. `--fail-on-no-changed-tests` reverses this for
  repositories that require a test change per pull request;
- `not_verified`, `head_failed` and `base_incompatible` fail. Undecidable is not
  acceptable as a gate, because `base_incompatible` means no behavioral proof
  was established and that is actionable.

The policy lives in `VerificationStatus::gate`, not in a workflow file, so every
consumer — CI, an MCP client, a local script — reaches the same verdict.

### Gate policy

Which results count as a pass can be committed to `witdiff.toml` under `[gate]`,
so a repository states it once instead of restating flags on every invocation:

```toml
[gate]
strict = false
fail_on_no_changed_tests = false
# require_signature = true
```

Command-line flags still work, but they may only **tighten** a committed policy.
Leaving `--strict` off cannot lower a `strict = true` in the file, or every
developer could quietly undo what CI enforces. The same rule applies to an MCP
client passing `strict`, which would otherwise be a way around a repository's
own gate.

`require_signature` is the one requirement about the receipt document rather
than the verdict, which is why it lives on `Receipt::gate` rather than on the
status. It applies uniformly, including when there is nothing to prove: a receipt
is still produced, and an operator who asked for signatures asked for them on
every receipt.

The policy is deliberately **not** a list of acceptable statuses. An allow-list
would let a repository write `pass = ["not_verified"]` and defeat the tool by
configuration. Policy decides how strictly a verdict is judged, never which
verdicts exist.

## Rules that fire on legitimate work

Some findings are ambiguous by nature, and WitDiff reports them rather than
guessing. Knowing which ones saves a confusing first encounter.

`changed_expected_value` fires whenever a comparison's expectation changes,
including when the implementation changed and the expectation was correctly
updated to match. Verified: fixing `add` from subtraction to addition and
updating `assert_eq!(add(3, 1), 2)` to `..., 4)` reports the rule, in Go and in
Rust alike.

WitDiff cannot distinguish "the expectation was updated because the behavior was
correctly fixed" from "the expectation was edited to make a failing test pass".
Both are a changed expectation, and the tool has no way to know which. It
reports, and a human decides.

This is deliberate. Silently accepting changed expectations would let the
second case through, which is the failure the rule exists to catch. The cost is
that a legitimate fix accompanied by a test update produces a finding.

Two ways to handle it:

- Set `block_on_integrity_findings = false` and read the findings as advisory.
  The receipt still records them, so nothing is hidden.
- Split the change: land the implementation fix first, then the test update. The
  second commit has no expectation change relative to the first, so no finding
  is produced.

## Framework classification

Whether the base experiment proves anything depends on classifying *why* it
failed. Only a recognized test failure yields a red/green proof; a compile error
yields `base_incompatible` and anything unrecognized yields `not_verified`
(ADR-0003).

That classification is framework-specific, because a framework's failure output
is (ADR-0012). WitDiff recognizes cargo, pytest, Jest/Vitest and Go. The
framework is named explicitly by `verification.framework` rather than sniffed
from output, because inferring it from text the candidate controls could turn a
broken invocation into a proof. An unrecognized name is an error, not a silent
fallback — silently using the wrong classifier is the failure this exists to
prevent.

Patterns key on each framework's own structural markers rather than generic
English, and every classifier is tested against captured real output including a
passing run, so a green suite is never reported as a test failure.

## Changed-code mutation

Mutation answers a question the red/green experiment cannot: a transplanted test
can fail on the base revision for the right reason and still barely constrain
the changed code. WitDiff mutates the changed production lines and observes
whether the suite notices (ADR-0011).

It is **opt-in** (`verification.mutation`, default off) and **supplementary**: it
never changes `status` and never sets `red_green_proven`. A surviving mutant is a
question about test strength, not a verdict about correctness, so the receipt
reports counts and per-mutant detail rather than a score.

Each mutant is classified into exactly one outcome:

- `killed` — the suite failed with a recognized test failure;
- `survived` — the suite passed, so the tests do not detect that change;
- `not_compiled` — the mutant did not build, so it was never executed;
- `timeout` — the run exceeded its deadline and decided nothing;
- `skipped` — a bound was reached before the mutant ran, or it could not be
  applied.

Only `killed` and `survived` are decisions about the tests. `not_compiled` in
particular is **not** a kill: counting it as one would inflate the signal with
mutants that were never observably wrong.

Mutants are generated only from changed lines of changed production files, and
never from test code. Mutating a test's own expected value would change the
oracle — the question rather than the answer. Some mutants are equivalent to the
original program and can never be killed by any test; WitDiff does not attempt
equivalence detection and says so rather than guessing.

## Coverage of changed code

Coverage answers a question neither the red/green experiment nor mutation asks
directly: of the lines **you changed**, how many did the tests actually execute?

A transplanted test can fail on the base for the right reason and still barely
touch the code the change rewrote, and a surviving mutant tells you the tests do
not notice a mutation but not how much of the diff they reach. Coverage is the
measure of reach.

It is deliberately **evidence and never a verdict**, for the same reason mutation
is (ADR-0011). An uncovered line is not evidence that the code is wrong — it is
a question about test strength, so it is reported and a human decides. It never
affects `status` or `red_green_proven`; a test run with coverage enabled and one
without are asserted to reach the same verdict.

Three details are deliberate:

- **It counts added lines, not whole files.** A change touching one line of a
  3000-line file is not 3000 lines of coverage or the absence of it. The report
  counts only lines the `-U0` patch added.
- **It is a separate run.** Coverage instruments the suite, so it costs a second
  full test run and is off by default. It runs after the proof, and a failure to
  measure it cannot undo evidence already gathered.
- **It is reported even when it is zero.** "No changed lines were measurable"
  and "no changed line was executed" are different claims, and conflating them
  would turn a genuine gap into a parsing failure that reads like a missing
  measurement.

Currently implemented for `cargo test` only; another framework is reported as
not measured rather than approximated.

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

## Sandboxed execution

`test_command` comes from the repository being verified, which means it comes
from whatever changeset is under review. Running it directly executes
candidate-controlled code on the host.

Setting `verification.sandbox_image` rewrites every run — head, base control,
base experiment, and mutants — into:

```text
docker run --rm --name <unique> --workdir <cwd> --volume <cwd>:<cwd> <image> <command>
```

The workspace is mounted at its own absolute path, so paths in the command's
output still resolve. Paths *outside* it do not: the host toolchain, its package
cache, and any absolute path above the workspace are absent. That is the
isolation, and it is why the image must contain what the command needs. Which
toolchain a project requires is not something WitDiff can infer, so the image is
the operator's.

Three properties are deliberately explicit rather than implied:

- **The container is named.** Killing `docker run` stops the *client*, not the
  container it started. On a timeout that would leave the candidate's code
  running indefinitely — a resource leak and a long-lived instance of exactly
  what the sandbox exists to contain. The name lets the timeout path remove it,
  and a test asserts no container is left behind.
- **Environment is an allow-list.** Only `WITDIFF=1` and the variables named in
  `sandbox_env` are forwarded. Inheriting the operator's shell would put
  credentials inside the container the sandbox is meant to be distrustful of.
- **No sandbox is not an accident.** A configured-but-empty image is refused
  rather than silently falling back to running on the host, because isolation
  that reports success while doing nothing is worse than no isolation.

It is opt-in: a project that cannot be containerized must still be verifiable,
and nothing about the receipt changes apart from what `head_run.command`
records.

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

Running the suite writes files, so "the workspace moved" alone is not the
question. When **every** path that moved during the run is build output —
`__pycache__`, `.pytest_cache`, `target/`, and friends — the source under
verification did not move and the evidence still describes it, so the run stays
fresh and the receipt says so in a note. Otherwise the gate is downgraded and
the note names what changed.

Two boundaries make that safe:

- **One moved source file keeps the evidence stale**, so a real edit cannot hide
  behind the artifacts a build always produces.
- **A fingerprint change with no attributable path stays stale.** "It was only a
  build" must not explain a change nobody can name; unexplained movement is
  exactly what freshness exists to catch.

Before this was separated, the receipt would call those paths irrelevant to the
verification and fail the gate for them in the same sentence — and the downgrade
applied unconditionally, so even after classification said otherwise a proof was
erased.

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
