use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;

use crate::{
    config::Config,
    git::{GitRepo, WorktreeGuard},
    integrity::analyze_test_diff,
    model::{
        ChangeKind, FailureKind, InspectReport, IntegrityFinding, Receipt, RunResult, Severity,
        VerificationStatus,
    },
    runner::{run, CommandSpec},
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
    let command = match options.command {
        Some(command) => command,
        None => CommandSpec::from_vec(config.verification.test_command.clone())?,
    };

    let before = repo.workspace_fingerprint(&base)?;
    let integrity_findings = collect_integrity(repo, &base, &inspect)?;

    let timeout = config.verification.timeout_secs.map(Duration::from_secs);
    let head_run = run(
        &command,
        repo.root(),
        config.verification.max_output_bytes,
        timeout,
    )?;

    let mut notes = Vec::new();
    if head_run.timed_out {
        notes.push(format!(
            "the test command exceeded the configured timeout of {} seconds and was terminated; this does not establish that the change is correct",
            config.verification.timeout_secs.unwrap_or_default()
        ));
    }
    if !inspect.inline_test_hints.is_empty() {
        notes.push(format!(
            "possible inline Rust tests changed inside production files: {}; v0.1 does not transplant individual inline-test hunks onto the base revision",
            inspect.inline_test_hints.join(", ")
        ));
    }

    let mut base_control_run = None;
    let mut base_run = None;
    let mut red_green_proven = false;
    let mut status = if !head_run.success {
        VerificationStatus::HeadFailed
    } else if inspect.changed_test_files.is_empty() {
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
                (|| -> Result<Option<RunResult>> {
                    let mut tracked_tests: Vec<String> = Vec::new();
                    for file in inspect.changed_files.iter().filter(|f| {
                        f.is_test && f.tracked && !matches!(f.kind, ChangeKind::Deleted)
                    }) {
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
                        .filter(|f| f.is_test && !f.tracked)
                        .map(|f| f.path.clone())
                        .collect();

                    let patch = repo.diff_for_paths(&base, &tracked_tests, 3)?;
                    repo.apply_patch(&worktree, &patch)?;
                    repo.copy_untracked_files(&worktree, &untracked_tests)?;
                    Ok(Some(run(
                        &command,
                        &worktree,
                        config.verification.max_output_bytes,
                        timeout,
                    )?))
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
            Some(result) => {
                let candidate_status = match result.failure_kind {
                    Some(FailureKind::TestFailure) if !result.success => {
                        red_green_proven = true;
                        let has_high = integrity_findings
                            .iter()
                            .any(|f| f.severity == Severity::High);
                        if config.verification.block_on_integrity_findings && has_high {
                            notes.push("red/green behavior was observed, but high-severity test-integrity findings block verification".into());
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
    })
}

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
