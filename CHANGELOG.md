# Changelog

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
