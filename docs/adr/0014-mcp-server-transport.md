# ADR-0014: The MCP server is a hand-written stdio adapter, not an SDK client

Status: accepted

## Context

PG-501 requires a "thin protocol wrapper" over `inspect`, `verify` and
`receipt`, with the constraint that it "must call `witdiff-core` and must not
create an alternative verification implementation". The architecture note in
`docs/architecture.md` already reserved a `witdiff-mcp` crate for exactly this.

The obvious implementation is the official Rust SDK, `rmcp`. It was evaluated
first, and the evaluation is what produced this decision.

## Evidence

Measured against this workspace (`rust-version = "1.78"`, 95 packages in the
lockfile, no async runtime):

- **`rmcp` 3.5.0 requires rustc 1.88.** Cargo does not error; it silently
  substitutes `rmcp` 2.2.0 to satisfy the workspace's declared `rust-version`.
  A silent major-version downgrade is a poor foundation for a protocol
  implementation.
- **It pulls 68 packages**, including `tokio` and `async-trait`. The workspace
  currently has no async runtime and no runtime dependencies beyond `serde` and
  `serde_json`.
- **It could not be built in this environment at all.** The sandbox denies
  writes to the cargo cache, so `rmcp` and its transitive dependencies could not
  be downloaded or compiled.

That last point is decisive on its own. Shipping a dependency that cannot be
compiled or tested here would mean committing code whose behavior was never
observed — the precise failure this project exists to detect in others.

## Decision

Implement the MCP stdio transport and the three tools directly, using
`serde_json`, which the workspace already depends on.

The scope is genuinely small, and the protocol schema was read rather than
recalled:

- newline-delimited JSON-RPC 2.0 over stdin/stdout;
- `initialize`, answering with the negotiated `protocolVersion` and a `tools`
  capability;
- `notifications/initialized`, which requires no response;
- `tools/list`, returning the three tool descriptors and their input schemas;
- `tools/call`, dispatching to `witdiff-core`;
- `ping`, answered trivially.

Everything else (`resources/*`, `prompts/*`, `sampling/*`, HTTP transports,
OAuth) is out of scope and answered with a JSON-RPC `Method not found` error, so
a client is told plainly rather than left waiting.

Two protocol details are load-bearing and are implemented deliberately:

1. **Tool failures are results, not protocol errors.** The specification states
   that errors originating from the tool are reported inside the result with
   `isError: true`, because a protocol-level error is invisible to the model,
   which then cannot self-correct. A WitDiff run that exits 2 is therefore a
   successful tool call reporting a failed gate, and `content` is always
   present as the schema requires.
2. **stdout carries only protocol frames.** Every diagnostic goes to stderr. A
   stray `println!` anywhere in the call path would corrupt the stream, so the
   handler writes exclusively through one writer and diagnostics use `eprintln`.

## Consequences

- Zero new dependencies. The adapter builds and is fully testable offline,
  which satisfies invariant 4 directly.
- The protocol surface is bounded and covered by tests that drive the server
  over a pair of in-memory buffers, so the framing, the request routing and the
  result shapes are all exercised without spawning a process.
- MCP frame compatibility is now WitDiff's responsibility. A protocol revision
  requires a code change rather than a dependency bump. This is the real cost of
  the decision, and it is accepted because the implemented subset is small and
  version negotiation is explicit in `initialize`.
- The tool must not drift from the CLI. Both call the same `witdiff-core`
  functions, and the tool results embed the receipt unchanged rather than
  re-rendering it, so a consumer sees the same `witdiff.receipt.v1` document the
  CLI writes. Rendering decisions stay in one place.
- The adapter does not decide gate policy. It returns the receipt and its
  `status`; gating belongs to `VerificationStatus::gate` (ADR-0013), so MCP
  cannot disagree with CI.

## Alternatives considered

- **Use `rmcp` anyway and accept the version substitution.** Rejected: it cannot
  be compiled or tested in this environment, and adopting a silently downgraded
  SDK major version is not a decision to make on faith.
- **Shell out to the `witdiff` binary instead of linking the core.** Rejected:
  it would add a process boundary and make the receipt a parsing exercise, when
  the requirement is explicitly to call the core. It would also mean the MCP
  server's behavior depends on which `witdiff` happens to be on `PATH`.
- **Wait for a lighter SDK, or for the workspace MSRV to rise.** Rejected as a
  blocker: the tools are three functions over an already-stable contract, and
  the work is small enough to do correctly now. If `rmcp` becomes buildable
  later, swapping the transport while keeping the tool layer is a contained
  change.
- **Expose more of the protocol (resources for receipts).** Rejected for now:
  the backlog permits only `inspect`, `verify` and `receipt`, and resources
  would add a second way to read a receipt that could drift from the tool
  result.
