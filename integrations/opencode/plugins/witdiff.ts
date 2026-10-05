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
 * The tool returns the receipt rather than a verdict summary. The agent's job
 * is to report what the tool said, including what it could not determine — so
 * the gate decision and the reasons travel together.
 */
import { tool, type Plugin } from "@opencode-ai/plugin"

const BASE_DESCRIPTION =
  "Git ref to verify against, normally the pull request base branch. " +
  "Defaults to origin/main."

export const WitDiffPlugin: Plugin = async ({ $, directory, worktree }) => {
  const root = worktree || directory

  /**
   * Run the CLI and capture its output plus exit code.
   *
   * Exit code 2 is a failed gate and 1 is a tool error, so neither is an
   * exception here: both are results the caller must report.
   */
  /**
   * Run the CLI and capture its output plus exit code.
   *
   * `args` is a fixed vector built by the caller, never a model-supplied
   * string. The base ref is the one value that comes from outside, so it is
   * validated rather than interpolated blindly: a ref beginning with `-` would
   * be read as a flag, and a shell metacharacter would change the command.
   */
  const run = async (args: string[]) => {
    // Passed as a single quoted command line because Bun's `$` joins an array
    // interpolation into one word: `` $`witdiff ${args}` `` would run
    // `witdiff "verify,--json,..."` and fail with no output, which reads as a
    // tool error rather than as a bug in this plugin.
    const command = ["witdiff", ...args].join(" ")
    const result = await $`${command}`.cwd(root).quiet().nothrow()
    return {
      code: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    }
  }

  /** A git ref safe to pass as a positional argument. */
  const safeRef = (ref: string) => {
    if (ref.startsWith("-") || !/^[\w./@^~{}-]+$/.test(ref)) {
      throw new Error(
        `refusing to verify against ${JSON.stringify(ref)}: a git ref may contain ` +
          "only word characters, dots, slashes, @, ^, ~ and braces",
      )
    }
    return ref
  }

  const parseReceipt = (stdout: string) => {
    // The receipt is the last JSON object printed. `--json` prints it alone,
    // but a wrapper or a future flag could add a line before it.
    const start = stdout.indexOf("{")
    if (start === -1) return null
    try {
      return JSON.parse(stdout.slice(start))
    } catch {
      return null
    }
  }

  return {
    tool: {
      witdiff_verify: tool({
        description:
          "Verify that the current change is proven by its tests. Runs WitDiff's " +
          "red/green experiment and returns the receipt. Call this before claiming " +
          "a change is complete, verified, or fixed. An exit code of 2 means the " +
          "gate failed and must not be reported as success.",
        args: {
          base: tool.schema
            .string()
            .optional()
            .describe(BASE_DESCRIPTION),
          strict: tool.schema
            .boolean()
            .optional()
            .describe(
              "Require exactly `verified`, refusing `verified_with_warnings`.",
            ),
        },
        async execute(args) {
          const flags = ["verify", "--json"]
          flags.push("--base", safeRef(args.base || "origin/main"))
          if (args.strict) flags.push("--strict")

          const { code, stdout, stderr } = await run(flags)
          const receipt = parseReceipt(stdout)

          if (code === 1) {
            return [
              "# WitDiff could not run",
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
            code === 2
              ? "**The gate failed. Do not report this change as complete.**"
              : "The gate passed.",
            "",
            `- exit code: ${code}`,
            `- status: \`${status}\``,
          ]

          if (receipt) {
            lines.push(
              `- red/green proven: ${receipt.red_green_proven}`,
              `- evidence fresh: ${receipt.evidence_fresh}`,
              `- changed tests: ${(receipt.changed_test_files || []).join(", ") || "none"}`,
            )
            if (receipt.verification_digest) {
              lines.push(`- digest: ${receipt.verification_digest.slice(0, 16)}…`)
            }
            const findings = receipt.integrity_findings || []
            if (findings.length > 0) {
              lines.push("", "## Integrity findings", "")
              for (const finding of findings) {
                lines.push(
                  `- **${finding.severity}** \`${finding.rule}\` ${finding.path}: ${finding.message}`,
                )
              }
            }
            if ((receipt.notes || []).length > 0) {
              lines.push("", "## Notes", "")
              for (const note of receipt.notes) lines.push(`- ${note}`)
            }
            lines.push(
              "",
              "Report the status and every finding above. Do not summarize them as",
              '"tests pass". If a note says a case was unsupported, say so — an',
              "unsupported case is not a pass.",
            )
          } else {
            lines.push("", "The receipt could not be parsed:", "```", stdout.trim(), "```")
          }

          return lines.join("\n")
        },
      }),
    },
  }
}
