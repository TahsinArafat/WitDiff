use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;

use crate::{
    config::Config,
    framework::TestFramework,
    git::{GitRepo, WorktreeGuard},
    integrity::analyze_test_diff,
    model::{
        ChangeKind, ChangedFile, FailureKind, InspectReport, IntegrityFinding, Receipt,
        RefusedInlineTests, RunResult, Severity, SplicedInlineTests, TestSelection,
        VerificationStatus,
    },
    runner::{run, CommandSpec, Sandbox},
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
    let inputs_before = repo.fingerprint_inputs(&base)?;
    let integrity_findings = collect_integrity(repo, &base, &inspect, config)?;

    let timeout = config.verification.timeout_secs.map(Duration::from_secs);
    let framework = config.verification.framework()?;
    // Built once: head, control and experiment must be sandboxed identically,
    // or the three runs would not be comparable.
    let sandbox = Sandbox::from_config(&config.verification);
    // A test command that cannot start must not discard the run: the receipt
    // still carries every integrity finding, which was computed before this
    // point and does not depend on the command (ADR-0019).
    let mut missing_program: Option<String> = None;
    let head_run = match run(
        &command,
        repo.root(),
        config.verification.max_output_bytes,
        timeout,
        framework,
        sandbox.as_ref(),
    ) {
        Ok(result) => result,
        Err(error) => {
            let program = command.as_vec().first().cloned().unwrap_or_default();
            if !is_missing_program_error(&error) {
                return Err(error);
            }
            missing_program = Some(program);
            RunResult::not_started(
                &command.as_vec(),
                &repo.root().display().to_string(),
                &format!("{error:#}"),
            )
        }
    };

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
    if let Some(program) = &missing_program {
        notes.push(format!(
            "the configured test command could not be started because `{program}` was not found. No proof was attempted, so the status is not_verified; the integrity findings above were still computed and do not depend on that program."
        ));
        if let Some(hint) = install_hint(program, config.verification.framework().ok()) {
            notes.push(hint);
        }
    }

    let mut base_control_run = None;
    let mut base_run = None;
    let mut red_green_proven = false;
    let mut status = if missing_program.is_some() {
        // No experiment: a proof requires the command to run.
        VerificationStatus::NotVerified
    } else if !head_run.success {
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
            framework,
            sandbox.as_ref(),
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
                            framework,
                            sandbox.as_ref(),
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

    // Mutation runs last and its result is deliberately not consulted above:
    // it is supplementary evidence that can never change `status` (ADR-0011).
    // It runs against the head revision in its own worktree, so it cannot
    // disturb the developer's working tree.
    let mutation = if config.verification.mutation {
        let production_paths: Vec<String> = inspect
            .changed_files
            .iter()
            .filter(|file| {
                !file.is_test
                    && !matches!(file.kind, ChangeKind::Deleted)
                    && !file.path_is_lossy
                    && file.path.ends_with(".rs")
            })
            .map(|file| file.path.clone())
            .collect();
        match crate::mutation_runner::analyze_mutations(
            repo,
            config,
            &command,
            &base,
            &production_paths,
            timeout,
            framework,
        ) {
            Ok(report) => report,
            Err(error) => {
                // A mutation failure must not fail the verification: the
                // red/green evidence is already collected and remains valid.
                notes.push(format!(
                    "mutation analysis did not complete: {error}; the red/green evidence above is unaffected"
                ));
                None
            }
        }
    } else {
        None
    };
    if let Some(report) = &mutation {
        if report.survived > 0 {
            notes.push(format!(
                "{} mutant(s) survived: the changed tests do not detect the listed behavior changes",
                report.survived
            ));
        }
        for note in &report.notes {
            notes.push(format!("mutation: {note}"));
        }
    }

    // The digest identifies the verified inputs, which the workspace
    // fingerprint cannot: on a clean tree the fingerprint is SHA-256 of the
    // empty string (ADR-0015).
    let production_paths: Vec<String> = inspect
        .changed_files
        .iter()
        .filter(|file| !file.is_test && !matches!(file.kind, ChangeKind::Deleted))
        .map(|file| file.path.clone())
        .collect();
    let verification_digest = crate::digest::collect(
        repo,
        &base,
        &inspect.head_commit,
        &command.as_vec(),
        &inspect.changed_test_files,
        &production_paths,
    )
    .map(|digest| digest.as_str().to_owned())
    .map_err(|error| {
        // A digest failure must not fail the verification, but it must not be
        // silent either: a receipt without a digest cannot be attested.
        notes.push(format!(
            "the verification digest could not be computed: {error}; this receipt identifies no verified inputs"
        ));
        error
    })
    .ok();

    // The environment is collected separately from the digest: the digest is
    // about *what* was verified and is recomputed later for freshness, while
    // this is about *where*. Folding it in would make every older receipt
    // report stale inputs the moment the toolchain changed.
    let environment = crate::environment::collect(repo, &command.as_vec());

    // Signing is opt-in and happens after the digest, so the signature covers
    // exactly what was verified. A signing failure never fails verification: the
    // receipt is still produced, unsigned, and the operator is told (ADR-0022).
    let mut signature = None;
    if let Some(key_path) = &config.verification.signing_key {
        match verification_digest.as_deref() {
            Some(digest) => {
                let resolved = if std::path::Path::new(key_path).is_absolute() {
                    std::path::PathBuf::from(key_path)
                } else {
                    repo.root().join(key_path)
                };
                match crate::signing::sign_digest(status.as_str(), digest, &resolved) {
                    Ok(signed) => signature = Some(signed),
                    Err(error) => notes.push(format!(
                        "the receipt could not be signed with `{}`: {error:#}. The receipt is unsigned; a signature proves only that it was not edited after the run",
                        resolved.display()
                    )),
                }
            }
            None => notes.push(
                "signing was requested but no verification digest could be computed, so the                  receipt is unsigned"
                    .to_owned(),
            ),
        }
    }

    let after = repo.workspace_fingerprint(&base)?;
    let evidence_fresh = before == after;
    if !evidence_fresh {
        // Name what moved, but summarize: a first build can touch hundreds of
        // paths under `target/`, and dumping them buries the one that matters.
        // A bare "something changed" is unhelpful; an exhaustive list is
        // unusable. The signal is the handful of source-level paths, so build
        // output is grouped and counted rather than enumerated.
        let inputs_after = repo.fingerprint_inputs(&base)?;
        let mut moved: Vec<&str> = Vec::new();
        for (path, digest) in &inputs_after {
            match inputs_before.get(path) {
                Some(previous) if previous == digest => {}
                _ => moved.push(path),
            }
        }
        for path in inputs_before.keys() {
            if !inputs_after.contains_key(path) {
                moved.push(path);
            }
        }
        moved.sort_unstable();
        moved.dedup();

        let (build_output, source_paths): (Vec<&str>, Vec<&str>) =
            moved.iter().partition(|path| is_build_output(path));

        let mut detail = Vec::new();
        if !source_paths.is_empty() {
            detail.push(format!("changed: {}", source_paths.join(", ")));
        }
        if !build_output.is_empty() {
            detail.push(format!(
                "{} build-output path(s) also appeared, which do not affect the verification",
                build_output.len()
            ));
        }
        if detail.is_empty() {
            detail
                .push("the changed content could not be attributed to a specific path".to_owned());
        }

        notes.push(format!(
            "workspace changed while verification was running, so the evidence is stale ({}). If a tracked file changed, commit it and re-run. Build output should be listed in .gitignore.",
            detail.join("; ")
        ));
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
        verification_digest,
        signature,
        mutation,
        environment,
    })
}

