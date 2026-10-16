# Changelog

## Unreleased

### Fixed
- **WitDiff classified its own config as production code.** Running
  `witdiff init` to learn the schema meant the next `witdiff inspect` reported
  `P witdiff.toml` — the tool counting its own configuration as part of the
  change under review. The config file is now excluded from the production
  list; it stays in `changed_files`, so provenance still sees it.
- **`base_dependency_dirs` had an undocumented path contract and no
  enforcement.** Entries must be repo-relative and resolve inside the base
  worktree. An absolute path was joined anyway — `worktree.join("/abs")`
  discards the worktree — and failed with a permission error against an
  unrelated temp directory; an escaping `..` failed the same way. Both are now
  rejected before anything is written, naming the config key and the rule.
  The contract is documented on the field itself.
- **A dependency copy that could not resolve was reported as reused.** The copy
  preserves symlinks, which is right for a default pnpm farm (relative links
  into `.pnpm/`, measured to resolve after the copy) and wrong for a tree that
  links outside itself — a workspace `link:` entry or a virtual store elsewhere
  — which copied faithfully and then failed in the base run with
  ERR_MODULE_NOT_FOUND while the note said the dependencies were reused. Dead
  links are now named in that note, with `pnpm install --node-linker=hoisted`
  as the documented workaround.
- **There was no way to read the config schema without creating it.** `init`
  discovered git first and refused outside a repository, so a user inspecting
  WitDiff's options had to run it inside the target repository — and the file it
  created then showed up as a production change in their next `inspect`.
  `witdiff init --print` prints what would be written, needs no repository, and
  writes nothing.

- **`install.sh --help` could not print its own usage through the documented
  one-liner.** `curl ... | sh -s -- --help` reads `$0` as the interpreter, so
  `sed "$0"` printed `sed: sh: No such file or directory` and exited 0 — and
  it downloaded every package tarball first, to show a usage block. Usage is
  now handled before the download, from a literal heredoc.

- **Targeted test selection built a command cargo rejects.** The changed-test
  targets were inserted before the subcommand (`cargo --test alpha test ...`),
  which the real binary answers with `error: unexpected argument '--test'
  found`. Every narrowed run therefore failed to start and reported
  `head_failed`. The targets now follow the subcommand, and the unit test that
  asserted the wrong order has been corrected and extended to the multi-target
  case.
- **Stale evidence left `reason: proven` on a downgraded receipt.** The
  staleness guard reset `status` to `not_verified` but not the machine
  `reason`, so a gate branching on the token read a stale run as a proof. The
  new `stale_evidence` reason travels with the status.
- **`--json` stdout was not machine-stable.** The `--install-toolchains` probe
  narration and the `--github-annotations` workflow commands were written to
  stdout, so `witdiff verify --json` with either flag could not be parsed as
  JSON at all. Both now go to stderr, where GitHub still sees the annotations.
- **`witdiff uninstall` failed on the Cursor integration.**
  `installed_integrations` returns `~/.cursor/rules/witdiff.mdc`, a file, but
  the removal loop called `remove_dir_all` unconditionally, so uninstall exited
  1 and left the file behind. Removal now dispatches on the path kind, and
  `removed` no longer counts a path it failed to remove.
- **`signing_failure_exit_code` was never read.** The key was documented and
  parsed, and an unsigned receipt exited 0 whatever the setting. The CLI now
  applies it when — and only when — a signing key was configured; the new
  additive `signing_requested` receipt field records that distinction.
- **A `[policy]` `block` override could escalate a finding past its natural
  severity.** `blocks()` short-circuited on a rule-level `Weight::Block`
  before looking at the severity, so a policy entry turned a `Warning` — which
  is reported but does not gate — into a gate failure. That contradicts the
  section's own doc comment and ADR-0026, which allows a policy to make a rule
  less blocking and nothing more. `Block` now means "do not waive this rule".
- **PHP could not see an attribute-style test.** PHPUnit 10 replaced `@test`
  with the `#[Test]` attribute, and a method carrying it summarized with zero
  functions — so a deleted attribute-style test reached `verified` with no
  integrity finding at all, while a real PHPUnit 13 ran it. Verified end to
  end against a real PHPUnit before and after the fix.
