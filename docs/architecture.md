# Architecture

## Current architecture

```text
witdiff CLI
     |
     v
witdiff-core
  |      |       |        |
 config  git   runner   integrity
                  \
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
4. classify changed files
5. detect dedicated changed tests
6. fingerprint current workspace
7. inspect test diff for integrity findings
8. run configured test command on current workspace
9. create detached base worktree
10. transplant only changed dedicated tests
11. copy untracked dedicated tests
12. run identical test command on base+test worktree
13. classify base failure
14. remove temporary worktree
15. fingerprint current workspace again
16. compute conservative status
17. emit witdiff.receipt.v1
```

## Why Git worktrees

The base experiment must not modify the developer's workspace. Worktrees give WitDiff an isolated filesystem representing the base commit while sharing the Git object database.

## Why full test commands in v0.1

Targeted test selection is framework-specific and can accidentally omit evidence. v0.1 prioritizes correctness and simplicity. A future test-adapter layer will select changed tests while recording exactly what was run.

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
