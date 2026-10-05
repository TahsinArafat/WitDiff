# Demo

A runnable demonstration of the one thing that distinguishes WitDiff from a
green CI run: it catches a test that passes on both revisions and therefore
proves nothing.

```bash
./demo/run.sh
```

The script builds a real Git repository in a temporary directory, with a real
bug, a real fix, and a real test — then runs plain `cargo test` and WitDiff
against the same change.

## What it shows

| Step | Plain CI | WitDiff |
| --- | --- | --- |
| A fix, plus a test that asserts `is_even(2) == is_even(2)` | green | `not_verified` |
| The same fix, with a test that asserts `is_even(2)` and `!is_even(3)` | green | `verified` |

Both tests are green in the workspace. Only the second constrains the
behaviour that changed. A reviewer skimming the diff sees a fix and a test in
either case.

The output WitDiff produces for the first:

```text
status           : not_verified
base + tests     : PASS
red/green proven : false
note             : changed tests also pass on the base revision; they do not
                   prove the behavioral change
```

## Running it

```bash
./demo/run.sh                        # with pauses
DEMO_PAUSE=0 ./demo/run.sh           # straight through, for a recording
WITDIFF=/path/to/witdiff ./demo/run.sh
```

Nothing is staged. The repository is real, cargo really builds, and the binary
is the one you have installed. If `witdiff` is not on `PATH` the script says so
and stops rather than substituting anything.

To record it:

```bash
asciinema rec demo.cast -c "DEMO_PAUSE=0 ./demo/run.sh"
```

## With a coding agent

The demo above runs the CLI directly. The agent path is the same command
mediated by a skill, which is what `integrations/` installs.

For Pi, the flow is:

```bash
pi install git:github.com/TahsinArafat/WitDiff@main
```

Then, in a repository with a bug:

```bash
pi -p "Fix the bug in src/lib.rs and add a regression test. Before you tell me \
it is done, verify the change using the witdiff skill, and report the exact \
status it gave you."
```

An agent with the skill loaded runs the verification and reports the receipt
rather than asserting success. Measured against a real Pi session on a
repository whose `add` subtracted instead of adding:

```text
- **src/lib.rs** — fixed `add` to compute `a + b` instead of `a - b`.
- **tests/regression.rs** (new) — regression test asserting `add(2, 3) == 5`.

## WitDiff verification
Exit code: **0**
"status": "verified"
"red_green_proven": true

The transplanted regression test failed on the base revision
(`base_run` exit 101, `failure_kind: "test_failure"`, panicking
`left: -1, right: 5`) and passed in the workspace.
```

The skill's value is not that it runs a command — the agent could do that
without it. It is that the agent reports the status and the supporting fields
instead of summarizing them as "tests pass", and says so when the result is
`not_verified`.
