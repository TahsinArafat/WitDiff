#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/debug/witdiff"

cargo build --manifest-path "$ROOT/Cargo.toml" -p witdiff

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
REPO="$TMP/demo"
mkdir -p "$REPO/src" "$REPO/tests"

cat > "$REPO/Cargo.toml" <<'TOML'
[package]
name = "witdiff-smoke-demo"
version = "0.1.0"
edition = "2021"
TOML

cat > "$REPO/src/lib.rs" <<'RS'
pub fn is_even(value: i32) -> bool {
    value % 2 == 1
}
RS

cat > "$REPO/tests/smoke.rs" <<'RS'
#[test]
fn unrelated_base_test() {
    assert_eq!(2 + 2, 4);
}
RS

git -C "$REPO" init -q
git -C "$REPO" config user.email witdiff@example.invalid
git -C "$REPO" config user.name "WitDiff Smoke"
git -C "$REPO" add .
git -C "$REPO" commit -qm base

cat > "$REPO/src/lib.rs" <<'RS'
pub fn is_even(value: i32) -> bool {
    value % 2 == 0
}
RS

cat > "$REPO/tests/regression.rs" <<'RS'
use witdiff_smoke_demo::is_even;

#[test]
fn even_numbers_are_even() {
    assert!(is_even(2));
    assert!(!is_even(3));
}
RS

cat > "$REPO/witdiff.toml" <<'TOML'
[project]
language = "rust"

[verification]
base = "HEAD"
test_command = ["cargo", "test"]
test_globs = ["tests/*.rs", "tests/**/*.rs"]
extra_test_paths = []
block_on_integrity_findings = true
max_output_bytes = 16384
TOML

echo "== WitDiff smoke demo =="
"$BIN" --repo "$REPO" inspect --base HEAD
"$BIN" --repo "$REPO" verify --base HEAD --strict

echo
echo "Smoke demo passed: HEAD green, pristine base green, base+regression red."
