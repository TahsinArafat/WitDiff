# Quickstart

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

Edit `witdiff.toml` if your test command or dedicated test paths differ from the defaults.

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
