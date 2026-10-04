use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub kind: ChangeKind,
    pub is_test: bool,
    pub tracked: bool,
    /// True when the path could not be decoded as UTF-8 and was lossily
    /// replaced. Such a path does not name a file that can be read back on
    /// disk, so WitDiff refuses to transplant it and says so in a note rather
    /// than silently writing a file to a name that does not exist.
    #[serde(default, skip_serializing_if = "is_false")]
    pub path_is_lossy: bool,
    /// For a rename or copy, whether the path the file came from was itself a
    /// test. A `false` here means the source path was production code, so the
    /// file must never be transplanted as part of a test-only patch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub previous_is_test: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectReport {
    pub repo_root: String,
    pub base: String,
    pub head_commit: String,
    pub workspace_dirty: bool,
    pub changed_files: Vec<ChangedFile>,
    pub changed_test_files: Vec<String>,
    pub changed_production_files: Vec<String>,
    pub inline_test_hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    High,
}

impl Severity {
    /// Stable machine-facing token; must always agree with the serialized
    /// receipt value. See `VerificationStatus::as_str`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityFinding {
    pub severity: Severity,
    pub path: String,
    pub line: String,
    pub rule: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    TestFailure,
    CompileError,
    CommandFailure,
    SpawnFailure,
    /// The command exceeded its configured wall-clock deadline and was killed.
    /// No proof path accepts this: a suite that never finishes has demonstrated
    /// nothing about the change.
    Timeout,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub command: Vec<String>,
    pub cwd: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
    pub failure_kind: Option<FailureKind>,
    /// True when the run was killed for exceeding its deadline. Recorded
    /// explicitly so a consumer never has to infer it from a null exit code.
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    VerifiedWithWarnings,
    NotVerified,
    NoChangedTests,
    HeadFailed,
    BaseIncompatible,
}

/// Whether a stored receipt still describes the working state.
///
/// A receipt is a claim about a revision. Nothing stops the code changing
/// afterwards, and until now `witdiff receipt` printed a stored receipt without
/// checking — measured: a receipt claiming head `8f4e5f2e` was printed while the
/// actual head was `62ec3f5`, with no warning. A reader had to notice the
/// mismatch themselves, and nothing in the output encouraged them to look
/// (ADR-0015 finding 3, backlog PG-504).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptFreshness {
    /// The receipt describes the current state.
    Current,
    /// The working state has moved on since the receipt was written.
    Stale { reasons: Vec<String> },
    /// Whether the receipt is current could not be determined.
    ///
    /// Distinct from `Current`: an unverifiable receipt must not be presented as
    /// though it had been checked.
    Unknown { reason: String },
}

impl Receipt {
    /// Check whether this receipt still describes the repository's state.
    ///
    /// Two independent checks, because either alone can miss a real change:
    ///
    /// - the head commit, which catches a new commit;
    /// - the workspace fingerprint, which catches uncommitted edits that the
    ///   head commit cannot see.
    ///
    /// The uncommitted case is the common one: a receipt written for a change,
    /// then the change edited, keeps the same `head_commit` while describing
    /// code that no longer exists.
    pub fn freshness(&self, head_commit: &str, fingerprint: &str) -> ReceiptFreshness {
        let mut reasons = Vec::new();
        if self.head_commit != head_commit {
            reasons.push(format!(
                "the receipt was written for {} but HEAD is now {}",
                short(&self.head_commit),
                short(head_commit)
            ));
        }
        if self.workspace_fingerprint_after != fingerprint {
            reasons.push("the workspace has changed since the receipt was written".to_owned());
        }
        if reasons.is_empty() {
            ReceiptFreshness::Current
        } else {
            ReceiptFreshness::Stale { reasons }
        }
    }
}

