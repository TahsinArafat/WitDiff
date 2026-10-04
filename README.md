# WitDiff

**Deterministic verification for AI-written code.**

WitDiff is a verification layer for coding agents and humans. It does not generate code and it does not decide whether code is correct by asking an LLM. Instead, it produces executable evidence that a change is actually supported by the tests that accompany it.

The **red/green proof** works for Rust, Python (pytest), Go, Java (JUnit via Maven or Gradle), Ruby (Minitest, RSpec) and JavaScript/TypeScript (Jest, Vitest). **Structural test-integrity analysis** works for Rust, Python, Go and Java: it warns when a change weakens its own tests, such as `assert result == expected` becoming `assert result`. Mutation analysis is Rust-only. See the [support matrix](docs/support-matrix.md) for exactly what is and is not covered per language, including an assessment of Java, .NET, PHP, Ruby and TypeScript.

The v0.1 proof is intentionally narrow and useful:

1. identify changed dedicated test files;
2. run the configured test suite against the current workspace (**green**);
3. run the untouched base as a control and require it to be green;
4. transplant only the changed tests onto the chosen base revision;
5. run the same command against base+changed-tests (**red**);
6. reject compile-only failures as insufficient behavioral evidence;
7. flag suspicious test weakening such as removed assertions or newly ignored tests;
8. fingerprint the workspace before and after verification so stale evidence is not silently accepted;
9. write a machine-readable receipt that Claude Code, Codex, Pi, OpenCode, CI, or a human can consume.

## Why

A coding agent can say “tests pass” even when the new test would also pass before the fix, when it weakened an assertion, or when the tests were run before a final edit. WitDiff makes those claims independently checkable.

```text
coding agent / human
        |
        v
    code change
        |
        v
    WitDiff
   /    |     \
 HEAD  BASE  integrity
 pass  fail   checks
   \    |     /
        v
  JSON proof receipt
```

## Current status

This starter implements a functional **Rust-first v0.1**. It is deliberately vendor-neutral and network-independent during verification.

Implemented:

- Git repository discovery
- automatic base selection (`origin/main`, `main`, `origin/master`, `master`, `HEAD~1`)
- changed-file classification
- configurable dedicated-test glob detection
- tracked and untracked changed-test support
- temporary detached Git worktree for the base revision
- test-only patch transplantation
- HEAD test execution
- BASE test execution
- optional targeted cargo test selection that records exactly what ran
- cargo failure classification (test failure vs compile failure)
- red/green proof status
- syntax-aware Rust test-integrity rules (removed/changed/weakened assertions, removed tests, unparsable files)
- line-oriented integrity rules for non-Rust test files
- NUL-delimited Git path handling so non-ASCII paths are classified correctly
- a test-only transplant boundary (no production diff enters a test transplant)
- workspace evidence fingerprinting
- JSON receipt persistence
- human and JSON CLI output
- `init`, `doctor`, `inspect`, `verify`, and `receipt` commands
- an MCP server (`witdiff-mcp`) exposing `inspect`, `verify` and `receipt` to agents
- a reusable GitHub Actions workflow and `--github-annotations` for PR checks

Rust inline `#[cfg(test)] mod tests` blocks are transplanted by span splicing: only the test module's bytes move, so production changes elsewhere in the same file stay at the base revision. A file is refused, and the reason recorded, when it cannot be spliced safely (`unparsable`, `no_counterpart_in_base`, `test_outside_test_module`). See ADR-0010.

Targeted test selection is cargo-only and opt-in (`verification.targeted_test_selection`, default `false`). When the configured command cannot be narrowed without changing what runs, WitDiff runs the full suite and records why. See [`docs/verification-model.md`](docs/verification-model.md).

Failure classification is framework-specific: cargo, pytest, Jest/Vitest and Go are recognized, selected with `verification.framework` (default `cargo`). Before this, a genuine pytest, Jest or Go test failure classified as an unrecognized command failure, which yields `not_verified` instead of a proof. See ADR-0012.

Changed-code mutation is opt-in (`verification.mutation`, default `false`) and **supplementary**: it never changes `status`. Each mutant is classified `killed`, `survived`, `not_compiled`, `timeout` or `skipped`, and only the first two are decisions about test strength — a mutant that failed to build was never executed and is not counted as a kill. See ADR-0011.

## Build

```bash
cargo build --workspace
cargo test --workspace
cargo install --path crates/witdiff-cli
```

## First use

From a Git repository with a Rust project:

```bash
witdiff init
witdiff doctor
witdiff inspect --base origin/main
witdiff verify --base origin/main
```

Strict mode is intended for CI/agents:

```bash
witdiff verify --base origin/main --strict --json
```

Exit codes:

- `0`: verification satisfies the requested mode;
- `1`: WitDiff itself failed to run;
- `2`: verification completed but the change was not sufficiently proven.

By default the receipt is written to:

```text
.witdiff/receipt.json
```

## Example result

```text
WitDiff verification
  status           : Verified
  base             : origin/main
  head             : 55cc9b...
  changed tests    : 1
  head tests       : PASS
  base control     : PASS
  base + tests     : FAIL
  red/green proven : true
  evidence fresh   : true
```

If the changed test passes on both revisions:

```text
status: NotVerified
note: changed tests also pass on the base revision; they do not prove the behavioral change
```

If the test fails to compile against base:

```text
status: BaseIncompatible
```

WitDiff v0.1 intentionally does **not** count compilation failure as proof of a regression because it does not demonstrate the intended behavioral failure.

## Agent compatibility

The core contract is just a CLI plus JSON, so no agent-specific SDK is required.

```text
Claude Code  -> shell -> witdiff verify --strict --json
Codex        -> shell -> witdiff verify --strict --json
Pi           -> shell -> witdiff verify --strict --json
OpenCode     -> shell -> witdiff verify --strict --json
CI           -> shell -> witdiff verify --strict --json
Human        -> shell -> witdiff verify
```

See [`AGENTS.md`](AGENTS.md) and [`docs/agents.md`](docs/agents.md).

## Design rules

1. **Deterministic evidence before model judgment.**
2. **An LLM may interpret a receipt, but it never creates a VERIFIED state.**
3. **Verification belongs to an exact workspace fingerprint.**
4. **Core verification must work offline.**
5. **Vendor integrations are thin wrappers around the same core CLI/API.**
6. **Unknown evidence is reported as unknown; never silently upgraded to proof.**

## Development roadmap

The next milestones are intentionally ordered so agents can work independently:

- the remaining mutation operators (condition negation, numeric return substitution);
- resolving `let` bindings so a rebound subject is not reported as a removal;
- targeted invocation and structural integrity analysis for the non-Rust adapters;
- receipt signing/attestation;
- signed receipt attestation.

See [`docs/roadmap.md`](docs/roadmap.md) for concrete work items.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) for the full text.
