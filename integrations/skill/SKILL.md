---
name: witdiff
description: Verify that a code change is actually proven by its tests, using WitDiff as an independent gate. Use before claiming a change is complete, verified, fixed, or done — or when asked whether tests prove a change, whether a regression test is real, or why verification failed.
license: GPL-3.0-or-later
compatibility: Requires the `witdiff` binary on PATH and a Git repository.
metadata:
  homepage: https://github.com/TahsinArafat/WitDiff
  tool: witdiff
---

# Verifying a change with WitDiff

WitDiff runs a deterministic experiment: it transplants the changed tests onto
the base revision and checks that they **fail there and pass here**. A test that
passes on both revisions proves nothing, no matter how confident anyone is.

Run it before saying a change is verified. Never decide a change is correct and
then check whether WitDiff agrees — run it and report what it said.

## Run it

```bash
witdiff verify --base origin/main --strict --json
```

Use the repository's actual target branch for `--base`. If the base is unclear,
`git symbolic-ref refs/remotes/origin/HEAD` names it, or `witdiff inspect`
prints what would be compared.

## Read the exit code as three signals, not two

| Code | Meaning |
| --- | --- |
| `0` | The claim was proven, or there was nothing to prove |
| `1` | WitDiff itself failed to run — a tool error, **not** a verification result |
| `2` | Verification did not pass |

Exit code 2 is a failed gate. It is not a crash and not a reason to retry.
**Do not report a change as complete while it exits 2.**

## Exit 0 is not by itself proof

Check `status` in the receipt:

| Status | Meaning |
| --- | --- |
| `verified` | The base experiment failed for a test reason and passes here. The claim is proven. |
| `verified_with_warnings` | Proven, with integrity findings. Read them. |
| `no_changed_tests` | Nothing was proven, because no test changed. Not a pass, not a failure. |
| `not_verified` | A proof was attempted and did not establish the claim, or the case was undecidable. |
| `head_failed` | The workspace's own tests fail. Fix that first. |
| `base_incompatible` | The transplanted test could not compile against the base, so no behavioural proof exists. |

## Report what it said

- Quote the receipt's `status` and integrity findings. Do not summarize them as
  "tests pass".
- Never rewrite, suppress, or reinterpret a failed result.
- If WitDiff reports an unsupported case — inline tests it declined to splice, a
  path whose encoding was lossy, a base revision it could not parse — state that
  limitation explicitly. An unsupported case is not a pass.
- If `evidence_fresh` is false, the workspace changed during the run. Re-run;
  do not cite the stale receipt.
- On `no_changed_tests`, say plainly that there is no proof because nothing was
  proven. Do not present it as success.

## Diagnosing

```bash
witdiff inspect --base origin/main    # how files were classified, before running
witdiff receipt --json                # last receipt, without re-running
witdiff doctor                        # is the configured command usable?
```

A test the tool marks ineligible, or a `test_source_unparsable` finding, means
structural analysis was skipped for that file. The red/green proof is unaffected,
but test-weakening rules did not run — say so.
