# Glossary

**BASE** — the comparison revision selected by `--base` or configuration.

**HEAD / current workspace** — the developer's current repository filesystem, including tracked working-tree changes and untracked files. The recorded Git HEAD commit alone is not sufficient to identify it; WitDiff also records a workspace fingerprint.

**Dedicated test file** — a file classified as test-only by configured path/glob rules and therefore safe enough for v0.1 to transplant as a whole onto BASE.

**Pristine base control** — the configured test command executed on untouched BASE before changed tests are applied.

**Red/green proof** — current workspace is green, pristine base is green, and base+changed-tests becomes red with a recognized behavioral test failure.

**Integrity finding** — deterministic evidence that the changed test itself may have been weakened or disabled.

**Receipt** — machine-readable immutable result of one WitDiff verification attempt.

**Fresh evidence** — evidence collected while the repository fingerprint remained unchanged.

**Base incompatible** — changed tests do not compile against BASE. This is informative but not behavioral regression proof in v0.1.

**Agent-neutral** — WitDiff semantics do not depend on Claude Code, Codex, Pi, OpenCode, or another agent vendor.