/// The first twelve characters of a digest, for readable messages.
fn short(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

impl ReceiptFreshness {
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }

    /// A one-line summary for human output, or `None` when current.
    pub fn warning(&self) -> Option<String> {
        match self {
            Self::Current => None,
            Self::Stale { reasons } => Some(format!(
                "this receipt is stale and no longer describes the working state: {}",
                reasons.join("; ")
            )),
            Self::Unknown { reason } => Some(format!(
                "whether this receipt still describes the working state could not be determined: {reason}"
            )),
        }
    }
}

/// How a verification status should be treated as a gate.
///
/// Three classes rather than two, because "no proof was attempted" is not the
/// same as "the proof failed" (ADR-0013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// The claim was proven, under the configured strictness.
    Satisfied,
    /// No proof was attempted because there was nothing to prove. Reported
    /// distinctly so a pass is never mistaken for a proof.
    NothingToProve,
    /// A proof was attempted and did not establish the claim, or the case was
    /// undecidable.
    Failed,
}

impl GateOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            GateOutcome::Satisfied => "satisfied",
            GateOutcome::NothingToProve => "nothing_to_prove",
            GateOutcome::Failed => "failed",
        }
    }

    /// Whether the gate passes.
    pub fn passes(self) -> bool {
        !matches!(self, GateOutcome::Failed)
    }
}

impl RunResult {
    /// A run that never started, because the program does not exist.
    ///
    /// Represented rather than propagated as an error so a receipt is still
    /// written. Aborting discards every finding computed before the run,
    /// including integrity findings that never needed this program — which is
    /// strictly less useful than a receipt saying the proof was not attempted
    /// (ADR-0019).
    pub fn not_started(command: &[String], cwd: &str, error: &str) -> Self {
        Self {
            command: command.to_vec(),
            cwd: cwd.to_owned(),
            success: false,
            exit_code: None,
            duration_ms: 0,
            stdout: String::new(),
            stderr: error.to_owned(),
            failure_kind: Some(FailureKind::SpawnFailure),
            timed_out: false,
        }
    }

    /// Whether the program could not be started at all.
    pub fn is_missing_program(&self) -> bool {
        self.failure_kind == Some(FailureKind::SpawnFailure)
    }
}

impl VerificationStatus {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified | Self::VerifiedWithWarnings)
    }

    pub fn is_strictly_verified(&self) -> bool {
        matches!(self, Self::Verified)
    }

    /// Whether this status means no proof was attempted, because there was
    /// nothing to prove.
    ///
    /// Distinct from a failure: the receipt is correct and the change simply
    /// contained no tests to transplant. CI gating treats this as satisfied by
    /// default, because failing a documentation-only pull request teaches
    /// operators to disable the check (ADR-0013).
    pub fn is_nothing_to_prove(&self) -> bool {
        matches!(self, Self::NoChangedTests)
    }

    /// How CI should treat this status, given the configured strictness.
    ///
    /// The policy lives here rather than in a workflow file so that every
    /// consumer — CI, MCP, a local script — reaches the same verdict.
    pub fn gate(&self, strict: bool, fail_on_no_changed_tests: bool) -> GateOutcome {
        if self.is_nothing_to_prove() {
            return if fail_on_no_changed_tests {
                GateOutcome::Failed
            } else {
                GateOutcome::NothingToProve
            };
        }
        let satisfied = if strict {
            self.is_strictly_verified()
        } else {
            self.is_verified()
        };
        if satisfied {
            GateOutcome::Satisfied
        } else {
            GateOutcome::Failed
        }
    }

    /// What the reader should do about this status.
    ///
    /// A diagnosis without a next step is what makes a verification tool get
    /// ignored. Every non-verified status gets a concrete action, and the
    /// verified ones say so plainly rather than inventing work.
    pub fn remediation(&self) -> &'static str {
        match self {
            Self::Verified => {
                "Nothing to do: the changed tests fail on the base revision and pass here."
            }
            Self::VerifiedWithWarnings => {
                "Read the integrity findings above. Each named rule describes something the change did to the tests."
            }
            Self::NotVerified => {
                "The tests do not yet prove this change. Usual causes: the test passes on the base revision too, so it does not pin the new behavior (add an assertion that fails without the change); or the base revision itself does not pass, so no failure can be attributed to the test."
            }
            Self::NoChangedTests => {
                "No dedicated test file changed, so no proof was attempted. This is not a failure. If the change should be covered, add or update a test; if it is a refactor or documentation change, nothing is needed."
            }
            Self::HeadFailed => {
                "The test command fails on the current workspace. Fix that first: WitDiff cannot attribute a base failure to a test that is already failing here."
            }
            Self::BaseIncompatible => {
                "The transplanted test does not compile against the base revision, so no behavioral proof is possible. Usually the test references an API this change adds. Split the change: land the production change first, then the test."
            }
        }
    }

    /// Whether this status indicates the change is fine but unproven, as
    /// opposed to something being wrong.
    ///
    /// Used to choose whether output should read as a failure. `not_verified`
    /// and `head_failed` are problems; `no_changed_tests` is not.
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            Self::NotVerified | Self::HeadFailed | Self::BaseIncompatible
        )
    }

    /// Stable machine-facing token. This must always agree with the value
    /// serialized into `witdiff.receipt.v1`; the schema enum is generated from
    /// the same set of names. Never print the Rust `Debug` representation of
    /// this enum in user-facing output, because it is not part of the contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::VerifiedWithWarnings => "verified_with_warnings",
            Self::NotVerified => "not_verified",
            Self::NoChangedTests => "no_changed_tests",
            Self::HeadFailed => "head_failed",
            Self::BaseIncompatible => "base_incompatible",
        }
    }
}