/// A concrete install instruction for a missing program.
///
/// The developer is never left to work out what to do. The hint names the
/// ecosystem's normal installer rather than a WitDiff-chosen version, because
/// the repository is the authority on which version it needs (ADR-0019).
fn install_hint(program: &str, framework: Option<TestFramework>) -> Option<String> {
    let hint = match (framework, program) {
        (Some(TestFramework::Cargo), _) => {
            "install Rust with https://rustup.rs, or run `rustup toolchain install` if a pin is committed"
        }
        (Some(TestFramework::Pytest), _) => {
            "install Python, or run `uv sync` / `pip install -r requirements.txt` to create the project's environment"
        }
        (Some(TestFramework::JavaScript), _) => {
            "install Node.js (https://nodejs.org), or run `npm ci` to install the project's dependencies"
        }
        (Some(TestFramework::Go), _) => {
            "install Go from https://go.dev/dl, or let the toolchain install the version named in go.mod by running `go version`"
        }
        (Some(TestFramework::Java), "mvn") => {
            "this project has no Maven. Use the repository's wrapper (./mvnw) if one is committed, or install Maven from https://maven.apache.org"
        }
        (Some(TestFramework::Java), "gradle") => {
            "this project has no Gradle. Use the repository's wrapper (./gradlew) if one is committed, or install Gradle from https://gradle.org"
        }
        (Some(TestFramework::Java), _) => {
            "install a JDK from https://adoptium.net; the Java analyzer needs a JDK rather than a bare JRE"
        }
        (Some(TestFramework::Ruby), _) => {
            "install Ruby from https://www.ruby-lang.org, then run `bundle install` to install the project's gems"
        }
        (None, other) => return Some(format!(
            "`{other}` was not found on PATH. Install it, or point verification.test_command at a command that exists."
        )),
    };
    Some(format!("next: {hint}."))
}

