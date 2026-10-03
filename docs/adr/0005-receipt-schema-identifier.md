# ADR-0005: The receipt schema identifier is `witdiff.receipt.v1`

Status: accepted

## Context

`AGENTS.md` invariant 8 requires that backward compatibility of the receipt
schema be preserved "unless an ADR explicitly introduces a new schema version."

The schema identifier is a wire value. It appears as `schema_version` in every
serialized receipt, as the `$id` of the published JSON Schema, and as the schema
document's file name. Because it is the string a consumer must match on, any
change to it is a breaking change and needs an explicit decision rather than an
incidental edit.

## Decision

The schema identifier is **`witdiff.receipt.v1`**.

The version number is `v1`, matching the schema document
`schemas/witdiff.receipt.v1.schema.json`, the `$id`
`https://witdiff.dev/schemas/witdiff.receipt.v1.schema.json`, and the
`schema_version` field emitted by `witdiff-core`.

## Consequences

- A consumer must match on exactly `"witdiff.receipt.v1"`. Any other string —
  including any older name for this project's schema — is not this schema.
- The field set, field types, and field meanings are the contract. Adding a
  field is a compatible change when it carries a serialization default, as
  `RunResult::timed_out` does; removing or repurposing a field is not.
- `schema_version` is part of the artifact precisely so a reader can refuse to
  interpret a receipt it does not understand. Consumers should check it rather
  than assume.

## Alternatives considered

- **Omit the identifier.** Rejected: without it a consumer cannot distinguish a
  receipt it understands from one it does not, and the proof artifact becomes
  self-describing only by convention.
- **Version it per release.** Rejected: the version tracks the schema shape, not
  the product version. A product release that does not change the shape must not
  invalidate existing consumers.
