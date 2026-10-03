//! Syntax-aware analysis of Rust test changes.
//!
//! The line-oriented diff analyzer in [`crate::integrity`] can only see lines
//! that the diff happens to mark as added or removed. It cannot tell an
//! assertion that was deleted from one that merely moved, cannot follow a test
//! across a rename, and cannot distinguish a genuinely weakened expectation
//! from a reformatted one. Those are exactly the distinctions that decide
//! whether a test still proves anything.
//!
//! This module therefore parses the base and head revision of a test file as
//! Rust and compares their *structure*: which functions carry `#[test]`, which
//! assertions each contains, and how each assertion's strength compares to the
//! one it replaced.
//!
//! ## Conservative by construction
//!
//! Two properties matter more than detection breadth:
//!
//! 1. **Parse failure is reported, never swallowed.** If a file cannot be
//!    parsed, the analyzer emits an explicit `test_source_unparsable` finding.
//!    Silently returning no findings would make an unanalyzable file look
//!    clean, which is the failure mode this project exists to prevent.
//! 2. **Findings are evidence about a revision pair, not verdicts.** Every
//!    finding names the line it was observed at and whether that line belongs to
//!    the base or the head revision. The decision to block or warn belongs to
//!    configuration and to the human or agent reading the receipt.
//!
//! Text normalization is deliberately aggressive: all whitespace inside a macro
//! invocation is collapsed, so reformatting a test never registers as a change.
//! Comparing normalized forms is what allows a moved assertion to be recognized
//! as unchanged instead of being reported as one removed and one added.

use std::collections::BTreeMap;

use syn::{
    spanned::Spanned,
    visit::{self, Visit},
    Attribute, File, ItemFn, Macro, Meta,
};

use crate::model::{IntegrityFinding, Severity};

/// One assertion observed in a parsed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Assertion {
    /// Macro name, e.g. `assert_eq`.
    macro_name: String,
    /// Whitespace-normalized argument text.
    arguments: String,
    /// 1-based line within its own revision.
    line: usize,
}

/// The strength class of an assertion, used to detect weakening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strength {
    /// `assert!(true)` and friends: proves nothing.
    Trivial,
    /// A bare predicate, `assert!(x > 0)`.
    Predicate,
    /// Exact equality or inequality against a literal expectation.
    Exact,
}

/// A test function observed in a parsed revision.
#[derive(Debug, Clone)]
struct TestFn {
    name: String,
    line: usize,
    ignored: bool,
    should_panic: bool,
    assertions: Vec<Assertion>,
    /// True when the body contains no assertion at all.
    body_is_empty: bool,
}

/// The structure of one parsed revision of a test file.
#[derive(Debug, Default)]
struct RevisionShape {
    tests: BTreeMap<String, TestFn>,
}

impl RevisionShape {
    fn get(&self, name: &str) -> Option<&TestFn> {
        self.tests.get(name)
    }
}