- **PHP dropped the polarity of a Pest expectation.** `notToBe` rendered an
  `Eq`, identical to `toBe`, and `->not->toBe(5)` rendered the bare subject, so
  inverting an expectation — the change that makes a passing test assert
  failure — reported nothing. The chain is now walked in full rather than
  stopping at its first member, so `->not`, `->and(...)` and `->skip()` are
  read too.
- **PHP could never report a skipped test.** Both `markTestSkipped()` and
  Pest's `->skip()` reported `skipped: false`, so `skipped_test` could not fire
  for PHP at all; `markTestSkipped` was additionally rendered as an assertion
  whose text was the skip message.
- **PHP predicate and ordered assertions dropped what they assert.**
  `assertTrue`/`assertFalse` rendered identically, and `assertGreaterThan`,
  `assertContains`, `assertCount` and friends rendered only one operand, so
  changing the subject, or replacing a subject with a value that makes the
  test pass, reported nothing.
- **A PHP helper whose name merely contains "test" was treated as a test.**
  `buildTestSubject()` or `getTestData()` are private helpers PHPUnit never
  runs, but renaming one produced a high-severity `removed_test` and failed
  the gate on a legitimate refactor.
- **The receipt schema forbade two operators the tool emits.**
  `condition_negation` and `numeric_substitution` shipped as real variants but
  were absent from `mutant_result.operator`, so any receipt produced with
  mutation enabled failed validation against the repository's own schema.
  Added, with a test that asserts both directions.

- **`doctor` reported a wrong framework as `ok`.** Two things combined.
  `Config::load` returned the Rust defaults whenever `witdiff.toml` was
  absent — and that is the state of every repository before its first
  `witdiff init` — so a node:test repository was classified `cargo` even
  though `inferred_for` already knew how to read a `package.json`. The line
  then printed `ok (cargo)`, because the check it performed is "does this name
  parse", which passes on a wrong value exactly as happily as on a right one.
  `load` now infers, and `doctor` distinguishes `inferred` from `ok` so a
  guessed classifier is never presented as a confirmed one.

- **`--keep-worktree` printed a path that did not exist.** Two independent
  causes, and the first one hid the second. Retention sat in the `Ok` arm of the
  experiment result, so a base control failure or an error part-way through
  returned before it, dropped the guard, and removed the worktree — the paths
  where an operator most wants the tree. Separately, the worktree was created
  *inside* a `TempDir`, which deletes its whole tree at the end of the run, so
  even a successful run left nothing at the path it had just named. A retained
  worktree now gets a directory nothing else cleans up, and retention happens
  immediately after the worktree is created. The existing test only checked that
  the note appeared, which is how both shipped; it now checks the directory.
- **`--no-write --json --output <path>` silently produced no file.** The receipt
  write is guarded by `if !no_write`, and `--output` is passed *into* the call
  that guard protects, so the path was discarded along with the write. Exit 2,
  no file, nothing on stderr. `--output` now conflicts with `--no-write`, so the
  pair is rejected at startup instead of being accepted and quietly ignored.

- **A node:test run was never recognised, and the advice sent the reader after
  the wrong problem.** The JavaScript classifier knew only Jest and Vitest,
  which share no vocabulary with the built-in runner: no `● ` glyph, no
  `Tests: N failed`. A genuine `ERR_ASSERTION` on the base run therefore
  classified as not-a-test-failure, so a correct red/green change reported
  `unrecognized_failure_kind` and the whole Node ecosystem on the built-in
  runner was uncovered. `--test-reporter=tap` did not help, though TAP is a
  standard format; both reporters are now recognised. Measured against node
  v26.7.0. A green run is explicitly still not a failure, since both reporters
  print a `fail` line on success.
- **The `next` line diagnosed a cause WitDiff had not established.** One string
  served every non-verified status, so `unrecognized_failure_kind` — where the
  tool knows only that it could not read the output — was told "the test passes
  on the base revision too... add an assertion that fails without the change",
  which is the opposite of what happened. The same sentence was printed for a
  base-control failure, where adding an assertion cannot help because the base
  suite is already red. The advice now follows `reason`, and a cause WitDiff
  does not understand is reported as unread rather than diagnosed.

