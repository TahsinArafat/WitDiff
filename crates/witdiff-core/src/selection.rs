//! Deriving a narrower test command from the set of changed test files.
//!
//! ## Why this is a separate, conservative step
//!
//! Running only the changed tests is faster, but "faster" must never come at
//! the cost of silently claiming less evidence than was gathered. Two failure
//! modes make a naive implementation dangerous, and both were verified against
//! the real `cargo` binary rather than assumed:
//!
//! 1. **`--all-targets` overrides `--test`.** A command carrying
//!    `--all-targets` still builds and runs every test target even when
//!    `--test <name>` is appended. The run would look targeted, the receipt
//!    would name one target, and the whole suite would actually have executed.
//!    That is precisely the "tests pass" claim WitDiff exists to distrust.
//! 2. **Not every test file is its own target.** Cargo compiles each *direct*
//!    child of a `tests/` directory into a separate integration-test target.
//!    Files nested deeper are modules of their parent target and cannot be
//!    selected on their own.
//!
//! When the configured command cannot be narrowed safely, this module returns
//! `None` rather than falling back to the full suite. The caller then records a
//! note and keeps full-suite evidence, so a narrowing that did not happen is
//! always visible in the receipt.

use crate::runner::CommandSpec;

/// Why a targeted command could not be built.
///
/// Reported so the receipt can explain the decision instead of leaving the
/// reader to infer whether narrowing was attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionOutcome {
    /// A narrower command was built from these cargo test target names.
    Targeted(Vec<String>),
    /// The changed tests cannot be expressed as cargo targets.
    NoTargets,
    /// The configured command cannot be narrowed without changing what runs.
    NotNarrowable(&'static str),
}

/// Cargo test target names for the changed dedicated test files.
///
/// Only direct children of a `tests/` directory are returned. A file deeper than
/// that is compiled into its parent target, so naming it as a target would
/// produce a command cargo rejects.
///
/// Returns `None` if *any* changed test file cannot be mapped, because a
/// selection that silently omits one changed test is not a selection over the
/// changed tests.
pub fn derive_test_targets(changed_test_files: &[String]) -> Option<Vec<String>> {
    if changed_test_files.is_empty() {
        return None;
    }

    let mut targets = Vec::new();
    for path in changed_test_files {
        let file = std::path::Path::new(path);
        let stem = file.file_stem()?.to_str()?;
        let parent = file.parent()?;
        if parent.file_name()?.to_str()? != "tests" {
            // Nested under tests/, or a unit-test module such as
            // `src/parser_test.rs`. Neither is independently selectable.
            return None;
        }
        targets.push(stem.to_owned());
    }

    targets.sort();
    targets.dedup();
    Some(targets)
}

/// Build a targeted test command, or explain why the configured one cannot be
/// narrowed.
///
/// Returns the original command unchanged when narrowing is not applicable, so
/// the caller can still run full-suite evidence.
pub fn build_targeted_command(
    base: &CommandSpec,
    changed_test_files: &[String],
) -> (CommandSpec, SelectionOutcome) {
    if let Some(reason) = unnarrowable_reason(base) {
        return (base.clone(), SelectionOutcome::NotNarrowable(reason));
    }

    let Some(targets) = derive_test_targets(changed_test_files) else {
        return (base.clone(), SelectionOutcome::NoTargets);
    };

    let mut args = Vec::with_capacity(base.args.len() + targets.len() * 2);
    for target in &targets {
        args.push("--test".to_owned());
        args.push(target.clone());
    }
    // Everything after a bare `--` belongs to the test harness, not to cargo,
    // so cargo-level flags must be inserted before it.
    let separator = base.args.iter().position(|arg| arg == "--");
    match separator {
        Some(index) => {
            args.extend(base.args[..index].iter().cloned());
            args.extend(base.args[index..].iter().cloned());
        }
        None => args.extend(base.args.iter().cloned()),
    }

    (
        CommandSpec {
            program: base.program.clone(),
            args,
        },
        SelectionOutcome::Targeted(targets),
    )
}

