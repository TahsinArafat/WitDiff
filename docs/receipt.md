# Receipt contract

The current schema ID is `witdiff.receipt.v1`.

The receipt records:

- generation time;
- base and head identities;
- exact workspace fingerprints before/after;
- changed files and changed tests;
- integrity findings;
- which test command variant produced the evidence, and the exact command used;
- HEAD command/evidence;
- pristine BASE control command/evidence;
- BASE+changed-tests command/evidence;
- red/green decision;
- final conservative status;
- human-readable notes for unsupported/ambiguous cases.

Consumers should prefer enum/status fields over parsing notes or stdout.

See `schemas/witdiff.receipt.v1.schema.json`.

Changes made after the first release are additive and optional within
`witdiff.receipt.v1`. There is no v2 and no breaking change: a receipt written
before a field existed still deserializes, and each new field defaults to the
value that matches how such receipts were actually produced.

## Test selection

- `test_selection`: `full_suite` or `targeted`, naming which variant produced
  `head_run` and `base_run`. It defaults to `full_suite`, which is what receipts
  written before this field existed recorded.
- `effective_test_command`: the exact argument vector used for the proof runs,
  after any narrowing. It is absent only when no command was run.

A `targeted` receipt makes a smaller claim than a `full_suite` receipt, so
consumers should read this field rather than infer scope from the configured
command. When narrowing was requested but declined, `test_selection` stays
`full_suite` and the reason appears in `notes`; narrowing never silently
substitutes evidence.

## Changed files

Each `changed_files` entry carries two optional fields:

- `path_is_lossy`: true when the path could not be decoded as UTF-8 and was
  lossily replaced. The lossy text names no file on disk, so WitDiff does not
  transplant it. Omitted when false.
- `previous_is_test`: for a rename or copy, whether the source path was itself a
  test. `false` means the file was moved from production code and must not be
  part of a test-only transplant. Omitted when true; absent means false.

A changed test excluded by either condition is named in `notes` and prevents
`verified` and `verified_with_warnings`, because a partial transplant is not
evidence about the change as a whole.

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