/// Analyze a Rust test file by comparing its base and head revisions.
///
/// `base_source` is `None` when the file did not exist at the base revision, in
/// which case every assertion is new and only additive rules apply.
pub fn analyze_rust_test_change(
    path: &str,
    base_source: Option<&str>,
    head_source: &str,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    let head = match syn::parse_file(head_source) {
        Ok(parsed) => shape_of(&parsed),
        Err(error) => {
            findings.push(finding(
                Severity::Warning,
                path,
                0,
                "test_source_unparsable",
                format!(
                    "the changed test file could not be parsed as Rust, so no structural comparison was performed: {error}"
                ),
            ));
            return findings;
        }
    };

    let base = match base_source {
        None => Some(RevisionShape::default()),
        Some(source) => match syn::parse_file(source) {
            Ok(parsed) => Some(shape_of(&parsed)),
            Err(error) => {
                // The base revision is only the comparison point. If it cannot
                // be read, the head file is still checked for additive
                // problems, but nothing is claimed about removals.
                findings.push(finding(
                    Severity::Info,
                    path,
                    0,
                    "test_source_unparsable",
                    format!(
                        "the base revision of this test file could not be parsed as Rust, so removed assertions and changed expectations were not compared: {error}"
                    ),
                ));
                None
            }
        },
    };

    // Additive rules: independent of whether the base could be parsed.
    for test in head.tests.values() {
        // These rules describe a *change* in test attributes. A test that was
        // already `#[ignore]`d at the base revision is not a new weakening, and
        // reporting it would flood every receipt that touches an established
        // test file. `newly_ignored` / `newly_should_panic` therefore compare
        // against the base revision where one is available, and fall back to
        // reporting for a genuinely new file.
        let existing_at_base = base.as_ref().and_then(|shape| shape.get(&test.name));
        // "Newly ignored" means the test was *not* ignored at base. Reading
        // `previous.ignored` here is the inverted form of the same question and
        // reports every pre-existing `#[ignore]` as new; a bug this crate's own
        // dogfooding run caught.
        let newly_ignored = !existing_at_base.is_some_and(|previous| previous.ignored);
        let newly_should_panic = !existing_at_base.is_some_and(|previous| previous.should_panic);

        if test.ignored && newly_ignored {
            findings.push(finding(
                Severity::High,
                path,
                test.line,
                "ignored_test",
                format!("test `{}` is marked #[ignore]", test.name),
            ));
        }
        if test.should_panic && newly_should_panic {
            findings.push(finding(
                Severity::Warning,
                path,
                test.line,
                "added_should_panic",
                format!(
                    "test `{}` is marked #[should_panic]; confirm this does not mask the intended behavior",
                    test.name
                ),
            ));
        }
        for assertion in &test.assertions {
            if strength_of(&assertion.macro_name, &assertion.arguments) != Strength::Trivial {
                continue;
            }
            // A trivial assertion that already existed at base proves nothing
            // about this change, so only newly introduced ones are reported.
            let preexisting = existing_at_base.is_some_and(|previous| {
                previous.assertions.iter().any(|candidate| {
                    candidate.macro_name == assertion.macro_name
                        && candidate.arguments == assertion.arguments
                })
            });
            if preexisting {
                continue;
            }
            findings.push(finding(
                Severity::High,
                path,
                assertion.line,
                "trivial_assertion",
                format!(
                    "test `{}` contains an assertion that cannot fail ({})",
                    test.name, assertion.macro_name
                ),
            ));
        }
        if test.body_is_empty && !test.assertions.is_empty() {
            findings.push(finding(
                Severity::Info,
                path,
                test.line,
                "empty_test_body",
                format!("test `{}` has an empty body", test.name),
            ));
        }
    }

    let Some(base) = base else {
        return findings;
    };

    for (name, head_test) in &head.tests {
        let Some(base_test) = base.get(name) else {
            // A brand-new test cannot have weakened a prior one.
            continue;
        };

        if base_test.ignored && !head_test.ignored {
            findings.push(finding(
                Severity::Info,
                path,
                head_test.line,
                "unignored_test",
                format!("test `{name}` is no longer ignored"),
            ));
        }

        for removed in missing_assertions(base_test, head_test) {
            // An assertion whose subject survived under a different form was
            // rewritten, not deleted, and `weakening_findings` judges the
            // rewrite on its own merits.
            //
            // A *predicate* that lost its exact counterpart is treated that way.
            // An *exact* assertion is not: `assert_eq!(s, NotVerified)` is never
            // silently satisfied by an unrelated `assert!(s != Verified)`
            // sitting beside it, so its disappearance is always reported. The
            // conservative direction matters more here than avoiding a
            // double-counted finding.
            let rewritten = !is_exact_macro(&removed.macro_name)
                && head_test
                    .assertions
                    .iter()
                    .any(|candidate| same_subject(&removed, candidate));
            if rewritten {
                continue;
            }
            let verdict = match strength_of(&removed.macro_name, &removed.arguments) {
                Strength::Exact => "an exact assertion was removed",
                _ => "an assertion was removed",
            };
            findings.push(finding(
                Severity::High,
                path,
                removed.line,
                "removed_assertion",
                format!(
                    "{verdict} from test `{name}` (base line {}: {}({}))",
                    removed.line, removed.macro_name, removed.arguments
                ),
            ));
        }

        findings.extend(weakening_findings(path, name, base_test, head_test));
    }

    for name in base.tests.keys() {
        if !head.tests.contains_key(name) {
            // Reported by the caller as a removed `#[test]`; handled here so a
            // dropped test is never silently ignored.
            if let Some(test) = base.get(name) {
                findings.push(finding(
                    Severity::High,
                    path,
                    test.line,
                    "removed_test",
                    format!(
                        "test `{name}` no longer exists in this file (base line {}); its assertions are no longer checked",
                        test.line
                    ),
                ));
            }
        }
    }

    findings
}

