use serde::{Deserialize, Serialize};

use crate::environment::Environment;

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
    /// Whether this command's outcome was confirmed by repeating it.
    ///
    /// Additive in v1: a receipt written before this field existed deserializes
    /// as `single_run`, which is exactly how those runs were performed.
    #[serde(default = "single_run")]
    pub stability: crate::run::Stability,
    /// How many times the command actually ran.
    #[serde(default = "one_run")]
    pub repeats: usize,
}

fn single_run() -> crate::run::Stability {
    crate::run::Stability::SingleRun
}

fn one_run() -> usize {
    1
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
/// Why a verification reached its status.
///
/// One token per distinct cause, because the causes have different remedies and
/// a caller that cannot tell them apart cannot act. `not_verified` in
/// particular covers several unrelated situations: a test that proves nothing,
/// a base that never ran, a flaky experiment, and a blocked integrity finding.
///
/// This is a machine-facing contract: the tokens are stable and covered by the
/// receipt schema. The human-readable explanation stays in `notes`; this says
/// which one it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The proof succeeded.
    Proven,
    /// Proven, but the change weakened its own tests.
    ProvenWithIntegrityWarnings,
    /// The changed tests also pass on the base revision, so they distinguish
    /// nothing. The remedy is a stronger assertion.
    BaseAlsoPasses,
    /// The pristine base revision did not pass the configured command, so no
    /// later failure can be attributed to the changed tests. The remedy is to
    /// fix or re-target the base.
    BaseControlFailed,
    /// The base control run exceeded its timeout.
    BaseControlTimedOut,
    /// The transplanted test does not compile against the base revision,
    /// usually because it calls an API this change adds. The remedy is to split
    /// the change.
    TestDidNotCompileOnBase,
    /// A run in the experiment disagreed with itself, so nothing it observed is
    /// evidence. The remedy is to find the flake.
    ExperimentUnstable,
    /// Red/green behaviour was observed but a high-severity integrity finding
    /// blocks the verdict. The remedy is to address the finding.
    BlockedByIntegrityFinding,
    /// Red/green behaviour was observed from a partial transplant, which cannot
    /// speak for the files it excluded.
    PartialTransplant,
    /// The base-plus-tests run did not terminate.
    BaseExperimentTimedOut,
    /// The base-plus-tests run failed, but not in a way the configured
    /// framework's classifier recognises as a test failure. Usually a wrong
    /// `framework`, or a runner whose output vocabulary is unknown.
    UnrecognizedFailureKind,
    /// The test command fails on the current workspace already.
    HeadTestsFailed,
    /// The test command could not be started at all.
    TestCommandUnavailable,
    /// No dedicated test file changed, so nothing was attempted.
    NoDedicatedTestsChanged,
    /// A receipt written before this field existed.
    #[default]
    Unrecorded,
}

