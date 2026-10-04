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

use quote::ToTokens;
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
    Attribute, ExprMatch, File, ItemFn, Macro, Meta, Pat,
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

/// One arm of a `match` expression, identified by what it matches
/// rather than by what it does.
///
/// The body is deliberately not part of the identity. Assertions
/// inside a body are already tracked as assertions, and a body that
/// changes without changing any assertion is not, by itself, evidence
/// of weakening. A guard *is* part of the identity: dropping a guard
/// makes the arm apply to more cases, so the old guarded arm counts
/// as removed and the unguarded one as added.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchArm {
    /// Whitespace-normalized pattern text.
    pattern: String,
    /// Whitespace-normalized guard expression, when the arm has one.
    guard: Option<String>,
    /// 1-based line within its own revision.
    line: usize,
}

/// A `match` expression observed in a parsed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchExpr {
    /// Whitespace-normalized scrutinee text, used to pair the same
    /// `match` across revisions.
    scrutinee: String,
    arms: Vec<MatchArm>,
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
    /// `match` expressions, keyed by nothing: pairing across
    /// revisions happens on the normalized scrutinee.
    matches: Vec<MatchExpr>,
    /// True when the body contains no assertion at all.
    body_is_empty: bool,
    /// Simple `let` bindings, used to resolve a rebound subject. See
    /// [`TestBodyCollector::bindings`].
    bindings: BTreeMap<String, String>,
    /// Failure guards: conditionals that fail the test without an assertion
    /// macro, such as `if r.is_err() { panic!("...") }`. Recorded as their
    /// normalized condition.
    guards: Vec<String>,
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

        // Resolution is scoped to this test's whole comparison, so a binding in
        // one test cannot make an unrelated assertion look equivalent in
        // another. The removal loop is inside the scope because its
        // "was this rewritten?" check resolves the head subject through a
        // binding; leaving it outside made the check silently ineffective.
        let findings_here = with_bindings(&head_test.bindings, || {
            let mut local: Vec<IntegrityFinding> = Vec::new();
            for removed in missing_assertions(base_test, head_test) {
                // An assertion whose subject survived under a different form was
                // rewritten, not deleted, and `weakening_findings` judges the
                // rewrite on its own merits.
                //
                // An *exact* assertion is only treated as rewritten when the
                // head counterpart resolves to the identical subject through a
                // `let` binding. That case is a pure refactor:
                // `assert_eq!(compute(), 4)` becoming
                // `let v = compute(); assert_eq!(v, 4)` asserts exactly the same
                // thing. Without it, the refactor is a false positive, and
                // because high-severity findings block verification it
                // downgrades a genuinely proven change.
                //
                // Anything looser would be wrong: an exact assertion is never
                // silently satisfied by an unrelated predicate beside it.
                let rewritten = head_test.assertions.iter().any(|candidate| {
                    if !same_subject(&removed, candidate) {
                        return false;
                    }
                    if !is_exact_macro(&removed.macro_name) {
                        return true;
                    }
                    let (Some(left), Some(right)) = (
                        first_arg(&removed.arguments),
                        first_arg(&candidate.arguments),
                    ) else {
                        return false;
                    };
                    left == right || resolve_binding(right).is_some_and(|resolved| resolved == left)
                });
                if rewritten {
                    continue;
                }
                let verdict = match strength_of(&removed.macro_name, &removed.arguments) {
                    Strength::Exact => "an exact assertion was removed",
                    _ => "an assertion was removed",
                };
                local.push(finding(
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
            local.extend(weakening_findings(path, name, base_test, head_test));
            local
        });

        findings.extend(findings_here);
        findings.extend(match_arm_findings(path, name, base_test, head_test));
        findings.extend(removed_guard_findings_rust(
            path, name, base_test, head_test,
        ));
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

/// Guards present at base and absent at head.
///
/// A guard is a conditional that fails the test without an assertion macro:
/// `if r.is_err() { panic!("expected a number"); }`. The assertion rules never
/// see it, and measured before this rule existed, deleting one produced **zero**
/// findings while the test still compiled and passed — the check was simply
/// gone, and nothing in the receipt said so.
///
/// Matched as a multiset on the normalized condition, so reordering or
/// reformatting is not a finding.
fn removed_guard_findings_rust(
    path: &str,
    test_name: &str,
    base: &TestFn,
    head: &TestFn,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();
    let mut unmatched: Vec<&String> = head.guards.iter().collect();

    for guard in &base.guards {
        match unmatched.iter().position(|candidate| *candidate == guard) {
            Some(index) => {
                unmatched.remove(index);
            }
            None => findings.push(finding(
                Severity::High,
                path,
                base.line,
                "removed_error_check",
                format!(
                    "test `{test_name}` no longer checks a failure path (was `{guard}`); the test still compiles and passes, but the condition it guarded is no longer verified"
                ),
            )),
        }
    }

    findings
}

/// Arms removed from a `match` that the change also edited.
///
/// A `match` is paired across revisions by its normalized scrutinee, and
/// its arms are compared as a multiset on `(pattern, guard)` — the same
/// matching [`missing_assertions`] uses. Consequences, all deliberate:
///
/// - An arm that merely moved within the function, or was reordered, is
///   not reported.
/// - An arm that survived in *any* `match` on the same scrutinee is not
///   reported, so splitting one `match` into two is not a weakening.
/// - Collapsing specific arms into a wildcard is reported, because the
///   specific arms are then absent.
/// - Dropping a guard changes the arm's identity, so the old guarded arm
///   is reported as removed; the unguarded survivor is an addition, and
///   additions are not findings.
///
/// A `match` that disappeared entirely is *not* reported here. Replacing
/// a `match` with an `if`/`else` chain or a `let`-`else` is a common
/// refactor, and any assertion inside the removed arms is already caught
/// by [`missing_assertions`]. Reporting the structural removal too would
/// turn a faithful refactor into a false positive.
fn match_arm_findings(
    path: &str,
    test_name: &str,
    base: &TestFn,
    head: &TestFn,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    let base_arms = arms_by_scrutinee(base);
    let head_arms = arms_by_scrutinee(head);

    for (scrutinee, base_match_arms) in &base_arms {
        let Some(head_match_arms) = head_arms.get(scrutinee) else {
            // The whole `match` is gone; see the module rationale above.
            continue;
        };

        let mut unmatched: Vec<&MatchArm> = head_match_arms.clone();
        for arm in base_match_arms {
            let survives = unmatched
                .iter()
                .position(|candidate| {
                    candidate.pattern == arm.pattern && candidate.guard == arm.guard
                })
                .map(|index| unmatched.remove(index))
                .is_some();
            if survives {
                continue;
            }
            let identity = match &arm.guard {
                Some(guard) => format!("{} if {}", arm.pattern, guard),
                None => arm.pattern.clone(),
            };
            findings.push(finding(
                Severity::High,
                path,
                arm.line,
                "removed_match_arm",
                format!(
                    "test `{test_name}` removed the `match` arm `{identity}` on `{scrutinee}` (base line {}); the cases it covered are no longer checked",
                    arm.line
                ),
            ));
        }
    }

    findings
}

/// All arms of all `match` expressions in a test, grouped by scrutinee.
///
/// Grouping — rather than pairing expression by expression — is what
/// makes the comparison robust to a `match` being split, merged or
/// reordered, while still catching any arm that no longer exists
/// anywhere on the same scrutinee.
fn arms_by_scrutinee(test: &TestFn) -> BTreeMap<String, Vec<&MatchArm>> {
    let mut grouped: BTreeMap<String, Vec<&MatchArm>> = BTreeMap::new();
    for expression in &test.matches {
        grouped
            .entry(expression.scrutinee.clone())
            .or_default()
            .extend(expression.arms.iter());
    }
    grouped
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

/// Detect assertions that survived but were made weaker, or whose
/// expected value was changed.
///
/// Pairing happens in two passes, and the order is the point:
///
/// 1. Every head assertion that is byte-identical to a base
///    assertion consumes it. Those assertions are untouched, and
///    must not remain available as the "before" side of a
///    weakening claim.
/// 2. Each remaining head assertion pairs with at most one
///    remaining base assertion that shares its subject.
///
/// Without the first pass, a *newly added* assertion whose subject
/// collides with an existing one — a second `match` arm asserting
/// a different expected value over the same expression — would be
/// reported as if the existing assertion had been rewritten.
fn weakening_findings(
    path: &str,
    test_name: &str,
    base: &TestFn,
    head: &TestFn,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    let mut remaining_base: Vec<&Assertion> = base.assertions.iter().collect();
    let mut remaining_head: Vec<&Assertion> = Vec::new();
    for head_assertion in &head.assertions {
        match remaining_base
            .iter()
            .position(|candidate| identical(candidate, head_assertion))
        {
            Some(index) => {
                remaining_base.remove(index);
            }
            None => remaining_head.push(head_assertion),
        }
    }

    for head_assertion in &remaining_head {
        // Pair an assertion with its true counterpart before considering a
        // looser match. Without this, a file containing both
        // `assert_eq!(receipt.status, NotVerified)` and
        // `assert!(receipt.status != Verified)` would pair the head `assert!`
        // with the base `assert_eq!` — because the `assert_eq!` happens to
        // come first and shares a subject — and report a weakening that
        // never happened.
        let Some(base_assertion) = remaining_base
            .iter()
            .find(|candidate| same_subject(candidate, head_assertion))
        else {
            continue;
        };

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
        //
        // The comparison is on the *expected* value, not on the whole argument
        // list. Comparing arguments reported `changed the expected value from
        // `4` to `4`` for a pure rebinding, because `compute(), 4` and `v, 4`
        // differ while both expect 4. A finding that reads as self-contradictory
        // is worse than no finding: it teaches a reader to distrust the rule.
        if base_strength == Strength::Exact
            && head_strength == Strength::Exact
            && base_assertion.macro_name == head_assertion.macro_name
            && expected_value(&base_assertion.arguments)
                != expected_value(&head_assertion.arguments)
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
    same_resolved_subject(left, right, a_is_eq, b_is_eq, a.macro_name == b.macro_name)
}

/// Compare two subject expressions, treating one as equal to the other when the
/// first is a simple binding of the second.
///
/// `left` comes from the base revision and `right` from the head. A head
/// assertion whose subject is a single identifier is resolved through that
/// revision's bindings before comparison, so introducing
/// `let v = compute();` above a surviving `assert_eq!(v, 4)` is recognized as
/// the same assertion rather than reported as removed.
fn same_resolved_subject(
    left: &str,
    right: &str,
    a_is_eq: bool,
    b_is_eq: bool,
    same_macro: bool,
) -> bool {
    if left == right {
        return a_is_eq == b_is_eq || same_macro;
    }
    // A rebinding: the head subject is a name the base subject was bound to.
    if let Some(resolved) = resolve_binding(right) {
        if resolved == left {
            return a_is_eq == b_is_eq || same_macro;
        }
    }
    if a_is_eq && !b_is_eq {
        return right == left || right.starts_with(&format!("{left} "));
    }
    false
}

/// Resolve a simple identifier through the bindings recorded for its test.
fn resolve_binding(subject: &str) -> Option<String> {
    RESOLUTION.with(|resolution| resolution.borrow().get(subject).cloned())
}

thread_local! {
    /// Bindings of the test pair currently being compared.
    ///
    /// A thread-local is used because `same_subject` is called from the
    /// comparison loop over many assertions, and threading a bindings map
    /// through every helper would obscure the rules. The value is set for the
    /// duration of one test's comparison and cleared immediately, so it cannot
    /// leak between tests or between the base and head revisions.
    static RESOLUTION: std::cell::RefCell<BTreeMap<String, String>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// Run `f` with the head revision's bindings available to subject resolution.
fn with_bindings<T>(bindings: &BTreeMap<String, String>, f: impl FnOnce() -> T) -> T {
    RESOLUTION.with(|resolution| {
        *resolution.borrow_mut() = bindings.clone();
        let result = f();
        resolution.borrow_mut().clear();
        result
    })
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

/// The expected value of an exact assertion, as written.
///
/// `assert_eq!(compute(), 4)` and `assert_eq!(v, 4)` both expect `4`, so a
/// rebinding does not look like a changed expectation.
fn expected_value(arguments: &str) -> Option<String> {
    last_arg(arguments)
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
                let mut body = TestBodyCollector::default();
                body.visit_block(&item.block);
                self.shape.tests.insert(
                    name.clone(),
                    TestFn {
                        name,
                        line: item.sig.ident.span().start().line,
                        ignored: has_attribute(&item.attrs, "ignore"),
                        should_panic: has_attribute(&item.attrs, "should_panic"),
                        assertions: body.assertions,
                        matches: body.matches,
                        body_is_empty: is_empty_block(&item.block),
                        bindings: body.bindings,
                        guards: body.guards,
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
struct TestBodyCollector {
    assertions: Vec<Assertion>,
    matches: Vec<MatchExpr>,
    /// Guards seen in the test body, as normalized conditions.
    ///
    /// A guard is an `if` whose body always fails the test — `panic!`,
    /// `unreachable!`, `assert!` counted separately, or a
    /// `return`/`std::process::exit`. These are not assertions, so the
    /// assertion rules never see them, and deleting one used to produce no
    /// finding at all while the test still compiled and passed.
    guards: Vec<String>,
    /// Simple `let` bindings in the test body, keyed by variable name.
    ///
    /// Used to resolve a rebound subject: `let v = compute(); assert_eq!(v, 4)`
    /// and `assert_eq!(compute(), 4)` assert the same thing, but without
    /// resolution the first is reported as a removed assertion. That is a false
    /// positive on a pure refactor, and a high-severity finding blocks
    /// verification, so it downgrades a genuinely proven change.
    ///
    /// Only *simple* bindings are recorded: a name bound directly to an
    /// expression, with no destructuring and no shadowing subtleties. Anything
    /// else is left unresolved, which keeps the conservative direction — an
    /// unresolved subject is reported rather than assumed equal.
    bindings: BTreeMap<String, String>,
}

impl<'ast> Visit<'ast> for TestBodyCollector {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        // Only `let x = expr;` with a plain identifier pattern and no type
        // annotation on the *value* side is recorded. A destructuring or a
        // reference is left alone, because resolving it would require
        // understanding the binding's use, not just its definition.
        if let syn::Pat::Ident(pattern) = &local.pat {
            if pattern.by_ref.is_none() && pattern.subpat.is_none() {
                if let Some(init) = &local.init {
                    let value = normalize_tokens(&render(&init.expr));
                    self.bindings.insert(pattern.ident.to_string(), value);
                }
            }
        }
        visit::visit_local(self, local);
    }

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

    fn visit_expr_if(&mut self, expression: &'ast syn::ExprIf) {
        // A guard fails the test unconditionally in its body. Only `panic!`,
        // `unreachable!` and `todo!`/`unimplemented!` qualify: an `assert!`
        // inside an `if` is already tracked as an assertion, and counting it
        // twice would report one deletion as two findings.
        if block_always_fails(&expression.then_branch) {
            self.guards
                .push(normalize_tokens(&render(&expression.cond)));
        }
        visit::visit_expr_if(self, expression);
    }

    fn visit_expr_match(&mut self, expression: &'ast ExprMatch) {
        let scrutinee = normalize_tokens(&render(&expression.expr));
        let arms = expression
            .arms
            .iter()
            .map(|arm| {
                let (pattern, guard) = split_arm(&arm.pat);
                MatchArm {
                    pattern,
                    guard,
                    line: arm.pat.span().start().line,
                }
            })
            .collect();
        self.matches.push(MatchExpr { scrutinee, arms });
        visit::visit_expr_match(self, expression);
    }
}

/// Split a match arm's pattern into its pattern and its guard.
///
/// `syn` 3 represents `pat if guard` as `Pat::Guard`, so the guard is
/// part of the pattern node rather than a separate field of the arm.
fn split_arm(pat: &Pat) -> (String, Option<String>) {
    match pat {
        Pat::Guard(guarded) => (
            normalize_tokens(&render(&guarded.pat)),
            Some(normalize_tokens(&render(&guarded.guard))),
        ),
        other => (normalize_tokens(&render(other)), None),
    }
}

/// Render a syntax node to normalized text.
///
/// `syn` implements `quote::ToTokens` for every node type, so this
/// covers scrutinees, patterns and guard expressions alike without
/// reaching for `Debug` formatting, whose shape is not guaranteed
/// stable across versions.
fn render<T: ToTokens>(node: &T) -> String {
    node.to_token_stream().to_string()
}

/// Whether a block unconditionally fails the test.
///
/// Recurses into nested blocks so `if x { { panic!() } }` is recognized, and
/// treats an `if`/`else` chain as failing only when every branch does, because
/// a branch that continues means the condition is not a guard.
fn block_always_fails(block: &syn::Block) -> bool {
    block.stmts.iter().any(statement_always_fails)
}

fn statement_always_fails(statement: &syn::Stmt) -> bool {
    match statement {
        syn::Stmt::Macro(mac) => mac.mac.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "panic" | "unreachable" | "unimplemented" | "todo"
            )
        }),
        syn::Stmt::Expr(syn::Expr::Block(inner), _) => block_always_fails(&inner.block),
        syn::Stmt::Expr(syn::Expr::If(branch), _) => {
            block_always_fails(&branch.then_branch)
                && branch.else_branch.as_ref().is_some_and(|(_, otherwise)| {
                    match otherwise.as_ref() {
                        syn::Expr::Block(inner) => block_always_fails(&inner.block),
                        syn::Expr::If(nested) => block_always_fails(&nested.then_branch),
                        _ => false,
                    }
                })
        }
        syn::Stmt::Expr(syn::Expr::Return(_), _) => true,
        _ => false,
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

    /// Superseded: this test asserted the old limitation that `let` bindings
    /// are not resolved. They now are, so the same input is recognized as a
    /// rebinding rather than a removal. The behavior in both directions is
    /// covered by `rebinding_a_subject_is_not_a_removed_assertion` and
    /// `rebinding_does_not_hide_a_real_change`.

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

    /// Regression: introducing a `let` binding reported the surviving
    /// assertion as removed. A pure refactor was therefore a high-severity
    /// finding, which blocks verification and downgrades a proven change.
    #[test]
    fn rebinding_a_subject_is_not_a_removed_assertion() {
        let base = "#[test]\nfn t() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn t() {\n    let v = compute();\n    assert_eq!(v, 4);\n}\n";
        let findings = analyze(base, head);
        assert!(
            findings.is_empty(),
            "rebinding a subject asserts the same thing, got {:?}",
            rules(&findings)
        );
    }

    /// The same, with the binding introduced in the head and the expectation
    /// unchanged. Reported before as `changed the expected value from `4` to
    /// `4``, which reads as self-contradictory.
    #[test]
    fn rebinding_does_not_report_a_changed_expected_value() {
        let base = "#[test]\nfn t() {\n    assert_eq!(receipt.status, NotVerified);\n}\n";
        let head =
            "#[test]\nfn t() {\n    let s = receipt.status;\n    assert_eq!(s, NotVerified);\n}\n";
        let findings = analyze(base, head);
        assert!(
            !rules(&findings).contains(&"changed_expected_value"),
            "the expectation did not change, got {:?}",
            rules(&findings)
        );
    }

    /// The dangerous direction: resolution must not make a genuinely changed
    /// expectation or a genuine removal disappear.
    #[test]
    fn rebinding_does_not_hide_a_real_change() {
        let base = "#[test]\nfn t() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn t() {\n    let v = compute();\n    assert_eq!(v, 5);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"changed_expected_value"),
            "a real expectation change must still be reported, got {:?}",
            rules(&findings)
        );
    }

    /// A binding only resolves within its own test. A name that happens to
    /// match in another test must not make an unrelated assertion equivalent.
    #[test]
    fn a_binding_does_not_leak_between_tests() {
        let base = "#[test]\nfn a() {\n    assert_eq!(compute(), 4);\n}\n\n#[test]\nfn b() {\n    assert_eq!(other(), 9);\n}\n";
        let head = "#[test]\nfn a() {\n    assert_eq!(compute(), 4);\n}\n\n#[test]\nfn b() {\n    let v = other();\n    assert_eq!(v, 9);\n}\n";
        let findings = analyze(base, head);
        assert!(
            findings.is_empty(),
            "each test resolves its own bindings, got {:?}",
            rules(&findings)
        );
    }

    /// A destructuring binding is not resolved, so the conservative direction
    /// is preserved: an unresolvable subject is reported rather than assumed
    /// equal.
    #[test]
    fn a_destructuring_binding_is_not_resolved() {
        let base = "#[test]\nfn t() {\n    assert_eq!(compute(), 4);\n}\n";
        let head = "#[test]\nfn t() {\n    let (v,) = (compute(),);\n    assert_eq!(v, 4);\n}\n";
        let findings = analyze(base, head);
        assert!(
            !findings.is_empty(),
            "an unresolvable subject must be reported rather than assumed equal"
        );
    }

    /// Regression: deleting a failure guard produced no finding at all. The
    /// test still compiled and passed, so red/green could not see it either.
    #[test]
    fn a_deleted_failure_guard_is_reported() {
        let base = "#[test]\nfn t() {\n    let r = parse(\"x\");\n    if r.is_err() {\n        panic!(\"expected a number\");\n    }\n    assert_eq!(r.unwrap(), 1);\n}\n";
        let head =
            "#[test]\nfn t() {\n    let r = parse(\"x\");\n    assert_eq!(r.unwrap(), 1);\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_error_check"),
            "a deleted guard removes a check and must be reported, got {:?}",
            rules(&findings)
        );
        let reported = findings
            .iter()
            .find(|f| f.rule == "removed_error_check")
            .expect("the finding");
        assert_eq!(reported.severity, Severity::High);
        assert!(
            reported.message.contains("still compiles and passes"),
            "the message should explain why red/green cannot see this, got {}",
            reported.message
        );
    }

    /// A guard that survives is not a finding.
    #[test]
    fn an_unchanged_guard_is_not_reported() {
        let source = "#[test]\nfn t() {\n    let r = parse(\"x\");\n    if r.is_err() {\n        panic!(\"bad\");\n    }\n    assert_eq!(r.unwrap(), 1);\n}\n";
        assert!(
            analyze(source, source).is_empty(),
            "an unchanged guard is not a change"
        );
    }

    /// The dangerous direction: a guard whose *condition* changed is a changed
    /// check, not a removed one, and the assertion rules already handle it. It
    /// must not also be reported as a removal, or one edit becomes two findings.
    #[test]
    fn a_reworded_guard_is_not_a_removal() {
        let base = "#[test]\nfn t() {\n    if a.is_err() {\n        panic!(\"one\");\n    }\n}\n";
        let head = "#[test]\nfn t() {\n    if a.is_err() {\n        panic!(\"two\");\n    }\n}\n";
        let findings = analyze(base, head);
        assert!(
            !rules(&findings).contains(&"removed_error_check"),
            "the guard condition is unchanged, so it is not removed, got {:?}",
            rules(&findings)
        );
    }

    /// An `assert!` inside an `if` is an assertion, not a guard. Counting it as
    /// both would report one deletion twice.
    #[test]
    fn an_assertion_inside_an_if_is_not_a_guard() {
        let base = "#[test]\nfn t() {\n    if flag {\n        assert_eq!(x, 1);\n    }\n}\n";
        let head = "#[test]\nfn t() {\n}\n";
        let findings = analyze(base, head);
        assert!(
            !rules(&findings).contains(&"removed_error_check"),
            "an assertion is tracked by the assertion rules, got {:?}",
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

    /// The canonical weakening this rule exists to catch: a specific
    /// case is deleted from a `match`, so inputs it covered are no
    /// longer checked.
    #[test]
    fn removed_match_arm_is_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_match_arm"),
            "a deleted arm must be reported, got {:?}",
            rules(&findings)
        );
        assert!(
            findings.iter().any(|f| f.severity == Severity::High),
            "a removed arm is high severity, got {:?}",
            findings
                .iter()
                .map(|f| f.severity.clone())
                .collect::<Vec<_>>()
        );
    }

    /// Collapsing specific arms into a wildcard is the same weakening
    /// with extra steps: every specific arm disappears.
    #[test]
    fn wildcard_collapse_is_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let findings = analyze(base, head);
        let removed = findings
            .iter()
            .filter(|f| f.rule == "removed_match_arm")
            .count();
        assert_eq!(
            removed,
            2,
            "both specific arms must be reported, got {:?}",
            rules(&findings)
        );
    }

    /// Arms that merely moved or swapped order are unchanged.
    #[test]
    fn reordered_match_arms_are_not_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n        1 => assert_eq!(cost(), 10),\n    }\n}\n";
        assert!(
            analyze(base, head).is_empty(),
            "reordering arms is not a change"
        );
    }

    /// Adding a case is coverage growth, not a finding.
    #[test]
    fn added_match_arm_is_not_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        assert!(analyze(base, head).is_empty());
    }

    /// An arm that survives in a *different* `match` on the same
    /// scrutinee still covers its cases, so splitting one `match` into
    /// two is not a weakening.
    #[test]
    fn arm_surviving_in_another_match_on_same_scrutinee_is_not_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        2 => assert_eq!(cost(), 20),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n    match value() {\n        2 => assert_eq!(cost(), 20),\n    }\n}\n";
        assert!(
            analyze(base, head).is_empty(),
            "an arm that survived on the same scrutinee still covers its case"
        );
    }

    /// Dropping a guard changes which inputs an arm covers: the guarded
    /// base arm no longer exists, so it is reported as removed.
    #[test]
    fn dropped_arm_guard_is_reported_as_removed_arm() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        v if v > 0 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    match value() {\n        v => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_match_arm"),
            "dropping a guard removes the guarded arm, got {:?}",
            rules(&findings)
        );
    }

    /// A `match` replaced by an `if`/`else` chain is a faithful
    /// refactor, not a weakening. Its assertions are still compared by
    /// the assertion rules; the structural removal is deliberately not
    /// reported.
    #[test]
    fn match_rewritten_as_if_else_is_not_reported() {
        let base = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    if value() == 1 {\n        assert_eq!(cost(), 10);\n    } else {\n        panic!(\"unexpected\");\n    }\n}\n";
        assert!(
            analyze(base, head).is_empty(),
            "a match refactored into if/else keeps its assertions"
        );
    }

    /// The visitor must descend into nested expressions, or a `match`
    /// hidden in a loop or closure would escape the comparison.
    #[test]
    fn match_inside_a_loop_is_compared() {
        let base = "#[test]\nfn cases() {\n    for v in values() {\n        match v {\n            1 => assert_eq!(cost(), 10),\n            _ => panic!(\"unexpected\"),\n        }\n    }\n}\n";
        let head = "#[test]\nfn cases() {\n    for v in values() {\n        match v {\n            _ => panic!(\"unexpected\"),\n        }\n    }\n}\n";
        let findings = analyze(base, head);
        assert!(
            rules(&findings).contains(&"removed_match_arm"),
            "an arm removed inside a loop must be reported, got {:?}",
            rules(&findings)
        );
    }

    /// A `match` that only exists in the head revision cannot have lost
    /// an arm, because there was nothing to lose.
    #[test]
    fn match_in_a_new_test_is_not_reported() {
        let head = "#[test]\nfn cases() {\n    match value() {\n        1 => assert_eq!(cost(), 10),\n        _ => panic!(\"unexpected\"),\n    }\n}\n";
        let findings = analyze_rust_test_change("tests/t.rs", None, head);
        assert!(
            !rules(&findings).contains(&"removed_match_arm"),
            "a brand-new test cannot have removed an arm"
        );
    }
}
