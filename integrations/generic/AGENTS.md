# Project instructions

<!--
  WitDiff verification gate.

  Copy the section between the markers into your project's agent guidance:
  AGENTS.md (Codex, Cursor, OpenCode, Amp, Jules), CLAUDE.md (Claude Code), or
  .cursor/rules/*.mdc with `alwaysApply: true`.

  Harness-specific installs are shipped under `integrations/` in the WitDiff
  repository — a skill for Claude Code, an OpenCode plugin with a callable tool,
  a Pi package, and a Cursor rule.
-->

--- BEGIN INSTRUCTION SNIPPET ---

## Verifying a change

WitDiff runs a deterministic experiment: it transplants the changed tests onto
the base revision and checks they **fail there and pass here**. A test that
passes on both revisions proves nothing, however confident anyone is.

Before claiming a change is complete or verified:

```bash
witdiff verify --base origin/main --strict --json
```

Exit codes are three signals, not two:

| Code | Meaning |
| --- | --- |
| `0` | The claim was proven, or there was nothing to prove |
| `1` | WitDiff itself failed to run — a tool error, not a verification result |
| `2` | Verification did not pass |

**Exit code 2 is a failed gate. Do not report a change as complete while it
exits 2.** It is not a crash and not a reason to retry.

Exit code 0 is not by itself proof. Read `status` in the receipt:

| Status | Meaning |
| --- | --- |
| `verified` | Proven: the base experiment failed for a test reason and passes here |
| `verified_with_warnings` | Proven, with integrity findings — read them |
| `no_changed_tests` | Nothing was proven, because no test changed |
| `not_verified` | A proof was attempted and did not establish the claim, or the case was undecidable |
| `head_failed` | The workspace's own tests fail; fix that first |
| `base_incompatible` | The transplanted test did not compile against the base, so no behavioural proof exists |

Rules for reporting:

- Quote the receipt's `status` and every integrity finding. Do not summarize
  them as "tests pass".
- Never rewrite, suppress, or reinterpret a failed WitDiff result.
- If WitDiff reports an unsupported case — inline tests it declined to splice, a
  path whose encoding was lossy, a base revision it could not parse — state that
  limitation explicitly. An unsupported case is not a pass.
- If `evidence_fresh` is false the workspace moved during the run. Re-run; do not
  cite the stale receipt.
- On `no_changed_tests`, say plainly that there is no proof because nothing was
  proven. Do not present it as success.

--- END INSTRUCTION SNIPPET ---

## Why these instructions are shaped this way

WitDiff exists because an agent asserting "the tests pass" is not evidence. The
snippet keeps that boundary: the agent runs a deterministic tool and reports
what it said, including what it could not determine. The three-way exit code
matters because "nothing to prove" and "the proof failed" are different facts,
and collapsing them either fails correct changes or passes unproven ones.

See `docs/verification-model.md` for what each status means, and
`docs/agents.md` for guidance on working on WitDiff itself.