/// The reason the configured command cannot be narrowed, if it cannot.
///
/// Every branch here corresponds to a way that appending `--test` would change
/// or misrepresent what cargo runs.
fn unnarrowable_reason(base: &CommandSpec) -> Option<&'static str> {
    let program = std::path::Path::new(&base.program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&base.program);
    if program != "cargo" {
        return Some("the configured test command is not a cargo invocation");
    }
    if base.args.first().map(String::as_str) != Some("test") {
        return Some("the configured cargo command does not invoke `cargo test`");
    }
    if base
        .args
        .iter()
        .any(|arg| arg == "--all-targets" || arg == "--all")
    {
        // Verified against cargo: `--all-targets` wins over `--test`, so the
        // full suite would run while the receipt claimed a narrow selection.
        return Some(
            "the configured command uses --all-targets, which overrides --test and would run the full suite",
        );
    }
    if base
        .args
        .iter()
        .any(|arg| arg == "--test" || arg == "--tests")
    {
        return Some("the configured command already selects tests explicitly");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn derives_targets_for_direct_children_of_tests() {
        let targets = derive_test_targets(&paths(&["tests/alpha.rs", "tests/beta.rs"]));
        assert_eq!(targets, Some(vec!["alpha".to_owned(), "beta".to_owned()]));
    }

    #[test]
    fn refuses_to_map_nested_files() {
        // `tests/nested/mod.rs` is a module of the `nested` target, not a
        // target itself; naming it would make cargo reject the command.
        assert_eq!(derive_test_targets(&paths(&["tests/nested/deep.rs"])), None);
    }

    #[test]
    fn refuses_when_any_file_is_unmappable() {
        assert_eq!(
            derive_test_targets(&paths(&["tests/alpha.rs", "src/thing_test.rs"])),
            None,
            "a partial selection would omit a changed test"
        );
    }

    #[test]
    fn refuses_when_there_are_no_changed_tests() {
        assert_eq!(derive_test_targets(&[]), None);
    }

    #[test]
    fn builds_a_narrowed_command() {
        let base = spec("cargo", &["test", "--all-features"]);
        let (command, outcome) = build_targeted_command(&base, &paths(&["tests/alpha.rs"]));
        assert_eq!(
            outcome,
            SelectionOutcome::Targeted(vec!["alpha".to_owned()])
        );
        assert_eq!(
            command.args,
            vec!["--test", "alpha", "test", "--all-features"]
        );
    }

    #[test]
    fn keeps_harness_arguments_after_the_separator() {
        let base = spec("cargo", &["test", "--quiet", "--", "--test-threads=1"]);
        let (command, _) = build_targeted_command(&base, &paths(&["tests/alpha.rs"]));
        let separator = command.args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(
            command.args[..separator],
            ["--test", "alpha", "test", "--quiet"],
            "cargo flags must precede the harness separator, got {:?}",
            command.args
        );
        assert_eq!(
            &command.args[separator..],
            ["--", "--test-threads=1"],
            "harness arguments must stay after the separator, got {:?}",
            command.args
        );
    }

    #[test]
    fn refuses_to_narrow_all_targets() {
        // Verified against cargo: --all-targets would run everything.
        let base = spec("cargo", &["test", "--all-targets", "--all-features"]);
        let (command, outcome) = build_targeted_command(&base, &paths(&["tests/alpha.rs"]));
        assert_eq!(
            outcome,
            SelectionOutcome::NotNarrowable(
                "the configured command uses --all-targets, which overrides --test and would run the full suite"
            )
        );
        assert_eq!(
            command.args, base.args,
            "the command must be returned unchanged"
        );
    }

    #[test]
    fn refuses_non_cargo_commands() {
        let base = spec("pytest", &["-q"]);
        let (_, outcome) = build_targeted_command(&base, &paths(&["tests/test_alpha.py"]));
        assert!(matches!(outcome, SelectionOutcome::NotNarrowable(_)));
    }

    #[test]
    fn reports_no_targets_when_files_do_not_map() {
        let base = spec("cargo", &["test"]);
        let (_, outcome) = build_targeted_command(&base, &paths(&["src/lib.rs"]));
        assert_eq!(outcome, SelectionOutcome::NoTargets);
    }
}