/// Whether an error means the program could not be started at all.
///
/// Distinguished from a command that ran and failed: only the former is a
/// missing toolchain, which is a fact about the environment rather than about
/// the change (ADR-0019).
fn is_missing_program_error(error: &anyhow::Error) -> bool {
    for cause in error.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return io.kind() == std::io::ErrorKind::NotFound
                || io.kind() == std::io::ErrorKind::PermissionDenied;
        }
    }
    false
}

/// Whether a path is build output rather than source.
///
/// Used to keep a staleness report readable. A first build can create hundreds
/// of files under a target directory, and enumerating them buries the one or
/// two source paths that actually matter.
fn is_build_output(path: &str) -> bool {
    const BUILD_DIRS: [&str; 8] = [
        "target/",
        "node_modules/",
        ".venv/",
        "venv/",
        "__pycache__/",
        "build/",
        "dist/",
        ".mypy_cache/",
    ];
    let lower = path.to_ascii_lowercase();
    BUILD_DIRS
        .iter()
        .any(|dir| lower.starts_with(dir) || lower.contains(&format!("/{dir}")))
        || lower.ends_with(".pyc")
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
    config: &Config,
) -> Result<Vec<IntegrityFinding>> {
    let mut findings = Vec::new();

    // Derived once from the project's own test command, so the analysis runs
    // under the interpreter the project tests with (ADR-0016). `None` means
    // the command does not name a Python interpreter, which is reported per
    // file rather than guessed at.
    let python_interpreter = crate::pyanalysis::Interpreter::from_test_command(
        &config.verification.test_command,
        repo.root(),
    );
    // Same idea for Go (ADR-0017): the toolchain is the one the project tests
    // with, so a repository that can run `go test` can run the analysis.
    let go_toolchain = crate::goanalysis::GoToolchain::from_test_command(
        &config.verification.test_command,
        repo.root(),
    );
    // Java too (ADR-0018). The JDK ships a parser, so a project that can
    // compile its tests can analyze them.
    let java_toolchain = crate::javaanalysis::JavaToolchain::from_test_command(
        &config.verification.test_command,
        repo.root(),
    );
    // Ruby too (ADR-0020). `ripper` ships with the interpreter, so a project
    // that can run its tests can analyze them.
    let ruby_toolchain = crate::rubyanalysis::RubyToolchain::from_test_command(
        &config.verification.test_command,
        repo.root(),
    );
    // JavaScript too (ADR-0021). The parser comes from the project rather than
    // from the runtime, so the working directory matters.
    let js_toolchain = crate::jsanalysis::JsToolchain::from_test_command(
        &config.verification.test_command,
        repo.root(),
    );
    for path in &inspect.changed_test_files {
        let tracked = inspect
            .changed_files
            .iter()
            .find(|file| file.path == *path)
            .map(|file| file.tracked)
            .unwrap_or(true);

        // JavaScript and TypeScript test files get structural analysis
        // (ADR-0021). The parser is required from the project, because Node
        // ships none, so a project without one is reported rather than analyzed
        // more weakly.
        if path.ends_with(".js")
            || path.ends_with(".jsx")
            || path.ends_with(".ts")
            || path.ends_with(".tsx")
            || path.ends_with(".mjs")
            || path.ends_with(".cjs")
        {
            if let Some(toolchain) = &js_toolchain {
                let head_path = repo.root().join(path);
                let base_source = if tracked {
                    repo.show_file_at(base, path)?
                } else {
                    None
                };
                findings.extend(crate::jsanalysis::analyze_javascript_test_change(
                    path,
                    toolchain,
                    &head_path,
                    base_source.as_deref(),
                ));
                continue;
            }
            let from_configured = config
                .verification
                .test_command
                .first()
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned());
            findings.push(IntegrityFinding {
                severity: Severity::Info,
                path: path.clone(),
                line: "0".into(),
                rule: "test_source_unparsable".into(),
                message: format!(
                    "this JavaScript test file was not analyzed structurally, because Node was not determined from the configured test command (`{from_configured}`). The red/green proof is unaffected; test-weakening findings are not available for this file."
                ),
            });
            continue;
        }

        // Ruby test files get structural analysis (ADR-0020). Minitest and
        // RSpec declare `assert_equal expected, actual`, so the summary tool
        // swaps the arguments to match the subject-first shared engine.
        if path.ends_with(".rb") {
            if let Some(toolchain) = &ruby_toolchain {
                let head_path = repo.root().join(path);
                let base_source = if tracked {
                    repo.show_file_at(base, path)?
                } else {
                    None
                };
                findings.extend(crate::rubyanalysis::analyze_ruby_test_change(
                    path,
                    toolchain,
                    &head_path,
                    base_source.as_deref(),
                ));
                continue;
            }
            let from_configured = config
                .verification
                .test_command
                .first()
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned());
            findings.push(IntegrityFinding {
                severity: Severity::Info,
                path: path.clone(),
                line: "0".into(),
                rule: "test_source_unparsable".into(),
                message: format!(
                    "this Ruby test file was not analyzed structurally, because a Ruby interpreter could not be determined from the configured test command (`{from_configured}`). The red/green proof is unaffected; test-weakening findings are not available for this file."
                ),
            });
            continue;
        }

        // Java test files get structural analysis (ADR-0018). JUnit declares
        // assertEquals(expected, actual), so the summary tool swaps the
        // arguments to match the subject-first form the shared engine uses.
        if path.ends_with(".java") {
            if let Some(toolchain) = &java_toolchain {
                let head_path = repo.root().join(path);
                let base_source = if tracked {
                    repo.show_file_at(base, path)?
                } else {
                    None
                };
                findings.extend(crate::javaanalysis::analyze_java_test_change(
                    path,
                    toolchain,
                    &head_path,
                    base_source.as_deref(),
                ));
                continue;
            }
            let from_configured = config
                .verification
                .test_command
                .first()
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned());
            findings.push(IntegrityFinding {
                severity: Severity::Info,
                path: path.clone(),
                line: "0".into(),
                rule: "test_source_unparsable".into(),
                message: format!(
                    "this Java test file was not analyzed structurally, because a Java toolchain could not be determined from the configured test command (`{from_configured}`). The red/green proof is unaffected; test-weakening findings are not available for this file."
                ),
            });
            continue;
        }

        // Go test files get structural analysis (ADR-0017). Go has no
        // assertion keyword, so the analyzer compares the condition guarding
        // each failure call rather than the failure message.
        if path.ends_with(".go") {
            if let Some(toolchain) = &go_toolchain {
                let head_path = repo.root().join(path);
                let base_source = if tracked {
                    repo.show_file_at(base, path)?
                } else {
                    None
                };
                findings.extend(crate::goanalysis::analyze_go_test_change(
                    path,
                    toolchain,
                    &head_path,
                    base_source.as_deref(),
                ));
                continue;
            }
            let from_configured = config
                .verification
                .test_command
                .first()
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned());
            findings.push(IntegrityFinding {
                severity: Severity::Info,
                path: path.clone(),
                line: "0".into(),
                rule: "test_source_unparsable".into(),
                message: format!(
                    "this Go test file was not analyzed structurally, because the Go toolchain could not be determined from the configured test command (`{from_configured}`). The red/green proof is unaffected; test-weakening findings are not available for this file."
                ),
            });
            continue;
        }

        // Python test files get structural analysis too (ADR-0016), using the
        // interpreter the project's own test command names. This is what closes
        // the gap the support matrix recorded: measured, a test weakened from
        // `assert is_even(3)` to `assert True` was previously reported
        // `verified` with zero integrity findings.
        if path.ends_with(".py") {
            if let Some(interpreter) = &python_interpreter {
                let head_path = repo.root().join(path);
                let base_source = if tracked {
                    repo.show_file_at(base, path)?
                } else {
                    None
                };
                findings.extend(crate::pyanalysis::analyze_python_test_change(
                    path,
                    interpreter,
                    &head_path,
                    base_source.as_deref(),
                ));
                continue;
            }
            // No Python interpreter could be determined from the test command,
            // so the file cannot be analyzed structurally. Say so rather than
            // falling through to a fallback that matches Rust syntax only and
            // would silently produce nothing.
            let from_configured = config
                .verification
                .test_command
                .first()
                .cloned()
                .unwrap_or_else(|| "(none)".to_owned());
            findings.push(IntegrityFinding {
                severity: Severity::Info,
                path: path.clone(),
                line: "0".into(),
                rule: "test_source_unparsable".into(),
                message: format!(
                    "this Python test file was not analyzed structurally, because no Python interpreter could be determined from the configured test command (`{from_configured}`). The red/green proof is unaffected; test-weakening findings are not available for this file."
                ),
            });
            continue;
        }

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
) -> Result<(PathBuf, Option<String>)> {
    let path = output.unwrap_or_else(|| repo_root.join(".witdiff").join("receipt.json"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating receipt directory {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(receipt).context("failed serializing receipt")?;
    fs::write(&path, bytes).with_context(|| format!("failed writing {}", path.display()))?;

    // Only after the receipt is on disk: recording a receipt that was never
    // written would make the chain claim something a consumer cannot hold.
    let problem = match receipt.verification_digest.as_deref() {
        Some(digest) => match crate::provenance::append(repo_root, digest) {
            Ok(_entry) => None,
            Err(error) => Some(format!(
                "the receipt was written but could not be added to the provenance chain: {error:#}"
            )),
        },
        // No digest, so nothing to chain on. Already noted by the digest step.
        None => None,
    };
    Ok((path, problem))
}

#[cfg(test)]
mod staleness_tests {
    use super::is_build_output;

    /// Build output must be grouped rather than enumerated: a first build can
    /// touch hundreds of paths, and listing them buries the source change that
    /// actually made the evidence stale.
    #[test]
    fn build_output_is_recognized() {
        for path in [
            "target/debug/deps/libfoo.rlib",
            "crates/x/target/debug/foo",
            "node_modules/left-pad/index.js",
            ".venv/lib/python3.11/site-packages/x.py",
            "src/__pycache__/mod.cpython-311.pyc",
            "dist/bundle.js",
            "src/thing.pyc",
        ] {
            assert!(is_build_output(path), "{path} should count as build output");
        }
    }

    /// The dangerous direction: a source file must never be mistaken for build
    /// output, or a real change would be collapsed into a count and hidden.
    #[test]
    fn source_files_are_not_build_output() {
        for path in [
            "src/lib.rs",
            "tests/add.rs",
            "Cargo.lock",
            "Cargo.toml",
            "generated.txt",
            "src/targeting.rs",
            "package.json",
        ] {
            assert!(
                !is_build_output(path),
                "{path} must be reported as a source-level change, not collapsed"
            );
        }
    }
}
