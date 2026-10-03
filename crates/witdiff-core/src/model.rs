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

impl VerificationStatus {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified | Self::VerifiedWithWarnings)
    }

    pub fn is_strictly_verified(&self) -> bool {
        matches!(self, Self::Verified)
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
}
