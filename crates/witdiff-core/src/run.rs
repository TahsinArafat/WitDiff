//! Repeating a run, so that a flaky suite cannot become a proof.
//!
//! The red/green experiment takes one sample per side. For a deterministic
//! suite that is enough. For a flaky one it is not: three single samples become
//! `verified` when the base run happened to fail, or `head_failed` when the
//! workspace run happened to fail, and nothing in the receipt says the evidence
//! was one draw from an unstable distribution.
//!
//! ## Why failure-only repeats
//!
//! Rerunning everything would double the cost of the ordinary case to buy
//! certainty about the rare one. A **passing** run has nothing to distinguish:
//! repeating it can only learn that it passed again. A **failing** run is the
//! one where the two explanations — a real regression and a flaky test — are
//! both live, so that is where a repeat is worth its cost.
//!
//! So: repeats happen when they were asked for, and the retained result is a
//! failure if any repeat failed. That direction is deliberate. An unstable suite
//! must never be readable as green, because a lucky pass standing as evidence is
//! the exact failure this project exists to prevent. A false red is the safe
//! direction — it is visible, and the receipt says why.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::framework::TestFramework;
use crate::model::RunResult;
use crate::runner::Sandbox;

/// How consistent a command's outcome was across repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    /// Run once. Nothing was compared, so nothing is claimed about consistency.
    ///
    /// Distinct from `Stable` on purpose: "we ran it once and it failed" and "we
    /// ran it three times and it failed every time" are different amounts of
    /// evidence, and a receipt that conflated them would overstate the weaker.
    SingleRun,
    /// Every repeat agreed.
    Stable,
    /// The repeats disagreed. The suite is flaky, so its result is not evidence
    /// for either outcome.
    Unstable,
}

impl Stability {
    pub fn as_str(self) -> &'static str {
        match self {
            Stability::SingleRun => "single_run",
            Stability::Stable => "stable",
            Stability::Unstable => "unstable",
        }
    }

    /// Whether a result measured this way can be used as evidence.
    ///
    /// Only `SingleRun` and `Stable` can. `Unstable` cannot, in either
    /// direction: a suite that gives two answers has not demonstrated anything
    /// about the code.
    pub fn is_evidence(self) -> bool {
        !matches!(self, Stability::Unstable)
    }
}

/// The outcome of running a command, possibly more than once.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// The result to report. A failure if any repeat failed.
    pub result: RunResult,
    /// How many times the command actually ran.
    pub repeats: usize,
    /// How many of those runs succeeded.
    pub observed_successes: usize,
    /// Whether the runs agreed.
    pub stability: Stability,
}

/// Run a command, repeating it up to `repeats` times.
///
/// `repeats` of 1 is the default and behaves exactly as a single run did before
/// this module existed. A value above 1 repeats **only while the run fails**:
/// the moment a repeat passes, the command is known to be unstable and further
/// runs would add nothing.
pub fn run_with_repeats(
    command: &[String],
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Duration,
    repeats: usize,
) -> Result<RunOutcome> {
    run_with_repeats_inner(
        command,
        cwd,
        max_output_bytes,
        Some(timeout),
        repeats,
        TestFramework::Cargo,
        None,
    )
}

/// The full form, used by the verifier so the framework and sandbox match the
/// surrounding runs.
#[allow(clippy::too_many_arguments)]
pub fn run_repeating(
    spec: &crate::runner::CommandSpec,
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Option<Duration>,
    framework: TestFramework,
    sandbox: Option<&Sandbox>,
    repeats: usize,
) -> Result<RunOutcome> {
    run_impl(
        spec,
        cwd,
        max_output_bytes,
        timeout,
        framework,
        sandbox,
        repeats,
    )
}

fn run_with_repeats_inner(
    command: &[String],
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Option<Duration>,
    repeats: usize,
    framework: TestFramework,
    sandbox: Option<&Sandbox>,
) -> Result<RunOutcome> {
    let spec = crate::runner::CommandSpec::from_vec(command.to_vec())?;
    run_impl(
        &spec,
        cwd,
        max_output_bytes,
        timeout,
        framework,
        sandbox,
        repeats,
    )
}

fn run_impl(
    spec: &crate::runner::CommandSpec,
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Option<Duration>,
    framework: TestFramework,
    sandbox: Option<&Sandbox>,
    repeats: usize,
) -> Result<RunOutcome> {
    let wanted = repeats.max(1);

    let mut result = crate::runner::run(spec, cwd, max_output_bytes, timeout, framework, sandbox)?;
    let mut repeat_count = 1usize;
    let mut successes = usize::from(result.success);

    // Already green: nothing to distinguish, so stop. Repeating a pass spends
    // the suite's cost again to learn that it still passes.
    //
    // A timeout is not repeated either. A suite that does not terminate has
    // already decided nothing, and running it again to confirm a hang would
    // multiply the deadline rather than sharpen the evidence.
    while !result.success && !result.timed_out && repeat_count < wanted {
        let next = crate::runner::run(spec, cwd, max_output_bytes, timeout, framework, sandbox)?;
        repeat_count += 1;
        if next.success {
            successes += 1;
            // Unstable: the first result is kept, so a lucky pass cannot stand
            // as evidence. `next` is deliberately not what is returned.
        }
    }

    let stability = if repeat_count == 1 {
        Stability::SingleRun
    } else if successes == 0 {
        Stability::Stable
    } else if successes == repeat_count {
        // Only reachable if a caller asks to repeat a passing run, which the
        // loop above does not do — kept so the classification is total.
        Stability::Stable
    } else {
        Stability::Unstable
    };

    if stability == Stability::Unstable {
        // A consumer must not read a passed run out of this result, so it is
        // forced to a failure and the classification is left to say why.
        result.success = false;
    }

    // Carried onto the result itself, because the receipt serializes the
    // `RunResult` rather than the outcome. Computing it only here left the
    // receipt claiming `single_run` for a failure that had been confirmed by
    // three agreeing runs — the field existed and never held anything.
    result.stability = stability;
    result.repeats = repeat_count;

    Ok(RunOutcome {
        result,
        repeats: repeat_count,
        observed_successes: successes,
        stability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_consistent_result_is_evidence() {
        assert!(Stability::SingleRun.is_evidence());
        assert!(Stability::Stable.is_evidence());
        assert!(
            !Stability::Unstable.is_evidence(),
            "a suite that gives two answers has demonstrated nothing about the code"
        );
    }

    #[test]
    fn stability_names_are_stable_for_consumers() {
        assert_eq!(Stability::SingleRun.as_str(), "single_run");
        assert_eq!(Stability::Stable.as_str(), "stable");
        assert_eq!(Stability::Unstable.as_str(), "unstable");
    }
}
