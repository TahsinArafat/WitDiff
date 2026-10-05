#!/bin/bash
# A runnable demonstration: WitDiff catches a test that plain CI accepts.
#
#   ./demo/run.sh
#
# Builds a small repository where an agent has "fixed" a bug and added a test
# that accompanies the fix. The test passes on the workspace, so `cargo test` is
# green and a reviewer skimming the diff sees a fix plus a test.
#
# It proves nothing. The test passes on the buggy revision too, so it cannot
# demonstrate that anything changed. WitDiff is what notices.
#
# Everything here is real: a real Git repository, real cargo builds, the real
# binary. Nothing is staged output.

set -eu

REPO_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

WITDIFF="${WITDIFF:-witdiff}"
if ! command -v "$WITDIFF" >/dev/null 2>&1; then
  echo "error: witdiff is not on PATH. Install it first:" >&2
  echo "  curl -fsSL https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/install.sh | sh" >&2
  exit 1
fi

pause() {
  printf '\n\033[2m%s\033[0m\n' "$1"
  sleep "${DEMO_PAUSE:-1}"
}
rule() { printf '\033[2m%s\033[0m\n' "------------------------------------------------------------"; }
head2() { printf '\n\033[1m== %s\033[0m\n' "$1"; }

cd "$WORK"
git init -q .
git config user.email demo@example.invalid
git config user.name "Demo"
printf 'target/\n/.witdiff\n' > .gitignore

mkdir -p src tests
cat > Cargo.toml <<'EOF'
[package]
name = "demo"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
EOF

# ---------------------------------------------------------------------------
printf '\033[1mWitDiff demonstration\033[0m\n'
rule
echo "A bug is fixed. A test is added alongside it."
echo "We will see what plain CI says, and what WitDiff says."
pause ""

# --- the base revision, containing the bug ---------------------------------
cat > src/lib.rs <<'EOF'
/// Returns true when a number is even.
pub fn is_even(value: i32) -> bool {
    value % 2 == 1
}
EOF

cat > tests/existing.rs <<'EOF'
#[test]
fn arithmetic_still_works() {
    assert_eq!(2 + 2, 4);
}
EOF

cargo generate-lockfile --quiet
git add -A
git add -f Cargo.lock
git commit -q -m "base: is_even is inverted"

head2 "The bug, at the base revision"
sed -n '2,4p' src/lib.rs
pause "As written, is_even(2) is false and is_even(3) is true."

# --- the change an agent would produce -------------------------------------
cat > src/lib.rs <<'EOF'
/// Returns true when a number is even.
pub fn is_even(value: i32) -> bool {
    value % 2 == 0
}
EOF

cat > tests/regression.rs <<'EOF'
use demo::is_even;

/// Accompanies the fix, and proves nothing.
///
/// This passes on the buggy revision as well: `is_even(2)` equals `is_even(2)`
/// no matter how `is_even` is implemented. It is a tautology wearing a test's
/// clothes.
#[test]
fn is_even_is_consistent_with_itself() {
    assert_eq!(is_even(2), is_even(2));
}
EOF

head2 "The change"
echo "src/lib.rs        is_even now uses  % 2 == 0"
echo "tests/regression.rs   a new test, 'is_even_is_consistent_with_itself'"
pause ""

head2 "1. What plain CI says"
rule
cargo test --quiet 2>&1 | sed 's/^/    /'
rule
printf '\033[32mGREEN\033[0m. Every test passed. A reviewer sees a fix and a test.\n'
pause ""

head2 "2. What WitDiff says"
rule
"$WITDIFF" verify --base HEAD || true
rule
pause ""

head2 "Why"
echo "WitDiff transplanted only tests/regression.rs onto the base revision and"
echo "ran it there. It passed — because the test asserts nothing about the"
echo "behaviour that changed. A test that passes before the fix cannot show"
echo "that the fix did anything."
echo
echo "Note what it did NOT need: no model was asked whether the fix is correct,"
echo "and no opinion about code quality was formed. It ran one experiment and"
echo "reported the exit code."
pause ""

# --- the credible version of the same fix ----------------------------------
cat > tests/regression.rs <<'EOF'
use demo::is_even;

/// The same fix, with a test that actually constrains it.
#[test]
fn is_even_identifies_even_numbers() {
    assert!(is_even(2));
    assert!(!is_even(3));
}
EOF

head2 "3. The same fix, with a test that constrains it"
rule
"$WITDIFF" verify --base HEAD || true
rule
pause ""

head2 "What changed"
echo "Nothing in src/lib.rs. The only difference is the test: this one asserts"
echo "is_even(2) is true and is_even(3) is false, which is false on the buggy"
echo "revision and true on the fixed one."
echo
echo "That is the whole claim WitDiff makes: the tests pass here, and fail"
echo "there for a reason that is about behaviour rather than compilation."
echo
echo "Plain CI cannot tell these two tests apart. Both are green."
