mod toolchain;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use witdiff_core::{
    config::Config, inspect_repository, model::MutantOutcome, verify::write_receipt,
    verify_repository, CommandSpec, GitRepo, Receipt, Severity, VerifyOptions,
};

#[derive(Debug, Parser)]
#[command(
    name = "witdiff",
    version,
    about = "Deterministic verification for AI-written code"
)]
struct Cli {
    /// Repository path. Defaults to the current directory.
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a starter witdiff.toml in the repository root.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Check local prerequisites and repository configuration.
    Doctor,
    /// Show what WitDiff considers changed tests vs production files.
    Inspect {
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Prove changed tests fail on base and pass on the current workspace.
    Verify {
        #[arg(long)]
        base: Option<String>,
        /// Override the configured test command, e.g. --command "cargo test --all-targets".
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        json: bool,
        /// Exit non-zero unless status is exactly VERIFIED.
        #[arg(long)]
        strict: bool,
        /// Keep the temporary base worktree for investigation.
        #[arg(long)]
        keep_worktree: bool,
        /// Write the JSON receipt to a custom path.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Do not persist a receipt file.
        #[arg(long)]
        no_write: bool,
        /// Treat `no_changed_tests` as a gate failure.
        ///
        /// Off by default: a change with no tests is not a failed proof, and
        /// failing it would make the check unusable on unrelated pull requests
        /// (ADR-0013).
        #[arg(long)]
        fail_on_no_changed_tests: bool,
        /// Try to obtain a missing test toolchain using the project's own
        /// installer before verifying.
        ///
        /// Runs only an installer the repository already commits, such as a
        /// Maven or Gradle wrapper, which downloads an exactly pinned
        /// distribution. WitDiff never chooses a version itself and never runs
        /// a package manager that executes project scripts (ADR-0019).
        #[arg(long)]
        install_toolchains: bool,
        /// Emit GitHub Actions workflow commands for the status and findings.
        ///
        /// Escaping is done here rather than in a workflow YAML file, so every
        /// consumer gets correctly escaped annotations (ADR-0013).
        #[arg(long)]
        github_annotations: bool,
    },
    /// Print the most recent receipt.
    Receipt {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("witdiff: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let start = match cli.repo {
        Some(path) => path,
        None => env::current_dir().context("could not read current directory")?,
    };

    match cli.command {
        Commands::Init { force } => init(&start, force),
        Commands::Doctor => doctor(&start),
        Commands::Inspect { base, json } => inspect(&start, base.as_deref(), json),
        Commands::Verify {
            base,
            command,
            json,
            strict,
            keep_worktree,
            output,
            no_write,
            fail_on_no_changed_tests,
            github_annotations,
            install_toolchains,
        } => verify(
            &start,
            VerifyArgs {
                base,
                command,
                json,
                strict,
                keep_worktree,
                output,
                no_write,
                fail_on_no_changed_tests,
                github_annotations,
                install_toolchains,
            },
        ),
        Commands::Receipt { path, json } => receipt(&start, path, json),
    }
}

fn init(start: &Path, force: bool) -> Result<ExitCode> {
    let repo = GitRepo::discover(start)?;
    let path = repo.root().join("witdiff.toml");
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            path.display()
        );
    }
    // Infer the project type rather than always writing Rust defaults: in a
    // Python or Go repository those would make the first verification attempt
    // to run `cargo test` with no Cargo.toml.
    let config = Config::inferred_for(repo.root());
    fs::write(&path, toml::to_string_pretty(&config)?)
        .with_context(|| format!("failed writing {}", path.display()))?;
    println!("created {}", path.display());
    println!("  language   : {}", config.project.language);
    println!("  framework  : {}", config.verification.framework);
    println!(
        "  test cmd   : {}",
        config.verification.test_command.join(" ")
    );
    println!();
    println!("Edit the file if the command or test globs are wrong, then run `witdiff doctor`.");
    Ok(ExitCode::SUCCESS)
}

