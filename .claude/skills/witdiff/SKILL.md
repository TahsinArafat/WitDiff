# WitDiff verification skill

Use WitDiff as an independent completion gate after making code changes.

1. Determine the repository's target/base branch.
2. Run `witdiff inspect --base <base>` if test classification is unclear.
3. Run `witdiff verify --base <base> --strict --json` before claiming the task is verified.
4. Treat exit code 2 as a verification failure, not as a WitDiff crash.
5. Do not hide or reinterpret `not_verified`, `head_failed`, `base_incompatible`, stale evidence, or integrity findings.
6. If WitDiff reports an unsupported case such as inline unit tests in v0.1, state that limitation explicitly and report what deterministic checks were actually run.