impl Reason {
    /// The stable token, matching the serialized form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proven => "proven",
            Self::ProvenWithIntegrityWarnings => "proven_with_integrity_warnings",
            Self::BaseAlsoPasses => "base_also_passes",
            Self::BaseControlFailed => "base_control_failed",
            Self::BaseControlTimedOut => "base_control_timed_out",
            Self::TestDidNotCompileOnBase => "test_did_not_compile_on_base",
            Self::ExperimentUnstable => "experiment_unstable",
            Self::BlockedByIntegrityFinding => "blocked_by_integrity_finding",
            Self::PartialTransplant => "partial_transplant",
            Self::BaseExperimentTimedOut => "base_experiment_timed_out",
            Self::UnrecognizedFailureKind => "unrecognized_failure_kind",
            Self::HeadTestsFailed => "head_tests_failed",
            Self::TestCommandUnavailable => "test_command_unavailable",
            Self::NoDedicatedTestsChanged => "no_dedicated_tests_changed",
            Self::Unrecorded => "unrecorded",
        }
    }

    /// Whether this reason means the change *could* be proven and simply is not
    /// yet, as opposed to a defect in the change or its tests.
    ///
    /// The distinction is what an agent needs most: a fixable absence of effort
    /// versus something actually wrong. `unrecognized_failure_kind` is the
    /// awkward one — it usually means the configuration, not the code, so it is
    /// reported as fixable but names the configuration as the likely cause.
    pub fn is_actionable_by_author(self) -> bool {
        matches!(self, Self::BaseAlsoPasses | Self::NoDedicatedTestsChanged)
    }
}

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
    /// `digest` is the digest of the current inputs, checked when the receipt
    /// carries one. Its absence is not a staleness reason on its own, so
    /// receipts written before the digest existed still work.
    pub fn freshness(
        &self,
        head_commit: &str,
        fingerprint: &str,
        digest: Option<&str>,
    ) -> ReceiptFreshness {
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
        // A digest mismatch is the strongest signal: it means the verified
        // inputs differ, even when the fingerprint happens to match. On a clean
        // tree the fingerprint is identical for any content, so this catches
        // what the other two checks cannot.
        if let (Some(recorded), Some(current)) = (self.verification_digest.as_deref(), digest) {
            if recorded != current {
                reasons.push(
                    "the verified inputs have changed since the receipt was written".to_owned(),
                );
            }
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

/// A detached signature and the digest it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptSignature {
    /// `ed25519`, recorded so a verifier knows what to check with.
    pub algorithm: String,
    /// Base64 signature over the domain-separated digest.
    pub value: String,
    /// The digest this signature covers.
    ///
    /// Stored so a consumer can check the signature against the value in the
    /// same receipt without recomputing it from the repository.
    pub digest: String,
}

/// Whether a detached signature is present and covers the recorded digest.
///
/// Only checks that the two agree. Verifying the cryptography requires the
/// public key, which WitDiff is deliberately not involved with.
pub fn signature_covers_digest(receipt: &Receipt) -> Option<bool> {
    let signature = receipt.signature.as_ref()?;
    let digest = receipt.verification_digest.as_deref()?;
    Some(signature.digest == digest)
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

/// Which results an organization will accept as a pass, committed once in
/// `witdiff.toml` rather than restated as flags on every invocation.
///
/// The set of questions is deliberately small: each is a decision the CLI
/// already offers as a flag, plus a signature requirement it did not offer at
/// all. It is *not* a free-form allow-list of statuses.
///
/// That restraint is the point. A list of acceptable statuses would let a
/// repository write `pass = ["not_verified"]` and defeat the whole tool by
/// configuration, which is exactly the review question "can an unknown or
/// unsupported state accidentally become `Verified`?" Policy decides how
/// strictly a verdict is judged, never which verdicts exist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatePolicy {
    /// Require exactly `verified`, refusing `verified_with_warnings`.
    pub strict: bool,
    /// Treat "nothing to prove" as a failure, for repositories that require a
    /// test change with every pull request.
    pub fail_on_no_changed_tests: bool,
    /// Require the receipt to carry a signature.
    ///
    /// Distinct from signing being *configured*: this makes an unsigned
    /// receipt a gate failure rather than a fact, which is what an operator who
    /// asked for signatures and did not get one wants.
    pub require_signature: bool,
}

impl GatePolicy {
    /// Fold command-line flags in.
    ///
    /// Flags may only **tighten** a committed policy, never relax it: a policy
    /// checked into the repository is a floor, so `--strict` can raise it and
    /// the absence of `--strict` cannot lower it. Otherwise every developer
    /// could quietly undo what CI enforces.
    pub fn tightened_by(mut self, strict: bool, fail_on_no_changed_tests: bool) -> Self {
        self.strict |= strict;
        self.fail_on_no_changed_tests |= fail_on_no_changed_tests;
        self
    }

    /// Whether this policy asks for anything the defaults do not.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

impl Receipt {
    /// Evaluate this receipt against a gate policy.
    ///
    /// This is the whole policy surface: the status verdict the CLI already
    /// computes, plus whatever else the policy demands. Keeping it on the
    /// receipt rather than the status is what makes a requirement about the
    /// *document* — a signature — expressible in the same place as a
    /// requirement about the *verdict*.
    pub fn gate(&self, policy: &GatePolicy) -> GateOutcome {
        if policy.require_signature && self.signature.is_none() {
            return GateOutcome::Failed;
        }
        self.status
            .gate(policy.strict, policy.fail_on_no_changed_tests)
    }

    /// Why this receipt fails the policy, or `None` when it does not fail one.
    ///
    /// Separate from [`Receipt::gate`] so a caller can print the reason while
    /// `GateOutcome` stays the three-valued type every consumer already
    /// pattern-matches on. Returns `Some` only for policy demands, never for a
    /// status verdict, because the status has its own explanation already.
    pub fn gate_reason(&self, policy: &GatePolicy) -> Option<String> {
        if policy.require_signature && self.signature.is_none() {
            return Some(
                "the gate requires a signed receipt, but this receipt has no signature".to_owned(),
            );
        }
        None
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
            // Nothing was run, so there is no repeat to compare.
            stability: crate::run::Stability::SingleRun,
            repeats: 0,
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
    /// Why the verification reached this status, as a stable token.
    ///
    /// `status` says *what* happened; this says *why*, and it exists because
    /// `not_verified` alone covers four different causes with four different
    /// remedies. An agent reading the receipt had to string-match the prose in
    /// `notes` to decide what to do, which is exactly the kind of
    /// interpretation this project exists to remove — and a reworded note would
    /// silently change the meaning.
    ///
    /// Additive in v1: a receipt written before this field existed deserializes
    /// as [`Reason::Unrecorded`], which claims nothing about why.
    #[serde(default)]
    pub reason: Reason,
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
    /// A digest over the material facts of this verification: the base and head
    /// revisions, the effective test command, and the content of the changed
    /// test files and base production source.
    ///
    /// Distinct from `workspace_fingerprint`, which detects movement during a
    /// run and is SHA-256 of the empty string on a clean tree. This identifies
    /// *which* code was verified, which is what a signature needs to cover
    /// (ADR-0015). Additive in v1 and optional, so receipts written before it
    /// existed still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_digest: Option<String>,
    /// A detached Ed25519 signature over `verification_digest`.
    ///
    /// Proves the receipt has not been edited since the run, and that the
    /// digest matches the code verified. It does **not** prove who produced the
    /// receipt: that is tamper-evidence, not non-repudiation (ADR-0022).
    ///
    /// Additive in v1 and optional. A receipt without one is unsigned, which is
    /// a fact and not a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ReceiptSignature>,
    /// Changed-code mutation evidence, when enabled.
    ///
    /// Supplementary only: this field never affects `status` (ADR-0011).
    /// Additive in v1 and omitted when mutation is disabled, so a receipt
    /// written before this field existed still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<MutationReport>,
    /// Coverage of the production lines this change added.
    ///
    /// Supplementary only: this field never affects `status`, for the same
    /// reason mutation does not (ADR-0011). An uncovered line is a question
    /// about test strength, not a judgment about correctness.
    ///
    /// Additive in v1 and omitted when coverage is disabled, so a receipt
    /// written before this field existed still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<crate::coverage::CoverageReport>,
    /// The environment the evidence was produced under: toolchain versions and
    /// the digests of the dependency manifests that pinned them.
    ///
    /// The digest answers *what* was verified; this answers *where*. A proof
    /// that holds under Python 3.9 and pytest 8.4 is a different claim than the
    /// same revision under Python 3.12 and pytest 9, and the receipt could
    /// previously not tell them apart.
    ///
    /// Deliberately **not** folded into `verification_digest`: that digest is
    /// recomputed by `witdiff receipt` against a stored receipt, and folding
    /// the environment in would make every older receipt report that its inputs
    /// changed the moment Python was upgraded. Deliberately not signed either,
    /// for the same reason the other receipt fields are not: ADR-0022 keeps
    /// signed bytes to the status and the digest so that adding a field cannot
    /// invalidate an existing signature. This is descriptive evidence for a
    /// reader and carries no authority on its own.
    ///
    /// Additive in v1, so receipts written before it existed parse as empty.
    #[serde(default)]
    pub environment: Environment,
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
        assert!(
            receipt.environment.is_empty(),
            "a receipt written before the environment field existed must not \
             invent one; an invented toolchain would be evidence nobody collected"
        );
    }

    /// The environment must round-trip, or the field disappears on the way to
    /// a consumer that re-serializes it.
    #[test]
    fn an_environment_survives_a_json_round_trip() {
        // Built from JSON rather than through a mutator, so the test also
        // covers the shape a consumer will actually send back.
        let environment: crate::environment::Environment =
            serde_json::from_str(r#"{"test_program":"cargo","manifest_Cargo.lock":"8268d187"}"#)
                .expect("deserialize");

        let text = serde_json::to_string(&environment).expect("serialize");
        let back: crate::environment::Environment =
            serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, environment);
        assert_eq!(back.get("test_program"), Some("cargo"));
        assert_eq!(back.get("manifest_Cargo.lock"), Some("8268d187"));
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
mod gate_policy_tests {
    use super::*;

    /// A minimal receipt with the status under test.
    fn receipt_with_status(status: VerificationStatus) -> Receipt {
        serde_json::from_value(serde_json::json!({
            "schema_version": "witdiff.receipt.v1",
            "generated_at": "2024-01-01T00:00:00Z",
            "status": status.as_str(),
            "repo_root": "/repo",
            "base": "HEAD~1",
            "head_commit": "abc123",
            "workspace_fingerprint_before": "aa",
            "workspace_fingerprint_after": "aa",
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

    fn signed(mut receipt: Receipt) -> Receipt {
        receipt.signature = Some(ReceiptSignature {
            algorithm: "ed25519".to_owned(),
            value: "AA".to_owned(),
            digest: "digest".to_owned(),
        });
        receipt
    }

    /// The property that makes the policy safe to commit: a flag can raise a
    /// committed policy but can never lower it. Without this, every developer
    /// could simply omit `--strict` and undo what CI enforces.
    #[test]
    fn flags_can_only_tighten_a_committed_policy() {
        let committed = GatePolicy {
            strict: true,
            fail_on_no_changed_tests: true,
            require_signature: true,
        };

        // No flags at all: nothing relaxes.
        let unchanged = committed.clone().tightened_by(false, false);
        assert_eq!(unchanged, committed);

        // Both flags raise: nothing to gain, but nothing lost either.
        let raised = committed.clone().tightened_by(true, true);
        assert_eq!(raised, committed);
        assert!(raised.strict && raised.fail_on_no_changed_tests);

        // A flag cannot be used to opt out of something already set.
        assert!(
            GatePolicy {
                strict: true,
                ..GatePolicy::default()
            }
            .tightened_by(false, false)
            .strict,
            "leaving --strict off must not unset a committed strict policy"
        );
    }

    /// The inverse direction is what a flag is for.
    #[test]
    fn a_flag_tightens_a_default_policy() {
        let policy = GatePolicy::default().tightened_by(true, false);
        assert!(policy.strict);
        assert!(!policy.fail_on_no_changed_tests);
        assert!(
            !policy.require_signature,
            "a flag cannot set unrelated demands"
        );
    }

    /// With no policy configured, behaviour is exactly what the CLI did before
    /// the policy existed. This is the regression guard for every consumer.
    #[test]
    fn a_default_policy_matches_the_status_gate() {
        for status in [
            VerificationStatus::Verified,
            VerificationStatus::VerifiedWithWarnings,
            VerificationStatus::NotVerified,
            VerificationStatus::NoChangedTests,
            VerificationStatus::HeadFailed,
            VerificationStatus::BaseIncompatible,
        ] {
            let receipt = receipt_with_status(status.clone());
            let policy = GatePolicy::default();
            assert_eq!(
                receipt.gate(&policy),
                status.gate(false, false),
                "a default policy must not change the verdict for {status:?}"
            );
        }
    }

    /// Strictness still behaves as documented under the new entry point.
    #[test]
    fn a_policy_passes_strict_as_strictly_verified() {
        let strict = GatePolicy {
            strict: true,
            ..GatePolicy::default()
        };
        assert!(receipt_with_status(VerificationStatus::Verified)
            .gate(&strict)
            .passes());
        assert!(
            !receipt_with_status(VerificationStatus::VerifiedWithWarnings)
                .gate(&strict)
                .passes(),
            "strict mode must refuse warnings"
        );
    }

    /// `require_signature` is the one requirement about the document rather
    /// than the verdict, which is why it lives on the receipt and not on
    /// `VerificationStatus`.
    #[test]
    fn a_policy_can_require_a_signature() {
        let policy = GatePolicy {
            require_signature: true,
            ..GatePolicy::default()
        };

        let unsigned = receipt_with_status(VerificationStatus::Verified);
        assert!(
            !unsigned.gate(&policy).passes(),
            "a policy demanding a signature must fail an unsigned receipt, even \
             when the verdict is otherwise proven"
        );
        assert_eq!(
            unsigned.gate_reason(&policy).as_deref(),
            Some("the gate requires a signed receipt, but this receipt has no signature")
        );

        let signed = signed(receipt_with_status(VerificationStatus::Verified));
        assert!(signed.gate(&policy).passes());
        assert_eq!(signed.gate_reason(&policy), None);

        // The requirement never manufactures a pass: an unsigned, unproven
        // receipt fails for both reasons and says why only about the policy.
        let failing = receipt_with_status(VerificationStatus::NotVerified);
        assert!(!failing.gate(&policy).passes());
    }

    /// The interaction between "nothing to prove" and a signature requirement
    /// is not obvious, so it is asserted rather than assumed. Applying the
    /// requirement uniformly is the honest reading: a receipt is still produced
    /// for a change with no tests, and an operator who demands signatures
    /// demands them on every receipt.
    #[test]
    fn a_signature_requirement_applies_even_when_there_is_nothing_to_prove() {
        let policy = GatePolicy {
            require_signature: true,
            ..GatePolicy::default()
        };
        let receipt = receipt_with_status(VerificationStatus::NoChangedTests);
        assert!(
            !receipt.gate(&policy).passes(),
            "the policy is a demand about the receipt, not about the verdict"
        );

        // Without that demand, the default still passes, so ADR-0013's
        // documentation-only pull request is unaffected.
        assert!(receipt.gate(&GatePolicy::default()).passes());
    }

    #[test]
    fn a_default_policy_is_reportable_as_default() {
        assert!(GatePolicy::default().is_default());
        assert!(!GatePolicy {
            strict: true,
            ..GatePolicy::default()
        }
        .is_default());
    }

    #[test]
    fn a_policy_round_trips_through_serde() {
        let policy = GatePolicy {
            strict: true,
            fail_on_no_changed_tests: false,
            require_signature: true,
        };
        let text = serde_json::to_string(&policy).expect("serialize");
        let back: GatePolicy = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, policy);
    }

    /// A partial `[gate]` section must load with the rest defaulted, or an
    /// operator could set only `strict` and silently disable the other fields.
    #[test]
    fn a_partial_policy_loads_with_the_rest_defaulted() {
        let policy: GatePolicy = toml::from_str("strict = true").expect("load");
        assert!(policy.strict);
        assert!(!policy.fail_on_no_changed_tests);
        assert!(!policy.require_signature);
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
            receipt.freshness("abc123", "fingerprint", None),
            ReceiptFreshness::Current
        );
        assert!(receipt
            .freshness("abc123", "fingerprint", None)
            .warning()
            .is_none());
    }

    /// The common case: the change is edited after verification. `head_commit`
    /// is unchanged, so only the fingerprint catches it.
    #[test]
    fn an_uncommitted_edit_makes_a_receipt_stale() {
        let receipt = receipt_at("abc123", "fingerprint");
        let freshness = receipt.freshness("abc123", "different", None);
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
        let freshness = receipt.freshness("999888777666", "fingerprint", None);
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
            .freshness("different", "different", None)
            .warning()
            .expect("stale");
        assert!(warning.contains("HEAD is now"), "got {warning}");
        assert!(warning.contains("workspace has changed"), "got {warning}");
    }

    /// The digest catches what the fingerprint cannot. On a clean tree the
    /// fingerprint is identical for any content, so without this check a
    /// receipt for one revision would look current in a repository containing
    /// entirely different code — the finding that motivated ADR-0015.
    #[test]
    fn a_different_digest_makes_a_receipt_stale() {
        let mut receipt = receipt_at("abc123", "fingerprint");
        receipt.verification_digest = Some("digest-one".to_owned());

        // Same head, same fingerprint, different content.
        let freshness = receipt.freshness("abc123", "fingerprint", Some("digest-two"));
        assert!(!freshness.is_current());
        let warning = freshness.warning().expect("stale");
        assert!(
            warning.contains("verified inputs have changed"),
            "the reason should name the inputs, got {warning}"
        );
    }

    #[test]
    fn a_matching_digest_stays_current() {
        let mut receipt = receipt_at("abc123", "fingerprint");
        receipt.verification_digest = Some("digest-one".to_owned());
        assert_eq!(
            receipt.freshness("abc123", "fingerprint", Some("digest-one")),
            ReceiptFreshness::Current
        );
    }

    /// A receipt written before the digest existed must still work: its absence
    /// is not a staleness reason on its own.
    #[test]
    fn a_receipt_without_a_digest_is_not_stale_by_absence() {
        let receipt = receipt_at("abc123", "fingerprint");
        assert!(receipt.verification_digest.is_none());
        assert_eq!(
            receipt.freshness("abc123", "fingerprint", Some("anything")),
            ReceiptFreshness::Current,
            "an old receipt must not be reported stale merely for lacking a digest"
        );
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

    /// Every reason has a stable token, and every token round-trips.
    ///
    /// The tokens are a machine contract: an agent branches on them. A typo or
    /// a renamed variant would silently change what a caller does, so the
    /// serialized form is asserted rather than assumed.
    #[test]
    fn reason_tokens_round_trip() {
        let all = [
            Reason::Proven,
            Reason::ProvenWithIntegrityWarnings,
            Reason::BaseAlsoPasses,
            Reason::BaseControlFailed,
            Reason::BaseControlTimedOut,
            Reason::TestDidNotCompileOnBase,
            Reason::ExperimentUnstable,
            Reason::BlockedByIntegrityFinding,
            Reason::PartialTransplant,
            Reason::BaseExperimentTimedOut,
            Reason::UnrecognizedFailureKind,
            Reason::HeadTestsFailed,
            Reason::TestCommandUnavailable,
            Reason::NoDedicatedTestsChanged,
            Reason::Unrecorded,
        ];
        for reason in all {
            let token = reason.as_str();
            let json = serde_json::to_string(&reason).expect("serialize");
            assert_eq!(
                json,
                format!("\"{token}\""),
                "the token and the serialized form must agree for {reason:?}"
            );
            let back: Reason = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, reason);
        }
    }

    /// A receipt written before this field existed must still parse, and must
    /// claim nothing about why.
    #[test]
    fn a_receipt_without_a_reason_deserializes_as_unrecorded() {
        let json = r#"{"schema_version":"witdiff.receipt.v1","generated_at":"x",
            "status":"not_verified","repo_root":"/tmp","base":"HEAD~1","head_commit":"abc",
            "workspace_fingerprint_before":"a","workspace_fingerprint_after":"a",
            "evidence_fresh":true,"changed_files":[],"changed_test_files":[],
            "integrity_findings":[],"red_green_proven":false,"notes":[],
            "head_run":{"command":[],"cwd":"/tmp","success":true,"exit_code":0,
            "duration_ms":0,"stdout":"","stderr":"","failure_kind":null,
            "timed_out":false,"stability":"single_run","repeats":1}}"#;
        let receipt: Receipt = serde_json::from_str(json).expect("legacy receipt parses");
        assert_eq!(reason_of(&receipt), Reason::Unrecorded);
    }

    fn reason_of(receipt: &Receipt) -> Reason {
        receipt.reason
    }

    /// The tokens must not collide, or two causes become indistinguishable.
    #[test]
    fn reason_tokens_are_distinct() {
        let tokens = [
            Reason::Proven.as_str(),
            Reason::ProvenWithIntegrityWarnings.as_str(),
            Reason::BaseAlsoPasses.as_str(),
            Reason::BaseControlFailed.as_str(),
            Reason::BaseControlTimedOut.as_str(),
            Reason::TestDidNotCompileOnBase.as_str(),
            Reason::ExperimentUnstable.as_str(),
            Reason::BlockedByIntegrityFinding.as_str(),
            Reason::PartialTransplant.as_str(),
            Reason::BaseExperimentTimedOut.as_str(),
            Reason::UnrecognizedFailureKind.as_str(),
            Reason::HeadTestsFailed.as_str(),
            Reason::TestCommandUnavailable.as_str(),
            Reason::NoDedicatedTestsChanged.as_str(),
            Reason::Unrecorded.as_str(),
        ];
        let unique: std::collections::BTreeSet<_> = tokens.iter().collect();
        assert_eq!(unique.len(), tokens.len(), "reason tokens must be unique");
    }

    /// The schema's `reason` enum and the Rust enum must not drift.
    ///
    /// Two hand-maintained lists that describe the same set will diverge, and
    /// the failure is silent: a client validating against the schema would
    /// reject a token the tool actually emits. The schema is read here and
    /// every one of its tokens must deserialize into a real variant.
    #[test]
    fn the_schema_reason_enum_matches_the_rust_enum() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|crates| crates.parent())
            .map(|root| root.join("schemas/witdiff.receipt.v1.schema.json"))
            .expect("schema path");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let schema: serde_json::Value = serde_json::from_str(&text).expect("schema is JSON");
        let tokens: Vec<String> = schema["properties"]["reason"]["enum"]
            .as_array()
            .expect("reason enum")
            .iter()
            .map(|v| v.as_str().expect("a string token").to_owned())
            .collect();

        for token in &tokens {
            let parsed: Reason = serde_json::from_value(serde_json::Value::String(token.clone()))
                .unwrap_or_else(|error| {
                    panic!(
                        "the schema names `{token}`, which the Rust \
                        enum cannot deserialize: {error}"
                    )
                });
            assert_eq!(
                parsed.as_str(),
                token,
                "token and serialized form must agree"
            );
        }

        // And the reverse: every variant must appear in the schema, or a real
        // token would fail a client's validation.
        for reason in [
            Reason::Proven,
            Reason::ProvenWithIntegrityWarnings,
            Reason::BaseAlsoPasses,
            Reason::BaseControlFailed,
            Reason::BaseControlTimedOut,
            Reason::TestDidNotCompileOnBase,
            Reason::ExperimentUnstable,
            Reason::BlockedByIntegrityFinding,
            Reason::PartialTransplant,
            Reason::BaseExperimentTimedOut,
            Reason::UnrecognizedFailureKind,
            Reason::HeadTestsFailed,
            Reason::TestCommandUnavailable,
            Reason::NoDedicatedTestsChanged,
            Reason::Unrecorded,
        ] {
            assert!(
                tokens.contains(&reason.as_str().to_owned()),
                "`{}` is emitted by the tool but missing from the schema",
                reason.as_str()
            );
        }
    }

    /// Additive in v1: `reason` must not be required, or every receipt written
    /// before it existed fails validation.
    #[test]
    fn the_schema_does_not_require_reason() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|crates| crates.parent())
            .map(|root| root.join("schemas/witdiff.receipt.v1.schema.json"))
            .expect("schema path");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("JSON");
        let required: Vec<String> = schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_owned())
            .collect();
        assert!(
            !required.contains(&"reason".to_owned()),
            "adding `reason` to `required` would break every older receipt"
        );
    }
}
