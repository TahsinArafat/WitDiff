# Roadmap

The roadmap is organized around proof quality, not feature count.

## M0 — repository foundation

- [x] Rust workspace and CLI
- [x] config loading
- [x] Git repository/base discovery
- [x] changed-file classification
- [x] dedicated test classification
- [x] JSON receipt model
- [x] agent documentation

## M1 — red/green proof (implemented in starter)

- [x] run HEAD test command
- [x] create detached base worktree
- [x] transplant tracked dedicated tests
- [x] copy untracked dedicated tests
- [x] run pristine-base control command
- [x] run identical command on base+changed-tests
- [x] distinguish test failure vs compile failure
- [x] remove worktree
- [x] workspace fingerprint freshness
- [x] basic integrity findings
- [x] strict/non-strict exit behavior

### M1 hardening — first agent tasks

- [x] add Git inspection integration test using a temporary repository
- [x] add full red/green end-to-end integration tests using a deterministic fixture runner
- [x] ensure worktree cleanup also occurs if patch application or test execution fails
- [x] verify behavior with spaces/non-UTF8 paths where platforms permit
- [x] improve renamed/deleted test handling
- [x] add bounded execution timeout without risking stdout/stderr pipe deadlock
- [x] normalize status display to stable kebab/snake strings instead of Rust debug output

## M2 — Rust-aware test analysis

- [x] parse Rust syntax instead of line heuristics
- [x] detect weakened comparisons, changed expected values, removed match arms
- [x] associate changed tests with test names
- [x] run only changed tests where equivalence is safe
- [x] support inline `#[cfg(test)] mod tests` transplant safely (ADR-0010)

See ADR-0006 (structural analysis), ADR-0007 (test-only transplant boundary),
ADR-0008 (targeted selection refuses rather than approximates), ADR-0009
(NUL-delimited Git paths), and ADR-0010 (inline test transplant).

## M3 — changed-code mutation proof

- [x] identify changed functions
- [x] mutation operators: equality flip, comparison boundary, logical flip, boolean literal flip, condition negation, numeric substitution
- [x] run changed tests against mutants
- [x] receipt fields for generated/killed/survived/not_compiled/timeout/skipped mutants
- [x] deterministic mutant IDs
- [x] cache by source/test fingerprint

All six planned operators are implemented. Mutation is opt-in
(`verification.mutation`) and supplementary: it never changes `status`. See
ADR-0011.

## M4 — language adapters

- [x] pytest (failure classification and structural integrity analysis)
- [x] Go test (failure classification and structural integrity analysis)
- [x] Java / JUnit (failure classification and structural integrity analysis)
- [x] Ruby / Minitest / RSpec (failure classification and structural integrity analysis)
- [x] Vitest/Jest (failure classification and structural integrity analysis)
- [x] test framework capability trait (classification and optional targeted invocation)
- [x] framework-specific failure classification

Verified end to end against real toolchains: pytest, Go, Java, Ruby/RSpec and
JavaScript/TypeScript repositories each reach `verified` and report integrity
findings, where before ADR-0012 the same pytest failure classified as
`CommandFailure` and could not produce a proof at all.

Structural integrity analysis exists for **Rust (ADR-0006), Python (ADR-0016),
Go (ADR-0017), Java (ADR-0018), Ruby (ADR-0020) and JavaScript/TypeScript
(ADR-0021)**. All share one rule engine (`witdiff_core::testshape`), so no
language can disagree about what a weakening is.

Every language above is now verified against its **real** parser or runner, not
a hand-written fixture. Doing so found nine bugs — six in JavaScript, two in
Ruby's classifier, and one in the shared rule engine that had affected every
language using it.

`verify_languages_end_to_end.rs` additionally drives the whole red/green chain
against real temporary repositories for **all five**: pytest, Go, Ruby/RSpec,
JavaScript/TypeScript and Java, each reaching `Verified`. The Java toolchain is
derived from the changed `.java` files as well as from the configured command,
so a project running a committed wrapper script no longer loses structural
analysis. See `docs/support-matrix.md`.

Still open:

- **Targeted invocation** for non-Rust frameworks, so the full suite runs and
  the receipt says `full_suite`.
- **Mutation** outside Rust; the operators are defined over Rust syntax.

