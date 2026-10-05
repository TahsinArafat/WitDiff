# Changelog

## Unreleased

### Added

- **End-to-end red/green proof for pytest, Go, Ruby/RSpec and JavaScript.**
  `verify_languages_end_to_end.rs` drives the whole v0.1 invariant against real
  temporary Git repositories containing real projects: HEAD green, pristine
  base control green, and base-plus-transplanted-test red **for a recognized
  test reason**. Each reaching `Verified` with
  `base_run.failure_kind = test_failure`.

  Verifying an analyzer against a real parser is not the same as proving the
  verification works, and this makes the distinction explicit. Python also gets
  the negative direction: a test that passes on the base is reported
  `not_verified`, and a gutted assertion is caught rather than accepted.

  **Java is included**, driven by a real JUnit 5 platform, and reaches
  `Verified` like the rest. The toolchain is derived from the changed `.java`
  files as well as from the configured command, so a project running a committed
  wrapper script — which names no JDK — no longer loses structural analysis.

  Assembling that JUnit classpath found a failure mode worth recording:
  **mixing platform versions is silent**. A `1.14.4` platform with a `6.0.1`
  Jupiter engine compiles the tests, runs them, prints `0 tests found`, and
  exits 0 — a green run that executed nothing.

- **Coverage of the changed production lines** (`verification.coverage`), as
  supplementary evidence: it reports how many of the lines a change added the
  tests actually executed, with per-file detail. Counts added lines from a
  `-U0` patch rather than whole files, so a one-line edit to a 3000-line file
  is measured as one line. Never affects `status` or `red_green_proven` —
  a run with coverage enabled and one without are asserted to reach the same
  verdict, for the same reason mutation does not (ADR-0011).

  Needs `cargo-llvm-cov`, and reports "not measured" rather than approximating
  for any other framework. It re-runs the suite with instrumentation, so it is
  a second full test run and off by default.

- **Environment evidence** on the receipt: the configured test program, its
  version, the toolchain that participates in it, and a digest of each
  dependency manifest. Kept out of the verification digest so upgrading Python
  cannot make an older receipt report itself stale.

- **A committed gate policy** (`[gate]` in `witdiff.toml`), which decides which
  results pass. Command-line flags may only tighten it, so a repository's own
  floor cannot be undone by leaving `--strict` off. Deliberately not an
  allow-list of statuses, which would let a repository configure itself out of
  the tool.

- **A provenance chain** (`.witdiff/provenance.json`) linking each receipt's
  digest to the one before it, so a sequence of verifications can be checked as
  a sequence. Editing, dropping or reordering an entry breaks the links that
  follow, and both `verify` and `receipt` report it.

- **Sandboxed execution** (`verification.sandbox_image`), which rewrites every
  run — head, control, experiment and mutants — into a named container, since
  `test_command` comes from the changeset under review. The container is named
  so a timeout can remove it rather than leaving candidate code running; the
  environment is an allow-list; `--network none` is available.

- **MCP server** (`crates/witdiff-mcp`, ADR-0014) exposing `witdiff_inspect`,
  `witdiff_verify` and `witdiff_receipt` over newline-delimited JSON-RPC 2.0 on
  stdio. It calls `witdiff-core` directly, returns the receipt unchanged, and
  takes its gate verdict from the same `VerificationStatus::gate` the CLI and CI
  use, so the three cannot disagree. Zero new dependencies: the protocol subset
  is implemented over `serde_json` because the official SDK requires rustc 1.88
  against this workspace's 1.78 and pulls 68 packages.

### Fixed

- **Java reached `VerifiedWithWarnings` and could never reach `Verified`.**
  The toolchain was derived only from the configured test command, so a project
  whose tests run through a committed wrapper script named no JDK at all: the
  red/green proof held, structural analysis was skipped, and the receipt only
  said analysis was unavailable. A test file ending in `.java` is now evidence
  of a Java project too.

  Both halves are tested, because they are different claims: a script-shaped
  command now yields `Verified`, and gutting an existing assertion while adding
  a credible regression test produces a `trivial_assertion` finding from the
  structural comparison rather than merely stopping the skip notice.

