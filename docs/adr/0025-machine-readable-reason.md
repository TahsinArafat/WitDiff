# ADR-0025: A machine-readable reason on every receipt

Status: accepted

## Context

WitDiff's primary consumers are coding agents. `status` told them what happened;
nothing told them *why*, and the remedies differ.

`not_verified` alone covered four unrelated situations:

- the changed tests also pass on the base, so they pin nothing;
- the base control run failed, so no later failure can be attributed;
- a run disagreed with itself, so nothing observed is evidence;
- behaviour was proven but a high-severity integrity finding blocks the verdict.

Each needs a different action — strengthen the assertion, fix the base, find the
flake, address the finding — and the receipt did not distinguish them in any
field a program could read. The specific cause existed only as English prose in
`notes`.

That is the wrong shape for this project. An agent had to substring-match a
sentence to branch, which means a reworded note silently changes a caller's
behaviour, and the caller's correctness depends on text that was never a
contract.

Measured before the change:

```json
{ "status": "not_verified", "red_green_proven": false,
  "notes": ["changed tests also pass on the base revision; they do not prove the behavioral change"] }
```

There is no way to tell that apart from a flaky experiment without reading
English.

## Decision

Every receipt carries `reason`: one stable token naming why the status was
reached. `status` answers *what*; `reason` answers *why*.

- Fourteen tokens, one per distinct cause, plus `unrecorded`.
- Emitted in `verify` output (both text and JSON), returned by the MCP `verify`
  tool with `reason_is_actionable_by_author` and `remediation`, documented in
  `docs/receipt.md`, and enumerated in `schemas/witdiff.receipt.v1.schema.json`.
- `reason_is_actionable_by_author` separates "the change could be proven and
  simply is not yet" from "something is wrong", which is the distinction an
  agent most needs and the one prose conveyed worst.

## Consequences

**Additive within `witdiff.receipt.v1`.** `reason` is not in the schema's
`required` list and defaults to `unrecorded`, so a receipt written before it
existed still parses and still validates. The field claims nothing rather than
guessing a cause from the status, because a guess presented as a token is worse
than an honest absence. Asserted by a test that parses a hand-written legacy
receipt.

**Two lists describing one set will drift, so a test binds them.** The schema
enum and the Rust enum are read together: every schema token must deserialize
into a real variant, and every variant must appear in the schema. Renaming a
token without updating the schema fails the suite — verified by doing it.

**`test_command_unavailable` is distinguished from `base_control_failed`.**
Both end in `not_verified`, and telling them apart matters: one is a toolchain
or a missing dependency directory, the other is the code. This came out of a
tester report about WordPress plugins, where a gitignored `vendor/` made the base
worktree unable to start and the receipt blamed the base revision.

**The prose stays.** `notes` is still written for a human reading a terminal,
and `remediation` still gives a sentence of advice. The token is for the
program; it does not replace the explanation, it stops the program needing one.

**This does not make WitDiff interpret anything.** The reason is computed from
the same deterministic facts as the status — which run failed, how, and whether
the classifier recognized it. It is a name for a condition the tool already
detected, not a judgment about the change.