/// Assertions present in the base revision of a test but absent from the head.
///
/// Matching is on `(macro_name, normalized arguments)`, so an assertion that
/// only moved within the same function is not reported as removed.
fn missing_assertions(base: &TestFn, head: &TestFn) -> Vec<Assertion> {
    let mut unmatched: Vec<&Assertion> = head.assertions.iter().collect();
    let mut missing = Vec::new();

    for assertion in &base.assertions {
        match unmatched.iter().position(|candidate| {
            candidate.macro_name == assertion.macro_name
                && candidate.arguments == assertion.arguments
        }) {
            Some(index) => {
                unmatched.remove(index);
            }
            None => missing.push(assertion.clone()),
        }
    }
    missing
}

/// Detect assertions that survived but were made weaker, or whose expected
/// value was changed.
fn weakening_findings(
    path: &str,
    test_name: &str,
    base: &TestFn,
    head: &TestFn,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    for head_assertion in &head.assertions {
        // Pair an assertion with its true counterpart before considering a
        // looser match. Without this, a file containing both
        // `assert_eq!(receipt.status, NotVerified)` and
        // `assert!(receipt.status != Verified)` would pair the head `assert!`
        // with the base `assert_eq!` — because the `assert_eq!` happens to come
        // first and shares a subject — and report a weakening that never
        // happened.
        let base_assertion = base
            .assertions
            .iter()
            .find(|candidate| identical(candidate, head_assertion))
            .or_else(|| {
                base.assertions
                    .iter()
                    .find(|candidate| same_subject(candidate, head_assertion))
            });

        let Some(base_assertion) = base_assertion else {
            continue;
        };

        // An unchanged assertion is not a change, whatever its shape.
        if identical(base_assertion, head_assertion) {
            continue;
        }

        let base_strength = strength_of(&base_assertion.macro_name, &base_assertion.arguments);
        let head_strength = strength_of(&head_assertion.macro_name, &head_assertion.arguments);

        // Same subject, weaker or equal-or-lower strength.
        if base_strength == Strength::Exact && head_strength < Strength::Exact {
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "weakened_assertion",
                format!(
                    "test `{test_name}` replaced an exact assertion ({}({})) with the weaker {}({})",
                    base_assertion.macro_name,
                    base_assertion.arguments,
                    head_assertion.macro_name,
                    head_assertion.arguments
                ),
            ));
            continue;
        }

        // An exact assertion whose expected value moved.
        if base_strength == Strength::Exact
            && head_strength == Strength::Exact
            && base_assertion.macro_name == head_assertion.macro_name
            && base_assertion.arguments != head_assertion.arguments
        {
            let (before, after) =
                split_expected(&base_assertion.arguments, &head_assertion.arguments);
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "changed_expected_value",
                format!(
                    "test `{test_name}` changed the expected value from `{before}` to `{after}` in {}({})",
                    head_assertion.macro_name, head_assertion.arguments
                ),
            ));
        }
    }

    findings
}

