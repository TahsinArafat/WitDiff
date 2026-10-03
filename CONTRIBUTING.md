# Contributing

Read `AGENTS.md` before opening a patch.

## Local checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Every feature that changes verification semantics should include:

- a positive case;
- a negative case;
- an ambiguous/unsupported case;
- documentation explaining why the outcome is conservative.

For significant proof-model changes, add an ADR under `docs/adr/`.
