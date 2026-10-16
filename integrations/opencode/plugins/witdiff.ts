/**
 * WitDiff verification plugin for OpenCode.
 *
 * Adds a `witdiff_verify` tool so the agent can run the verification and read
 * the receipt without shelling out by hand, and blocks the failure mode this
 * tool exists to prevent: announcing a change is complete after a failed gate.
 *
 * Install: place this file in `.opencode/plugins/` (project) or
 * `~/.config/opencode/plugins/` (global). OpenCode loads it at startup.
 *
 * ## OpenCode v2
 *
 * v2 replaced the v1 plugin shape entirely. A v1 plugin — a named export of an
 * async function returning `{ tool: { ... } }`, run with Bun's `` $`` — is
 * rejected at load with:
 *
 *     Plugin must export a default definition with an id and an effect or setup
 *     function.
 *
 * v2 requires a **default export** of `{ id, setup(context) }` (or
 * `{ id, effect(context) }`). The tool is registered with
 * `context.tool.add(...)` during setup, and the shell helper `` $`` is gone:
 * v2's `shell` domain exposes only a `create.before` hook, so the CLI is spawned
 * through `node:child_process` against an argument vector, never a shell
 * string. `define` is not on the package root either — it lives in the
 * `promise` submodule.
 *
 * Both shapes are exported: `default` for v2, and the named `WitDiffPlugin` for
 * v1, which is what a v1 host looks for. Whichever the host understands is the
 * one it uses, and neither is a stub.
 *
 * The tool returns the receipt rather than a verdict summary. The agent's job
 * is to report what the tool said, including what it could not determine — so
 * the gate decision and the reasons travel together.
 */
import { spawn } from "node:child_process"
import { define } from "@opencode-ai/plugin/promise/plugin"

const BASE_DESCRIPTION =
  "Git ref to verify against, normally the pull request base branch. " +
  "Defaults to origin/main."

/** A git ref safe to pass as a positional argument. */
function checkRef(ref: string): string | null {
  if (ref.startsWith("-") || !/^[\w./@^~{}-]+$/.test(ref)) {
    return (
      `refusing to verify against ${JSON.stringify(ref)}: a git ref may contain ` +
      "characters that would change the meaning of the command."
    )
  }
  return null
}

/**
 * Run the CLI and capture its output plus exit code.
 *
 * `args` is a fixed vector built by the caller, never a model-supplied string.
 * The base ref is the one value that comes from outside, so it is validated
 * before it reaches the argument vector, and the vector is handed to `spawn`
 * with `shell: false` — there is no shell to inject into, which is what v1's
 * string interpolation relied on and what v2 removed.
 *
 * Exit code 2 is a failed gate and 1 is a tool error, so neither is an
 * exception here: both are results the caller must report.
 */
function runCli(
  command: string[],
  root: string,
): Promise<{ code: number; stdout: string; stderr: string }> {
  const [bin, ...args] = command
  return new Promise((resolve) => {
    const child = spawn(bin!, args, { cwd: root, shell: false })
    let stdout = ""
    let stderr = ""
    child.stdout.on("data", (chunk) => (stdout += chunk))
    child.stderr.on("data", (chunk) => (stderr += chunk))
    child.on("error", (error) =>
      resolve({ code: 1, stdout, stderr: stderr + String(error) }),
    )
    child.on("close", (code) => resolve({ code: code ?? 1, stdout, stderr }))
  })
}

/** The receipt, or `null` when the output is not one. */
function parseReceipt(stdout: string): any | null {
  try {
    return JSON.parse(stdout)
  } catch {
    return null
  }
}

/** Render the receipt as the report the agent must pass on. */
function render(
  receipt: any | null,
  code: number,
  stdout: string,
  stderr: string,
): string {
  if (code === 1) {
    return [
      `# WitDiff could not run (exit code ${code})`,
      "",
      "This is a tool error, not a verification result. Fix the invocation",
      "before drawing any conclusion about the change.",
      "",
      "```",
      (stderr || stdout).trim(),
      "```",
    ].join("\n")
  }

  const status = receipt?.status ?? "unknown"
  const lines = [
    `# WitDiff: ${status}`,
    "",
    `- exit code: ${code}`,
    `- status: \`${status}\``,
  ]

  if (receipt) {
    lines.push(`- red/green proven: ${receipt.red_green_proven}`)
    lines.push("", "## Notes", "")
    for (const note of receipt.notes) lines.push(`- ${note}`)
  } else {
    lines.push(
      "",
      "The receipt could not be parsed:",
      "```",
      stdout.trim(),
      "```",
    )
  }

  lines.push(
    "",
    "Report the status and every finding above. Do not summarize them as",
    '"tests pass". If a note says a case was unsupported, say so — an',
    "unsupported case is not a pass.",
  )
  return lines.join("\n")
}

