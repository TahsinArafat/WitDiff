#!/bin/sh
# Copy the canonical skill to every place a harness reads it.
#
#   integrations/sync-skill.sh
#
# Edit `integrations/skill/SKILL.md`, run this, and commit. A test asserts the
# copies match, so a forgotten one fails CI rather than silently leaving a
# harness on a stale instruction set.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE="$ROOT/integrations/skill/SKILL.md"

[ -f "$SOURCE" ] || { echo "error: $SOURCE not found" >&2; exit 1; }

for dest in \
  "skills/witdiff/SKILL.md" \
  ".claude/skills/witdiff/SKILL.md" \
  "integrations/claude/skills/witdiff/SKILL.md" \
  "integrations/opencode/skills/witdiff/SKILL.md" \
  "integrations/pi/skills/witdiff/SKILL.md" \
; do
  mkdir -p "$ROOT/$(dirname -- "$dest")"
  cp "$SOURCE" "$ROOT/$dest"
  echo "synced $dest"
done

echo "done"