impl std::fmt::Display for VerificationStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub schema_version: String,
    pub generated_at: String,
    pub status: VerificationStatus,
    pub repo_root: String,
    pub base: String,
    pub head_commit: String,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub evidence_fresh: bool,
    pub changed_files: Vec<ChangedFile>,
    pub changed_test_files: Vec<String>,
    pub integrity_findings: Vec<IntegrityFinding>,
    pub head_run: RunResult,
    pub base_control_run: Option<RunResult>,
    pub base_run: Option<RunResult>,
    pub red_green_proven: bool,
    pub notes: Vec<String>,
    /// Which test command variant actually produced `head_run` and `base_run`.
    ///
    /// Additive in v1: a receipt written before this field existed deserializes
    /// with `full_suite`, which matches how those runs were performed.
    #[serde(default)]
    pub test_selection: TestSelection,
    /// The exact command used for the proof runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_test_command: Option<Vec<String>>,
    /// Inline `#[cfg(test)]` test modules transplanted by span splicing.
    ///
    /// Additive in v1: a receipt written before this field existed deserializes
    /// as empty, which is correct because nothing was spliced then. See
    /// ADR-0010.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spliced_inline_tests: Vec<SplicedInlineTests>,
    /// Production files that changed inline tests but were *not* spliced, with
    /// the reason. Reported so a reader can tell "WitDiff declined" from
    /// "WitDiff did not try".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused_inline_tests: Vec<RefusedInlineTests>,
    /// Changed-code mutation evidence, when enabled.
    ///
    /// Supplementary only: this field never affects `status` (ADR-0011).
    /// Additive in v1 and omitted when mutation is disabled, so a receipt
    /// written before this field existed still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<MutationReport>,
}

/// The outcome of running the suite against one mutant.
///
/// `NotCompiled`, `Timeout` and `Skipped` are deliberately distinct from
/// `Survived`: none of them is a decision about test strength, and collapsing
/// any of them into a kill or a survivor would misreport what was observed
/// (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutantOutcome {
    /// The suite failed with a recognized test failure.
    Killed,
    /// The suite passed: the tests did not notice the change.
    Survived,
    /// The mutated source did not build, so it was never executed.
    NotCompiled,
    /// The run exceeded the deadline. No proof path accepts this.
    Timeout,
    /// A bound was reached before this mutant ran.
    Skipped,
}

