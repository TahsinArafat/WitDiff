# Receipt contract

The current schema ID is `witdiff.receipt.v1`.

The receipt records:

- generation time;
- base and head identities;
- exact workspace fingerprints before/after;
- changed files and changed tests;
- integrity findings;
- HEAD command/evidence;
- pristine BASE control command/evidence;
- BASE+changed-tests command/evidence;
- red/green decision;
- final conservative status;
- human-readable notes for unsupported/ambiguous cases.

Consumers should prefer enum/status fields over parsing notes or stdout.

See `schemas/witdiff.receipt.v1.schema.json`.

## Run results

Each recorded run (`head_run`, `base_control_run`, `base_run`) carries:

- `command`, `cwd`, `success`, `exit_code`, `duration_ms`;
- `stdout` / `stderr`, truncated to `verification.max_output_bytes`;
- `failure_kind`: `test_failure`, `compile_error`, `command_failure`,
  `spawn_failure`, `timeout`, `unknown`, or `null` on success;
- `timed_out`: true when the run was killed for exceeding
  `verification.timeout_secs`.

`failure_kind = "timeout"` is never accepted by a proof path. See
`docs/verification-model.md` for the status each timeout produces.
