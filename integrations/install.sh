#!/bin/sh
# Install the WitDiff agent integrations into a project (or your home config).
#
#   ./install.sh                     # into the current directory
#   ./install.sh /path/to/project    # into a specific project
#   ./install.sh --global            # into ~/.claude, ~/.config/opencode
#
# Existing files are never overwritten: a skill you have edited is not something
# an installer should silently replace. Each skipped file is reported.

set -eu

SRC=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
GLOBAL=0
TARGET=""

for arg in "$@"; do
  case "$arg" in
    --global) GLOBAL=1 ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) TARGET="$arg" ;;
  esac
done

if [ "$GLOBAL" -eq 1 ]; then
  CLAUDE_DIR="${HOME}/.claude"
  OPENCODE_DIR="${HOME}/.config/opencode"
  SCOPE="your home configuration"
else
  [ -n "$TARGET" ] || TARGET=$(pwd)
  if [ ! -d "$TARGET" ]; then
    echo "error: $TARGET is not a directory" >&2
    exit 1
  fi
  CLAUDE_DIR="${TARGET}/.claude"
  OPENCODE_DIR="${TARGET}/.opencode"
  SCOPE="$TARGET"
fi

installed=0
skipped=0

# Copy a file only when the destination does not already exist.
place() {
  src=$1
  dest=$2
  if [ -e "$dest" ]; then
    printf '  exists, left alone : %s\n' "$dest"
    skipped=$((skipped + 1))
    return
  fi
  mkdir -p "$(dirname -- "$dest")"
  cp "$src" "$dest"
  printf '  installed          : %s\n' "$dest"
  installed=$((installed + 1))
}

# rm -rf without checking would delete whatever is at the destination. The
# installer only ever removes paths it can identify as something it created.
copy_tree() {
  src_dir=$1
  dest_dir=$2
  for path in "$src_dir"/*; do
    [ -e "$path" ] || continue
    name=$(basename -- "$path")
    if [ -d "$path" ]; then
      mkdir -p "$dest_dir/$name"
      copy_tree "$path" "$dest_dir/$name"
    else
      place "$path" "$dest_dir/$name"
    fi
  done
}

echo "Installing WitDiff agent integrations into ${SCOPE}"
echo

echo "Claude Code (.claude/skills/)"
copy_tree "$SRC/claude/skills" "$CLAUDE_DIR/skills"

echo
echo "OpenCode (.opencode/)"
copy_tree "$SRC/opencode/plugins" "$OPENCODE_DIR/plugins"
copy_tree "$SRC/opencode/skills" "$OPENCODE_DIR/skills"

echo
echo "Cursor (.cursor/rules/)"
if [ "$GLOBAL" -eq 1 ]; then
  echo "  cursor rules are per-project; skipped in --global mode"
else
  copy_tree "$SRC/cursor/rules" "$TARGET/.cursor/rules"
fi

echo
echo "Pi"
if command -v pi >/dev/null 2>&1; then
  # An absolute path is used because `pi install --local` records the source
  # relative to the settings file it writes, and a relative path that escapes
  # the project becomes a chain of `../` that breaks if either directory moves.
  if [ "$GLOBAL" -eq 1 ]; then
    pi install "$SRC/pi" || echo "  pi install failed; see its message above"
  else
    (cd "$TARGET" && pi install "$SRC/pi" --local) || echo "  pi install failed; see its message above"
  fi
  echo "  note: pi records this source in settings; re-run after moving WitDiff"
else
  echo "  pi not found on PATH; install it, then run: pi install $SRC/pi"
fi

echo
echo "Done: ${installed} installed, ${skipped} left alone."
echo
echo "Verify that witdiff itself is usable before relying on it:"
echo "  witdiff doctor"
