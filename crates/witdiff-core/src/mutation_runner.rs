//! Orchestration for changed-code mutation.
//!
//! See ADR-0011. This module decides *which* mutants to attempt, runs the suite
//! against each, and classifies the outcome. It produces supplementary evidence
//! only, and the caller never consults it when computing a status.
//!
//! ## Where mutants run
//!
//! A mutant is applied to the *head* workspace copy, not the developer's
//! working tree. Mutating in place and restoring afterwards would leave the
//! developer's source briefly modified, and a crash mid-run would leave it
//! permanently modified. Instead the head revision is materialized into a
//! temporary worktree, mutants are spliced into it one at a time, and the
//! original is restored from a saved copy between runs.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};

use crate::{
    config::Config,
    git::{GitRepo, WorktreeGuard},
    model::{FailureKind, MutantOutcome, MutantResult, MutationReport},
    mutation::{mutants_for_file, test_regions, Mutant},
    runner::{run, CommandSpec},
};

/// Run mutation analysis over the changed production code.
///
/// Returns `None` when there is nothing to mutate, so the receipt omits the
/// section rather than carrying an empty one.
pub fn analyze_mutations(
    repo: &GitRepo,
    config: &Config,
    command: &CommandSpec,
    base: &str,
    changed_production_paths: &[String],
    timeout: Option<Duration>,
) -> Result<Option<MutationReport>> {
    let mut report = MutationReport::default();

    // Generate every candidate first, so the bounds apply to a deterministic
    // population rather than to whatever order the files happen to be in.
    let mut candidates: Vec<Mutant> = Vec::new();
    for path in changed_production_paths {
        let Some(source) = repo.worktree_source(path)? else {
            continue;
        };
        // Only the lines this change touches are mutated, so cost tracks the
        // change rather than the file.
        let changed_lines = match repo.changed_head_lines(base, path) {
            Ok(lines) if !lines.is_empty() => lines,
            // A path whose diff cannot be attributed (a new file, or a diff we
            // cannot parse) is mutated across the whole file rather than
            // skipped, and the report says so.
            _ => {
                report.notes.push(format!(
                    "{path}: changed lines could not be determined, so the whole file was considered"
                ));
                (1..=source.lines().count().max(1)).collect()
            }
        };
        let regions = test_regions(&source);
        candidates.extend(mutants_for_file(path, &source, &changed_lines, &regions));
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    // Apply the per-function bound, then the global one. Both are deterministic:
    // candidates are already ordered by path and offset.
    let mut per_function: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut bounded: Vec<Mutant> = Vec::new();
    for mutant in candidates {
        let key = (
            mutant.path.clone(),
            mutant.function.clone().unwrap_or_default(),
        );
        let count = per_function.entry(key).or_default();
        if *count >= config.verification.max_mutants_per_function {
            continue;
        }
        *count += 1;
        bounded.push(mutant);
    }

    let generated = bounded.len();
    let truncated = generated > config.verification.max_mutants;
    let attempted: Vec<Mutant> = bounded
        .into_iter()
        .take(config.verification.max_mutants)
        .collect();

    report.generated = generated;
    if truncated {
        report.notes.push(format!(
            "mutation was bounded to {} of {generated} generated mutants; the remainder were not attempted",
            config.verification.max_mutants
        ));
    }
    report.skipped = generated.saturating_sub(attempted.len());

    // Materialize the head revision into a temporary worktree. Mutating the
    // developer's own files, even with restoration, risks leaving them changed
    // if this process dies mid-run.
    let tmp = tempfile::TempDir::new().context("failed to allocate mutation directory")?;
    let worktree = tmp.path().join("head");
    let mut guard = WorktreeGuard::create(repo, &worktree, "HEAD")?;

    // Overlay uncommitted head content: the change under verification is
    // normally not committed yet.
    let mut originals: Vec<(String, String)> = Vec::new();
    for path in changed_production_paths {
        let Some(source) = repo.worktree_source(path)? else {
            continue;
        };
        repo.write_into_worktree(&worktree, path, &source)?;
        originals.push((path.clone(), source));
    }

    let tests_fingerprint = repo.workspace_fingerprint(base).unwrap_or_default();
    let mut cache = MutationCache::load(repo);

    let result = run_mutants(
        repo,
        config,
        command,
        &worktree,
        &originals,
        &attempted,
        timeout,
        &tests_fingerprint,
        &mut cache,
        &mut report,
    );

    // Surface a cleanup failure only when the analysis itself succeeded.
    match result {
        Ok(()) => {
            guard.remove()?;
            // A cache write failure must not fail the analysis: the evidence is
            // already collected, and the cache is an optimization.
            if let Err(error) = cache.save(repo) {
                report
                    .notes
                    .push(format!("mutation cache could not be written: {error}"));
            }
            Ok(Some(report))
        }
        Err(error) => Err(error),
    }
}

/// Outcomes remembered across runs, keyed by the mutant and its inputs.
///
/// Without this, every verification re-runs the whole suite once per mutant,
/// which makes the feature too slow to leave enabled (ADR-0011).
#[derive(Debug, Default)]
struct MutationCache {
    entries: BTreeMap<String, MutantOutcome>,
    dirty: bool,
}

impl MutationCache {
    fn load(repo: &GitRepo) -> Self {
        let path = cache_path(repo);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<BTreeMap<String, MutantOutcome>>(&text) {
            Ok(entries) => Self {
                entries,
                dirty: false,
            },
            // A corrupt cache is discarded rather than trusted. It is an
            // optimization, not evidence, so losing it costs time and nothing
            // else.
            Err(_) => Self::default(),
        }
    }

    fn get(&self, key: &str) -> Option<MutantOutcome> {
        self.entries.get(key).copied()
    }

    fn insert(&mut self, key: String, outcome: MutantOutcome) {
        if self.entries.insert(key, outcome).is_none() {
            self.dirty = true;
        }
    }

    fn save(&mut self, repo: &GitRepo) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let path = cache_path(repo);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }
        let text = serde_json::to_string(&self.entries)
            .context("failed serializing the mutation cache")?;
        std::fs::write(&path, text)
            .with_context(|| format!("failed writing {}", path.display()))?;
        self.dirty = false;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn run_mutants(
    repo: &GitRepo,
    config: &Config,
    command: &CommandSpec,
    worktree: &Path,
    originals: &[(String, String)],
    attempted: &[Mutant],
    timeout: Option<Duration>,
    tests_fingerprint: &str,
    cache: &mut MutationCache,
    report: &mut MutationReport,
) -> Result<()> {
    let sources: BTreeMap<&str, &str> = originals
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();

    for mutant in attempted {
        let key = cache_key(mutant, tests_fingerprint, command);
        if let Some(outcome) = cache.get(&key) {
            report.count(outcome);
            report.results.push(result_for(mutant, outcome, true));
            continue;
        }

        let Some(original) = sources.get(mutant.path.as_str()) else {
            report.notes.push(format!(
                "{}: source became unavailable before mutant {} ran",
                mutant.path, mutant.id
            ));
            report.count(MutantOutcome::Skipped);
            report
                .results
                .push(result_for(mutant, MutantOutcome::Skipped, false));
            continue;
        };

        let Some(mutated) = mutant.apply(original) else {
            // The span no longer matches, which means the source changed under
            // us. Refusing is the only safe response: applying a shifted edit
            // would test something other than what was recorded.
            report.notes.push(format!(
                "{}: mutant {} could not be applied because the source changed",
                mutant.path, mutant.id
            ));
            report.count(MutantOutcome::Skipped);
            report
                .results
                .push(result_for(mutant, MutantOutcome::Skipped, false));
            continue;
        };

        repo.write_into_worktree(worktree, &mutant.path, &mutated)?;
        let outcome = run(
            command,
            worktree,
            config.verification.max_output_bytes,
            timeout,
        );
        // Restore before classifying, so a failure below cannot leave the
        // mutant in place for the next iteration.
        let restore: Result<()> = repo.write_into_worktree(worktree, &mutant.path, original);
        restore?;

        let classified = match outcome {
            Ok(result) => classify(&result),
            Err(error) => {
                report.notes.push(format!(
                    "{}: mutant {} could not be run: {error}",
                    mutant.path, mutant.id
                ));
                MutantOutcome::Skipped
            }
        };
        report.count(classified);
        // Only decided outcomes are cached. A timeout or a spawn failure is a
        // fact about this machine or this moment, not about the mutant, and
        // remembering it would make a transient failure permanent.
        if matches!(
            classified,
            MutantOutcome::Killed | MutantOutcome::Survived | MutantOutcome::NotCompiled
        ) {
            cache.insert(key, classified);
        }
        report.results.push(result_for(mutant, classified, false));
    }

    Ok(())
}

/// Classify a run against a mutant.
///
/// Only a recognized test failure counts as a kill. A mutant that failed to
/// build was never executed and says nothing about test strength, and a
/// timed-out run decided nothing at all (ADR-0011).
fn classify(result: &crate::model::RunResult) -> MutantOutcome {
    if result.timed_out || result.failure_kind == Some(FailureKind::Timeout) {
        return MutantOutcome::Timeout;
    }
    if result.success {
        return MutantOutcome::Survived;
    }
    match result.failure_kind {
        Some(FailureKind::TestFailure) => MutantOutcome::Killed,
        Some(FailureKind::CompileError) => MutantOutcome::NotCompiled,
        // A spawn failure or an unrecognized command failure is not evidence
        // about the tests, so it is not counted as a decision.
        _ => MutantOutcome::Skipped,
    }
}

fn result_for(mutant: &Mutant, outcome: MutantOutcome, cached: bool) -> MutantResult {
    MutantResult {
        id: mutant.id.clone(),
        path: mutant.path.clone(),
        operator: mutant.operator.clone(),
        line: mutant.line,
        function: mutant.function.clone(),
        original: mutant.original.clone(),
        replacement: mutant.replacement.clone(),
        outcome,
        cached,
    }
}

/// The cache key for a mutant outcome.
///
/// Determined by the mutated source, the test code and the command: those three
/// fingerprints fully determine the outcome, so a re-run over an unchanged
/// revision pair does not need to repeat the suite (ADR-0011).
pub fn cache_key(mutant: &Mutant, tests_fingerprint: &str, command: &CommandSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(mutant.id.as_bytes());
    hasher.update(mutant.replacement.as_bytes());
    hasher.update(tests_fingerprint.as_bytes());
    for argument in command.as_vec() {
        hasher.update(argument.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

/// Path to the on-disk mutation cache inside a repository.
pub fn cache_path(repo: &GitRepo) -> PathBuf {
    repo.root().join(".witdiff").join("mutation-cache.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RunResult;

    fn result(success: bool, failure: Option<FailureKind>, timed_out: bool) -> RunResult {
        RunResult {
            command: vec!["cargo".into(), "test".into()],
            cwd: "/tmp".into(),
            success,
            exit_code: Some(if success { 0 } else { 101 }),
            duration_ms: 1,
            stdout: String::new(),
            stderr: String::new(),
            failure_kind: failure,
            timed_out,
        }
    }

    #[test]
    fn a_passing_suite_means_the_mutant_survived() {
        assert_eq!(
            classify(&result(true, None, false)),
            MutantOutcome::Survived
        );
    }

    #[test]
    fn a_test_assertion_failure_is_a_kill() {
        assert_eq!(
            classify(&result(false, Some(FailureKind::TestFailure), false)),
            MutantOutcome::Killed
        );
    }

    /// A mutant that never built was never executed, so it is not a kill.
    #[test]
    fn a_compile_error_is_not_a_kill() {
        assert_eq!(
            classify(&result(false, Some(FailureKind::CompileError), false)),
            MutantOutcome::NotCompiled
        );
    }

    #[test]
    fn a_timeout_decides_nothing() {
        assert_eq!(
            classify(&result(false, Some(FailureKind::Timeout), true)),
            MutantOutcome::Timeout
        );
    }

    #[test]
    fn an_unrecognized_failure_is_not_counted_as_a_decision() {
        assert_eq!(
            classify(&result(false, Some(FailureKind::SpawnFailure), false)),
            MutantOutcome::Skipped
        );
    }

    #[test]
    fn decided_excludes_undecided_outcomes() {
        let mut report = MutationReport::default();
        report.count(MutantOutcome::Killed);
        report.count(MutantOutcome::Survived);
        report.count(MutantOutcome::NotCompiled);
        report.count(MutantOutcome::Timeout);
        report.count(MutantOutcome::Skipped);
        assert_eq!(
            report.decided(),
            2,
            "only kills and survivors are decisions"
        );
        assert_eq!(report.not_compiled, 1);
    }
}