impl MutantOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            MutantOutcome::Killed => "killed",
            MutantOutcome::Survived => "survived",
            MutantOutcome::NotCompiled => "not_compiled",
            MutantOutcome::Timeout => "timeout",
            MutantOutcome::Skipped => "skipped",
        }
    }
}

/// One mutant and what the suite did about it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MutantResult {
    pub id: String,
    pub path: String,
    pub operator: String,
    pub line: usize,
    pub function: Option<String>,
    pub original: String,
    pub replacement: String,
    pub outcome: MutantOutcome,
    /// True when the outcome came from the cache rather than a fresh run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cached: bool,
}

/// Mutation evidence for a verification run.
///
/// Supplementary by construction: nothing in this struct can change
/// [`VerificationStatus`]. It reports what was decided, and how much of the
/// generated mutant set was actually decided, so a consumer is never left to
/// infer a score (ADR-0011).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MutationReport {
    /// Mutants generated for the changed production code.
    pub generated: usize,
    pub killed: usize,
    pub survived: usize,
    pub not_compiled: usize,
    pub timeout: usize,
    pub skipped: usize,
    /// Results for every mutant that was attempted, plus skipped placeholders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<MutantResult>,
    /// Why mutation did not cover everything it could have, when applicable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl MutationReport {
    /// Counts by outcome, for the summary line.
    pub fn count(&mut self, outcome: MutantOutcome) {
        match outcome {
            MutantOutcome::Killed => self.killed += 1,
            MutantOutcome::Survived => self.survived += 1,
            MutantOutcome::NotCompiled => self.not_compiled += 1,
            MutantOutcome::Timeout => self.timeout += 1,
            MutantOutcome::Skipped => self.skipped += 1,
        }
    }

    /// Mutants whose outcome is a decision about the tests.
    ///
    /// Excludes `not_compiled`, `timeout` and `skipped`, which are facts about
    /// the mutant or the bounds rather than about test strength.
    pub fn decided(&self) -> usize {
        self.killed + self.survived
    }
}

/// A file whose inline test module was transplanted onto the base revision.
///
/// The file exists in neither revision: its production code is the base
/// revision's and its test module is the head revision's. Recording that is the
/// difference between a reviewer understanding the experiment and being misled
/// by it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SplicedInlineTests {
    /// Repository-relative path.
    pub path: String,
    /// The `#[cfg(test)]` modules transplanted, as module paths.
    pub modules: Vec<String>,
}

/// A file that carries inline test changes that could not be transplanted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefusedInlineTests {
    /// Repository-relative path.
    pub path: String,
    /// Stable token naming the failed precondition, from ADR-0010.
    pub reason: String,
    /// Operator-facing explanation of the precondition that failed.
    pub explanation: String,
}

/// How much of the suite the proof runs actually exercised.
///
/// A narrowed run is a smaller claim than a full-suite run, so the receipt
/// records which one occurred instead of leaving a consumer to assume.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TestSelection {
    /// The configured command ran unchanged.
    #[default]
    FullSuite,
    /// The command was narrowed to cargo targets of the changed dedicated tests.
    Targeted,
}

impl TestSelection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FullSuite => "full_suite",
            Self::Targeted => "targeted",
        }
    }
}

