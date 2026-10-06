//! Integrity-rule policy: how a *finding* is weighted, per repository.
//!
//! See ADR-0026. This is PG-202, and it is deliberately narrow.
//!
//! ## What it may and may not do
//!
//! An override changes **how a finding is reported**, never **whether the
//! observation happened**. That distinction is the whole design, and it is
//! enforced rather than documented:
//!
//! - the finding is still computed, still in `integrity_findings`, still
//!   visible in the receipt, and still printed by `witdiff verify`;
//! - only its effect on the *verdict* changes;
//! - every overridden finding is recorded in a separate `waivers` list naming
//!   the rule and why, so a reader can always see that a rule was silenced and
//!   by which policy.
//!
//! A mechanism that removed a finding from the receipt would let a repository
//! hide a weakening from its own reviewers, which is the failure mode the tool
//! exists to catch. Silencing the *gate* is a legitimate policy decision;
//! silencing the *observation* is not, so it is not offered.
//!
//! ## Why `ignore` needs a reason
//!
//! Every override must state why. An unexplained suppression is indistinguishable
//! from an accident six months later, and the reason is what a reviewer reads
//! when the policy is next audited. The reason is required by the type, not by
//! convention.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::Severity;

/// How a rule's findings should be weighted for this repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weight {
    /// Findings from this rule block verification, as `High` does today.
    Block,
    /// Findings are reported and do not block.
    Warn,
    /// Findings are reported and do not block, and the receipt records a waiver.
    ///
    /// Identical to `Warn` in effect. It exists as a separate name because the
    /// intent differs: `warn` is "this rule is advisory here", `ignore` is
    /// "this rule fired and we have accepted it". The receipt distinguishes
    /// them so an auditor can find the accepted ones.
    Ignore,
}

/// One rule override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleOverride {
    pub rule: String,
    pub weight: Weight,
    /// Why this override exists. Required: an unexplained suppression cannot be
    /// audited.
    pub reason: String,
}

/// A path pattern whose findings are waived, with the rule set it applies to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Per-rule weight overrides.
    ///
    /// A list rather than a map so the reason travels with the rule, and so an
    /// operator can read the policy top to bottom in `witdiff.toml`.
    #[serde(default)]
    pub rules: Vec<RuleOverride>,
    /// Files whose findings are waived entirely.
    ///
    /// Matched as a prefix against the repository-relative path, so
    /// `vendor/` covers everything beneath it. Applied after the per-rule
    /// overrides, and recorded as a waiver.
    #[serde(default)]
    pub ignore_paths: Vec<IgnoredPath>,
}

/// A path whose findings are waived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredPath {
    pub path: String,
    pub reason: String,
}

/// A finding that was observed but whose effect on the verdict was waived.
///
/// Recorded in the receipt. Without this, a reader could not tell a repository
/// with no findings from one that had silenced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    /// The rule whose finding was waived.
    pub rule: String,
    /// The file the finding was in.
    pub path: String,
    /// Why, quoted from the policy.
    pub reason: String,
    /// Whether the waiver came from a rule override or a path ignore.
    pub source: WaiverSource,
}

/// Where a waiver came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiverSource {
    Rule,
    Path,
}

