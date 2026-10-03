# Product specification

## Product statement

WitDiff is a local-first, agent-neutral verification engine that produces executable evidence for code changes, beginning with regression-test credibility.

## User problem

Coding agents frequently produce code and tests in the same patch. A green test suite on the final patch does not establish that the new test detects the original defect. The test may already pass before the fix, may have been weakened, or may belong to an older workspace state.

## v0.1 user promise

For repositories with dedicated changed test files, WitDiff can establish whether:

- the configured test command passes on the current workspace;
- the changed test-only patch fails on the base revision for a recognized test reason;
- suspicious test weakening is present;
- the workspace stayed unchanged while evidence was collected.

WitDiff reports uncertainty conservatively.

## Primary users

- developers reviewing agent-generated patches;
- coding agents that need an external completion gate;
- CI systems enforcing evidence requirements;
- maintainers accepting external contributions.

## Non-users / non-goals

WitDiff is not:

- a code generator;
- a generic LLM agent framework;
- a replacement for code review;
- a security scanner;
- a coverage percentage tool;
- an AI “confidence score.”

## Product principles

### Evidence over assertion

“Tests pass” is a claim. A recorded command, exact workspace fingerprint, exit status, and red/green comparison are evidence.

### Conservative truth

If WitDiff cannot distinguish a useful failure from an environment/setup failure, it must not call the patch verified.

### Vendor neutrality

Claude Code, Codex, Pi, OpenCode, CI, and humans all use the same engine.

### Local-first

The verifier should not require source upload or a cloud account.

### Composable artifacts

Every verification emits a structured receipt so other tools can reason over facts without scraping console prose.