impl std::fmt::Display for TestSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STATUSES: [VerificationStatus; 6] = [
        VerificationStatus::Verified,
        VerificationStatus::VerifiedWithWarnings,
        VerificationStatus::NotVerified,
        VerificationStatus::NoChangedTests,
        VerificationStatus::HeadFailed,
        VerificationStatus::BaseIncompatible,
    ];

    /// The human-facing token must be byte-identical to the JSON token, because
    /// consumers of the receipt and consumers of the console output are the same
    /// callers (agents, CI) and a divergence would be an undetectable contract break.
    #[test]
    fn display_token_matches_serialized_token() {
        for status in ALL_STATUSES {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(
                json,
                format!("\"{}\"", status.as_str()),
                "Display/as_str disagree with the serialized receipt value"
            );
        }
    }

    #[test]
    fn display_never_leaks_rust_debug_naming() {
        assert_eq!(
            VerificationStatus::VerifiedWithWarnings.to_string(),
            "verified_with_warnings"
        );
        assert_eq!(
            VerificationStatus::BaseIncompatible.to_string(),
            "base_incompatible"
        );
    }

    #[test]
    fn strict_mode_only_accepts_exact_verification() {
        assert!(VerificationStatus::Verified.is_strictly_verified());
        assert!(!VerificationStatus::VerifiedWithWarnings.is_strictly_verified());
        assert!(VerificationStatus::VerifiedWithWarnings.is_verified());
        assert!(!VerificationStatus::NotVerified.is_verified());
    }

    /// A receipt written by 0.1 carries none of the fields added in 1.0. It must
    /// still deserialize, and the missing fields must take their conservative
    /// defaults rather than silently claiming a narrower or cleaner run.
    ///
    /// This is the concrete guarantee behind "additive within v1": an agent or
    /// CI system that stored receipts keeps working across the upgrade.
    #[test]
    fn receipts_written_before_1_0_still_deserialize() {
        let legacy = r#"{
          "schema_version": "witdiff.receipt.v1",
          "generated_at": "2024-01-01T00:00:00Z",
          "status": "verified",
          "repo_root": "/repo",
          "base": "origin/main",
          "head_commit": "abc123",
          "workspace_fingerprint_before": "aa",
          "workspace_fingerprint_after": "aa",
          "evidence_fresh": true,
          "changed_files": [
            {"path": "tests/a.rs", "previous_path": null, "kind": "added",
             "is_test": true, "tracked": false}
          ],
          "changed_test_files": ["tests/a.rs"],
          "integrity_findings": [],
          "head_run": {
            "command": ["cargo", "test"], "cwd": "/repo", "success": true,
            "exit_code": 0, "duration_ms": 10, "stdout": "", "stderr": "",
            "failure_kind": null
          },
          "base_run": null,
          "red_green_proven": true,
          "notes": []
        }"#;

        let receipt: Receipt = serde_json::from_str(legacy).expect("a 0.1 receipt must parse");

        assert_eq!(receipt.status, VerificationStatus::Verified);
        assert_eq!(
            receipt.test_selection,
            TestSelection::FullSuite,
            "a receipt without the field must not claim a targeted run"
        );
        assert_eq!(
            receipt.effective_test_command, None,
            "an absent field must stay absent rather than being invented"
        );
        assert!(
            !receipt.changed_files[0].path_is_lossy && !receipt.changed_files[0].previous_is_test,
            "added path flags must default to the safe values"
        );
    }

    #[test]
    fn test_selection_tokens_match_the_schema() {
        assert_eq!(
            serde_json::to_string(&TestSelection::FullSuite).unwrap(),
            "\"full_suite\""
        );
        assert_eq!(
            serde_json::to_string(&TestSelection::Targeted).unwrap(),
            "\"targeted\""
        );
        assert_eq!(TestSelection::Targeted.to_string(), "targeted");
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// The change this ADR makes: a correct receipt with nothing to prove must
    /// not be reported as a gate failure.
    #[test]
    fn no_changed_tests_passes_by_default_but_is_labelled_truthfully() {
        let outcome = VerificationStatus::NoChangedTests.gate(true, false);
        assert_eq!(outcome, GateOutcome::NothingToProve);
        assert!(outcome.passes(), "nothing to prove must not fail the gate");
        assert_ne!(
            outcome,
            GateOutcome::Satisfied,
            "it must not be reported as a proof either"
        );
    }

    #[test]
    fn fail_on_no_changed_tests_restores_the_strict_behavior() {
        assert_eq!(
            VerificationStatus::NoChangedTests.gate(true, true),
            GateOutcome::Failed
        );
    }

    /// A proof was attempted and failed. This is the outcome the tool exists to
    /// report, so it must never pass.
    #[test]
    fn not_verified_always_fails() {
        assert_eq!(
            VerificationStatus::NotVerified.gate(false, false),
            GateOutcome::Failed
        );
        assert_eq!(
            VerificationStatus::NotVerified.gate(true, false),
            GateOutcome::Failed
        );
    }

    #[test]
    fn head_failed_always_fails() {
        assert_eq!(
            VerificationStatus::HeadFailed.gate(false, false),
            GateOutcome::Failed
        );
    }

    /// Undecidable is not acceptable as a gate: ADR-0003 exists precisely to
    /// flag this case rather than let it pass.
    #[test]
    fn base_incompatible_fails_as_undecidable() {
        assert_eq!(
            VerificationStatus::BaseIncompatible.gate(false, false),
            GateOutcome::Failed
        );
    }

    #[test]
    fn warnings_pass_unless_strict() {
        assert_eq!(
            VerificationStatus::VerifiedWithWarnings.gate(false, false),
            GateOutcome::Satisfied
        );
        assert_eq!(
            VerificationStatus::VerifiedWithWarnings.gate(true, false),
            GateOutcome::Failed
        );
    }

    #[test]
    fn verified_passes_in_both_modes() {
        assert_eq!(
            VerificationStatus::Verified.gate(false, false),
            GateOutcome::Satisfied
        );
        assert_eq!(
            VerificationStatus::Verified.gate(true, false),
            GateOutcome::Satisfied
        );
    }

    /// Every status must map to a defined outcome, so no case is accidental.
    #[test]
    fn every_status_has_a_defined_gate_outcome() {
        let statuses = [
            VerificationStatus::Verified,
            VerificationStatus::VerifiedWithWarnings,
            VerificationStatus::NotVerified,
            VerificationStatus::NoChangedTests,
            VerificationStatus::HeadFailed,
            VerificationStatus::BaseIncompatible,
        ];
        for status in statuses {
            let loose = status.gate(false, false);
            let strict = status.gate(true, false);
            // Strictness may only ever make the gate harder, never easier.
            if strict.passes() {
                assert!(
                    loose.passes(),
                    "strict mode must not be more permissive than default for {:?}",
                    status
                );
            }
        }
    }
}

