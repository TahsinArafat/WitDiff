# ADR-0024: Update notification, and why it is not automatic

Status: accepted

## Context

Users have no way to learn that a newer WitDiff exists. The `install.sh` script
resolves and installs the latest release, and `--force` re-installs over an
existing one, but nothing tells an installed copy that a newer release is
available. The request was for an auto-update that runs with the tool.

That request collides with two shipped invariants:

- `AGENTS.md` #4: "Core verification must remain usable without network access."
- `README.md` principle #4: "Core verification must work offline."

and with a third property that is not written as an invariant but follows from
what a receipt is: **a signed receipt must describe a tool that still exists.**
A check that ran inside `verify` could, in principle, replace the binary
mid-run, leaving a receipt that refers to a version no longer installed.

## Decision

WitDiff notifies about updates. It does not install them, and it does not check
during verification.

- The check runs on `doctor`, which is already an informational command, and
  never on `verify`, `inspect` or `receipt` — none of which may touch the
  network.
- It **prints a line at most**. It does not download, does not replace the
  binary, and does not prompt. `install.sh --force` is the documented way to
  take the update, and the notice names it.
- Every failure is silent. No network, no `curl`, a timeout, malformed JSON,
  an unparseable tag: each returns "unavailable" and prints nothing.
- Answers are cached for 24 hours under `$XDG_CACHE_HOME/witdiff/update-check`,
  never inside a repository, because a file written into the working tree
  changes the workspace fingerprint and would invalidate the run's own evidence.
- Two explicit opt-outs: `WITDIFF_NO_UPDATE_CHECK=1` in the environment, or
  `check = false` under `[updates]` in `witdiff.toml`. The environment variable
  wins, because CI sets the environment and the repository may be someone
  else's.
- The **channel is inherited**: a pre-release install is offered pre-releases, a
  stable install is offered stable releases. Moving a user off the channel they
  chose is not a courtesy.

## Consequences

**The offline guarantee is unchanged.** Measured: `verify` completes with no
`curl` on `PATH`, and makes no network call.

**Version comparison is not string comparison.** `1.0.0-alpha.10` must sort
after `1.0.0-alpha.9`, or update offers would stop at the ninth alpha. The key
is `(major, minor, patch, channel_rank, prerelease_number)` where channel rank
is 0 for a pre-release and 1 for a release, so a full release outranks its own
alpha.

**The first implementation of that key was backwards.** It ranked a release 0
and a pre-release 1, which made `1.0.0` sort *older* than `1.0.0-alpha.3`. The
direction matters: the reverse would have told every alpha user they were
already up to date on the day `v1.0.0` shipped. Two tests failed and were fixed
before the commit, which is the only reason it did not ship.

**An unparseable version is not compared at all.** Treating it as `0.0.0` would
make `doctor` announce an update to anyone whose version contained a typo.

**No new dependency.** The check shells out to `curl`, which `install.sh`
already requires. Adding a TLS stack to the binary for a courtesy notice would
enlarge the dependency surface of a tool whose value is that it has almost none
(16 workspace dependencies, none an HTTP client).

**A true self-updating binary remains refused.** It would make verification
depend on network reachability, break the air-gapped case the project promises,
and make a receipt-producing run non-reproducible. If that is ever wanted it
needs a different ADR that explicitly supersedes the offline invariant, and the
cost should be stated plainly rather than discovered.