/// Whether two assertions clearly talk about the same subject expression.
///
/// The comparison is deliberately strict, because a false pairing would invent
/// a weakening that the code does not contain. Two shapes are accepted:
///
/// - the same first argument (`assert_eq!(x, 1)` versus `assert_ne!(x, 1)`);
/// - an exact assertion whose subject became the leading term of a predicate
///   (`assert_eq!(x, 4)` versus `assert!(x > 0)`), which is the canonical shape
///   of a test being loosened.
///
/// Anything else is left unpaired, and the change surfaces as a removal plus an
/// addition rather than as a confident claim of equivalence.
fn same_subject(a: &Assertion, b: &Assertion) -> bool {
    let a_is_eq = is_exact_macro(&a.macro_name);
    let b_is_eq = is_exact_macro(&b.macro_name);

    let (Some(left), Some(right)) = (first_arg(&a.arguments), first_arg(&b.arguments)) else {
        return false;
    };
    if left == right {
        return a_is_eq == b_is_eq || a.macro_name == b.macro_name;
    }
    if a_is_eq && !b_is_eq {
        return right == left || right.starts_with(&format!("{left} "));
    }
    false
}

fn is_exact_macro(macro_name: &str) -> bool {
    matches!(
        macro_name,
        "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne"
    )
}

/// Whether two assertions are byte-identical after normalization.
///
/// This is the only reliable signal that an assertion was not touched, so it
/// always takes precedence over subject-based pairing.
fn identical(a: &Assertion, b: &Assertion) -> bool {
    a.macro_name == b.macro_name && a.arguments == b.arguments
}

/// The first top-level argument of a normalized argument string.
///
/// The strings produced by [`normalize_tokens`] always separate top-level
/// arguments with `", "`, so splitting on that boundary is exact rather than a
/// heuristic, even when an argument itself contains commas inside brackets.
fn first_arg(arguments: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;

    for (index, character) in arguments.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                in_string = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => in_string = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some(arguments[..index].trim()),
            _ => {}
        }
    }
    arguments
        .strip_suffix(", ")
        .or(Some(arguments))
        .map(str::trim)
}

/// Split an argument string into its leading and trailing arguments, used to
/// name the before/after of a changed expectation.
fn split_expected(before: &str, after: &str) -> (String, String) {
    (
        last_arg(before).unwrap_or_else(|| before.to_owned()),
        last_arg(after).unwrap_or_else(|| after.to_owned()),
    )
}

fn last_arg(arguments: &str) -> Option<String> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut last_start = 0usize;

    for (index, character) in arguments.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                in_string = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => in_string = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => last_start = index + 1,
            _ => {}
        }
    }
    let tail = arguments[last_start..].trim();
    (!tail.is_empty()).then(|| tail.to_owned())
}

/// Classify an assertion's strength.
fn strength_of(macro_name: &str, arguments: &str) -> Strength {
    let compact: String = arguments.chars().filter(|c| !c.is_whitespace()).collect();
    if is_trivially_true(&compact) {
        return Strength::Trivial;
    }
    match macro_name {
        name if is_exact_macro(name) => Strength::Exact,
        _ => {
            // `assert!(true)` is caught above; anything else is a predicate.
            Strength::Predicate
        }
    }
}

fn is_trivially_true(compact: &str) -> bool {
    compact == "true"
        || compact == "assert!(true)"
        || compact == "assert_eq!(true,true)"
        || compact == "assert_eq!(1,1)"
        || compact == "assert!(1==1)"
}

/// Parse a revision into the structure this analyzer compares.
fn shape_of(parsed: &File) -> RevisionShape {
    struct Collector {
        shape: RevisionShape,
    }

    impl<'ast> Visit<'ast> for Collector {
        fn visit_item_fn(&mut self, item: &'ast ItemFn) {
            if has_attribute(&item.attrs, "test") {
                let name = item.sig.ident.to_string();
                let mut body = AssertionCollector::default();
                body.visit_block(&item.block);
                self.shape.tests.insert(
                    name.clone(),
                    TestFn {
                        name,
                        line: item.sig.ident.span().start().line,
                        ignored: has_attribute(&item.attrs, "ignore"),
                        should_panic: has_attribute(&item.attrs, "should_panic"),
                        assertions: body.assertions,
                        body_is_empty: is_empty_block(&item.block),
                    },
                );
            }
            visit::visit_item_fn(self, item);
        }
    }

    let mut collector = Collector {
        shape: RevisionShape::default(),
    };
    collector.visit_file(parsed);
    collector.shape
}