#[cfg(test)]
mod remediation_tests {
    use super::*;

    /// Every status must tell the reader what to do. A diagnosis without a next
    /// step is what makes a verification tool get ignored.
    #[test]
    fn every_status_explains_the_next_step() {
        let statuses = [
            VerificationStatus::Verified,
            VerificationStatus::VerifiedWithWarnings,
            VerificationStatus::NotVerified,
            VerificationStatus::NoChangedTests,
            VerificationStatus::HeadFailed,
            VerificationStatus::BaseIncompatible,
        ];
        for status in statuses {
            let text = status.remediation();
            assert!(
                text.len() > 40,
                "{:?} needs a concrete recommendation, got {text:?}",
                status
            );
            assert!(
                text.ends_with('.'),
                "{:?} should read as a sentence, got {text:?}",
                status
            );
        }
    }

    /// The one that matters most in practice: the single most common
    /// AI-authored failure is a test that passes on the base revision too.
    #[test]
    fn not_verified_explains_the_most_common_cause() {
        let text = VerificationStatus::NotVerified.remediation();
        assert!(
            text.contains("passes on the base revision"),
            "the most common cause must be named, got {text:?}"
        );
        assert!(
            text.to_lowercase().contains("add an assertion"),
            "the reader needs a concrete action, got {text:?}"
        );
    }