- **A rebound subject was reported as a removed assertion.** `assert_eq!(compute(), 4)`
  rewritten to `let v = compute(); assert_eq!(v, 4)` asserts the same thing, but
  the analyzer cannot see through the binding and reported a removal. Simple
  bindings are now resolved — a single name bound to a single expression — while
  reassignment, destructuring and cross-test names deliberately stay unresolved
  and are still reported rather than assumed equal. Shipped after 1.0.0, which
  recorded the limitation in its own Known-limitations list.

- **A run whose only movement was build output lost its proof.** Freshness was
  `before == after` on the raw fingerprint while `is_build_output` only formatted
  a message, so the receipt would call those paths irrelevant to the
  verification and fail the gate for them in the same sentence. The downgrade
  also ran unconditionally, so even a classification that said "only a build"
  was erased afterwards. CI caught this: the `pytest` proof failed on
  `__pycache__` written by Python 3.12 and passed on 3.9 locally.

- **`cargo-llvm-cov` is installed in CI.** Without it the coverage tests skip,
  which is the same trap the ignored suite was in before toolchains were wired
  in — a green run that exercised nothing.

- **JavaScript integrity analysis reported no expectation changes, at all.**
  The analyzer normalized Babel's `NumericLiteral`/`StringLiteral` but not
  ESTree's `Literal`, which is what `acorn` emits — so every literal rendered as
  the bare word `Literal`, and `expect(x).toBe(2)` and `expect(x).toBe(3)`
  produced identical strings. The rule that is the entire point of the
  comparison never fired for a JavaScript project.

  Found by installing `acorn` and running the analyzer against it. Every test
  that had covered this path passed beforehand, because each asserted against
  hand-written output rather than the real parser's.

- **`expect(x).not.toBe(1)` produced no assertion.** acorn places `.not`
  between the call and the matcher, and the traversal unwrapped the wrong
  level, so every negated expectation yielded nothing. Inverting an assertion —
  which makes it pass for the wrong reason — was therefore silent.

- **`test.skip`, `test.only` and `test.each` produced no test function.** Only
  the bare `test(...)` form was recognized, so newly skipping a test was
  invisible, which is the same class of silence as an ignored Rust test.

- **Node's `assert` module was invisible.** `assert.strictEqual(a, 1)` and
  `assert.deepEqual(a, 1)` are member calls; only a bare `assert*(...)` form
  was recognized.

- **Every TypeScript file either crashed or analyzed as empty.** The
  TypeScript AST tags nodes with `kind`, not `type`, so the ESTree traversal
  visited nothing; with `setParentNodes` on, the tree was cyclic and the
  traversal recursed until `Maximum call stack size exceeded`. Literal kinds
  also reverse-map to the alias `FirstLiteralToken`, so `2` normalized to that
  word. And a parser that could not read the file was treated as final rather
  than a reason to try the next one, so a TypeScript project with `acorn`
  installed failed on every `.ts` file.

- **`.tsx` files were rejected.** Every file was written to disk as
  `input.js`, so the parser never saw a `.tsx` extension and rejected valid
  JSX. The extension is load-bearing and is now preserved.

- **Inverting an assertion reported nothing, in every language.**
  `testshape`'s `expectation_of` returned only the right-hand side, so `Eq 1`
  and `NotEq 1` compared equal. This was in the shared rule engine, not in the
  JavaScript analyzer, so it affected every language using the operator-string
  form; Rust escaped it only because its analyzer compares `macro_name`. The
  comparison now includes the operator.

- **Ruby classified a single-example red suite as an unrecognized command
  failure.** The summary matcher required the plural `" examples,"`, but real
  RSpec prints `1 example, 1 failure` for a one-example suite — and an
  unrecognized failure cannot produce a proof, so every single-example Ruby
  suite silently lost its red/green evidence. Minitest has the same singular
  shape and the same gap.

- **Ruby reported a suite that failed to load as a behavioural regression.**
  Real RSpec reports a `LoadError` as `0 examples, 0 failures, 1 error occurred
  outside of examples` — a non-zero error count on a summary line whose failure
  count is zero. The test-failure check ran first and claimed it. Ruby now
  checks compile failure before test failure, the reverse of every other
  framework, because a load error is never behavioural evidence.