Closed since:

- [x] **Gitignored dependencies in the base worktree.** `git worktree` contains
  only committed files, so a project whose dependencies are gitignored
  (`vendor/`, `node_modules/`, `.venv/`) could not start its test command on the
  base revision, and the receipt reported `base control: FAIL` — which reads as
  "the base is broken" rather than "the base could not start". Closed by
  `verification.base_dependency_dirs`, opt-in and guarded by the lockfile. The
  dependencies are **copied**, not symlinked: a symlink let Composer's
  autoloader resolve `dirname(vendor)` back to the workspace, so the base
  experiment ran head's code. Measured, then fixed.

## M5 — ecosystem integrations

- [x] GitHub Actions reusable workflow (`.github/workflows/witdiff.yml`)
- [x] PR annotations/check summary (`--github-annotations`; see ADR-0013)
- [x] universal agent skill/instruction package (`examples/agent-instruction.txt`)
- [x] installable harness packages: a Claude Code skill, an OpenCode plugin with
  a callable tool, a Pi package, and a Cursor rule (`integrations/`), with an
  installer and a test asserting each harness's loading conventions
- [x] prebuilt binaries per platform on GitHub Releases
- [x] a runnable demo (`demo/run.sh`) showing a vacuous test being caught, and an
  agent-driven walkthrough (`demo/README.md`) (`.github/workflows/release.yml`)
- [x] MCP server wrapping core methods (`crates/witdiff-mcp`, ADR-0014)
- [x] content digest and revision binding for receipts (ADR-0015 prerequisites)
- [x] signature over the verification digest (ADR-0022)

CI gating treats "nothing to prove" as distinct from "the proof failed", so a
documentation-only pull request no longer fails the check. The policy lives in
the CLI rather than the workflow file, so CI, MCP and local scripts agree.

Signed receipts are implemented (ADR-0022, superseding the ADR-0015 design). The
signature is a detached Ed25519 signature over the domain separator, the status
and the content digest, computed by `ed25519-dalek` in Rust — no external
runtime. It is tamper-evidence, not non-repudiation, and the operator supplies
the key; WitDiff never creates or stores one.

## M6 — advanced evidence

- [x] coverage of changed lines as evidence (not as sole proof)
- [x] dependency/version/environment fingerprint
- [x] sandboxed verification runner
- [x] provenance chain across multiple verification stages
- [x] policy file for organization-specific gates

Shipped so far in M6:

- **Environment evidence** — the receipt records the configured test program, its
  version, the toolchain that participates in it, and a digest of every
  dependency manifest present. It answers *where* the evidence came from, which
  the digest deliberately does not, and it is kept out of the digest so that
  upgrading Python cannot make an older receipt report itself stale.
- **Gate policy** — `[gate]` in `witdiff.toml` commits which results pass, so a
  repository states it once rather than restating flags on every invocation.
  Flags may only tighten it.
- **Flake detection** — `verification.flake_repeats` repeats a failing run and
  reports disagreement as unstable rather than as evidence. Only failures are
  repeated, the retained result is the failure, and each run records whether its
  outcome was a single sample or confirmed by agreement.
- **Coverage of changed lines** — the receipt reports how many of the lines this
  change added the tests executed, with per-file detail.

  This item originally said *changed branches*. Measured: on the stable
  toolchain this workspace pins, `cargo llvm-cov --json` reports
  `branches: {count: 0}` for a file with an if/else, and `--branch` fails
  outright — branch instrumentation needs `-Zbranch-coverage`, which is nightly.
  The wording is corrected to match what a stable toolchain can produce rather
  than left claiming something the receipt cannot carry. Branch coverage
  returns if the project ever moves to nightly, and the report has room for it. Counts added lines from
  a `-U0` patch rather than whole files, is a separate opt-in run, and never
  touches the verdict.
- **Provenance chain** — `.witdiff/provenance.json` links each receipt's digest
  to the one before it, so a sequence of verifications can be checked as a
  sequence rather than one at a time. Editing, dropping or reordering an entry
  breaks the links that follow, and both `verify` and `receipt` report it.
  Verified against a real tampering: rewriting a recorded digest is caught.
