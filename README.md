# WitDiff

**Deterministic verification for AI-written code.**

A coding agent can say "tests pass" when the new test would have passed before
the fix, when it quietly weakened an assertion, or when the run happened before
the final edit. WitDiff makes those claims independently checkable.

It runs one experiment: **transplant the changed tests onto the base revision
and require them to fail there and pass here.** A test that passes on both
revisions proves nothing, however confident anyone is.

```text
     change          base revision
        |                  |
        v                  v
   HEAD suite  ->  transplant tests  ->  BASE suite
      PASS                                  FAIL
        \                 |                /
         `---- proof: red/green ----------'
                        |
                        v
              JSON receipt + exit code
```

No LLM is asked whether the work is correct. The evidence is exit codes, test
output, diffs and hashes.

## Install

Prebuilt binaries for Linux, macOS and Windows (x64 and arm64) are on the
[releases page](https://github.com/TahsinArafat/WitDiff/releases). Download,
verify against `SHA256SUMS`, and put `witdiff` on your `PATH`.

```bash
# macOS / Linux
curl -fsSL https://github.com/TahsinArafat/WitDiff/releases/latest/download/witdiff-aarch64-apple-darwin.tar.gz \
  | tar xz -C /usr/local/bin
```

<details>
<summary>Other ways to install</summary>

From source, which needs Rust 1.88 or newer:

```bash
cargo install --git https://github.com/TahsinArafat/WitDiff witdiff
```

From a checkout:

```bash
cargo install --path crates/witdiff-cli
```

Or run the agent-integration installers in [`integrations/`](integrations),
which set WitDiff up as a skill, plugin or rule for the harness you use.

</details>

## Use

```bash
cd your-repo
witdiff init                             # detect the toolchain, write witdiff.toml
witdiff verify --base origin/main        # run the proof
```

`--base` should be the branch you are merging into.

```text
WitDiff verification
  status           : verified
  base             : origin/main
  head             : 55cc9b...
  changed tests    : 1
  head tests       : PASS
  base control     : PASS
  base + tests     : FAIL
  red/green proven : true
  evidence fresh   : true
```

For CI and agents, add `--strict --json`:

```bash
witdiff verify --base origin/main --strict --json
```

The exit code is three signals, not two:

| Code | Meaning |
| --- | --- |
| `0` | Proven, or there was nothing to prove |
| `1` | WitDiff itself failed to run — a tool error, not a result |
| `2` | Verification did not pass |

Exit 0 is not by itself proof. `status` in the receipt says what was
established; [`docs/verification-model.md`](docs/verification-model.md) explains
every value.

## What it checks

**Red/green proof** — the core claim, for Rust, Python (pytest), Go,
Java (JUnit), Ruby (Minitest, RSpec) and JavaScript/TypeScript (Jest, Vitest).

**Test-integrity findings** — a change that weakens its own tests is reported:
removed or trivialized assertions, changed expectations, newly skipped tests,
removed error checks, removed `match` arms. Structural, through a real parser
for all six languages.

**Supplementary evidence**, neither of which changes the verdict:

- **Mutation** (Rust) — does the suite notice a change to the code it covers?
- **Coverage** (Rust) — of the lines this change added, how many did the tests
  run?

**Receipts** — a content digest binding the receipt to the code it describes,
optional Ed25519 signatures, the toolchain and dependency manifests used, and an
append-only provenance chain across runs.

## Work with your agent

WitDiff is a CLI plus JSON, so any harness can use it. Ready-made integrations:

| Harness | What you get | Install |
| --- | --- | --- |
| [Claude Code](integrations/claude) | `witdiff` skill | copy to `.claude/skills/` |
| [OpenCode](integrations/opencode) | plugin + `witdiff_verify` tool | copy to `.opencode/plugins/` |
| [Pi](integrations/pi) | package with skill | `pi install ./integrations/pi` |
| [Cursor](integrations/cursor) | always-on rule | copy to `.cursor/rules/` |
| Anything else | project instructions | [AGENTS.md snippet](integrations/generic/AGENTS.md) |

Each one carries the same rule: run the tool, quote the receipt, and never
present a failed gate as success.

Also available: a [GitHub Actions workflow](.github/workflows/witdiff.yml) that
annotates pull requests, and an [MCP server](crates/witdiff-mcp) exposing
`inspect`, `verify` and `receipt` over stdio.

## Documentation

| | |
| --- | --- |
| [Support matrix](docs/support-matrix.md) | What is verified per language, and what is not |
| [Verification model](docs/verification-model.md) | What each status means, and why |
| [Receipt format](docs/receipt.md) | Every field a consumer can rely on |
| [Quickstart](docs/quickstart.md) | A first run, step by step |
| [Roadmap](docs/roadmap.md) | What is done, and what is next |
| [Architecture decisions](docs/decisions.md) | The 22 ADRs behind the design |
| [Working on WitDiff](AGENTS.md) | The engineering contract for contributors |

## Principles

1. **Deterministic evidence before model judgment.**
2. **An LLM may interpret a receipt, but it never creates a `verified` state.**
3. **Verification belongs to an exact workspace fingerprint.**
4. **Core verification must work offline.**
5. **Vendor integrations are thin wrappers around the same core CLI and API.**
6. **Unknown evidence is reported as unknown; never silently upgraded to proof.**

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
