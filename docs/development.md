# Development environment

## Native

Requirements:

- Rust stable with Cargo;
- Git 2.x;
- a POSIX shell only for helper scripts (the WitDiff binary itself does not require one).

Recommended setup:

```bash
rustup component add rustfmt clippy
./scripts/check.sh
./scripts/smoke-demo.sh
```

## Dev container

A basic `.devcontainer/devcontainer.json` and `Dockerfile` are included. They are optional and are not part of runtime architecture.

## Fast iteration

```bash
cargo test -p witdiff-core
cargo run -p witdiff -- --help
cargo run -p witdiff -- inspect --base HEAD~1
```

## End-to-end verification tests

`crates/witdiff-core/tests/verify_end_to_end.rs` drives `verify_repository`
against real temporary Git repositories and real `cargo` builds. Those tests are
`#[ignore]`d so that a plain `cargo test` stays offline-capable and fast. Run
them explicitly:

```bash
cargo test --workspace --all-features -- --ignored --test-threads=1
```

Most of those tests spawn a real toolchain, and a missing one is **skipped
rather than failed** — which makes a green run indistinguishable from a run that
tested nothing. Install what CI installs, or expect coverage:

```bash
npm install                      # acorn + typescript, for the JavaScript analyzer
gem install rspec                # the Ruby end-to-end proof runs a live RSpec
python3 -m pip install pytest    # the Python proof
rustup component add llvm-tools-preview && cargo install cargo-llvm-cov   # coverage
```

Java additionally needs a JUnit 5 platform; the Java test discovers it from
`WITDIFF_JUNIT_JARS` or from an IDE extension directory, and **fails loudly**
when the variable is set but no runtime is found rather than skipping.

Two constraints apply when adding fixtures:

1. **The fixture must be fingerprint-stable.** `workspace_fingerprint` hashes
   the diff, `git status`, and every untracked file, by design: build byproducts
   that appear mid-run must invalidate evidence. A fixture that lets the first
   build create `Cargo.lock` or `target/` will therefore be reported as stale
   evidence — correctly, but unhelpfully. Fixtures must ignore `target/` and
   commit `Cargo.lock` before verification (`Fixture::warm_lockfile`).

2. **`#[ignore]` is not a red state.** A skipped test passes, so a fixture whose
   only change is adding `#[ignore]` produces a passing transplant and a passing
   control. To reach the integrity-blocking branch, the transplant must still
   fail on base for a test reason, with the integrity finding layered on top.

## Before handing work to another agent

Leave the repository in a state where the next agent can answer these questions from Git and docs:

- what invariant changed;
- which tests cover it;
- which receipt/schema fields changed;
- what remains unsupported;
- which roadmap item should happen next.

Update `AGENTS.md`, an ADR, or `docs/roadmap.md` when that information would otherwise live only in chat history.
