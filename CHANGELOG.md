# Changelog

## Unreleased

### Added

- **Framework-specific failure classification** (ADR-0012), selected with
  `verification.framework` and defaulting to `cargo`. Cargo, pytest, Jest/Vitest
  and Go output are now recognized. An unrecognized framework name is an
  explicit error rather than a silent fallback to the Rust classifier.

### Fixed

- **Non-Rust test failures were misclassified, so no proof was possible.**
  `classify_failure` matched cargo output only. Measured against real framework
  output, pytest, Jest and Go each classified a genuine test failure as
  `CommandFailure`, which yields `not_verified` instead of a red/green proof. A
  pytest repository therefore could not obtain a proof at all, and the receipt
  could not say why. Verified end to end: a pytest repository and a Go
  repository each now reach `verified`, where the same pytest output previously
  classified as `CommandFailure`.

- **Changed-code mutation analysis** (ADR-0011), opt-in via
  `verification.mutation` and off by default. Mutants are generated for the
  changed lines of changed production Rust files and confined to code outside
  every `#[cfg(test)]` module, from the parsed tree rather than by text
  matching. Operators: equality flip, comparison boundary, logical flip,
  boolean literal flip. Each mutant is classified `killed`, `survived`,
  `not_compiled`, `timeout` or `skipped` — and only the first two are decisions
  about test strength. A mutant that failed to build was never executed and is
  never counted as a kill.
- **Mutation is supplementary and cannot create a proof.** It never affects
  `status` and never sets `red_green_proven`; a surviving mutant is a question
  about test strength, not a verdict. Runs are bounded by `max_mutants` and
  `max_mutants_per_function`, and outcomes are cached by mutant ID, test
  fingerprint and command so a re-run does not repeat the suite.
- Receipt fields for mutation: `generated`, `killed`, `survived`,
  `not_compiled`, `timeout`, `skipped`, per-mutant `results` and `notes`. All
  additive and optional, so a receipt written before the field existed still
  parses; the section is omitted entirely when mutation is disabled.

- **Inline `#[cfg(test)]` transplantation** (ADR-0010). A changed inline test
  module is now transplanted onto the base revision instead of being reported
  as an unsupported case. `syn` locates the module spans in head and those spans
  replace the corresponding base spans, so only the test module's bytes move and
  the author's formatting is preserved. Every precondition failure — an
  unparsable revision, a module new in head, a changed `#[test]` outside any
  module — produces a reported refusal rather than a partial splice. The receipt
  records `spliced_inline_tests` and `refused_inline_tests`.
- **Removed `match`-arm detection** (ADR-0006). Arms are compared as a
  multiset of `(pattern, guard)` pairs grouped by normalized scrutinee, so
  reordering, moving an arm between `match` expressions on the same
  scrutinee, and splitting one `match` into two produce no finding, while
  a deleted arm — including a specific arm collapsed into a wildcard, and
  a guarded arm whose guard was dropped — is reported as `removed_match_arm`
  (high severity). A `match` replaced by an `if`/`else` chain is a
  documented non-finding: its assertions are still compared.

### Fixed

- **Inline test detection missed the common cases.** The detector looked for
  marker substrings (`#[test]`, `assert!(`, …) on changed lines, so a changed
  assertion body such as `is_even(3)` becoming `is_even(4)` was not recognized
  as an inline test change at all, and a file whose production code changed
  while merely *containing* a test module produced no candidate. Detection is
  now structural: a file is a candidate when it parses, contains a
  `#[cfg(test)]` module, and changed.
- **A newly added assertion could be reported as a changed expected
  value.** Weakening comparisons paired each head assertion with the first
  same-subject base assertion, without first consuming base assertions that
  an identical head assertion had already matched. A second `match` arm
  asserting `assert_eq!(cost(), 20)` next to a surviving
  `assert_eq!(cost(), 10)` was therefore reported as if the `10` had been
  rewritten to `20`. Pairing now consumes identical assertions first, then
  pairs the remainder by subject.

## 1.0.0

First stable release. The red/green proof semantics are unchanged from 0.1; this
release closes the two remaining M1 hardening gaps and implements M2 Rust-aware
analysis. All receipt changes are **additive within `witdiff.receipt.v1`** — no
v2, no breaking change, and receipts written by 0.1 still deserialize.

### Added

- **Syntax-aware test integrity analysis** (`crates/witdiff-core/src/rustanalysis.rs`,
  ADR-0006). Rust test files are parsed with `syn` and compared structurally
  between base and head, instead of scanning added and removed diff lines. New
  rules: `removed_assertion`, `changed_expected_value`, `weakened_assertion`,
  `removed_test`, plus informational `unignored_test` and `empty_test_body`.
  Existing rules (`ignored_test`, `added_should_panic`, `trivial_assertion`) are
  retained.