impl PolicyConfig {
    /// Whether any policy is configured at all.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.ignore_paths.is_empty()
    }

    /// The weight configured for a rule, if any.
    fn weight_for(&self, rule: &str) -> Option<&RuleOverride> {
        self.rules.iter().find(|entry| entry.rule == rule)
    }

    /// The path ignore covering a file, if any.
    fn ignore_for(&self, path: &str) -> Option<&IgnoredPath> {
        self.ignore_paths.iter().find(|entry| {
            path == entry.path
                || path.starts_with(&format!("{}/", entry.path.trim_end_matches('/')))
        })
    }

    /// Whether a finding from `rule` in `path` blocks verification.
    ///
    /// Default is unchanged: a `High` finding blocks. A policy may only make a
    /// rule *less* blocking, never more, because the tool has no notion of a
    /// rule that should block when it otherwise would not — `High` already
    /// means the strongest signal the analyzer produces.
    pub fn blocks(&self, rule: &str, path: &str, severity: Severity) -> bool {
        if self.ignore_for(path).is_some() {
            return false;
        }
        match self.weight_for(rule).map(|entry| entry.weight) {
            Some(Weight::Block) => true,
            Some(Weight::Warn | Weight::Ignore) => false,
            None => severity == Severity::High,
        }
    }

    /// The waivers this policy applies to a set of findings.
    ///
    /// Recorded rather than discarded: the receipt must show that a rule was
    /// observed and waived, or a review cannot tell the difference between a
    /// clean run and a silenced one.
    pub fn waivers(&self, findings: &[(String, String, Severity)]) -> Vec<Waiver> {
        let mut out = Vec::new();
        for (rule, path, severity) in findings {
            if let Some(ignore) = self.ignore_for(path) {
                out.push(Waiver {
                    rule: rule.clone(),
                    path: path.clone(),
                    reason: ignore.reason.clone(),
                    source: WaiverSource::Path,
                });
                continue;
            }
            if let Some(entry) = self.weight_for(rule) {
                // Only a waiver if it actually changed the outcome. A rule
                // configured `warn` that produced a `Warning` was never
                // blocking, so nothing was waived and recording it would be
                // noise.
                if entry.weight != Weight::Block && *severity == Severity::High {
                    out.push(Waiver {
                        rule: rule.clone(),
                        path: path.clone(),
                        reason: entry.reason.clone(),
                        source: WaiverSource::Rule,
                    });
                }
            }
        }
        out
    }

    /// Rule names known to be overridden, for diagnostics.
    pub fn overridden_rules(&self) -> BTreeMap<&str, Weight> {
        self.rules
            .iter()
            .map(|entry| (entry.rule.as_str(), entry.weight))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PolicyConfig {
        PolicyConfig {
            rules: vec![RuleOverride {
                rule: "assertion_free_test".into(),
                weight: Weight::Warn,
                reason: "legacy suite; these are tracked in JIRA-1234".into(),
            }],
            ignore_paths: vec![IgnoredPath {
                path: "vendor".into(),
                reason: "generated dependency code, not ours".into(),
            }],
        }
    }

    #[test]
    fn with_no_policy_a_high_finding_blocks() {
        let empty = PolicyConfig::default();
        assert!(empty.blocks("removed_assertion", "tests/a.rs", Severity::High));
        assert!(!empty.blocks("removed_assertion", "tests/a.rs", Severity::Warning));
        assert!(!empty.blocks("removed_assertion", "tests/a.rs", Severity::Info));
    }

    #[test]
    fn a_warn_override_stops_a_high_finding_from_blocking() {
        let policy = policy();
        assert!(!policy.blocks("assertion_free_test", "tests/a.rs", Severity::High));
    }

    /// The override is per-rule. Silencing one must never silence another.
    #[test]
    fn an_override_does_not_leak_to_other_rules() {
        let policy = policy();
        assert!(
            policy.blocks("removed_assertion", "tests/a.rs", Severity::High),
            "only the named rule may be affected"
        );
    }

    #[test]
    fn a_path_ignore_covers_everything_beneath_it() {
        let policy = policy();
        assert!(!policy.blocks("removed_assertion", "vendor/acme/lib.rs", Severity::High));
        assert!(!policy.blocks("removed_assertion", "vendor", Severity::High));
        // A sibling with the same prefix is a different path.
        assert!(policy.blocks("removed_assertion", "vendored/a.rs", Severity::High));
    }

    /// `Block` exists so a policy can be explicit about a rule it considers
    /// blocking, and it must still block a `Warning`.
    #[test]
    fn an_explicit_block_override_blocks() {
        let policy = PolicyConfig {
            rules: vec![RuleOverride {
                rule: "trivial_assertion".into(),
                weight: Weight::Block,
                reason: "this repository requires real assertions".into(),
            }],
            ignore_paths: Vec::new(),
        };
        assert!(policy.blocks("trivial_assertion", "tests/a.rs", Severity::Warning));
    }

    /// The waiver record is what stops a silenced rule from being invisible.
    #[test]
    fn a_waived_high_finding_is_recorded_with_its_reason() {
        let policy = policy();
        let waivers = policy.waivers(&[(
            "assertion_free_test".into(),
            "tests/a.rs".into(),
            Severity::High,
        )]);
        assert_eq!(waivers.len(), 1);
        assert_eq!(waivers[0].rule, "assertion_free_test");
        assert_eq!(waivers[0].source, WaiverSource::Rule);
        assert!(waivers[0].reason.contains("JIRA-1234"), "{:?}", waivers[0]);
    }

    #[test]
    fn a_waived_path_finding_names_the_path_reason() {
        let policy = policy();
        let waivers = policy.waivers(&[(
            "removed_assertion".into(),
            "vendor/acme/lib.rs".into(),
            Severity::High,
        )]);
        assert_eq!(waivers.len(), 1);
        assert_eq!(waivers[0].source, WaiverSource::Path);
        assert!(waivers[0].reason.contains("generated"), "{:?}", waivers[0]);
    }

    /// A `warn` override on a rule that produced a `Warning` waived nothing:
    /// it was never blocking. Recording it would train a reader to ignore the
    /// waiver list.
    #[test]
    fn an_override_that_changed_nothing_is_not_a_waiver() {
        let policy = policy();
        assert!(policy
            .waivers(&[(
                "assertion_free_test".into(),
                "tests/a.rs".into(),
                Severity::Warning
            )])
            .is_empty());
    }

    #[test]
    fn the_reason_is_part_of_the_serialized_policy() {
        let text = toml::to_string(&policy()).expect("serialize");
        assert!(text.contains("reason"), "{text}");
        assert!(text.contains("JIRA-1234"), "{text}");
    }

    #[test]
    fn an_empty_policy_serializes_to_nothing_meaningful() {
        let text = toml::to_string(&PolicyConfig::default()).expect("serialize");
        let back: PolicyConfig = toml::from_str(&text).expect("round trip");
        assert!(back.is_empty());
    }

    /// The policy section parses from the same TOML a user writes.
    ///
    /// Asserted through `Config`, not `PolicyConfig`, so the section name and
    /// its nesting are covered: a policy that parsed in isolation but sat under
    /// the wrong key would never take effect.
    #[test]
    fn the_policy_section_parses_from_config() {
        use crate::config::Config;
        let text = "[project]\nlanguage = \"rust\"\n\n[policy]\nrules = [\n  \
                    { rule = \"assertion_free_test\", weight = \"warn\", reason = \"legacy\" },\n]\n\
                    ignore_paths = [\n  { path = \"vendor\", reason = \"not ours\" },\n]\n";
        let config: Config = toml::from_str(text).expect("the policy section must parse");
        assert_eq!(config.policy.rules.len(), 1);
        assert_eq!(
            config.policy.rules[0].weight,
            Weight::Warn,
            "the weight must survive parsing"
        );
        assert_eq!(config.policy.ignore_paths.len(), 1);
        assert!(!config.policy.is_empty());
    }

    /// A repository with no policy behaves exactly as before.
    #[test]
    fn a_config_without_a_policy_section_is_unchanged() {
        use crate::config::Config;
        let config: Config =
            toml::from_str("[verification]\ntest_command = [\"cargo\", \"test\"]\n")
                .expect("parse");
        assert!(config.policy.is_empty());
        assert!(
            config
                .policy
                .blocks("removed_assertion", "tests/a.rs", Severity::High),
            "with no policy a high finding must still block"
        );
    }

    /// An override without a reason must not parse. The reason is what an
    /// auditor reads; making it optional would let it be omitted in practice.
    #[test]
    fn an_override_without_a_reason_is_rejected() {
        use crate::config::Config;
        let text = "[policy]\nrules = [{ rule = \"removed_assertion\", weight = \"warn\" }]\n";
        let parsed: Result<Config, _> = toml::from_str(text);
        assert!(
            parsed.is_err(),
            "an unexplained suppression must not be expressible"
        );
    }

    /// An unknown weight must not silently become the default.
    #[test]
    fn an_unknown_weight_is_rejected() {
        use crate::config::Config;
        let text = "[policy]\nrules = [{ rule = \"x\", weight = \"maybe\", reason = \"r\" }]\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }
}
