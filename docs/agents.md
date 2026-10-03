# Working on WitDiff with coding agents

WitDiff is intentionally designed so Claude Code, Codex, Pi, OpenCode, IDE agents, CI, and human developers all consume the same deterministic interface.

## First context to load

Agents should read these files in order:

1. `/AGENTS.md`
2. `/docs/product.md`
3. `/docs/architecture.md`
4. `/docs/verification-model.md`
5. `/docs/roadmap.md`
6. the implementation files related to the assigned task

Do not preload every document and source file into the prompt. The documents above define the stable contract; source should be inspected as needed.

## Universal completion protocol

An agent that changed WitDiff itself should run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

An agent using WitDiff in another repository should run:

```bash
witdiff verify --base <target-branch> --strict --json
```

and treat exit code 2 as a failed verification gate, not as a tool crash.

## Recommended system instruction snippet

Use this in Claude Code/Codex/Pi/OpenCode project guidance when WitDiff is installed:

```text
Before claiming that a code change is complete or verified, run WitDiff using the repository's configured base branch. Treat WitDiff as an independent evidence system. Do not rewrite, reinterpret, or suppress a failed WitDiff result. If WitDiff reports an unsupported case, state the limitation and provide the raw deterministic evidence you did obtain.
```

## Why the core must remain agent-neutral

Agent products change quickly. WitDiff should survive those changes. The compatibility boundary is:

```text
agent -> process execution -> witdiff CLI -> JSON receipt
```

A future MCP server is allowed, but it must call the same core verification functions and return the same receipt semantics.

## Parallel-agent rules for this repository

If multiple coding agents work simultaneously:

- assign non-overlapping roadmap tasks where possible;
- use separate Git worktrees/branches;
- avoid simultaneous edits to `model.rs` and receipt schema;
- if two tasks need schema changes, merge the schema task first;
- one agent owns an ADR while it is being drafted;
- integration tests should be merged early because they provide shared constraints.

## Agent task template

```text
Goal:

Invariant affected:

Files likely involved:

Non-goals:

Acceptance tests:

Docs to update:

Compatibility constraints:
```

## Review checklist for agent-generated patches

A reviewer should ask:

- Did the patch weaken any conservative failure mode?
- Can an unknown/unsupported state accidentally become `Verified`?
- Does the receipt still identify exact base, head, and fingerprint?
- Are external command errors distinguishable from test failures?
- Is the code vendor-neutral?
- Were docs updated if semantics changed?