- **`test_source_unparsable` finding.** A test file that cannot be parsed is
  reported and falls back to the line-oriented rules. It is never silently
  treated as clean.
- **Targeted test selection** (`crates/witdiff-core/src/selection.rs`,
  ADR-0008), opt-in via `verification.targeted_test_selection`, default
  `false`. Narrows a cargo test command to the cargo targets of the changed
  dedicated tests. It refuses to narrow, and runs the full suite with a note,
  whenever doing so would change or misrepresent what actually runs — notably
  when the command uses `--all-targets`, which overrides `--test` in cargo.
- **Receipt fields**: `test_selection` (`full_suite` / `targeted`),
  `effective_test_command`, and `changed_files[].path_is_lossy` /
  `changed_files[].previous_is_test`. All additive and optional.
- End-to-end coverage for reformatting-is-not-a-finding, changed expectations,
  and the production-rename transplant boundary.
- Unit coverage for NUL-delimited Git path decoding, including a non-UTF-8 path
  decoded from raw bytes.

### Fixed

- **Non-ASCII paths were silently dropped from verification.** `git diff
  --name-status` and `git ls-files --others` were read by splitting on newlines,
  so Git C-quoted any non-ASCII path (`tests/café.rs` arrived as
  `"tests/caf\303\251.rs"`). Such files matched no test glob and were
  classified as production code, so they were never transplanted. Path output is
  now read NUL-delimited with `-z` (ADR-0009). A filename containing a newline
  is also now a single path rather than two.
- **A rename from production code into a test directory could make the proof
  prove the wrong thing.** The old production path was added to the transplant
  pathspec, so the production file's diff rode along in a "test-only" transplant
  and the base worktree could receive the fix. Such files are now excluded,
  named in the receipt `notes`, and prevent a verified status (ADR-0007).
- A path that could not be decoded as UTF-8 is no longer lossily decoded in
  silence; the loss is recorded at decode time, where it is still recoverable,
  and the path is not transplanted.

### Known limitations

- WitDiff does not resolve `let` bindings, so an assertion rewritten as
  `assert_eq!(compute(), 4)` → `assert_eq!(v, 4)` is reported as a removal. The
  analyzer cannot prove it merely moved, and reporting is the conservative
  direction.
- Inline `#[cfg(test)] mod tests` inside production files still cannot be
  transplanted independently. WitDiff detects likely inline-test edits and
  reports them as a note rather than claiming to have verified them.
- Targeted selection is Rust/cargo only and defaults to off.

## 0.1.0 - initial release

First release. WitDiff was developed under an earlier working name that was
never published; this repository carries only the released name.

### Added

- Bounded execution. `verification.timeout_secs` (default 900) caps each
  individual test command run. On expiry the child is killed and the run is
  recorded with `timed_out: true` and the new `FailureKind::Timeout`, which no
  proof path accepts. Defaults keep a cold CI build from being mistaken for a
  hang while still bounding a genuinely stuck suite.
- End-to-end red/green verification tests (`crates/witdiff-core/tests/verify_end_to_end.rs`)
  exercising `verify_repository` against real temporary Git repositories with
  real cargo builds: verified red/green, a test that also passes on base, a
  non-passing pristine control, a transplant that fails to compile, and a
  high-severity integrity finding blocking verification. The cargo-driven cases
  are `#[ignore]`d so a plain `cargo test` remains offline-capable; run them with
  `cargo test --workspace --all-features -- --ignored --test-threads=1`.
- `WorktreeGuard`, an RAII owner for the temporary base worktree. Cleanup now
  happens on every `Result` path, including a failed patch transplant or a
  failed test spawn, instead of only on the success path.
- `VerificationStatus::as_str` / `Severity::as_str` and `Display` impls giving
  stable snake_case tokens for human output.

### Changed

- CLI output no longer uses Rust `Debug` formatting for status or integrity
  severity. Console tokens now match the receipt schema exactly (for example
  `verified_with_warnings` instead of `VerifiedWithWarnings`).
- `Config` derives `Default` instead of hand-writing an identical impl.

### Fixed

- A failed test-search patch application or test spawn could leak a registered
  Git worktree pointing at a deleted temporary directory, which then broke later
  `git worktree add` calls.
- Console and JSON status representations could disagree about the same run.
- `cargo fmt --all -- --check` and `cargo clippy -D warnings` failures.

## 0.1.0 - initial development snapshot

- Rust workspace with `witdiff-core` and `witdiff` CLI.
- `init`, `doctor`, `inspect`, `verify`, and `receipt` commands.
- Git base resolution and changed-file classification.
- Dedicated changed-test detection.
- Pristine-base control execution.
- Test-only transplantation into a detached base worktree.
- Red/green proof with compile-failure distinction.
- Rust test-integrity heuristics.
- Workspace freshness fingerprint.
- JSON receipt v1 and schema.
- Agent guidance and Claude-compatible skill.
- GitHub CI examples and local smoke scenario.