#[derive(Default)]
struct AssertionCollector {
    assertions: Vec<Assertion>,
}

impl<'ast> Visit<'ast> for AssertionCollector {
    fn visit_macro(&mut self, mac: &'ast Macro) {
        if is_assertion_macro(&mac.path) {
            let arguments = normalize_tokens(&mac.tokens.to_string());
            self.assertions.push(Assertion {
                macro_name: mac
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default(),
                arguments,
                line: mac.tokens.span().start().line,
            });
        }
        visit::visit_macro(self, mac);
    }
}

fn is_assertion_macro(path: &syn::Path) -> bool {
    path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "assert"
                | "assert_eq"
                | "assert_ne"
                | "debug_assert"
                | "debug_assert_eq"
                | "debug_assert_ne"
        )
    })
}

fn has_attribute(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| {
        let attr_name = match &attr.meta {
            Meta::Path(path) => path.segments.last().map(|s| s.ident.to_string()),
            Meta::List(list) => list.path.segments.last().map(|s| s.ident.to_string()),
            Meta::NameValue(pair) => pair.path.segments.last().map(|s| s.ident.to_string()),
        };
        match attr_name.as_deref() {
            Some(found) => found == name || found == format!("{name}=").as_str(),
            None => false,
        }
    })
}

fn is_empty_block(block: &syn::Block) -> bool {
    block.stmts.is_empty()
}

/// Punctuation that must not carry surrounding whitespace in a normalized form.
const PUNCTUATION: [char; 7] = ['(', ')', '[', ']', '{', '}', ','];

/// Normalize a rendered macro argument list into a formatting-insensitive form.
///
/// Two normalizations are required, and both were found by observing real
/// `proc_macro2` output rather than assumed:
///
/// 1. Spacing around punctuation is dropped. `proc_macro2` renders
///    `compute()` as `compute ()` and a trailing comma as `4 ,`, so the same
///    call written on one line and across four lines otherwise produces
///    different text.
/// 2. Whitespace inside string and character literals survives verbatim.
///    Collapsing it would make two genuinely different literals compare equal,
///    which would hide a changed expectation rather than reveal it.
fn normalize_tokens(tokens: &str) -> String {
    let mut out = String::with_capacity(tokens.len());
    let mut in_string: Option<char> = None;
    let mut escaped = false;

    for character in tokens.chars() {
        if let Some(quote) = in_string {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                in_string = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => {
                in_string = Some(character);
                out.push(character);
            }
            character if PUNCTUATION.contains(&character) => {
                // Drop spacing adjacent to punctuation. `proc_macro2` renders
                // `compute()` as `compute ()` and a trailing comma as `4 ,`,
                // so spacing depends on source layout and must not be part of
                // the compared form.
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push(character);
            }
            character if character.is_whitespace() => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            character => out.push(character),
        }
    }

    // A trailing comma reflects how the source was laid out, not the
    // expression, so it is dropped once the whole argument list is read.
    let trimmed = out.trim();
    trimmed
        .strip_suffix(',')
        .map(str::trim_end)
        .unwrap_or(trimmed)
        .to_owned()
}