- **The reusable workflow never captured WitDiff's exit code.** The verify step
  was `witdiff verify ... | tee out; echo "exit_code=$?"`, but GitHub runs an
  unspecified shell as `bash -e`, so a non-zero verify aborted the step before
  the `echo`. `exit_code` stayed unset and the enforce step saw an empty string:
  a **failed verification gate was reported as a WitDiff malfunction**, which
  inverts the three-way distinction ADR-0013 exists to preserve.

  Reproduced directly against `bash -eo pipefail`, which is what GitHub uses,
  then fixed by capturing the code immediately with `set +e` around the run.
  The enforce step now also reports an empty verdict as a tool error rather than
  letting it pass.

- **`witdiff verify --json` emitted a stray commit hash, making its output
  unparseable.** `git_status` used `.status()`, which inherits the parent's
  stdout, and `git rev-parse --verify <ref>` prints the resolved hash there. So
  every verification wrote a bare hash before its own output, corrupting the
  JSON stream for any machine consumer. The child's stdout is now captured.

  This was latent since 0.1 and was found by the MCP transport, where a single
  stray line desynchronizes the protocol stream and the failure is immediate
  rather than subtle.

- **GitHub Actions integration**: a reusable workflow
  (`.github/workflows/witdiff.yml`) and `--github-annotations`, which emits
  workflow commands for the status and every integrity finding. Escaping is done
  in the CLI rather than in workflow YAML, so no consumer re-implements it and a
  message containing quotes, backticks or newlines cannot produce a malformed
  annotation.
- **`--fail-on-no-changed-tests`**, for repositories that require a test change
  per pull request.
- An expanded drop-in agent instruction package (`examples/agent-instruction.txt`)
  covering the three-way exit code, every status, and the reporting rules.

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

### Changed

- **CI runs the tests that prove the analyzers work.** The 100+ `#[ignore]`d
  tests — the ones exercising real parsers and real runners — were skipped by
  `cargo test`, so none of it was exercised on any pull request. CI now installs
  Node, Python, Go, Ruby and a JDK, adds `acorn`/`typescript` via `npm ci`,
  installs RSpec and pytest, and runs the `--ignored` suite. The Java
  end-to-end proof additionally downloads a JUnit 5 platform, because mixing
  JUnit versions silently reports `0 tests found` and exits 0 — a green run that
  executed nothing.

- **Signing and verification are pure Rust** (`ed25519-dalek`, ADR-0022). They
  previously shelled out to Node's built-in `crypto`, which ADR-0022 recorded
  as an environment workaround: the cargo cache could not be written, so the
  crate could not be fetched or exercised. With that restriction lifted the
  crate was added and exercised directly. No JavaScript runtime is now needed to
  sign or verify a receipt.

  The two properties that define what a signature *means* are preserved and
  asserted against the real implementation: the domain separator
  `witdiff.receipt-signature.v1\0`, and the status inside the signed bytes. The
  latter is not cosmetic — signing the digest alone let a receipt forged from
  `not_verified` to `verified` still verify as VALID.

  Keys are raw 32-byte files: a seed to sign, a public key to verify.
  Verification deliberately does not accept a private seed, so a verifier never
  needs private key material.

- **`--strict` no longer fails when there is nothing to prove.** `no_changed_tests`
  now passes by default and is reported as `nothing_to_prove` rather than as a
  gate failure, because failing a documentation-only pull request on a correct
  receipt teaches operators to disable the check (ADR-0013). The gate policy
  lives in the core (`VerificationStatus::gate`) so CI, MCP and local scripts
  reach the same verdict, and `--fail-on-no-changed-tests` restores the previous
  behavior explicitly.

- **Framework-specific failure classification** (ADR-0012), selected with
  `verification.framework` and defaulting to `cargo`. Cargo, pytest, Jest/Vitest
  and Go output are now recognized. An unrecognized framework name is an
  explicit error rather than a silent fallback to the Rust classifier.

### Documentation

- **ADR-0015 designs signed receipts and declines to implement them yet.** The
  design work found that a signature over the receipt as currently shaped would
  attest that a run happened, not which code was verified. On a clean tree the
  workspace fingerprint is exactly SHA-256 of the empty string, so two
  repositories containing entirely different source produce the same
  fingerprint; it is a staleness check, not a content hash. A content digest
  over the verified inputs and a checkable revision binding are prerequisites.
  Findings recorded: a receipt is trivially forgeable, the fingerprint does not
  identify the verified code, and nothing detects a receipt that has gone stale
  against the working tree.

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
