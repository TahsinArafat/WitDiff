# ADR-0002: Use Git worktrees for base experiments

Status: accepted

## Decision

Base-revision experiments execute in a detached temporary Git worktree.

## Rationale

This isolates file state without copying the full repository object database and does not mutate the developer's current checkout.