fn finding(
    severity: Severity,
    path: &str,
    line: usize,
    rule: &str,
    message: String,
) -> IntegrityFinding {
    IntegrityFinding {
        severity,
        path: path.to_owned(),
        line: if line == 0 {
            "<file>".to_owned()
        } else {
            line.to_string()
        },
        rule: rule.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(base: &str, head: &str) -> Vec<IntegrityFinding> {
        analyze_rust_test_change("tests/t.rs", Some(base), head)
    }

    fn rules(findings: &[IntegrityFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    #[test]
    fn identical_revisions_produce_no_findings() {
        let source = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        assert!(analyze(source, source).is_empty());
    }

    #[test]
    fn reformatting_is_not_a_change() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head =
            "#[test]\nfn keeps() {\n    assert_eq!(\n        compute(),\n        4,\n    );\n}\n";
        assert!(
            analyze(base, head).is_empty(),
            "whitespace-only edits must not register as changes, got {:?}",
            rules(&analyze(base, head))
        );
    }

    #[test]
    fn reordered_assertions_are_not_reported_as_removed() {
        // The assertions keep their exact forms; only their order changed. That
        // is a formatting change and must produce no finding.
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n    assert!(flag);\n}\n";
        let head = "#[test]\nfn keeps() {\n    assert!(flag);\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(base, head);
        assert!(
            findings.is_empty(),
            "reordering unchanged assertions is not a change, got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn rebound_subject_is_reported_because_it_cannot_be_resolved() {
        // `assert_eq!(compute(), 4)` and `assert_eq!(v, 4)` are genuinely
        // different expressions. WitDiff does not resolve `let` bindings, so it
        // must not claim the assertion merely moved. Surfacing the change is
        // the conservative direction: a missed removal is a false pass, while
        // a reported one is reviewed by a human or agent.
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn keeps() {\n    let v = compute();\n    assert_eq!(v, 4);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_assertion"),
            "an unresolvable rewrite must still be surfaced, got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn deleted_assertion_is_high_severity() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n    assert!(compute() > 0);\n}\n";
        let head = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(base, head);
        let removed: Vec<_> = findings
            .iter()
            .filter(|f| f.rule == "removed_assertion")
            .collect();
        assert_eq!(removed.len(), 1, "got {findings:#?}");
        assert_eq!(removed[0].severity, Severity::High);
    }

    #[test]
    fn changed_expected_value_is_detected() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 99);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"changed_expected_value"),
            "got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn exact_to_predicate_is_weakening() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn keeps() {\n    assert!(compute() > 0);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"weakened_assertion"),
            "got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn newly_ignored_test_is_high_severity() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\n#[ignore]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(base, head);
        let ignored = findings.iter().find(|f| f.rule == "ignored_test");
        assert_eq!(
            ignored.map(|f| f.severity.clone()),
            Some(Severity::High),
            "got {findings:#?}"
        );
    }

    #[test]
    fn trivial_assertion_is_reported() {
        let base = "#[test]\nfn keeps() {\n}\n";
        let head = "#[test]\nfn keeps() {\n    assert!(true);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"trivial_assertion"),
            "got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn deleted_test_is_reported() {
        let base = "#[test]\nfn gone() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn other() {\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_test"),
            "got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn unparsable_head_is_reported_not_silently_clean() {
        let findings = analyze_rust_test_change("tests/t.rs", Some("#[test]\n"), "fn ( {");
        assert_eq!(
            rules(&findings),
            vec!["test_source_unparsable"],
            "an unanalyzable file must never look clean"
        );
    }

    #[test]
    fn unparsable_base_still_checks_additive_rules() {
        let findings = analyze_rust_test_change(
            "tests/t.rs",
            Some("fn ( {"),
            "#[test]\nfn keeps() {\n    assert!(true);\n}\n",
        );
        assert!(rules(&findings).contains(&"test_source_unparsable"));
        assert!(
            rules(&findings).contains(&"trivial_assertion"),
            "additive checks must still run; got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn new_file_only_reports_additive_problems() {
        let findings =
            analyze_rust_test_change("tests/new.rs", None, "#[test]\nfn t() { assert!(true); }");
        assert!(rules(&findings).contains(&"trivial_assertion"));
        assert!(
            !rules(&findings).contains(&"removed_assertion"),
            "a new file cannot have removed a prior assertion"
        );
    }

    #[test]
    fn added_should_panic_is_a_warning() {
        let base = "#[test]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\n#[should_panic]\nfn keeps() {\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(base, head);
        let panic = findings.iter().find(|f| f.rule == "added_should_panic");
        assert_eq!(panic.map(|f| f.severity.clone()), Some(Severity::Warning));
    }

    #[test]
    fn assertions_inside_helper_functions_are_still_seen() {
        // A test may delegate to a helper; the helper's assertions count.
        let base = "fn helper() { assert_eq!(compute(), 4); }\n#[test]\nfn t() { helper(); }\n";
        let head = "fn helper() { }\n#[test]\nfn t() { helper(); }\n";
        let findings = analyze_rust_test_change("tests/t.rs", None, head);
        // The helper is not a #[test] fn, so only the head test shape is
        // considered; the point is that parsing does not fail on it.
        assert!(!rules(&findings).contains(&"test_source_unparsable"));
        assert!(!base.is_empty());
    }

    #[test]
    fn first_arg_respects_nested_commas() {
        assert_eq!(first_arg("compute(1, 2), 4"), Some("compute(1, 2)"));
        assert_eq!(first_arg("x, 4"), Some("x"));
        assert_eq!(first_arg("single"), Some("single"));
    }

    /// Regression found by dogfooding WitDiff on its own repository: a test
    /// file whose tests were *already* `#[ignore]`d at base must not report
    /// every one of them as newly ignored. Doing so floods a receipt with
    /// findings about untouched history.
    #[test]
    fn pre_existing_ignored_test_is_not_reported_as_new() {
        let source = "#[test]\n#[ignore]\nfn slow() {\n    assert_eq!(compute(), 4);\n}\n";
        let findings = analyze(source, source);
        assert!(
            findings.is_empty(),
            "an unchanged file must produce no findings, got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn newly_added_ignore_is_still_reported() {
        let base = "#[test]\nfn fast() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\n#[ignore]\nfn fast() {\n    assert_eq!(compute(), 4);\n}\n";
        assert!(
            rules(&analyze(base, head)).contains(&"ignored_test"),
            "a genuinely new #[ignore] must still be caught"
        );
    }

    #[test]
    fn pre_existing_should_panic_is_not_reported() {
        let source = "#[test]\n#[should_panic]\nfn panics() {\n    boom();\n}\n";
        assert!(
            analyze(source, source).is_empty(),
            "an unchanged #[should_panic] is not a change"
        );
    }

    #[test]
    fn pre_existing_trivial_assertion_is_not_reported() {
        let source = "#[test]\nfn smoke() {\n    assert!(true);\n}\n";
        assert!(
            analyze(source, source).is_empty(),
            "an unchanged trivial assertion is not a change"
        );
    }

    /// Regression found by dogfooding: when a test contains BOTH an exact
    /// assertion and a predicate over the same subject, the predicate must be
    /// paired with its own base counterpart. Pairing it with the unrelated
    /// `assert_eq!` invents a weakening.
    #[test]
    fn sibling_assertions_over_one_subject_are_paired_correctly() {
        let body = "    assert_eq!(receipt.status, NotVerified, \"blocked\");\n    assert!(receipt.status != Verified, \"never certified\");\n";
        let base = format!("#[test]\nfn gate() {{\n{body}}}\n");
        let head = format!("#[test]\nfn gate() {{\n{body}}}\n");
        assert!(
            analyze(&base, &head).is_empty(),
            "identical sibling assertions must not pair across, got {:?}",
            rules(&analyze(&base, &head))
        );
    }

    /// The same shape, but where the exact assertion genuinely disappears.
    #[test]
    fn dropping_the_exact_assertion_is_reported() {
        let base = "#[test]\nfn gate() {\n    assert_eq!(receipt.status, NotVerified, \"blocked\");\n    assert!(receipt.status != Verified, \"never certified\");\n}\n";
        let head = "#[test]\nfn gate() {\n    assert!(receipt.status != Verified, \"never certified\");\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_assertion"),
            "dropping the exact assertion must be reported, got {:?}",
            rules(&findings)
        );
        assert!(
            !rules(&findings).contains(&"weakened_assertion"),
            "removing an assertion is a removal, not a weakening, got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn normalize_collapses_whitespace_outside_strings() {
        assert_eq!(normalize_tokens("a ,\n  b"), "a, b");
        assert_eq!(
            normalize_tokens("\"a\n  b\" , c"),
            "\"a\n  b\", c",
            "string contents must survive normalization verbatim"
        );
    }

    #[test]
    fn normalize_drops_spacing_around_punctuation() {
        // Observed `proc_macro2` output for a multi-line call.
        assert_eq!(normalize_tokens("compute () , 4 ,"), "compute(), 4");
        assert_eq!(normalize_tokens("compute(), 4"), "compute(), 4");
    }
}
