# Product specification

## Product statement

WitDiff is a local-first, agent-neutral verification engine that produces executable evidence for code changes, beginning with regression-test credibility.

## User problem

Coding agents frequently produce code and tests in the same patch. A green test suite on the final patch does not establish that the new test detects the original defect. The test may already pass before the fix, may have been weakened, or may belong to an older workspace state.

## v0.1 user promise

For repositories with dedicated changed test files, WitDiff can establish whether:

- the configured test command passes on the current workspace;
- the changed test-only patch fails on the base revision for a recognized test reason;
- suspicious test weakening is present;
- the workspace stayed unchanged while evidence was collected.

WitDiff reports uncertainty conservatively.

## The proof, step by step

1. identify changed dedicated test files;
2. run the configured test suite against the current workspace — the **green**
   run;
3. run the untouched base as a control, and require it to be green;
4. transplant only the changed tests onto the chosen base revision;
5. run the same command against base plus changed tests — the **red** run;
6. refuse to count a compile-only failure as behavioural evidence;
7. flag test weakening, such as a removed assertion or a newly ignored test;
8. fingerprint the workspace before and after, so stale evidence is never
   silently accepted;
9. write a machine-readable receipt that an agent, CI, or a human can consume.

Nothing in that sequence asks a model for an opinion. Steps 2, 3 and 5 are exit
codes; step 6 is a classification of output; step 7 is a structural comparison
between two revisions; step 8 is a hash.

## What is implemented

- Git repository discovery and automatic base selection
- changed-file classification and configurable dedicated-test globs
- tracked and untracked changed-test support across a detached base worktree
- test-only patch transplantation, with ineligible files named rather than
  smuggled through
- HEAD, pristine-base control and base-plus-tests runs, each with a bounded
  timeout
- framework-specific failure classification: cargo, pytest, Jest/Vitest, Go,
  Java and Ruby
- structural test-integrity analysis for Rust, Python, Go, Java, Ruby and
  JavaScript/TypeScript, sharing one rule engine
- NUL-delimited Git path handling, so a non-ASCII path is classified correctly
- workspace evidence fingerprinting that distinguishes build output from a real
  source change
- a content digest and revision binding, plus optional Ed25519 signatures
- environment evidence: the toolchain and dependency manifests the run used
- coverage of the changed production lines (Rust, opt-in)
- a sandboxed runner that executes the candidate's test command in a container
  (opt-in)
- an append-only provenance chain across verifications
- a committed gate policy deciding which results pass
- JSON receipt persistence, human and JSON CLI output
- `init`, `doctor`, `inspect`, `verify` and `receipt` commands
- an MCP server exposing `inspect`, `verify` and `receipt`
- a reusable GitHub Actions workflow and `--github-annotations`

Which parts are verified against real toolchains, as opposed to implemented, is
in [the support matrix](support-matrix.md).

## Primary users

- developers reviewing agent-generated patches;
- coding agents that need an external completion gate;
- CI systems enforcing evidence requirements;
- maintainers accepting external contributions.

## Non-users / non-goals

WitDiff is not:

- a code generator;
- a generic LLM agent framework;
- a replacement for code review;
- a security scanner;
- a coverage percentage tool **as a claim of proof** — coverage of the changed
  lines is recorded as supplementary evidence and never changes the verdict;
- an AI “confidence score.”

## Product principles

### Evidence over assertion

“Tests pass” is a claim. A recorded command, exact workspace fingerprint, exit status, and red/green comparison are evidence.

### Conservative truth

If WitDiff cannot distinguish a useful failure from an environment/setup failure, it must not call the patch verified.

### Vendor neutrality

Claude Code, Codex, Pi, OpenCode, CI, and humans all use the same engine.

### Local-first

The verifier should not require source upload or a cloud account.

### Composable artifacts

Every verification emits a structured receipt so other tools can reason over facts without scraping console prose.
