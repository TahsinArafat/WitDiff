use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;

use crate::{
    config::Config,
    git::{GitRepo, WorktreeGuard},
    integrity::analyze_test_diff,
    model::{
        ChangeKind, ChangedFile, FailureKind, InspectReport, IntegrityFinding, Receipt,
        RefusedInlineTests, RunResult, Severity, SplicedInlineTests, TestSelection,
        VerificationStatus,
    },
    runner::{run, CommandSpec},
    rustanalysis::analyze_rust_test_change,
    selection::{build_targeted_command, SelectionOutcome},
};

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    pub requested_base: Option<String>,
    pub command: Option<CommandSpec>,
    pub keep_worktree: bool,
}

pub fn inspect_repository(
    repo: &GitRepo,
    config: &Config,
    requested_base: Option<&str>,
) -> Result<InspectReport> {
    let base = repo.choose_base(requested_base.or(config.verification.base.as_deref()))?;
    let matcher = config.test_matcher()?;
    let changed_files = repo.changed_files(&base, &matcher)?;
    let changed_test_files = changed_files
        .iter()
        .filter(|f| f.is_test && !matches!(f.kind, ChangeKind::Deleted))
        .map(|f| f.path.clone())
        .collect();
    let changed_production_files = changed_files
        .iter()
        .filter(|f| !f.is_test)
        .map(|f| f.path.clone())
        .collect();
    let inline_test_hints = repo.inline_test_hints(&base, &changed_files)?;

    Ok(InspectReport {
        repo_root: repo.root().display().to_string(),
        base,
        head_commit: repo.head_commit()?,
        workspace_dirty: repo.is_dirty()?,
        changed_files,
        changed_test_files,
        changed_production_files,
        inline_test_hints,
    })
}