/**
 * Register the tool. Shared by both plugin shapes so there is one
 * implementation and no chance of the v1 and v2 tools drifting apart.
 *
 * `input` is a plain JSON schema: v2's `Tool.ValueSchema` accepts a
 * `JsonSchema`, which is what a v1-style tool declaration also was, so the
 * same object serves both hosts.
 */
function witdiffVerify(root: string) {
  return {
    name: "witdiff_verify",
    description:
      "Verify that the current change is proven by its tests. Runs WitDiff's " +
      "red/green experiment and returns the receipt. Call this before claiming " +
      "a change is complete, verified, or fixed. An exit code of 2 means the " +
      "gate failed and must not be reported as success.",
    input: {
      type: "object" as const,
      properties: {
        base: { type: "string" as const, description: BASE_DESCRIPTION },
      },
      required: [],
    },
    async execute(input: unknown, _context: unknown) {
      // v2 types the tool input as `unknown` — the schema is a JSON schema the
      // host validates at runtime, not a type the compiler can narrow to. The
      // base ref is re-checked below regardless, so nothing here is trusted.
      const { base = "origin/main" } = (input ?? {}) as { base?: string }
      const refusal = checkRef(base)
      if (refusal) throw new Error(refusal)
      const result = await runCli(
        ["witdiff", "verify", "--base", base, "--json", "--no-write"],
        root,
      )
      // v2's `execute` must return a Result object, not a bare string. Returning
      // the string directly typechecks nowhere and at runtime is not a tool
      // result at all; `content` is the field the model reads.
      return {
        title: "WitDiff verification",
        content: render(
          parseReceipt(result.stdout),
          result.code,
          result.stdout,
          result.stderr,
        ),
        metadata: {
          exitCode: result.code,
          status: parseReceipt(result.stdout)?.status ?? "unknown",
        },
      }
    },
  }
}

/**
 * v1 shape: a named export of an async function returning `{ tool: { ... } }`,
 * with the CLI run through Bun's `` $``. Kept so a v1 host still loads the
 * integration; v2 ignores it and uses the default export below.
 *
 * The parameter is typed loosely on purpose: a v1 host supplies BunShell and
 * the v1 `tool()` helper, neither of which exists in the v2 types this file is
 * checked against. Without the cast this file would not typecheck on v2, and a
 * v1-only typecheck would not see the v2 default export.
 */
export const WitDiffPlugin = async ({ $ }: any) => {
  const run = async (command: string, root: string) => {
    // Passed as a single quoted command line because Bun's `$` joins an array
    // interpolation into one word: `` $`witdiff ${args}` `` would run
    // `witdiff "verify,--json,..."` and fail with no output, which reads as a
    // tool error rather than as a bug in this plugin.
    const result = await $`${command}`.cwd(root).quiet().nothrow()
    return {
      code: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    }
  }
  return {
    tool: {
      witdiff_verify: {
        description:
          "Verify that the current change is proven by its tests. Runs WitDiff's " +
          "red/green experiment and returns the receipt. Call this before claiming " +
          "a change is complete, verified, or fixed. An exit code of 2 means the " +
          "gate failed and must not be reported as success.",
        args: {
          base: {
            type: "string",
            description: BASE_DESCRIPTION,
            required: false,
          },
        },
        async execute(args: { base?: string }) {
          const ref = args.base ?? process.env.WITDIFF_BASE ?? "origin/main"
          const refusal = checkRef(ref)
          if (refusal) throw new Error(refusal)
          const result = await run(
            `witdiff 'verify,--base,${ref},--json,--no-write'`,
            ".",
          )
          return render(
            parseReceipt(result.stdout),
            result.code,
            result.stdout,
            result.stderr,
          )
        },
      },
    },
  }
}

/**
 * v2 shape: a default export of `{ id, setup }`.
 *
 * The tool is registered through `context.tool.transform(editor => editor.add(…))`:
 * `ToolDomain` exposes only `transform`, `reload` and `hook`, so a plugin adds
 * to the live tool set through the editor that `transform` hands it. Calling
 * `context.tool.add` — the shape the v1 documentation implied — does not exist
 * and typechecks to nothing.
 *
 * The working directory is the context's `location`: v2 removed `worktree` from
 * the plugin input, so the tool runs where the session is rooted.
 */
export default define({
  id: "witdiff",
  setup(context) {
    const tool = witdiffVerify(context.location.directory)
    context.tool.transform((editor) => {
      editor.add(tool)
    })
  },
})