- **Python: a `raises` block was not an assertion, and reported the test as
  empty.** `with pytest.raises(...)` and `with self.assertRaises(...)` were
  collected nowhere, so a test built from one summarized with zero assertions
  *and* zero guards: deleting the block produced a summary byte-identical to
  keeping it, and the test was separately reported `assertion_free_test`,
  which is false — it fails when nothing is raised. `if not ok: raise
  AssertionError` — the exact case `testshape.rs:57` names as one that must be
  seen — was also uncollected. Both are now guards. A plain `raise ValueError`
  is still not a guard: that is the code under test, not a check. Measured with
  a real interpreter before the fix.
- **Java: `assertThrows` rendered as nothing.** Its argument is an `Executable`
  lambda, which the JDK parser never descends into, so the call produced no
  assertion and the test was reported `assertion_free_test` — false, since
  assertThrows fails when the exception is not raised. It now renders as an
  exception expectation, the same form the other analyzers use. Measured with a
  real JDK before the fix.

- **Go: testify assertions were invisible.** `failureCall` recognised only
  the six `*testing.T` methods, so `assert.Equal`, `require.NoError` and
  `assert.True` were not counted as assertions *or* as guards — a testify-only
  test summarized with both empty. Deleting its last assertion was therefore
  indistinguishable from an unchanged test, and it reported nothing. Measured
  against the real Go toolchain before the fix. `assert` and `require` are now
  rendered as the comparison they make, and deliberately identically: they
  differ only in whether the test aborts, not in what it claims.

- **Go: a guard negation is reported as a changed expectation.** Go has no
  assertion keyword, so the guard condition *is* the assertion and the engine
  compares it whole; `if isEven(2)` -> `if !isEven(2)` therefore reads as a
  changed expectation rather than as the same check. Reported in both
  directions, and measured with the real Go toolchain before and after. This
  is now pinned by a test rather than changed: a variant fix that stripped the
  `!` also silenced the genuinely weakening direction, which is a worse failure
  than a blunt one.

### Documentation
- Removed the stale "22 ADRs" count from the README, replaced "a real parser
  for all seven languages" with wording that is true of PHP's tokenizer and
  JS's project-supplied parser, and fixed a mangled signing note in `verify.rs`.
- `support-matrix.md` said capabilities 2 and 3 are Rust-only while its own
  matrix listed structural integrity findings for every language, and listed
  JUnit and RSpec as frameworks that "fail loudly" while recognising them as
  supported. Both corrected.
- AGENTS.md claimed CI runs the `--ignored` suite "with every toolchain
  installed". CI has no PHP step, so the three PHP end-to-end tests skip. Now
  says so and points at roadmap M9.

## 1.0.0-alpha.4

Three things an agent can act on without reading English: a machine-readable
`reason` on every receipt, a `[policy]` section for waiving a rule's effect on
the gate, and an update notice that names a command you can actually run.

### Added

- **`witdiff uninstall`.** The binary can now remove itself and the agent
  integrations it installed, with `--dry-run` and `--keep-integrations`. The
  installer had printed `install.sh --uninstall`, but the documented install
  path is `curl ... | sh`, which saves no copy of the script: measured from a
  real shell, that command answered `zsh: command not found: install.sh`.
  Anything a user must run to undo an install has to work with what the install
  left behind. It removes only what WitDiff created — a skill the user wrote is
  reported and left alone.
- **`[policy]` in `witdiff.toml`: per-rule weights and path ignores.** A
  repository can now record that it has accepted a particular weakening, or
  that a directory's findings are not its own, instead of leaving the gate red.
  A policy changes the **verdict**, never the **finding**: a waived finding is
  still observed, still reported at its original severity, and the receipt lists
  it in `waivers` with the reason. Every override requires a `reason`, enforced
  by the type. See ADR-0026.
- **Every receipt carries a machine-readable `reason`.** `status` said what
  happened; `reason` says why, as one stable token. `not_verified` alone covered
  four unrelated causes with four different remedies, so an agent had to
  string-match the English in `notes` to decide what to do — and rewording a
  note would silently change the meaning. The token is printed by `verify`
  (human and JSON), returned by the MCP `verify` tool alongside
  `reason_is_actionable_by_author` and `remediation`, documented in
  `docs/receipt.md`, and enumerated in the receipt schema. Additive in v1:
  a receipt written before the field exists reads as `unrecorded`, which claims
  nothing. See ADR-0025.