fn doctor(start: &Path) -> Result<ExitCode> {
    let repo = GitRepo::discover(start)?;
    let config = Config::load(repo.root())?;
    let git_ok = command_exists("git", &["--version"]);

    // Check the binary this project actually uses, not `cargo`. A Python or Go
    // repository has no reason to have cargo installed, and reporting it as a
    // failure would be wrong.
    let program = config.verification.test_command.first().cloned();
    let command_ok = program
        .as_deref()
        .map(|program| command_exists(program, &["--version"]))
        .unwrap_or(false);

    // Validate the framework name here rather than letting it surface at
    // verify time. A preflight check that reports "ok" for a config that
    // cannot run is not doing its job (ADR-0012).
    let framework = config.verification.framework();

    println!("WitDiff doctor");
    println!("  repository : {}", repo.root().display());
    println!("  git        : {}", mark(git_ok));
    println!(
        "  config     : {}",
        if repo.root().join("witdiff.toml").exists() {
            "configured"
        } else {
            "using defaults"
        }
    );
    println!(
        "  test cmd   : {} {}",
        mark(command_ok),
        program.as_deref().unwrap_or("(none configured)")
    );
    match &framework {
        Ok(framework) => println!("  framework  : ok ({})", framework.as_str()),
        Err(error) => println!("  framework  : failed ({error})"),
    }

    let ok = git_ok && command_ok && framework.is_ok();
    if !ok {
        println!();
        println!(
            "  next       : fix the entries marked failed above, then re-run `witdiff doctor`."
        );
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

fn inspect(start: &Path, base: Option<&str>, json: bool) -> Result<ExitCode> {
    let repo = GitRepo::discover(start)?;
    let config = Config::load(repo.root())?;
    let report = inspect_repository(&repo, &config, base)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("WitDiff inspect");
        println!("  base             : {}", report.base);
        println!("  head             : {}", report.head_commit);
        println!("  workspace dirty  : {}", report.workspace_dirty);
        println!("  changed tests    : {}", report.changed_test_files.len());
        for path in &report.changed_test_files {
            println!("    T {path}");
        }
        println!(
            "  production files : {}",
            report.changed_production_files.len()
        );
        for path in &report.changed_production_files {
            println!("    P {path}");
        }
        if !report.inline_test_hints.is_empty() {
            println!("  inline-test hints:");
            for path in &report.inline_test_hints {
                println!("    ! {path}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
/// Options for `verify`.
///
/// A struct rather than positional parameters: the command now carries several
/// independent booleans, and a call site passing eight bare arguments is where
/// transposition mistakes happen.
struct VerifyArgs {
    base: Option<String>,
    command: Option<String>,
    json: bool,
    strict: bool,
    keep_worktree: bool,
    output: Option<PathBuf>,
    no_write: bool,
    fail_on_no_changed_tests: bool,
    github_annotations: bool,
    install_toolchains: bool,
}

fn verify(start: &Path, args: VerifyArgs) -> Result<ExitCode> {
    let VerifyArgs {
        base,
        command,
        json,
        strict,
        keep_worktree,
        output,
        no_write,
        fail_on_no_changed_tests,
        github_annotations,
        install_toolchains,
    } = args;
    let repo = GitRepo::discover(start)?;
    let config = Config::load(repo.root())?;

    // Installation happens before verification and only when asked for. The
    // core never performs network access (invariant 4).
    //
    // A successful wrapper run also *replaces* the configured command with the
    // wrapper. A real `mvnw` puts Maven on PATH for its own invocation only, so
    // leaving the command as `mvn` would mean the tool is present but
    // unreachable, and the run would fail for the same reason as before. The
    // substitution is not silent: the receipt records the command that actually
    // ran in `effective_test_command`.
    let mut command_override = None;
    if install_toolchains {
        let configured_program = config.verification.test_command.first().cloned();
        if let Some(program) = configured_program {
            if command_exists(&program, &["--version"]) {
                println!("  toolchain        : `{program}` is present");
            } else {
                println!(
                    "  toolchain        : `{program}` is missing; looking for a project installer"
                );
                match toolchain::find_installer(repo.root(), &program) {
                    Some(installer) => {
                        println!("  toolchain        : running {}", installer.description);
                        match toolchain::run_installer(&installer) {
                            Ok(()) => {
                                println!(
                                    "  toolchain        : wrapper succeeded; using it for the test command"
                                );
                                command_override = installer.command_line();
                            }
                            Err(error) => {
                                // A failed install is reported and verification
                                // continues: it may still produce findings.
                                println!("  toolchain        : installation failed ({error:#})");
                            }
                        }
                    }
                    None => println!(
                        "  toolchain        : no project installer for `{program}`; install it manually"
                    ),
                }
            }
        }
    }

    let command = match command_override {
        Some(parts) => Some(witdiff_core::CommandSpec::from_vec(parts)?),
        None => command
            .map(|text| {
                let parts = shell_words::split(&text)
                    .with_context(|| format!("could not parse --command: {text}"))?;
                CommandSpec::from_vec(parts)
            })
            .transpose()?,
    };

    let receipt = verify_repository(
        &repo,
        &config,
        VerifyOptions {
            requested_base: base,
            command,
            keep_worktree,
        },
    )?;

    if !no_write {
        let path = write_receipt(repo.root(), &receipt, output)?;
        if !json {
            println!("  receipt          : {}", path.display());
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        print_receipt_summary(&receipt);
    }

    if github_annotations {
        print_github_annotations(&receipt, strict, fail_on_no_changed_tests);
    }

    let gate = receipt.status.gate(strict, fail_on_no_changed_tests);
    Ok(if gate.passes() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

/// Escape a string for a GitHub Actions workflow command.
///
/// The command format is `::name key=value::message`, and the percent, carriage
/// return and newline characters must be escaped in the message. Receipt
/// messages contain quotes, backticks and occasionally newlines, so getting
/// this wrong produces a malformed annotation rather than a visible error.
fn escape_github_command(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escape a workflow-command *property* value, which has a wider escape set.
fn escape_github_property(text: &str) -> String {
    escape_github_command(text)
        .replace(':', "%3A")
        .replace(',', "%2C")
}

/// Emit GitHub Actions annotations for a receipt.
///
/// Written to stdout as workflow commands so the workflow file needs no logic
/// and no string escaping of its own (ADR-0013).
fn print_github_annotations(
    receipt: &witdiff_core::Receipt,
    strict: bool,
    fail_on_no_changed_tests: bool,
) {
    let gate = receipt.status.gate(strict, fail_on_no_changed_tests);
    let summary = format!(
        "WitDiff: {} ({} changed test file(s), {} changed production file(s))",
        receipt.status.as_str(),
        receipt.changed_test_files.len(),
        receipt
            .changed_files
            .iter()
            .filter(|file| !file.is_test)
            .count()
    );

    match gate {
        witdiff_core::model::GateOutcome::Failed => {
            println!("::error::{}", escape_github_command(&summary));
        }
        witdiff_core::model::GateOutcome::NothingToProve => {
            // A notice, not a warning: the receipt is correct, there was simply
            // nothing to prove.
            println!("::notice::{}", escape_github_command(&summary));
        }
        witdiff_core::model::GateOutcome::Satisfied => {
            println!("::notice::{}", escape_github_command(&summary));
        }
    }

    for finding in &receipt.integrity_findings {
        let level = match finding.severity {
            witdiff_core::Severity::High => "error",
            witdiff_core::Severity::Warning => "warning",
            witdiff_core::Severity::Info => "notice",
        };
        println!(
            "::{level} file={},line={},title={}::{}",
            escape_github_property(&finding.path),
            finding.line,
            escape_github_property(&format!("witdiff {}", finding.rule)),
            escape_github_command(&finding.message)
        );
    }

    if let Some(mutation) = &receipt.mutation {
        for result in mutation
            .results
            .iter()
            .filter(|result| result.outcome == witdiff_core::model::MutantOutcome::Survived)
        {
            println!(
                "::warning file={},line={},title={}::{}",
                escape_github_property(&result.path),
                result.line,
                escape_github_property("witdiff survived mutant"),
                escape_github_command(&format!(
                    "a mutant survived: `{}` -> `{}` in {}. The changed tests do not detect this change.",
                    result.original,
                    result.replacement,
                    result.function.as_deref().unwrap_or("this code")
                ))
            );
        }
    }

    for note in &receipt.notes {
        println!("::notice::{}", escape_github_command(note));
    }
}

fn receipt(start: &Path, path: Option<PathBuf>, json: bool) -> Result<ExitCode> {
    let repo = GitRepo::discover(start)?;
    let path = path.unwrap_or_else(|| repo.root().join(".witdiff").join("receipt.json"));
    let bytes = fs::read(&path).with_context(|| format!("failed reading {}", path.display()))?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid receipt {}", path.display()))?;

    // A stored receipt is a claim about a revision, and nothing stops the code
    // changing afterwards. Checking is the difference between a reader knowing
    // the evidence is stale and having to notice a mismatched hash themselves.
    // The current digest is recomputed from the same inputs the receipt
    // recorded, so a mismatch means the verified content differs even when the
    // fingerprint matches — which it does for any content on a clean tree.
    let config = Config::load(repo.root())?;
    let production_paths: Vec<String> = receipt
        .changed_files
        .iter()
        .filter(|file| !file.is_test && !matches!(file.kind, witdiff_core::ChangeKind::Deleted))
        .map(|file| file.path.clone())
        .collect();
    let current_digest = match repo.head_commit() {
        Ok(head) => witdiff_core::digest::collect(
            &repo,
            &receipt.base,
            &head,
            &config.verification.test_command,
            &receipt.changed_test_files,
            &production_paths,
        )
        .ok()
        .map(|digest| digest.as_str().to_owned()),
        Err(_) => None,
    };

    let freshness = match (
        repo.head_commit(),
        repo.workspace_fingerprint(&receipt.base),
    ) {
        (Ok(head), Ok(fingerprint)) => {
            receipt.freshness(&head, &fingerprint, current_digest.as_deref())
        }
        (Err(error), _) | (_, Err(error)) => witdiff_core::model::ReceiptFreshness::Unknown {
            reason: format!("{error:#}"),
        },
    };

    if json {
        // The JSON form stays machine-stable and gains the check as an additive
        // top-level field rather than changing the receipt document.
        let mut document = serde_json::to_value(&receipt)?;
        if let Some(object) = document.as_object_mut() {
            object.insert(
                "receipt_current".to_owned(),
                serde_json::Value::Bool(freshness.is_current()),
            );
            if let Some(warning) = freshness.warning() {
                object.insert(
                    "receipt_warning".to_owned(),
                    serde_json::Value::String(warning),
                );
            }
        }
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        print_receipt_summary(&receipt);
        if let Some(warning) = freshness.warning() {
            println!("  stale            : {warning}");
            println!(
                "  next             : re-run `witdiff verify` to produce evidence for the current state."
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_receipt_summary(receipt: &Receipt) {
    println!("WitDiff verification");
    println!("  status           : {}", receipt.status);
    println!("  base             : {}", receipt.base);
    println!("  head             : {}", receipt.head_commit);
    println!("  changed tests    : {}", receipt.changed_test_files.len());
    println!(
        "  head tests       : {}",
        pass_fail(receipt.head_run.success)
    );
    println!(
        "  base control     : {}",
        receipt
            .base_control_run
            .as_ref()
            .map(pass_fail_run)
            .unwrap_or("not run")
    );
    println!(
        "  base + tests     : {}",
        receipt
            .base_run
            .as_ref()
            .map(pass_fail_run)
            .unwrap_or("not run")
    );
    if receipt.head_run.timed_out {
        println!(
            "  timeout          : head run killed after {}s",
            receipt.head_run.duration_ms / 1000
        );
    }
    println!("  red/green proven : {}", receipt.red_green_proven);
    println!("  evidence fresh   : {}", receipt.evidence_fresh);

    let high = receipt
        .integrity_findings
        .iter()
        .filter(|finding| finding.severity == Severity::High)
        .count();
    if !receipt.integrity_findings.is_empty() {
        println!(
            "  integrity        : {} finding(s), {} high severity",
            receipt.integrity_findings.len(),
            high
        );
        for finding in &receipt.integrity_findings {
            println!(
                "    {} {} [{}] {}",
                finding.severity.as_str(),
                finding.path,
                finding.rule,
                finding.message
            );
        }
    }
    for entry in &receipt.spliced_inline_tests {
        println!(
            "  spliced          : {} (inline test modules: {})",
            entry.path,
            entry.modules.join(", ")
        );
    }
    for entry in &receipt.refused_inline_tests {
        println!(
            "  not spliced      : {} [{}] {}",
            entry.path, entry.reason, entry.explanation
        );
    }
    if let Some(mutation) = &receipt.mutation {
        println!(
            "  mutation         : {} generated, {} killed, {} survived, {} not compiled, {} timeout, {} skipped",
            mutation.generated,
            mutation.killed,
            mutation.survived,
            mutation.not_compiled,
            mutation.timeout,
            mutation.skipped
        );
        for result in mutation
            .results
            .iter()
            .filter(|result| result.outcome == MutantOutcome::Survived)
        {
            println!(
                "    survived {} {}:{} `{}` -> `{}`{}",
                result.operator,
                result.path,
                result.line,
                result.original,
                result.replacement,
                result
                    .function
                    .as_ref()
                    .map(|name| format!(" in {name}"))
                    .unwrap_or_default()
            );
        }
    }
    for note in &receipt.notes {
        println!("  note             : {note}");
    }
    // Printed last and prefixed so it is not mistaken for a finding: the
    // diagnosis above says what happened, this says what to do about it.
    println!("  next             : {}", receipt.status.remediation());
}

fn pass_fail(success: bool) -> &'static str {
    if success {
        "PASS"
    } else {
        "FAIL"
    }
}

/// Like [`pass_fail`], but distinguishes a killed run from an ordinary failure,
/// because "the suite hung" and "the suite failed" call for different actions.
fn pass_fail_run(run: &witdiff_core::RunResult) -> &'static str {
    if run.success {
        "PASS"
    } else if run.timed_out {
        "TIMEOUT"
    } else {
        "FAIL"
    }
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "missing/failing"
    }
}

fn command_exists(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A newline in a message would terminate the workflow command early and
    /// silently truncate the annotation, so it must be escaped.
    #[test]
    fn github_message_escaping_covers_newlines_and_percent() {
        assert_eq!(escape_github_command("a\nb"), "a%0Ab");
        assert_eq!(escape_github_command("a\rb"), "a%0Db");
        assert_eq!(escape_github_command("100% done"), "100%25 done");
        // A literal backslash-n is not a newline and must survive.
        assert_eq!(escape_github_command("a\\nb"), "a\\nb");
    }

    /// Property values live inside `key=value,` and must also escape the
    /// delimiters, or the command is parsed as having extra properties.
    #[test]
    fn github_property_escaping_covers_delimiters() {
        assert_eq!(escape_github_property("a:b"), "a%3Ab");
        assert_eq!(escape_github_property("a,b"), "a%2Cb");
        // A Windows path is the realistic case: `C:\dir` would otherwise be
        // read as the property name `C`.
        assert_eq!(escape_github_property("C:\\dir"), "C%3A\\dir");
    }

    /// The realistic message: receipt findings contain quotes and backticks.
    #[test]
    fn a_typical_finding_message_is_escaped_not_broken() {
        let message = "test `t` changed the expected value from `10` to `20`\nand more";
        let escaped = escape_github_command(message);
        assert!(!escaped.contains('\n'), "no raw newline may survive");
        assert!(escaped.contains("%0A"));
        assert!(escaped.contains('`'), "backticks need no escaping");
    }
}
