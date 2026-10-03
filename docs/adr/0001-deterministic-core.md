# ADR-0001: Deterministic core before AI integrations

Status: accepted

## Decision

Verification state is computed exclusively from deterministic repository/tool evidence. Model output may explain or suggest experiments but may not create `Verified`.

## Consequences

- core remains useful without an API key;
- behavior is testable and reproducible;
- agent integrations are thin;
- some useful semantic judgments are postponed until they can be clearly separated from proof state.