- **`witdiff doctor` reports the installed version and, when a newer release
  exists on the same channel, says so.** It never installs anything and never
  runs during `verify` — see ADR-0024. Silent when offline, cached for a day
  outside the repository, and disabled by `WITDIFF_NO_UPDATE_CHECK=1` or
  `check = false` under `[updates]` in `witdiff.toml`.

### Fixed

- **`install.sh --uninstall` refused a directory that `--to` had installed
  into.** `installed_path` returned `$DEST/witdiff` and the removal guard then
  rejected that same path as "outside the known install locations", so an
  install made with the two flags the README documents could not be undone with
  them. Found by running the documented commands against the published
  `v1.0.0-alpha.3` release rather than reading the script.
- **The Windows build did not compile.** `basedeps.rs` called
  `std::os::unix::fs::symlink` unconditionally, which broke
  `x86_64-pc-windows-msvc`. Three further unguarded `std::os::unix` uses were
  latent in test code. All now behind `#[cfg(unix)]` or a platform-aware helper.

## 1.0.0-alpha.3

PHP support, and two false-proof fixes. The headline is that PHP projects can now
adopt WitDiff for its full value rather than for the proof alone; the second is
that one of these fixes was itself the bug it was meant to remove.

### Added

- **`verification.base_dependency_dirs`**, so the base experiment can start in a
  project that gitignores its dependencies. A `git worktree` contains only
  committed files, so a Composer/`npm`/Python project's base control previously
  failed with `Could not open input file: vendor/bin/phpunit` and the receipt
  blamed the base revision for a failure that was really missing dependencies.
  Opt-in, and guarded by the lockfile: if a lockfile changed between base and
  head, nothing is reused and the receipt says so. The dependencies are
  **copied**, never symlinked — see Fixed.
- **`assertion_free_test`**, for a newly added test that contains no assertion
  and no exception expectation. `removed_assertion` only fires when the base
  version had assertions, so a brand-new test that cannot fail was invisible.
  Warning severity, deliberately not High: it reports without blocking, because
  a repository may legitimately contain such a test.
- **PHP support: PHPUnit and Pest.** A PHP project previously received the
  red/green proof but **no integrity findings at all** — the rules that catch a
  change weakening its own tests did not run. PHP now joins the shared rule
  engine (`testshape`) as a seventh language, using `token_get_all` from the
  standard library, so no Composer dependency is required for the analysis.
  Both `assertEquals(expected, actual)` and `expect($x)->toBe($y)` normalize to
  subject-first so a changed expectation is reported as a change rather than as
  a removal plus an addition. Verified against real PHPUnit 10.5.66 and Pest 3:
  a credible PHP test reaches `verified` with `red_green_proven: true`, and a
  vacuous one that plain PHPUnit calls green is reported `not_verified`. See
  ADR-0023.
- **PHP project detection.** `composer.json`, `phpunit.xml`, `phpunit.xml.dist`
  and `pest.php` are recognized by `witdiff init`, checked before
  `package.json` because a WordPress plugin commonly ships both.

### Fixed

- **The base experiment could run the *head* revision's code.** The first
  implementation of `base_dependency_dirs` symlinked the workspace's dependency
  directory into the base worktree. Composer's generated autoloader computes
  `$baseDir = dirname($vendorDir)`, so through a symlink that resolved back to
  the workspace: measured on a base computing `a - b` with a test asserting
  `add(2, 3) == 5`, the base experiment reported `OK (2 tests, 2 assertions)`.
  A false proof in the direction that hides a regression. Dependencies are now
  copied.
- **A gutted PHP test could reach `verified`.** A WordPress-style test whose only
  check is `expectException` was summarized with **zero assertions**, so deleting
  that guard was invisible to the integrity rules. Combined with a change that
  made the code stop throwing, the test passed while asserting nothing — and
  PHPUnit itself flags that shape (`Assertions: 0, Risky: 1`). An exception
  expectation now counts as an assertion, so the existing "no longer asserts
  anything" rule fires. No new rule was needed; the summary was reporting
  nothing for the engine to compare.
