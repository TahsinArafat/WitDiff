//! Coverage of the changed production lines, as supplementary evidence.
//!
//! See `docs/verification-model.md`. This answers a question the red/green
//! experiment cannot: a transplanted test can fail on the base for the right
//! reason and still barely touch the code the change rewrote. Coverage says how
//! much of *what you changed* the suite actually executed.
//!
//! It is deliberately **evidence and never a verdict**, for the same reason
//! mutation is (ADR-0011): a surviving mutant is a question about test
//! strength, not a judgment about correctness, and an uncovered line is not
//! evidence that the code is wrong. Nothing here affects `status`.
//!
//! ## Why a separate run
//!
//! Coverage re-runs the suite with instrumentation on, so it is a second run
//! and a second cost. It therefore runs only when configured, and only after
//! the proof has been established — the receipt's green/red evidence is
//! collected by the ordinary run and is not affected by whether coverage
//! succeeded.
//!
//! ## What "changed lines" means
//!
//! Not "the file changed, so all its lines count". A large refactor touching
//! one line of a 3000-line file is not 3000 covered or uncovered lines. The
//! report counts only lines the diff added, read from the same `-U0` patch the
//! verifier already uses.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::git::GitRepo;
use crate::runner::CommandSpec;

/// Coverage of the lines this change added, per file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverage {
    /// Repository-relative path of the changed production file.
    pub path: String,
    /// How many added lines the file has.
    pub lines: usize,
    /// How many of them the tests executed.
    pub covered: usize,
}

/// Coverage of changed production code.
///
/// Supplementary: this never affects `status` or `red_green_proven`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    /// Added lines across every changed production file examined.
    pub changed_lines: usize,
    /// How many of those the tests executed.
    pub changed_lines_covered: usize,
    /// Per-file detail, sorted by path so two receipts compare.
    pub files: Vec<FileCoverage>,
}

impl CoverageReport {
    pub fn covered_percent(&self) -> Option<f64> {
        (self.changed_lines > 0)
            .then(|| (self.changed_lines_covered as f64 / self.changed_lines as f64) * 100.0)
    }
}