pub fn verify_repository(
    repo: &GitRepo,
    config: &Config,
    options: VerifyOptions,
) -> Result<Receipt> {
    let inspect = inspect_repository(repo, config, options.requested_base.as_deref())?;
    let base = inspect.base.clone();
    let configured = match options.command {
        Some(command) => command,
        None => CommandSpec::from_vec(config.verification.test_command.clone())?,
    };

    // Decide once, before any evidence is collected, so the head, control and
    // base runs all use the same command. Narrowing after the head run would
    // make the two sides of the experiment incomparable.
    let mut selection_notes = Vec::new();
    let (command, test_selection) = if config.verification.targeted_test_selection {
        let (narrowed, outcome) = build_targeted_command(&configured, &inspect.changed_test_files);
        match outcome {
            SelectionOutcome::Targeted(targets) => {
                selection_notes.push(format!(
                    "test selection: ran only the changed dedicated test targets ({})",
                    targets.join(", ")
                ));
                (narrowed, TestSelection::Targeted)
            }
            SelectionOutcome::NoTargets => {
                selection_notes.push(
                    "targeted test selection was requested but the changed tests do not map to individually selectable cargo targets; the full suite was run instead"
                        .to_owned(),
                );
                (configured, TestSelection::FullSuite)
            }
            SelectionOutcome::NotNarrowable(reason) => {
                selection_notes.push(format!(
                    "targeted test selection was requested but is not safe here because {reason}; the full suite was run instead"
                ));
                (configured, TestSelection::FullSuite)
            }
        }
    } else {
        (configured, TestSelection::FullSuite)
    };
    let effective_test_command = Some(command.as_vec());

    let before = repo.workspace_fingerprint(&base)?;
    let integrity_findings = collect_integrity(repo, &base, &inspect)?;

    let timeout = config.verification.timeout_secs.map(Duration::from_secs);
    let head_run = run(
        &command,
        repo.root(),
        config.verification.max_output_bytes,
        timeout,
    )?;

    let mut splice_notes: Vec<String> = Vec::new();
    let mut spliced_inline_tests: Vec<SplicedInlineTests> = Vec::new();
    let mut refused_inline_tests: Vec<RefusedInlineTests> = Vec::new();

    let mut notes = selection_notes;
    notes.append(&mut splice_notes);
    if head_run.timed_out {
        notes.push(format!(
            "the test command exceeded the configured timeout of {} seconds and was terminated; this does not establish that the change is correct",
            config.verification.timeout_secs.unwrap_or_default()
        ));
    }
    let mut base_control_run = None;
    let mut base_run = None;
    let mut red_green_proven = false;
    let mut status = if !head_run.success {
        VerificationStatus::HeadFailed
    } else if inspect.changed_test_files.is_empty() && inspect.inline_test_hints.is_empty() {
        // With no dedicated test *and* no inline-test candidate there is
        // nothing to transplant, so no experiment can be run.
        VerificationStatus::NoChangedTests
    } else {
        let tmp = TempDir::new().context("failed to allocate temporary verification directory")?;
        let worktree = tmp.path().join("base");
        // RAII: the worktree is removed on every exit path, including an early
        // `?` from patch application or test spawning.
        let mut guard = WorktreeGuard::create(repo, &worktree, &base)?;

        // Control experiment: the untouched base must pass before we can attribute
        // a later failure to the transplanted regression tests.
        let control_result = run(
            &command,
            &worktree,
            config.verification.max_output_bytes,
            timeout,
        );
        let experiment_result = match control_result {
            Ok(control) if control.success => {
                base_control_run = Some(control);
                (|| -> Result<Option<BaseExperiment>> {
                    let mut tracked_tests: Vec<String> = Vec::new();
                    let mut blocked_tests: Vec<String> = Vec::new();
                    for file in inspect.changed_files.iter().filter(|f| {
                        f.is_test && f.tracked && !matches!(f.kind, ChangeKind::Deleted)
                    }) {
                        // A rename whose source path was production code cannot be
                        // transplanted as a test-only change: including the old
                        // path would pull the production file's diff onto base
                        // and make the experiment prove nothing about the test.
                        let from_production =
                            file.previous_path.is_some() && !file.previous_is_test;
                        if from_production || file.path_is_lossy {
                            blocked_tests.push(file.path.clone());
                            continue;
                        }
                        if let Some(previous) = &file.previous_path {
                            tracked_tests.push(previous.clone());
                        }
                        tracked_tests.push(file.path.clone());
                    }
                    tracked_tests.sort();
                    tracked_tests.dedup();
                    let untracked_tests: Vec<String> = inspect
                        .changed_files
                        .iter()
                        .filter(|f| f.is_test && !f.tracked && !f.path_is_lossy)
                        .map(|f| f.path.clone())
                        .collect();

                    let patch = repo.diff_for_paths(&base, &tracked_tests, 3)?;
                    repo.apply_patch(&worktree, &patch)?;
                    repo.copy_untracked_files(&worktree, &untracked_tests)?;

                    // Inline `#[cfg(test)]` modules live in production files, so
                    // they were not part of the patch above. Splice them in
                    // separately; a file that cannot be spliced safely is
                    // recorded rather than approximated (ADR-0010).
                    let mut spliced = Vec::new();
                    let mut refused = Vec::new();
                    splice_inline_tests_into(
                        repo,
                        &worktree,
                        &base,
                        &inspect.changed_files,
                        &inspect.inline_test_hints,
                        &mut blocked_tests,
                        &mut spliced,
                        &mut refused,
                    )?;

                    Ok(Some(BaseExperiment {
                        result: run(
                            &command,
                            &worktree,
                            config.verification.max_output_bytes,
                            timeout,
                        )?,
                        blocked_tests,
                        spliced,
                        refused,
                    }))
                })()
            }
            Ok(control) => {
                if control.timed_out {
                    notes.push("the pristine base revision exceeded the configured timeout, so WitDiff cannot attribute a later failure to the changed tests".into());
                } else {
                    notes.push("the pristine base revision does not pass the configured test command, so WitDiff cannot attribute a later failure to the changed tests".into());
                }
                base_control_run = Some(control);
                Ok(None)
            }
            Err(error) => Err(error),
        };

        // Surface a cleanup failure only when the experiment itself succeeded;
        // otherwise the experiment error is the actionable one and the guard's
        // `Drop` still performs best-effort removal.
        let experiment = match experiment_result {
            Ok(result) => {
                if options.keep_worktree {
                    let retained = guard.retain();
                    notes.push(format!("base worktree retained at {}", retained.display()));
                } else {
                    guard.remove()?;
                }
                result
            }
            Err(error) => return Err(error),
        };

        match experiment {
            None => VerificationStatus::NotVerified,
            Some(BaseExperiment {
                result,
                blocked_tests,
                spliced,
                refused,
            }) => {
                spliced_inline_tests = spliced;
                refused_inline_tests = refused;
                for blocked in &blocked_tests {
                    notes.push(format!(
                        "changed test {blocked} was not transplanted: it was renamed from a non-test path or its name is not representable as UTF-8, so a test-only transplant could not be constructed for it"
                    ));
                }
                let candidate_status = match result.failure_kind {
                    Some(FailureKind::TestFailure) if !result.success => {
                        red_green_proven = true;
                        let has_high = integrity_findings
                            .iter()
                            .any(|f| f.severity == Severity::High);
                        if config.verification.block_on_integrity_findings && has_high {
                            notes.push("red/green behavior was observed, but high-severity test-integrity findings block verification".into());
                            VerificationStatus::NotVerified
                        } else if !blocked_tests.is_empty() {
                            // Proof came from a partial transplant. The
                            // experiment cannot speak for the excluded files.
                            notes.push("red/green behavior was observed, but not every changed test could be transplanted, so the result is not a complete proof".into());
                            VerificationStatus::NotVerified
                        } else if integrity_findings.is_empty() {
                            VerificationStatus::Verified
                        } else {
                            VerificationStatus::VerifiedWithWarnings
                        }
                    }
                    Some(FailureKind::CompileError) if !result.success => {
                        notes.push("the transplanted test did not compile against the base revision; v0.1 treats compile failure as incompatible rather than behavioral red/green proof".into());
                        VerificationStatus::BaseIncompatible
                    }
                    Some(FailureKind::Timeout) if !result.success => {
                        notes.push("the base + changed-tests run exceeded the configured timeout; a suite that does not terminate cannot demonstrate a behavioral regression".into());
                        VerificationStatus::NotVerified
                    }
                    _ if result.success => {
                        notes.push("changed tests also pass on the base revision; they do not prove the behavioral change".into());
                        VerificationStatus::NotVerified
                    }
                    _ => {
                        notes.push("base+changed-tests command failed, but not with a recognized test assertion failure".into());
                        VerificationStatus::NotVerified
                    }
                };
                base_run = Some(result);
                candidate_status
            }
        }
    };

    // Reported here rather than before the experiment: whether an inline test
    // module could be transplanted is only known once the splice has been
    // attempted. Checking earlier made the condition always true and produced
    // this note for files that were in fact spliced.
    let unspliced: Vec<&String> = inspect
        .inline_test_hints
        .iter()
        .filter(|hint| {
            !spliced_inline_tests
                .iter()
                .any(|entry| &entry.path == *hint)
                && !refused_inline_tests
                    .iter()
                    .any(|entry| &entry.path == *hint)
        })
        .collect();
    if !unspliced.is_empty() {
        notes.push(format!(
            "possible inline Rust tests changed inside production files: {}; no inline test module could be transplanted, so they were not part of the proof",
            unspliced
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let after = repo.workspace_fingerprint(&base)?;
    let evidence_fresh = before == after;
    if !evidence_fresh {
        notes.push("workspace changed while verification was running; evidence is stale".into());
        if status.is_verified() {
            status = VerificationStatus::NotVerified;
        }
    }

    Ok(Receipt {
        schema_version: "witdiff.receipt.v1".into(),
        generated_at: Utc::now().to_rfc3339(),
        status,
        repo_root: inspect.repo_root,
        base,
        head_commit: inspect.head_commit,
        workspace_fingerprint_before: before,
        workspace_fingerprint_after: after,
        evidence_fresh,
        changed_files: inspect.changed_files,
        changed_test_files: inspect.changed_test_files,
        integrity_findings,
        head_run,
        base_control_run,
        base_run,
        red_green_proven,
        notes,
        test_selection,
        effective_test_command,
        spliced_inline_tests,
        refused_inline_tests,
    })
}

/// Everything the base experiment produced that the receipt needs.
///
/// A named struct rather than a tuple because the result now carries the run,
/// the tests that could not be transplanted, and the inline modules that were
/// spliced or refused — a tuple of four unrelated types at the call site is
/// where transcription mistakes happen.
struct BaseExperiment {
    result: RunResult,
    blocked_tests: Vec<String>,
    spliced: Vec<SplicedInlineTests>,
    refused: Vec<RefusedInlineTests>,
}

/// Splice inline `#[cfg(test)]` test modules from production files onto the
/// base worktree. See ADR-0010.
///
/// Every precondition is checked before anything is written, and a file that
/// fails one is recorded in `refused` rather than spliced partially. A partial
/// splice would produce a worktree that exists in neither revision, and the
/// resulting red or green would be attributed to a code state that never
/// existed.
#[allow(clippy::too_many_arguments)]
fn splice_inline_tests_into(
    repo: &GitRepo,
    worktree: &std::path::Path,
    base: &str,
    changed_files: &[ChangedFile],
    inline_test_hints: &[String],
    blocked_tests: &mut Vec<String>,
    spliced: &mut Vec<SplicedInlineTests>,
    refused: &mut Vec<RefusedInlineTests>,
) -> Result<()> {
    for path in inline_test_hints {
        let Some(file) = changed_files
            .iter()
            .find(|candidate| &candidate.path == path)
        else {
            continue;
        };
        // A deleted or renamed-away file has no head source to splice from.
        if matches!(file.kind, ChangeKind::Deleted) || file.path_is_lossy {
            continue;
        }
        // A rename whose source was production code is refused by ADR-0007
        // already; splicing it would reintroduce that failure through a
        // different door.
        if file.previous_path.is_some() && !file.previous_is_test {
            blocked_tests.push(file.path.clone());
            continue;
        }

        // The head revision is the working tree, not the `HEAD` commit: the
        // change being verified is usually uncommitted. Reading `HEAD` here
        // would compare the base against itself and find nothing to splice.
        let Some(head_source) = repo.worktree_source(&file.path)? else {
            continue;
        };
        let Some(base_source) = repo.show_file_at(base, &file.path)? else {
            // New in head: there is no base file to splice into, and the whole
            // file is a new-file transplant handled elsewhere.
            continue;
        };

        let outcome = crate::inline::splice_inline_tests(&base_source, &head_source);
        let modules = match outcome {
            crate::inline::SpliceOutcome::NoInlineTestChange => continue,
            crate::inline::SpliceOutcome::Refused(reason) => {
                refused.push(RefusedInlineTests {
                    path: file.path.clone(),
                    reason: reason.as_str().to_owned(),
                    explanation: reason.explanation().to_owned(),
                });
                continue;
            }
            crate::inline::SpliceOutcome::Spliced(source) => source,
        };

        // Production changes elsewhere in the same file are *not* a reason to
        // refuse, and refusing would defeat the feature: fixing a bug and
        // tightening the inline test in one commit is the normal shape of the
        // change. Splicing moves only the test module's bytes, so an outside
        // change is simply left behind at the base revision, which is exactly
        // what the experiment needs. Measured: base production plus a head test
        // module yields a genuine test failure, while a test module that
        // references a head-only production item yields a compile error, which
        // ADR-0003 already classifies as `base_incompatible` rather than a
        // proof.
        //
        // The one thing that must not happen is a *partial* module splice, and
        // `splice_inline_tests` refuses that by construction: it replaces whole
        // module spans or returns a refusal.

        repo.write_into_worktree(worktree, &file.path, &modules)?;
        spliced.push(SplicedInlineTests {
            path: file.path.clone(),
            modules: crate::inline::test_module_identities(&head_source),
        });
    }

    if !spliced.is_empty() {
        spliced.sort_by(|a, b| a.path.cmp(&b.path));
    }
    if !refused.is_empty() {
        refused.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(())
}

/// Collect test-integrity findings for every changed dedicated test.
///
/// Rust test files are compared structurally: the base and head revisions are
/// both parsed and their test/assertion shapes diffed. That is strictly more
/// accurate than reading added and removed lines, because it can tell an
/// assertion that merely moved from one that was deleted.
///
/// The line-oriented analyzer remains the fallback for a file that cannot be
/// parsed as Rust, so a non-Rust or temporarily broken test file still produces
/// findings rather than silently producing none.
fn collect_integrity(
    repo: &GitRepo,
    base: &str,
    inspect: &InspectReport,
) -> Result<Vec<IntegrityFinding>> {
    let mut findings = Vec::new();
    for path in &inspect.changed_test_files {
        let tracked = inspect
            .changed_files
            .iter()
            .find(|file| file.path == *path)
            .map(|file| file.tracked)
            .unwrap_or(true);

        if path.ends_with(".rs") {
            let head_source = fs::read_to_string(repo.root().join(path))
                .with_context(|| format!("failed reading test {path}"))?;
            let base_source = if tracked {
                repo.show_file_at(base, path)?
            } else {
                None
            };

            let structural = analyze_rust_test_change(path, base_source.as_deref(), &head_source);
            let parsed = !structural
                .iter()
                .any(|finding| finding.rule == "test_source_unparsable");

            if parsed {
                findings.extend(structural);
                continue;
            }

            // The structural pass could not read the file, so fall back to the
            // line-oriented rules. Both sets are kept: the structural pass may
            // still have produced additive findings that remain valid.
            findings.extend(structural);
            let diff = if tracked {
                repo.diff_text_for_path(base, path)?
            } else {
                head_source
                    .lines()
                    .map(|line| format!("+{line}\n"))
                    .collect()
            };
            findings.extend(analyze_test_diff(path, &diff));
            continue;
        }

        let diff = if tracked {
            repo.diff_text_for_path(base, path)?
        } else {
            let content = fs::read_to_string(repo.root().join(path))
                .with_context(|| format!("failed reading untracked test {path}"))?;
            content.lines().map(|line| format!("+{line}\n")).collect()
        };
        findings.extend(analyze_test_diff(path, &diff));
    }
    Ok(findings)
}

pub fn write_receipt(
    repo_root: &std::path::Path,
    receipt: &Receipt,
    output: Option<PathBuf>,
) -> Result<PathBuf> {
    let path = output.unwrap_or_else(|| repo_root.join(".witdiff").join("receipt.json"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating receipt directory {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(receipt).context("failed serializing receipt")?;
    fs::write(&path, bytes).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(path)
}