- **A PHP verification failed its own freshness gate on the suite's cache.**
  `.phpunit.result.cache` and Pest's
  `vendor/pestphp/pest/.temp/test-results` are created by the act of running
  the tests, and were not classified as build output, so a green run reported
  that the source under verification had moved.
- **A Pest test that errors on something other than an assertion could not
  produce a proof.** Pest prints neither `FAILURES!` nor `failed asserting
  that`, so such a run classified as `CommandFailure` — which cannot prove
  anything. Pest projects silently lost their red/green evidence while
  appearing to work.
- **`every_supported_framework_recognizes_its_own_failure_output` no longer
  covers every framework.** It enumerated four in a hand-written list and was
  never extended when Java and Ruby shipped; adding PHP did not extend it
  either. It is now driven by `TestFramework::all()` with a length assertion.

## 1.0.0-alpha.2

Installing works. The first alpha's installers were broken in ways only running
them revealed, and this fixes those along with two verification gaps.

### Fixed

- **`curl ... | sh` installed nothing.** When a script is piped into a shell,
  `$0` is the interpreter, so the integrations installer looked for its packages
  beside `/bin`, found none, copied nothing, and **exited 0** — a success report
  for an install that did not happen. It now detects that its own directory lacks
  the packages and fetches them.
- **The binary installer never handled Windows.** `detect_target` had Linux and
  Darwin branches only, so a Windows user under Git Bash was told there was no
  prebuilt binary, for a platform the release publishes.
- **Pi recorded a deleted path.** A piped install gave Pi a temporary directory
  that the cleanup trap removed, so the recorded source resolved to nothing the
  moment the install finished. It now uses a stable git source.
- **The uninstall hint named a temporary path**, in the one message whose purpose
  is telling someone how to undo an install.
- **`stability` was computed and never written to the receipt.** `run_impl`
  classified the outcome on `RunOutcome` while the receipt serializes the
  `RunResult`, so the field existed and always held its default.
- **`red_green_proven` was set before any stability check**, claiming a proof from
  a run that could have been a flake.

### Added

- **Real installers, with uninstall.** `install.sh` fetches the release for the
  current platform, verifies it against `SHA256SUMS`, and fails closed on a
  mismatch. It falls back across `sha256sum`, `shasum` and `openssl` so no single
  platform's tooling is required, never uses `sudo`, and reports an unwritable
  destination rather than escalating.
- **Harness detection.** The integrations installer now installs only the
  harnesses found on `PATH`, with `WITDIFF_INSTALL_ALL=1` to force every one.
  Finding none installs all of them, because "not on my PATH" is not "not used".
- **Uninstall for both installers.** Removals are aimed only at paths the
  installer created, and shared directories are emptied with `rmdir` so a skill
  you wrote yourself survives.
- **Repeated runs, so a flaky suite cannot become a proof.**
  `verification.flake_repeats` (default 1, the previous behaviour) repeats a
  failing run and reports disagreement as unstable. Only failures are repeated,
  the retained result is the failure, and each run records `stability` as
  `single_run`, `stable` or `unstable`.

## 1.0.0-alpha.1

First prebuilt release. The proof semantics are unchanged from 1.0.0; what this
adds is the work needed to put WitDiff in front of someone who is not building
it from source — installable harness packages, a release pipeline, and the
verification gaps closed against real toolchains.

It is an **alpha**: the four open items in `docs/roadmap.md` are feature scope,
not defects, and the Java and RSpec paths were verified against live toolchains
for the first time during this cycle. Use it, but read
`docs/support-matrix.md` for what is proven per language rather than assuming
the feature list means uniform coverage.

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

### Added

- **Repeated runs, so a flaky suite cannot become a proof.**
  `verification.flake_repeats` (default 1, the previous behaviour) repeats a
  failing run and reports disagreement as an unstable suite. Only failures are
  repeated — a pass has nothing to distinguish — and the retained result is the
  failure, so an unstable suite can never read as green. Each run records
  `stability`: `single_run`, `stable` or `unstable`, three-valued because "run
  once and it failed" and "run three times and it failed every time" are
  different amounts of evidence.

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
