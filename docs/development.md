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

## Agent integrations

`integrations/` holds ready-made packages for Claude Code, OpenCode, Pi and
Cursor, plus a generic instruction snippet. Each is written against that
harness's documented extension format, and
`crates/witdiff-cli/tests/integrations.rs` asserts the conventions those
harnesses enforce: a skill's `name` matching its directory, the Agent Skills
name pattern, Cursor's required frontmatter, and that Pi and OpenCode manifests
point at files that exist.

That test exists because a malformed integration does not fail loudly. The
harness simply does not load it, which is indistinguishable from a working
integration with nothing to say.

Two installers ship, both tested:

- **`install.sh`** (root) fetches a release for the current platform, verifies it
  against `SHA256SUMS`, and installs the binary. It never uses `sudo`, and it
  fails closed: a checksum mismatch reports and stops rather than warning. It
  falls back across `sha256sum`, `shasum` and `openssl`, because the first is
  Linux-only and the second macOS-only.
- **`integrations/install.sh`** places the harness packages. It never overwrites
  an existing file, so an edit you made is never lost to a re-run.

Both support `--uninstall`. Removals are aimed only at paths the installer
created — shared directories are emptied with `rmdir`, which refuses when
non-empty, so a skill you wrote yourself survives. A test asserts each removal
targets an installed path, and that both scripts document the way back out.

The platform list in `install.sh` is checked against the release matrix in
`.github/workflows/release.yml` by executing the installer's own archive-mapping
logic, not by searching its source: a string search passes while the mapping is
broken, because the target triples also appear in the platform-detection branch.

## Building a release

`.github/workflows/release.yml` builds binaries for five targets on a tag
matching `v*`, verifies every platform is present before publishing, and
attaches the archives plus `SHA256SUMS`. Each target is built on its own runner
rather than cross-compiled.

The workflow asserts the workspace's declared `rust-version` still builds. That
is worth having as a check rather than a claim: the workspace declared 1.78
while a transitive dependency had moved to edition 2024 and `globset` required
1.88, so the documented floor was false until it was measured.

The matrix is four targets, and every one must build or the release is refused —
a partial release hands users a download that does not exist for them:

| Runner | Target |
| --- | --- |
| `ubuntu-latest` | `x86_64-unknown-linux-gnu` |
| `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` |
| `macos-latest` | `aarch64-apple-darwin` |
| `windows-latest` | `x86_64-pc-windows-msvc` |

Intel macOS was in the matrix initially and was dropped. Its runner label is
valid, but it queued for 16 to 19 minutes across three runs and then failed,
while the other four started in about two minutes and finished — a 21-day-old
Apple Silicon Mac runs the arm64 binary under Rosetta, so the Intel build bought
nothing worth that. Adding it back means accepting a release that may not
publish, which is why the requirement is now unconditional rather than
configurable.

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