/// The lines a patch adds, on the new side, sorted ascending.
///
/// Read from a `-U0` patch so the counts are about added lines rather than
/// every line in a hunk: with context, an unchanged line inside a hunk would be
/// counted as changed, and a reformatted function would dominate the report.
pub fn added_lines(patch: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    let mut current: Option<usize> = None;
    for line in patch.lines() {
        if line.starts_with("@@") {
            current = parse_hunk_header(line);
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            if let Some(number) = current {
                lines.push(number);
                current = current.map(|n| n + 1);
            }
        } else if line.starts_with(' ') || line.starts_with('-') {
            // A context or removed line does not advance the new-side counter.
            // `-U0` emits none of these inside a hunk, but a caller that passes
            // a non-zero unified count would otherwise misnumber the rest.
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// The starting line of a `@@ -a,b +c,d @@` hunk.
///
/// The right-hand side of the header is followed by ` @@` and often by a
/// section heading, so the token is delimited by whitespace *or* a comma — not
/// by a comma alone. Splitting only on `,` left `"1 @@"` unparseable for the
/// common one-line hunk and silently reported zero changed lines for the whole
/// file, which presents as "no coverage measured" rather than as an error.
fn parse_hunk_header(line: &str) -> Option<usize> {
    let rest = line.split('+').nth(1)?;
    let token = rest.split([' ', ',']).next()?;
    token.parse::<usize>().ok()
}

/// Which lines llvm-cov observed as executed, given its segment list.
///
/// Measured against a fixture where exactly one of three functions was called:
/// llvm-cov's JSON reports `segments`, not `lines`, and a segment carries a
/// count. A line is executed when any segment sitting on it reports a count
/// greater than zero. Segments with `hasCount == false` are region ends and
/// carry the running count of the region that just closed, which is why the
/// whole line's set is considered rather than only the first segment.
pub fn executed_lines(segments: &[[serde_json::Value; 6]]) -> Vec<usize> {
    use std::collections::BTreeSet;
    let mut executed = BTreeSet::new();
    for segment in segments {
        let line = segment[0].as_u64().unwrap_or(0) as usize;
        let count = segment[2].as_u64().unwrap_or(0);
        if line > 0 && count > 0 {
            executed.insert(line);
        }
    }
    let mut lines: Vec<usize> = executed.into_iter().collect();
    lines.sort_unstable();
    lines
}

/// Collect coverage for the changed production files.
///
/// `production_paths` are repository-relative paths at the head revision.
/// Returns an error only when coverage was asked for and could not be
/// produced; the caller reports it as a note and continues.
pub fn collect(
    repo: &GitRepo,
    base: &str,
    test_command: &CommandSpec,
    production_paths: &[String],
    timeout: std::time::Duration,
) -> Result<CoverageReport> {
    if production_paths.is_empty() {
        bail!(
            "coverage needs at least one changed production file, and this change \
             has none"
        );
    }

    let profile = coverage_json(repo.root(), test_command, timeout)?;

    // Parse before touching git, so a malformed profile is reported as such
    // rather than as a missing diff. Relativized here, because llvm-cov names
    // files absolutely and `changed_files` records them repository-relatively;
    // a mismatch would silently report zero coverage for every file.
    let covered = relativize(repo.root(), executed_by_file(&profile)?);

    let mut files = Vec::new();
    for path in production_paths {
        // The path's own lines at the head revision, so an added line is
        // counted even though it is untracked or otherwise absent from git's
        // ordinary diff against the base.
        let patch = repo
            .diff_for_paths(base, std::slice::from_ref(path), 0)
            .with_context(|| format!("could not diff {path}"))?;
        let patch_text = String::from_utf8_lossy(&patch).into_owned();
        let added = added_lines(&patch_text);
        if added.is_empty() {
            continue;
        }
        let executed = covered.get(path.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        let lines_in_file = added.len();
        let covered_in_file = added
            .iter()
            .filter(|line| executed.binary_search(line).is_ok())
            .count();
        files.push(FileCoverage {
            path: path.clone(),
            lines: lines_in_file,
            covered: covered_in_file,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    let changed_lines = files.iter().map(|file| file.lines).sum();
    let changed_lines_covered = files.iter().map(|file| file.covered).sum();
    Ok(CoverageReport {
        changed_lines,
        changed_lines_covered,
        files,
    })
}

/// Run `cargo llvm-cov --json` and capture its profile.
///
/// Not the ordinary [`crate::runner::run`]: that truncates output at
/// `max_output_bytes`, which is 16 KiB by default and nowhere near enough for a
/// coverage profile of a real workspace. Truncated JSON would parse as an error
/// and report coverage as unavailable, which would be a confusing way to learn
/// that a size limit is wrong.
fn coverage_json(
    root: &Path,
    test_command: &CommandSpec,
    timeout: std::time::Duration,
) -> Result<String> {
    // Compared by file name: a project's configured command may name cargo by
    // an absolute path (the test fixtures pass `$CARGO`, and a wrapper script
    // may too), and requiring the bare word `cargo` rejected those.
    let program = Path::new(&test_command.program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let subcommand = test_command.args.first().map(String::as_str);
    if program != "cargo" || subcommand != Some("test") {
        bail!(
            "coverage is only implemented for a `cargo test` command; this project \
             runs `{} {}`",
            test_command.program,
            subcommand.unwrap_or("(no subcommand)")
        );
    }

    let profile =
        std::env::temp_dir().join(format!("witdiff-coverage-{}.json", std::process::id()));
    let file = std::fs::File::create(&profile)
        .with_context(|| format!("failed creating {}", profile.display()))?;

    // Cargo-level flags only. Everything after `--` belongs to the test binary
    // and is not understood by `cargo llvm-cov`, which already knows how to run
    // the suite itself.
    let forwarded: Vec<String> = test_command
        .args
        .iter()
        .skip(2)
        .take_while(|argument| argument.as_str() != "--")
        .cloned()
        .collect();

    let mut child = Command::new("cargo")
        .arg("llvm-cov")
        .arg("--json")
        .args(&forwarded)
        .current_dir(root)
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()
        .context(
            "failed to run `cargo llvm-cov`; install it with `cargo install cargo-llvm-cov`",
        )?;

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    bail!(
                        "coverage run exceeded the {}s deadline and was killed",
                        timeout.as_secs()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => bail!("failed waiting for the coverage run: {error}"),
        }
    };
    let stderr = child
        .wait_with_output()
        .map(|output| String::from_utf8_lossy(&output.stderr).into_owned())
        .unwrap_or_default();

    let text = std::fs::read_to_string(&profile)
        .with_context(|| format!("failed reading {}", profile.display()))?;
    let _ = std::fs::remove_file(&profile);

    if !status.success() {
        bail!(
            "`cargo llvm-cov` failed: {}",
            stderr.lines().last().unwrap_or("(no diagnostic)").trim()
        );
    }
    if text.trim().is_empty() {
        bail!("`cargo llvm-cov` produced no output");
    }
    Ok(text)
}

/// The executed lines llvm-cov observed, keyed by repository-relative path.
///
/// llvm-cov names files absolutely, so paths are relativized against the
/// repository root before lookup: a receipt records repository-relative paths
/// throughout, and a mismatch here would silently report zero coverage for every
/// file.
pub fn executed_by_file(profile: &str) -> Result<std::collections::BTreeMap<String, Vec<usize>>> {
    let document: serde_json::Value =
        serde_json::from_str(profile).context("the coverage profile is not valid JSON")?;

    let mut result = std::collections::BTreeMap::new();
    for data in document["data"].as_array().cloned().unwrap_or_default() {
        for file in data["files"].as_array().cloned().unwrap_or_default() {
            let Some(filename) = file["filename"].as_str() else {
                continue;
            };
            let Some(segments) = file["segments"].as_array() else {
                continue;
            };
            let segments: Vec<[serde_json::Value; 6]> = segments
                .iter()
                .filter_map(|segment| {
                    let items = segment.as_array()?;
                    Some([
                        items.first()?.clone(),
                        items.get(1)?.clone(),
                        items.get(2)?.clone(),
                        items.get(3)?.clone(),
                        items.get(4)?.clone(),
                        items.get(5)?.clone(),
                    ])
                })
                .collect();
            let executed = executed_lines(&segments);
            if executed.is_empty() {
                continue;
            }
            // Absolute in, absolute out until the caller relativizes.
            result.insert(filename.to_owned(), executed);
        }
    }
    Ok(result)
}

/// Relativize absolute coverage keys to the repository root.
///
/// The report needs repository-relative paths to line up with
/// `changed_files`; llvm-cov emits absolute ones.
pub fn relativize(
    root: &Path,
    executed: std::collections::BTreeMap<String, Vec<usize>>,
) -> std::collections::BTreeMap<String, Vec<usize>> {
    executed
        .into_iter()
        .map(|(path, lines)| {
            let key = Path::new(&path)
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or(path);
            (key, lines)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(text: &str) -> String {
        text.to_owned()
    }

    #[test]
    fn added_lines_read_the_new_side_of_the_hunk() {
        let patch = patch(
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,3 +1,4 @@\n\
              keep this\n\
             -old line\n\
             +new line\n\
             +another new line\n",
        );
        assert_eq!(added_lines(&patch), vec![1, 2]);
    }

    /// A hunk that starts partway through the file must start counting there,
    /// or every added line would be numbered from 1.
    #[test]
    fn a_hunk_starting_mid_file_numbers_from_the_right_place() {
        let patch = patch("@@ -40,1 +41,2 @@\n+first\n+second\n");
        assert_eq!(added_lines(&patch), vec![41, 42]);
    }

    /// The common one-line hunk, `@@ -1 +1 @@`, has no comma in its right-hand
    /// side. Splitting only on `,` left the token as `"1 @@"`, which does not
    /// parse, so `current` became `None` and **every** added line in the file
    /// was dropped. The report then read "no changed lines were measurable"
    /// rather than raising anything, which is how the bug survived until an end
    /// to end run showed zero.
    #[test]
    fn a_one_line_hunk_header_parses() {
        let patch = patch("@@ -1 +1 @@\n+pub fn add(a: i32) -> i32 { a + a }\n");
        assert_eq!(added_lines(&patch), vec![1]);
    }

    #[test]
    fn a_one_line_hunk_followed_by_a_positional_hunk() {
        let patch = patch("@@ -1 +1 @@\n-old\n+new\n@@ -2,0 +3 @@ pub fn other()\n+added later\n");
        assert_eq!(added_lines(&patch), vec![1, 3]);
    }

    #[test]
    fn an_empty_patch_adds_nothing() {
        assert!(added_lines("").is_empty());
        assert!(added_lines("@@ -1,0 +1,0 @@\n").is_empty());
    }

    /// Measured against a fixture where exactly one of three functions ran:
    /// llvm-cov reports segments, and a line is executed when any segment on it
    /// carries a non-zero count.
    #[test]
    fn executed_lines_follow_the_measured_rule() {
        let segments: Vec<[serde_json::Value; 6]> = vec![
            [
                1.into(),
                1.into(),
                1.into(),
                true.into(),
                true.into(),
                false.into(),
            ],
            [
                1.into(),
                34.into(),
                0.into(),
                false.into(),
                false.into(),
                false.into(),
            ],
            [
                2.into(),
                1.into(),
                0.into(),
                true.into(),
                true.into(),
                false.into(),
            ],
            [
                3.into(),
                1.into(),
                0.into(),
                true.into(),
                true.into(),
                false.into(),
            ],
        ];
        assert_eq!(executed_lines(&segments), vec![1]);
    }

    #[test]
    fn a_line_is_executed_when_any_segment_on_it_reports_a_count() {
        let segments: Vec<[serde_json::Value; 6]> = vec![
            [
                5.into(),
                1.into(),
                0.into(),
                true.into(),
                true.into(),
                false.into(),
            ],
            [
                5.into(),
                9.into(),
                3.into(),
                true.into(),
                true.into(),
                false.into(),
            ],
        ];
        assert_eq!(executed_lines(&segments), vec![5]);
    }

    #[test]
    fn no_segments_means_nothing_was_executed() {
        assert!(executed_lines(&[]).is_empty());
    }

    /// llvm-cov names files absolutely while a receipt records them
    /// repository-relatively. A mismatch reports zero coverage for everything,
    /// which looks like evidence rather than like an error.
    #[test]
    fn absolute_paths_are_relativized_to_the_repository() {
        let root = Path::new("/repo");
        let mut executed = std::collections::BTreeMap::new();
        executed.insert("/repo/src/lib.rs".to_owned(), vec![1, 2]);
        executed.insert("/elsewhere/other.rs".to_owned(), vec![1]);

        let relativized = relativize(root, executed);
        assert_eq!(relativized.get("src/lib.rs").map(Vec::len), Some(2));
        // A path outside the root is kept as-is rather than dropped, so a
        // mismatch is visible instead of silently reporting nothing.
        assert!(relativized.contains_key("/elsewhere/other.rs"));
    }

    #[test]
    fn a_malformed_profile_is_an_error_not_empty_coverage() {
        assert!(executed_by_file("not json").is_err());
        assert!(executed_by_file("{}").is_ok());
    }

    /// The report must say zero rather than nothing when nothing was covered,
    /// so an uncovered change is visible in the receipt rather than absent.
    #[test]
    fn a_report_with_no_covered_lines_still_reports_totals() {
        let report = CoverageReport {
            changed_lines: 4,
            changed_lines_covered: 0,
            files: vec![FileCoverage {
                path: "src/lib.rs".to_owned(),
                lines: 4,
                covered: 0,
            }],
        };
        assert_eq!(report.covered_percent(), Some(0.0));
        assert_eq!(report.changed_lines, 4);
    }

    #[test]
    fn a_report_with_no_changed_lines_has_no_percentage() {
        let report = CoverageReport {
            changed_lines: 0,
            changed_lines_covered: 0,
            files: Vec::new(),
        };
        assert_eq!(report.covered_percent(), None);
    }
}
