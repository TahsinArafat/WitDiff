# Quickstart

Not sure whether your language is covered? Read the
[support matrix](support-matrix.md) first. The red/green proof works for Rust,
Python, Go and JavaScript; structural test-integrity analysis works for Rust and
Python; mutation analysis is Rust-only.

## Build

```bash
cargo build --workspace
cargo test --workspace
```

Install locally:

```bash
cargo install --path crates/witdiff-cli
```

## Configure a repository

```bash
cd your-project
witdiff init
```

`init` detects the project type from its manifest and writes a matching
configuration: `cargo test` for Rust, `python3 -m pytest` for Python, `go test
./...` for Go, `npm test` for JavaScript. Edit `witdiff.toml` if the command or
the dedicated test globs differ.

Then check the setup before running anything:

```bash
witdiff doctor
```

`doctor` verifies that git is present, that the configured test binary exists,
and that the configured `framework` is recognized. It exits 2 if anything is
wrong, so it works as a preflight step in CI.

## Inspect classification

```bash
witdiff inspect --base origin/main
```

Confirm that changed dedicated tests are listed as `T` and production files as `P`.

## Verify

```bash
witdiff verify --base origin/main
```

For an agent or CI gate:

```bash
witdiff verify --base origin/main --strict --json
```

Exit codes are three-way, not two: `0` the claim was proven or there was nothing
to prove, `1` WitDiff itself failed to run, `2` verification did not pass. Exit
2 is a failed gate, not a crash to retry.

Output ends with a `next` line giving the action for the resulting status. Every
status and its meaning:

| status | meaning |
| --- | --- |
| `verified` | the changed tests fail on the base revision and pass here |
| `verified_with_warnings` | proven, with integrity findings to read |
| `no_changed_tests` | no dedicated test changed, so no proof was attempted; not a failure |
| `not_verified` | a proof was attempted and did not establish the claim |
| `head_failed` | the test command fails on the current workspace |
| `base_incompatible` | the transplanted test does not compile against the base revision |

The most common `not_verified` cause is a test that also passes on the base
revision, which means it does not pin the new behavior. Add an assertion that
fails without the change.

## Read the last receipt

```bash
witdiff receipt
witdiff receipt --json
```

## Dogfood the included end-to-end smoke scenario

From the WitDiff repository:

```bash
./scripts/smoke-demo.sh
```

The script creates a temporary Rust repository with a real bug, fixes it in the working tree, adds a regression test, and expects WitDiff to establish:

```text
HEAD                 PASS
pristine BASE        PASS
BASE + changed test  FAIL
=> VERIFIED
```
