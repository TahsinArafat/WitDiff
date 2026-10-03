# Bundle validation

The generated starter bundle was validated structurally in its creation environment.

Completed checks:

- all TOML files parse successfully;
- all JSON files parse successfully;
- receipt schema contains the expected v1 fields;
- helper shell scripts pass `bash -n` syntax checking;
- `git diff --check` reports no whitespace errors across the generated repository;
- a real Git-based integration test is included for changed-file/test classification;
- `scripts/smoke-demo.sh` is included as the intended end-to-end red/green smoke test.

The creation environment did **not** contain `rustc`/`cargo`, so Cargo compilation, rustfmt, clippy, unit tests, and the smoke demo could not be executed here. The repository therefore includes three immediate verification paths for the first development machine/agent:

```bash
./scripts/check.sh
./scripts/smoke-demo.sh
cargo run -p witdiff -- --help
```

GitHub CI is also configured to run format, clippy, and tests on every pull request.

If the first Rust-capable run reveals a compile/API-version issue, treat it as P0 and fix it before feature development; do not weaken any verification invariant to make the build pass.
