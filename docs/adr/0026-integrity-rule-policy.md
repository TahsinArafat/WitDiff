# ADR-0026: Integrity-rule policy waives the gate, never the observation

Status: accepted

## Context

Every `High` integrity finding blocks `verified` (ADR-0013). That is the right
default, and it is also too blunt for a repository with a legitimate exception:
a project that has accepted a particular weakening, or whose tests include a
directory it does not own, had no way to record that decision. Its only options
were to leave the gate red, or to stop running WitDiff.

`AGENTS.md` had this as open work (PG-202): "Organizations marking an individual
rule block/warn/ignore. Distinct from `[gate]`, which decides which *statuses*
pass; this would decide how a *finding* is weighted. **It must change only how
an observation is reported, never the observation itself.**"

That last sentence is the whole design constraint, and it is the reason the
obvious implementation is wrong.

## Decision

A committed `[policy]` section in `witdiff.toml` declares rule weights and path
ignores. It changes the **verdict**, never the **finding**.

```toml
[policy]
rules = [
  { rule = "removed_assertion", weight = "warn", reason = "reviewed by hand; JIRA-1234" },
]
ignore_paths = [
  { path = "vendor", reason = "generated dependency code, not ours" },
]
```

- `Weight` is `block` | `warn` | `ignore`. With no policy the behaviour is
  unchanged: a `High` finding blocks.
- Every waiver is recorded in the receipt's `waivers` list, naming the rule, the
  path, the reason, and whether it came from a rule override or a path ignore.
- The finding stays in `integrity_findings` at its original severity and is
  still printed by `witdiff verify`, immediately above the waiver list.
- `reason` is required on every override, by the type rather than by convention.

## Consequences

**A repository cannot hide a weakening from its own reviewers.** The finding is
still there, still `High`, still printed. What changed is only whether it
withholds the verdict. A mechanism that removed the finding from the receipt
would let a policy silently convert a gutted test into a clean run — which is
the failure this tool exists to catch, so it is not offered.

**A waiver that changed nothing is not recorded.** A rule configured `warn`
that produced a `Warning` was never blocking, so nothing was waived; recording
it would train a reader to skip the waiver list.

**The override is per-rule and does not leak.** Asserted in both the unit tests
and end to end, because a leak would disable every rule at once — the opposite
of a narrowing policy. Confirmed by breaking `weight_for` to return the first
override for every rule and watching two tests fail.

**A policy may only make a rule less blocking.** There is no `high` weight that
would make a `Warning` block. `High` already means the strongest signal the
analyzer produces; a policy that could escalate a `Warning` would be inventing a
severity the analysis never assigned, which is a judgment about the code rather
than a weighting of an observation.

**This is not an allow-list of findings.** A waiver names a *rule*, not an
individual finding, so it cannot be used to silence one specific occurrence
while leaving the rule enforced elsewhere. That is deliberate: a per-finding
suppression would need identity for each finding, and identity across revisions
is exactly the kind of unstable key that makes a policy silently stop applying.
If a single occurrence needs waiving, a path ignore is the expressible form.

**Found while building it: the blocking check was named `has_high` and was
counting the wrong thing.** It counted every `High` finding rather than every
*blocking* one. Renamed to `blocked` when the policy made the two differ, since
a name that lies is how the next change goes wrong.

**Also found: two `verify` runs against one fixture are not comparable.** The
end-to-end fixture shares one `[build] target-dir` between the workspace and the
base worktree, so a second run can observe a stale build and fail at
`head_failed` before reaching any integrity logic. The waiver tests therefore
build their own fixture rather than reusing one. This is a property of the
fixture, documented at the top of that file, not of the policy.
