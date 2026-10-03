# Architecture

## Current architecture

```text
witdiff CLI
     |
     v
witdiff-core
  |       |        |           |
config   git     runner      integrity ── rustanalysis
  |       |        |           |      (syntax-aware)
  |       |        |           |
  |       |        |        selection
  |       |        |           |
   \      |        |          /
          verify
            |
            v
          Receipt
```

`witdiff-cli` is intentionally thin. Verification semantics belong in `witdiff-core` so future interfaces (MCP, library embedding, CI service) cannot diverge from CLI behavior.

## Verification flow

```text
1. discover repository
2. load witdiff.toml (or defaults)
3. resolve base ref
4. classify changed files (NUL-delimited Git output)
5. detect dedicated changed tests
6. resolve the test command, narrowing it only if that is provably safe
7. fingerprint current workspace
8. compare changed Rust tests structurally against base for integrity findings
9. run the resolved test command on current workspace
10. create detached base worktree
11. transplant only changed dedicated tests, excluding ineligible files
12. copy untracked dedicated tests
13. run the identical test command on base+test worktree
14. classify base failure
15. remove temporary worktree
16. fingerprint current workspace again
17. compute conservative status
18. emit witdiff.receipt.v1
```

## Why Git worktrees

The base experiment must not modify the developer's workspace. Worktrees give WitDiff an isolated filesystem representing the base commit while sharing the Git object database.

## Why the full suite is still the default

Running only the changed tests is faster, but narrowing can silently omit
evidence, and omitting evidence is the failure this project exists to prevent.
A narrowed run is also a strictly smaller claim than a full-suite run, so the
receipt distinguishes the two: `test_selection` and `effective_test_command`
record what actually ran.

Narrowing therefore stays opt-in (`verification.targeted_test_selection`, off by
default) and is attempted only when the configured command can be narrowed
without changing what cargo would execute. The clearest case is `--all-targets`,
which overrides `--test`: narrowing such a command would run the whole suite
while the receipt named one target. In every case WitDiff cannot narrow safely,
it runs the full suite and records a note saying why. See ADR-0008.

## Why test integrity is compared structurally

A line-oriented diff cannot distinguish a deleted assertion from one that merely
moved, and it records only that a line changed, never what changed in it. Both
kinds of noise train an operator to ignore the rules that matter. Rust test files
are therefore parsed and compared as structure between the base and head
revisions. Where a file cannot be parsed, that is reported and the line-oriented
rules apply, so an unanalyzable file never looks clean. See ADR-0006.

## Why compile failures do not prove red/green behavior

A changed test may reference an API introduced by the patch. Applying that test to the base can fail at compilation. That demonstrates incompatibility, but it does not demonstrate that the old behavior violates the intended assertion. Therefore v0.1 returns `base_incompatible`, not `verified`.

## Process boundary

The test command is stored as an argument vector, not a shell script. This avoids shell interpolation and makes command execution easier to audit.

## Future module boundaries

Add a new crate only when a stable boundary appears. Likely future boundaries:

- `witdiff-mutate`: mutation operators and orchestration;
- `witdiff-mcp`: protocol adapter only;
- `witdiff-adapter-*`: language/test-runner integration if dependencies justify isolation.

Do not split the repository simply to imitate a large architecture.
