//! Flake detection: a run that does not give the same answer twice must not
//! become a proof, and must not become a false failure either.
//!
//! The red/green experiment runs each side once. That is fine for a
//! deterministic suite and wrong for a flaky one: three single samples become
//! `verified` when the base run happened to fail, or `head_failed` when the
//! workspace run happened to fail, with nothing in the receipt saying the
//! evidence was one sample of an unstable distribution.
//!
//! Rather than rerunning everything — which doubles the cost of the common case
//! to buy certainty about the rare one — a run that **fails** is repeated only
//! when the caller asks, and disagreement between repeats is reported as an
//! unstable suite rather than as evidence.

use std::path::Path;

use witdiff_core::run::{RunOutcome, Stability};

/// A command whose outcome alternates, so a single run is never trustworthy.
const ALTERNATING: &str = r#"
n=$(cat counter 2>/dev/null || echo 0)
n=$((n + 1))
echo $n > counter
if [ $((n % 2)) -eq 1 ]; then
  echo "test result: FAILED. 0 passed; 1 failed"
  exit 101
fi
echo "test result: ok. 1 passed; 0 failed"
exit 0
"#;

/// A command that always fails.
const ALWAYS_FAILS: &str = r#"
echo "test result: FAILED. 0 passed; 1 failed"
exit 101
"#;

/// A command that always passes.
const ALWAYS_PASSES: &str = r#"
echo "test result: ok. 1 passed; 0 failed"
exit 0
"#;

fn run_in(dir: &Path, script: &str, repeats: usize) -> RunOutcome {
    witdiff_core::run::run_with_repeats(
        &["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        dir,
        8192,
        std::time::Duration::from_secs(30),
        repeats,
    )
    .expect("the command should run")
}

#[test]
fn a_single_run_of_an_alternating_command_is_reported_as_one_sample() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALTERNATING, 1);

    assert_eq!(
        outcome.stability,
        Stability::SingleRun,
        "with no repeats there is nothing to compare, and the receipt must say \
         so rather than implying the result was confirmed"
    );
    assert_eq!(outcome.repeats, 1);
}

/// The property the whole feature exists for: a command that gives two
/// different answers is not evidence for either answer.
#[test]
fn an_alternating_command_is_reported_as_unstable() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALTERNATING, 3);

    assert_eq!(
        outcome.stability,
        Stability::Unstable,
        "a command that alternates between pass and fail must be reported as \
         unstable, not accepted as whichever answer came first"
    );
    assert!(
        outcome.observed_successes > 0 && outcome.observed_successes < outcome.repeats,
        "both outcomes must have been observed, got {} successes in {} runs",
        outcome.observed_successes,
        outcome.repeats
    );
    assert!(
        !outcome.result.success,
        "an unstable run is never a success; treating one as green would let a \
         lucky pass stand as evidence"
    );
}

/// A failure repeated and agreeing every time is `Stable` — strictly more
/// evidence than one sample, and the receipt says which it got.
#[test]
fn a_consistently_failing_command_is_stable() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALWAYS_FAILS, 3);

    assert_eq!(outcome.stability, Stability::Stable);
    assert_eq!(outcome.observed_successes, 0);
    assert!(!outcome.result.success);
}

/// A pass is not repeated, so it is `SingleRun` rather than `Stable`.
///
/// This is deliberate and the distinction matters: "we ran it three times and
/// it passed every time" and "we ran it once and it passed" are different
/// amounts of evidence, and labelling the second `Stable` would overstate it.
/// Repeats exist to separate a real failure from a flake, and a pass has
/// nothing to separate — repeating it would spend the suite's cost again to
/// learn that it still passes.
#[test]
fn a_passing_command_is_not_repeated_and_says_so() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALWAYS_PASSES, 3);

    assert_eq!(
        outcome.stability,
        Stability::SingleRun,
        "a pass stops after one run, and the receipt must not claim it was \
         confirmed by repeats that never happened"
    );
    assert_eq!(outcome.repeats, 1, "a passing run is not repeated");
    assert!(outcome.result.success);
    assert!(outcome.stability.is_evidence());
}

/// A passing run must not be repeated further than asked. Reruns exist to
/// distinguish a real failure from a flake, and a pass has nothing to
/// distinguish — repeating it would spend the whole suite's cost again to learn
/// nothing.
#[test]
fn the_retained_result_is_a_failure_when_any_run_failed() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALTERNATING, 2);

    assert_eq!(
        outcome.stability,
        Stability::Unstable,
        "first run fails, second passes"
    );
    assert!(
        !outcome.result.success,
        "the retained result must be the failure, so an unstable suite cannot \
         be mistaken for a green one"
    );
}

/// Each repeat must be a fresh observation, not a cached one, or the whole
/// mechanism reports the same sample repeatedly and finds nothing.
#[test]
fn repeats_are_independent_observations() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let outcome = run_in(dir.path(), ALTERNATING, 4);

    assert_eq!(outcome.repeats, 4);
    assert_eq!(
        outcome.observed_successes, 2,
        "four runs of an alternating command produce exactly two of each; a \
         cached or reused run would not"
    );
}
