# ADR-0019: Missing toolchains — use the project's pin, report, never guess

Status: accepted

## Context

Requested behavior: auto-install a missing test toolchain (`--install-on-demand`)
rather than failing. Investigating that request produced a more important finding
first.

Measured, in a Java repository whose `test_command` is `mvn test` on a machine
with no Maven:

```text
$ witdiff verify
witdiff: failed to execute test command: mvn: No such file or directory
$ ls .witdiff/
(no receipt exists)
```

The run aborts and **no receipt is written at all**. Every finding is discarded,
including integrity findings that never needed Maven — in that repository the
test had been gutted to `assertTrue(true)`, which the JDK-only analyzer detects
without Maven being present. A developer without Maven learns nothing, when
WitDiff could have told them their test no longer asserts anything.

That is a defect independent of auto-install, and it is worse than the missing
feature.

## Evidence

**The test command can fail to start.** `mvn`, `gradle`, `dotnet` and similar
may be absent. Today this is a hard error that discards the whole run.

**Analysis toolchains and test toolchains are different things.** Java's
integrity analysis needs only a JDK; its test command may need Maven. Python's
analysis needs the interpreter; Go's needs the toolchain, which is also its test
runner. Conflating them is why a missing Maven loses JDK-based findings.

**Three ecosystems already ship a pinning mechanism, and two ship an installer.**

| Ecosystem | Pin | Installer already present |
| --- | --- | --- |
| Rust | `rust-toolchain.toml` with an exact `channel` | `rustup` |
| Python | `.python-version` (pyenv), or the command's own interpreter | `uv`, `pyenv` |
| Node | `.nvmrc`, `package.json` `engines` | `nvm`, `corepack` |
| Go | `go.mod` `go` directive, `.go-version` | `go` toolchain (self-installing since 1.21) |
| Java | `mvnw` + `.mvn/wrapper/maven-wrapper.properties` | **the wrapper itself** |
| Java | `gradlew` + `gradle/wrapper/gradle-wrapper.properties` | **the wrapper itself** |

The Java row is decisive. Maven and Gradle projects conventionally commit a
wrapper script that downloads and runs an exactly pinned distribution, and
Gradle's properties file supports `distributionSha256Sum`. So the correct action
for a Java project is not for WitDiff to fetch Maven — it is to use the
wrapper the project already committed, which is pinned, checksummed, and the
same thing a human developer would run.

## Decision

### 1. A missing test toolchain no longer discards the run

When the configured test command cannot be started, WitDiff:

- writes a receipt;
- sets `status` to `not_verified`, because no proof was performed;
- records a `head_run` whose `failure_kind` is `spawn_failure`, so a consumer can
  distinguish "the tool was absent" from "the tests failed";
- keeps every integrity finding, which was computed before the run and does not
  depend on it;
- adds a note naming the missing program and, when one is known, the command to
  install it.

`no_changed_tests` and `not_verified` already exist; this uses them rather than
inventing a status, and the distinction that matters — "nothing was proven" — is
already representable.

### 2. `--install-toolchains` is opt-in, off by default, and narrow

The flag is named for what it does rather than for a general "on demand",
because the scope is deliberately bounded:

- **It runs the project's own installer when one is committed.** For a Java
  project with `mvnw`, it runs `./mvnw`. For Gradle, `./gradlew`. Those downloads
  are pinned by the project's own files and are what a developer would run
  anyway. WitDiff is not choosing a version; the repository is.
- **It never picks a version itself.** No "latest", no version inferred from a
  filename. If the project has no pinned version and no installer, WitDiff
  reports what to install and stops.
- **It runs no package manager that executes arbitrary build code.** `npm
  install`, `pip install` and `go install` run project-controlled scripts. That
  is the developer's decision, not a verification tool's.
- **It never installs silently.** Every attempt is recorded in the receipt and
  printed, with the exact command run and whether it succeeded.

### 3. Report the exact command when it will not install

Every missing-toolchain path ends with an actionable line naming the program and
a concrete install command for the detected ecosystem, so the developer is never
left to work out what to do.

### 4. Network access stays out of the core

Installation is performed by the CLI, before verification begins, using the same
process-spawning path as any other external command. `witdiff-core` gains no
network capability (invariant 4). The default path — no flag — performs no
network access at all.

## Consequences

- A developer without Maven now receives a receipt with the integrity findings
  and a clear statement that the proof was not attempted. That is strictly more
  useful than an error with no output.
- Auto-install is bounded by what the repository already declares, which is both
  safer and more reproducible than WitDiff choosing a version.
- The flag cannot make a verification *pass*: it can only allow the test command
  to start. Status is still decided by the red/green experiment.
- A toolchain installed by the flag lands in the ecosystem's normal cache
  (`~/.m2/wrapper`, `~/.gradle/wrapper`, `~/.rustup`), not inside the verified
  repository, so it does not perturb the workspace fingerprint. This must be
  verified per ecosystem rather than assumed: an installer that writes into the
  repository would change the fingerprint mid-run and invalidate its own
  evidence.
- Windows wrappers are `.cmd`/`.bat` rather than shell scripts, so wrapper
  detection must check for those too rather than assuming `./mvnw` is
  executable.

## Alternatives considered

- **Install whatever is missing, from a package manager.** Rejected: it runs
  arbitrary project-controlled scripts, picks versions WitDiff has no basis to
  choose, and makes the same commit verify differently on two machines.
- **Install silently by default.** Rejected: downloading and executing
  third-party code is a decision the operator must make explicitly, and a
  verification tool that mutates the environment without being asked is one
  people stop trusting.
- **Auto-detect and install the "right" version from a file WitDiff parses.**
  Rejected for anything except delegating to the project's own wrapper. Version
  resolution across six ecosystems is a large surface with no verification value,
  and getting it wrong is silent.
- **Keep aborting when the test command is missing.** Rejected: it discards
  findings that do not depend on that command, which is the actual bug found
  here.
- **Add a new `toolchain_missing` status.** Rejected: `not_verified` with
  `failure_kind: spawn_failure` already expresses it precisely, and a new status
  would need a schema change for no additional information.