- **Sandboxed runner** — `sandbox_image` rewrites every run into a named
  container, so the candidate-controlled `test_command` no longer executes on the
  host. Named so a timeout can remove it rather than leaving it running; an
  allow-listed environment; `--network none` on request. The image must carry the
  toolchain, because which one a project needs cannot be inferred.


## M7 — agent-facing contract

WitDiff's primary callers are coding agents, not people reading a terminal. That
changes what "finished" means: a verdict an agent cannot act on is a verdict it
will ignore, and an agent that ignores a `not_verified` is worse served than one
that never ran the tool.

- [x] machine-readable `reason` on every receipt (ADR-0025)
- [x] `[policy]` for waiving a rule's effect on the gate (ADR-0026)
- [x] self-uninstalling binary, so no user needs a copy of the install script
- [ ] a `reasons.md` page an agent can be pointed at, mirroring `docs/receipt.md`
- [ ] exit-code and reason coverage in every harness skill, asserted rather than
      assumed

Shipped so far in M7:

- **`reason`** — one stable token per cause, because `not_verified` alone covered
  four unrelated situations with four different remedies. Before it, an agent
  had to substring-match the prose in `notes` to decide what to do, which meant
  rewording a note silently changed a caller's behaviour. Deliberately distinct:
  `test_command_unavailable` and `base_control_failed` both end in
  `not_verified`, and one is a toolchain problem while the other is the code.
- **`[policy]`** — a repository with a legitimate exception could previously
  only leave its gate red forever or stop running WitDiff. It can now record the
  decision, with the constraint that a waiver changes the verdict and never the
  observation: the finding stays in `integrity_findings` at its severity, and
  the receipt lists it in `waivers` with the reason.
- **`witdiff uninstall`** — measured from a real shell: the installer printed
  `install.sh --uninstall`, but the documented install path is `curl ... | sh`,
  which saves no copy, and the command answered `zsh: command not found:
  install.sh`. Anything a user must run to undo an install has to work with what
  the install left behind.

## M8 — closing the language gap

Three capabilities are Rust-only. Each is a real reduction in value for a
non-Rust project, and each is documented in `docs/support-matrix.md` rather than
quietly omitted.

- [ ] **Targeted test invocation outside Rust.** Cargo's `--test <target>` is
      implemented (ADR-0008); every other framework runs the full suite and the
      receipt says `full_suite`. Narrowing must be provably equivalent to what
      the framework would otherwise run, or it must refuse. ADR-0008 is the
      precedent: the mechanism exists, and refusing is a correct answer.
- [ ] **Mutation outside Rust.** The operators are defined over Rust syntax
      (ADR-0011). Other languages get red/green proof and structural analysis
      but no mutation signal, which is the strongest evidence WitDiff produces.
- [ ] **Mock substitution around changed behaviour**, the last item from the
      original integrity list.

## M9 — trust and distribution

- [ ] **A benchmark against real agent-generated pull requests.** The question
      "how often does this catch a vacuous test?" has an honest answer only with
      a corpus. No number has been published, and none should be invented —
      a fabricated figure would be exactly the unfalsifiable claim this project
      refuses elsewhere.
- [ ] **Exercise the release pipeline with a stable tag.** Every release so far
      is a pre-release, and the publish job has twice been cancelled while
      queued on macOS and Windows runners. A stable `v1.0.0` should be cut only
      once a second and third release have gone through cleanly.
- [ ] **Run one verification inside `sandbox_image` end to end.** The runner is
      implemented and tested for container naming, removal and environment
      allow-listing, but a full verification inside a sandbox needs an image
      carrying a Rust toolchain and none is available locally. Until then the
      claim is "the rewrite is correct", not "a sandboxed verification works".
- [ ] **Signed releases.** Receipts can be signed (ADR-0022); the binaries that
      produce them are not. A signature over the artifact a user downloads is
      the missing half.
- [ ] **PHP in CI.** Three end-to-end tests skip without `WITDIFF_PHPUNIT_VENDOR`
      set, and CI has no PHP step, so the PHP path is exercised locally rather
      than on every change.

## Explicitly postponed

- cloud control plane;
- generic agent orchestration;
- AI-generated correctness scores;
- automatic code fixes;
- dashboard before receipt semantics are mature.
