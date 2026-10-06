# Agent integrations

WitDiff is a CLI plus JSON, so any harness can use it by running one command.
These directories carry the ready-made pieces, each written against the
harness's own documented extension format.

| Directory | Harness | Format | Install to |
| --- | --- | --- | --- |
| [`claude/`](claude) | Claude Code | skill (Agent Skills spec) | `.claude/skills/` |
| [`opencode/`](opencode) | OpenCode | plugin + skill | `.opencode/plugins/`, `.opencode/skills/` |
| [`pi/`](pi) | Pi | package (`pi` manifest) | `pi install ./pi` |
| [`cursor/`](cursor) | Cursor | project rule (`.mdc`) | `.cursor/rules/` |
| [`generic/`](generic) | any | AGENTS.md snippet | `AGENTS.md`, `CLAUDE.md` |

Every one carries the same instructions: run the verification, quote the
receipt, and never present a failed gate as success. They differ only in how the
harness loads them.

## Install

```bash
# From anywhere — no repository needed.
URL=https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/integrations/install.sh
curl -fsSL $URL | sh                          # into the current project
curl -fsSL $URL | sh -s -- --global           # into your user config
curl -fsSL $URL | sh -s -- --uninstall        # remove only what this installed
```

From inside a clone the same script is a local file, so the flags are shorter:
`integrations/install.sh --global`. Both forms behave identically.

It installs only the harnesses you actually have, and it never overwrites a file
you have edited — each skipped file is reported.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/integrations/install.sh | sh -s -- --uninstall
```

Removes the four files it ships: the Claude Code skill, the OpenCode plugin and
skill, and the Cursor rule. Shared directories are removed with `rmdir` so a
directory still holding your own files is left alone, and the script says what
it declined to remove.

**Pi keeps its own record**, so it is not touched by the above. Remove the
package with:

```bash
pi remove <path-to>/integrations/pi
```

or, if it was installed with `--local`, edit `.pi/settings.json`.

## The one command

```bash
witdiff verify --base origin/main --strict --json
```

Three exit codes, not two:

- `0` — proven, or there was nothing to prove
- `1` — WitDiff itself failed to run; a tool error, not a verification result
- `2` — verification did not pass

Exit 0 is not by itself proof. The receipt's `status` says what was established.

## Claude Code

```bash
mkdir -p .claude/skills
cp -r integrations/claude/skills/witdiff .claude/skills/
```

The skill loads when Claude judges it relevant, or on `/witdiff`. For every
project on the machine, copy to `~/.claude/skills/` instead.

## OpenCode

```bash
mkdir -p .opencode/plugins .opencode/skills
cp integrations/opencode/plugins/witdiff.ts .opencode/plugins/
cp -r integrations/opencode/skills/witdiff .opencode/skills/
```

The plugin adds a **`witdiff_verify` tool**, so the agent calls verification
directly instead of composing a shell command, and gets the receipt back with a
prominent warning when the gate failed. Put it under `~/.config/opencode/` for
every project.

The plugin needs `@opencode-ai/plugin`, which OpenCode provides:

```json
// .opencode/package.json
{ "dependencies": { "@opencode-ai/plugin": "*" } }
```

## Pi

```bash
pi install ./integrations/pi
```

Or from git once tagged:

```bash
pi install git:github.com/TahsinArafat/WitDiff@v1.0.0
```

The package declares its skill under the `pi` key in `package.json`, so Pi
discovers it without a conventional-directory guess. Add `--local` to record the
package in `.pi/settings.json` for the project rather than your user settings.

## Cursor

```bash
mkdir -p .cursor/rules
cp integrations/cursor/rules/witdiff.mdc .cursor/rules/
```

The rule sets `alwaysApply: true`, so it is in context for every session — the
verification step is not something to remember conditionally. Cursor also reads
`AGENTS.md`, so the generic snippet below works as an alternative.

## Anything else

[`generic/AGENTS.md`](generic/AGENTS.md) holds an instruction snippet that works
in any harness reading project guidance — Codex, Amp, Jules, or a plain
`AGENTS.md`/`CLAUDE.md`. Copy the section between the markers.

## Keeping these honest

Each harness's format was verified against that project's own documentation at
the time of writing, and the OpenCode plugin is typechecked against the real
`@opencode-ai/plugin` package. If a harness changes its plugin or skill API,
these files need updating — a silently unloaded skill looks exactly like a
working one that had nothing to say.
