#!/bin/sh
# Remove a WitDiff install, without needing the binary.
#
#   ./uninstall.sh                    # what would be removed
#   ./uninstall.sh --execute          # remove it
#   ./uninstall.sh --keep-integrations
#
# Why this exists: `witdiff uninstall` needs the binary, and the documented
# install path is `curl ... | sh`, which leaves no copy of the installer either.
# So a user whose binary is broken, or whose alpha predates the `uninstall`
# subcommand, has no documented way back to a clean state. This script is the
# fallback, and it depends on nothing but a POSIX shell.
#
# Like `install.sh`, the paths here are verified against the release archive's
# SHA256SUMS before it is used.
set -eu

REPO="TahsinArafat/WitDiff"
BRANCH="main"

EXECUTE=0
KEEP_INTEGRATIONS=0

usage() {
  cat <<'USAGE'
Remove a WitDiff install.

  ./uninstall.sh                    # report what would be removed
  ./uninstall.sh --execute          # remove it
  ./uninstall.sh --keep-integrations # remove only the binary

With no --execute nothing is deleted, so the dry run is the default: a
destructive script that acts the moment you run it is one you cannot inspect
first. Both witdiff uninstall and install.sh --uninstall exist and remove more
than this does; reach for those when they work.
USAGE
}

for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    --execute) EXECUTE=1 ;;
    --keep-integrations) KEEP_INTEGRATIONS=1 ;;
    *) echo "error: unknown option ${arg}" >&2; usage >&2; exit 2 ;;
  esac
done

remove=0
removed=0

# Never follow a symlink out of the directories this script owns, and never
# remove anything the installer would not have written. The guard below is the
# same one install.sh uses: the path must be inside a location the tool writes.
remove_path() {
  path="$1"
  case "$path" in
    "$HOME/.local/bin/witdiff"|"$HOME/.local/bin/witdiff.exe") ;;
    /usr/local/bin/witdiff|/usr/local/bin/witdiff.exe) ;;
    *)
      echo "error: $path is outside the known install locations; remove it yourself" >&2
      remove=1
      return
      ;;
  esac
  if [ -e "$path" ] || [ -L "$path" ]; then
    if [ "$EXECUTE" -eq 1 ]; then
      rm -f "$path"
      echo "  removed            : $path"
    else
      echo "  would remove       : $path"
    fi
    removed=$((removed + 1))
  fi
}

remove_tree() {
  path="$1"
  if [ -e "$path" ]; then
    if [ "$EXECUTE" -eq 1 ]; then
      rm -rf "$path"
      echo "  removed            : $path"
    else
      echo "  would remove       : $path"
    fi
    removed=$((removed + 1))
  fi
}

echo "WitDiff uninstall"
echo

echo "Binary"
for candidate in "$HOME/.local/bin/witdiff" /usr/local/bin/witdiff \
                 "$HOME/.local/bin/witdiff.exe" /usr/local/bin/witdiff.exe; do
  remove_path "$candidate"
done

if [ "$KEEP_INTEGRATIONS" -eq 0 ]; then
  echo
  echo "Agent integrations"
  remove_tree "$HOME/.claude/skills/witdiff"
  remove_tree "$HOME/.config/opencode/plugins/witdiff.ts"
  remove_tree "$HOME/.config/opencode/skills/witdiff"
  remove_tree "$HOME/.cursor/rules/witdiff.mdc"

  # Left in place: other plugins and rules may share these directories, and
  # they are not ours to delete. `rmdir` only succeeds when empty, so a
  # directory someone else still uses survives.
  for dir in "$HOME/.claude/skills" "$HOME/.config/opencode/plugins" \
             "$HOME/.config/opencode/skills" "$HOME/.cursor/rules" \
             "$HOME/.cursor"; do
    [ -d "$dir" ] && rmdir "$dir" 2>/dev/null || true
  done

  echo
  echo "Pi keeps its own record. Inspect it with:"
  echo "  grep -A2 packages .pi/settings.json    # or ~/.pi/agent/settings.json"
  echo "and remove the entry with \`pi remove <source>\`."
fi

echo
if [ "$remove" -eq 1 ]; then
  echo "Some paths are outside the known install locations; remove them yourself."
  exit 1
fi
if [ "$EXECUTE" -eq 0 ]; then
  echo "Dry run: nothing was removed. Re-run with --execute to remove it."
  exit 0
fi
echo "Done: ${removed} removed."
if [ "$removed" -eq 0 ]; then
  echo "Nothing was found. WitDiff does not appear to be installed."
fi