    /// "Nothing to do" and "something is wrong" must be distinguishable, or
    /// output cannot be styled honestly.
    #[test]
    fn problems_are_distinguished_from_non_problems() {
        assert!(!VerificationStatus::Verified.is_problem());
        assert!(!VerificationStatus::NoChangedTests.is_problem());
        assert!(VerificationStatus::NotVerified.is_problem());
        assert!(VerificationStatus::HeadFailed.is_problem());
        assert!(VerificationStatus::BaseIncompatible.is_problem());
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;

    fn receipt_at(head: &str, fingerprint: &str) -> Receipt {
        // Only the fields the freshness check reads need to be meaningful; the
        // rest is filled from a minimal valid receipt so the test states its
        // intent rather than its boilerplate.
        serde_json::from_value(serde_json::json!({
            "schema_version": "witdiff.receipt.v1",
            "generated_at": "2024-01-01T00:00:00Z",
            "status": "verified",
            "repo_root": "/repo",
            "base": "HEAD~1",
            "head_commit": head,
            "workspace_fingerprint_before": fingerprint,
            "workspace_fingerprint_after": fingerprint,
            "evidence_fresh": true,
            "changed_files": [],
            "changed_test_files": [],
            "integrity_findings": [],
            "head_run": {
                "command": ["cargo", "test"], "cwd": "/repo", "success": true,
                "exit_code": 0, "duration_ms": 1, "stdout": "", "stderr": "",
                "failure_kind": null
            },
            "base_run": null,
            "red_green_proven": true,
            "notes": []
        }))
        .expect("the fixture must be a valid receipt")
    }

    #[test]
    fn an_unchanged_state_is_current() {
        let receipt = receipt_at("abc123", "fingerprint");
        assert_eq!(
            receipt.freshness("abc123", "fingerprint"),
            ReceiptFreshness::Current
        );
        assert!(receipt
            .freshness("abc123", "fingerprint")
            .warning()
            .is_none());
    }

    /// The common case: the change is edited after verification. `head_commit`
    /// is unchanged, so only the fingerprint catches it.
    #[test]
    fn an_uncommitted_edit_makes_a_receipt_stale() {
        let receipt = receipt_at("abc123", "fingerprint");
        let freshness = receipt.freshness("abc123", "different");
        assert!(!freshness.is_current());
        let warning = freshness.warning().expect("a stale receipt must warn");
        assert!(
            warning.contains("workspace has changed"),
            "the reason should name the workspace, got {warning}"
        );
    }

    /// A new commit after verification.
    #[test]
    fn a_new_commit_makes_a_receipt_stale() {
        let receipt = receipt_at("abc123def456", "fingerprint");
        let freshness = receipt.freshness("999888777666", "fingerprint");
        assert!(!freshness.is_current());
        let warning = freshness.warning().expect("a stale receipt must warn");
        assert!(
            warning.contains("HEAD is now"),
            "the reason should name the new head, got {warning}"
        );
    }

    /// Both signals are reported, because either alone can miss a change and a
    /// reader should see everything that moved.
    #[test]
    fn both_signals_are_reported() {
        let receipt = receipt_at("abc123", "fingerprint");
        let warning = receipt
            .freshness("different", "different")
            .warning()
            .expect("stale");
        assert!(warning.contains("HEAD is now"), "got {warning}");
        assert!(warning.contains("workspace has changed"), "got {warning}");
    }

    /// An unverifiable receipt must not be presented as though it were checked.
    #[test]
    fn unknown_is_not_current() {
        let unknown = ReceiptFreshness::Unknown {
            reason: "git is unavailable".to_owned(),
        };
        assert!(!unknown.is_current());
        assert!(unknown.warning().is_some());
    }

    /// Short hashes keep the message readable and must not panic on a short or
    /// empty value.
    #[test]
    fn short_hashes_are_safe() {
        assert_eq!(short("abcdefghijklmnop"), "abcdefghijkl");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }
}
