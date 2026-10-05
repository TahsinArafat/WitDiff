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

One command, on Linux, macOS or Windows:

```bash
curl -fsSL https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/install.sh | sh
```

It detects your platform, downloads the matching archive, **verifies it against
the release's `SHA256SUMS`**, and installs to `~/.local/bin`. If the checksum
does not match, nothing is installed. It never uses `sudo`; an unwritable
destination is reported, not escalated.

<details>
<summary>Options, other platforms, and manual install</summary>

```bash
./install.sh --version v1.0.0-alpha.1   # pin a release
./install.sh --to /usr/local/bin        # choose the directory
./install.sh --uninstall                # remove it
```

**Windows (PowerShell)** — the installer is a POSIX shell script. Download the
zip and verify it directly:

```powershell
$V = "1.0.0-alpha.1"
$u = "https://github.com/TahsinArafat/WitDiff/releases/download/v$V"
Invoke-WebRequest "$u/witdiff-x86_64-pc-windows-msvc.zip" -OutFile witdiff.zip
Invoke-WebRequest "$u/SHA256SUMS" -OutFile SHA256SUMS
$expected = (Select-String -Path SHA256SUMS -Pattern "witdiff-x86_64-pc-windows-msvc.zip").Line.Split()[0]
$actual = (Get-FileHash witdiff.zip -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) { throw "checksum mismatch; not installing" }
Expand-Archive witdiff.zip -DestinationPath .
# move witdiff.exe somewhere on your PATH
```

**From source**, needing Rust 1.88 or newer:

```bash
cargo install --git https://github.com/TahsinArafat/WitDiff witdiff
```

**Every published platform:**

| Platform | Archive |
| --- | --- |
| Linux x64 | `witdiff-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `witdiff-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `witdiff-aarch64-apple-darwin.tar.gz` |
| Windows x64 | `witdiff-x86_64-pc-windows-msvc.zip` |

Intel macOS is not built — the runner could not be relied on to schedule, and a
Mac from 2020 onward runs the arm64 binary under Rosetta. See
[`docs/development.md`](docs/development.md).

</details>

### Wire it into your agent

```bash
curl -fsSL https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/integrations/install.sh | sh
```

Detects **Claude Code**, **OpenCode**, **Cursor** and **Pi** and installs the
matching package into the current project. It never overwrites a file you have
edited, and it installs nothing you do not have.

```bash
integrations/install.sh --global      # user config instead of this project
integrations/install.sh --uninstall   # remove only what it installed
```

See [`integrations/`](integrations) for what each harness gets.

### Uninstall

```bash
install.sh --uninstall              # the binary
integrations/install.sh --uninstall # the agent packages
```

Both remove only what they installed, and both report what they removed. Files
you wrote yourself are left alone, even in the same directories.

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
