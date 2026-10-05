# ADR-0009: Git path output is read NUL-delimited

Status: accepted

## Context

WitDiff learns which files changed by running `git diff --name-status` and
`git ls-files --others`. Both were read by splitting their output on line
breaks.

Git does not emit raw paths in that mode. A path containing any byte outside
plain ASCII is C-quoted: `tests/café.rs` arrives as the literal 22-character
text `"tests/caf\303\251.rs"`, quotes and backslash escapes included. That string
matches no configured test glob and names no file on disk, so such a change was
silently classified as production code and never transplanted. The failure was
silent in the worst way: the receipt simply reported no changed tests.

Line splitting is independently unsafe, because a filename may legally contain
a newline, which desynchronizes the whole record stream.

## Decision

Path-producing plumbing commands are invoked with `-z` and their output is
parsed as NUL-delimited records, each decoded to a UTF-8 `String`.

Non-UTF-8 paths cannot be represented in a JSON receipt, so they are lossily
decoded — and that loss is recorded per record at decode time, because once
decoded the lossy form is itself valid UTF-8 and the fact can no longer be
recovered. `ChangedFile::path_is_lossy` marks those entries, and such a path is
never transplanted (see ADR-0007).

## Consequences

- Repositories with non-ASCII test filenames are classified correctly instead of
  being silently skipped.
- A filename containing a newline is a single path, not two.
- The `ChangedFile` schema gained one additive, optional field.
- The unit tests cover decoding directly from raw bytes, because creating a
  non-UTF-8 filename is not permitted on every platform and under every
  sandbox; the integration test covers valid-but-tricky names end to end.

## Alternatives considered

- **Set `core.quotePath=false` globally.** Rejected: it changes repository
  configuration, does not survive every Git version identically, and still
  leaves newline-containing paths ambiguous.
- **Unquote the C-escapes ourselves.** Rejected: that is a second parser for a
  format Git already offers a clean mode for.
