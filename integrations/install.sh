#!/bin/sh
# Install the WitDiff agent integrations into a project (or your home config).
#
#   ./install.sh                     # into the current directory
#   ./install.sh /path/to/project    # into a specific project
#   ./install.sh --global            # into ~/.claude, ~/.config/opencode
#   ./install.sh --uninstall         # remove what this script installed
#
# Existing files are never overwritten: a skill you have edited is not something
# an installer should silently replace. Each skipped file is reported.

set -eu

REPO="TahsinArafat/WitDiff"
BRANCH="main"

# When this script is piped into a shell, `$0` is the interpreter (`sh`), not a
# path, so its directory is `/bin` — and the packages it installs are siblings
# of the script in the repository. `curl ... | sh` therefore found no packages
# and reported "0 installed" while exiting 0, which reads as success.
#
# The check is whether this script's own directory actually contains the files
# it is about to copy, not whether `$0` looks like a path: a symlink, a wrapper,
# or a different working directory would defeat a test on `$0`.
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || echo "")
FETCHED=""
SRC="$SCRIPT_DIR"

if [ ! -d "$SCRIPT_DIR/opencode" ] || [ ! -d "$SCRIPT_DIR/pi" ]; then
  echo "The packages are not beside this script (it was piped into a shell)."
  echo "Downloading them from ${REPO}@${BRANCH}..."
  echo
  FETCHED=$(mktemp -d)
  trap 'rm -rf "$FETCHED"' EXIT INT TERM
  curl -fsSL --proto '=https' --tlsv1.2 \
    "https://codeload.github.com/${REPO}/tar.gz/refs/heads/${BRANCH}" \
    -o "$FETCHED/repo.tar.gz" || {
      echo "error: could not download the packages; install from a checkout instead:" >&2
      echo "  git clone https://github.com/${REPO} && cd WitDiff" >&2
      exit 1
    }
  tar xzf "$FETCHED/repo.tar.gz" -C "$FETCHED" || {
    echo "error: could not unpack the download" >&2
    exit 1
  }
  SRC=$(find "$FETCHED" -maxdepth 1 -type d -name 'WitDiff-*' | head -1)/integrations
  [ -d "$SRC/pi" ] || {
    echo "error: the download did not contain integrations/pi" >&2
    exit 1
  }
fi
GLOBAL=0
TARGET=""
UNINSTALL=0

for arg in "$@"; do
  case "$arg" in
    --global) GLOBAL=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help)
      sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
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

# --- uninstall --------------------------------------------------------------
# Only files this installer would have written are candidates. A skill you
# author yourself lives at the same path, so removal is limited to the exact
# names shipped here and the directories those names created.
if [ "$UNINSTALL" -eq 1 ]; then
  removed=0
  remove_if_present() {
    if [ -e "$1" ]; then
      rm -rf "$1"
      printf '  removed            : %s\n' "$1"
      removed=$((removed + 1))
    fi
  }

  echo "Removing WitDiff agent integrations from ${SCOPE}"
  echo
  remove_if_present "$CLAUDE_DIR/skills/witdiff"
  remove_if_present "$OPENCODE_DIR/plugins/witdiff.ts"
  remove_if_present "$OPENCODE_DIR/skills/witdiff"
  if [ "$GLOBAL" -eq 0 ]; then
    remove_if_present "$TARGET/.cursor/rules/witdiff.mdc"
  fi

  # Left in place: other plugins and rules may share these directories, and
  # they are not ours to delete.
  for dir in "$CLAUDE_DIR/skills" "$OPENCODE_DIR/plugins" "$OPENCODE_DIR/skills" \
             "$TARGET/.cursor/rules" "$TARGET/.cursor"; do
    [ -d "$dir" ] && rmdir "$dir" 2>/dev/null || true
  done

  echo
  echo "Done: ${removed} removed."
  if command -v pi >/dev/null 2>&1; then
    echo
    echo "Pi keeps its own record. Remove the package with:"
    echo "  pi remove $SRC/pi"
    echo "or, if it was installed with --local, edit .pi/settings.json."
  fi
  exit 0
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
  # When the packages were downloaded, the local path is a temporary directory
  # this script deletes on exit. Pi records the source it is given, so a path
  # into that directory resolves to nothing the moment the trap fires —
  # observed: `pi install` succeeded and the recorded path was already gone.
  # A git source is stable and needs no local copy.
  if [ -n "$FETCHED" ]; then
    PI_SOURCE="git:github.com/${REPO}@${BRANCH}"
  else
    PI_SOURCE="$SRC/pi"
  fi

  if [ "$GLOBAL" -eq 1 ]; then
    pi install "$PI_SOURCE" || echo "  pi install failed; see its message above"
  else
    (cd "$TARGET" && pi install "$PI_SOURCE" --local) || echo "  pi install failed; see its message above"
  fi
  echo "  note: pi records this source in settings; re-run after moving a local checkout"
else
  echo "  pi not found on PATH; install it, then run:"
  echo "    pi install git:github.com/${REPO}@${BRANCH}"
fi

echo
echo "Done: ${installed} installed, ${skipped} left alone."
echo
echo "Verify that witdiff itself is usable before relying on it:"
echo "  witdiff doctor"
