# ADR-0021: JavaScript integrity analysis uses the project's own parser

Status: accepted; verified against real `acorn` and `typescript` after publication

## Context

JavaScript is the last language without structural integrity analysis. It was
deferred because Node ships no parser, which is a different problem from the one
every other language had. This ADR resolves it.

## Evidence

- **Node has no built-in parser.** `vm.SourceTextModule` is reachable only with
  `--experimental-vm-modules`, throws when its namespace is accessed
  (`Module status must not be unlinked or linking`), and exposes no AST at all
  (`'ast' in module` is `false`). Verified directly.
- **`acorn` and `@babel/parser` are absent from a bare Node install.** Verified.
- **But a real Jest or Vitest project always has one in its own
  `node_modules`.** Verified that Node resolves a parser from the project's
  directory.
- **Node resolves `require` relative to the script's directory, not the working
  directory.** This was the bug that nearly shipped: running the tool from a
  temporary directory failed to find the project's `acorn` *even with the
  working directory set to the project*, verified directly. The project
  directory is therefore passed as an argument and each candidate is required by
  absolute path.

That last point is what makes this the same shape as every other language: the
parser is present in any project that can run its tests. The one difference is
where it comes from — the runtime for Python, Go, Java and Ruby, and the
project's dependency tree for JavaScript.

## Decision

Analyze JavaScript by requiring a parser **from the project under
verification**, trying, in order:

1. `@babel/parser` — present in any Jest project through Babel;
2. `acorn` — the reference ESTree parser, present in much of the ecosystem;
3. `typescript` — which parses TypeScript and is present in any project using
   Vitest or typed tooling.

All three produce an ESTree-compatible tree, so one traversal serves them all.

When no parser is found, WitDiff **reports that the file was not analyzed**,
naming the packages it looked for and how to install one. It never falls back to
regex or to a weaker analysis, because a silent degradation is what invariant 7
forbids, and a line-based matcher against `expect(...)` cannot tell an inverted
expectation from a reworded string.

TypeScript is supported wherever the `typescript` parser is present, which is the
common case for a TS project: `ts.createSourceFile` gives the AST directly and
reports syntactic errors the way the other tools do.

## Consequences

- JavaScript and TypeScript gain the same rules as the other five languages.
- **A project with no parser gets fewer findings, not wrong ones**, and the
  receipt says why. This is the honest outcome and is the same position Ruby was
  in before ADR-0020.
- No dependency is added. Requiring `acorn` or `typescript` would mean the
  analysis silently does nothing in projects that lack them, which is precisely
  the failure the support matrix records for the line-based fallback.
- The traversal targets ESTree, which all three parsers produce, so supporting a
  fourth parser later is a lookup rather than new logic.
- ~~The development environment has no parser installed and none can be added.~~
  **Superseded: it now runs against real parsers.** `acorn` and `typescript` are
  dev dependencies and the end-to-end tests exercise both. That closed the last
  verification gap in this ADR and found six bugs the parser-absence contract
  and the hand-written ESTree cases had both missed — among them that ESTree's
  `Literal` was never normalized, so `toBe(2)` and `toBe(3)` rendered
  identically and `changed_expected_value` never fired for JavaScript at all.

  The absence contract is still tested, and still matters: a project with no
  parser is reported rather than analyzed more weakly.
- The parser is resolved **from the project directory**, not from WitDiff's own
  installation. A parser WitDiff bundled could differ from the one the project
  tests with, and the difference would show up as inexplicable findings.

## Alternatives considered

- **Bundle a parser with WitDiff.** Rejected: the analysis would then use a
  different parser from the project's, and a version difference could appear as
  spurious findings. Worse, it would mean `witdiff install` must fetch code to
  verify code.
- **Shell out to the project's own build tool** (`tsc --noEmit` and similar).
  Rejected: that reports type errors, not assertion structure, and does not give
  a tree.
- **Regex over the source.** Rejected on the same evidence as every other
  language: it cannot distinguish `expect(a).toBe(1)` from
  `expect(a).toBe(2)`, and it reports noise on reformatting.
- **Keep JavaScript unsupported.** Rejected as the end state: it is the largest
  remaining gap by user base, and the parser availability question now has a
  measured answer rather than an unknown one.
