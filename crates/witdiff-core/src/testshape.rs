//! The language-neutral comparison engine shared by the structural analyzers.
//!
//! See ADR-0016 (Python) and ADR-0017 (Go). Each analyzer supplies a summary of
//! its language's test structure; this module decides what the difference
//! between two summaries means.
//!
//! ## Why this is extracted
//!
//! The rule set is the product's semantics, and it must not drift between
//! languages. Duplicating it per language would mean a fix to the pairing logic
//! — such as the false positive where a leading negation looked like a removed
//! assertion — had to be applied in every copy, and a missed copy is a silent
//! wrong answer rather than a compile error.
//!
//! ## Normalized form
//!
//! Each analyzer renders expressions to a string in a form that is stable
//! across formatting and across toolchain versions. Comparison is on those
//! strings, so reformatting never registers as a change. The vocabulary of
//! comparison operators differs per language and is supplied by the caller
//! rather than hardcoded here.

use serde::{Deserialize, Serialize};

use crate::model::{IntegrityFinding, Severity};

/// One assertion observed in a parsed revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    /// 1-based line within its own revision.
    pub line: usize,
    /// Normalized test expression.
    pub test: String,
    /// Normalized failure message, when the assertion has one.
    #[serde(default)]
    pub message: Option<String>,
}

/// One test function observed in a parsed revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFunction {
    pub name: String,
    pub line: usize,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    /// The test is skipped, so it checks nothing.
    #[serde(default)]
    pub skipped: bool,
    /// The body is empty.
    #[serde(default)]
    pub body_is_empty: bool,
}

/// The structural shape of one revision of a test file.
#[derive(Debug, Default, Deserialize)]
pub struct FileSummary {
    pub format: u32,
    /// Set when the file could not be parsed, with the reason.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub functions: Vec<TestFunction>,
}

/// How strongly an assertion constrains behavior.
///
/// Mirrors the Rust model so a reader learns one set of rules across languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    /// Asserts a constant, `assert True` or `if true`.
    Trivial,
    /// A bare truthiness or existence check with no comparison.
    Predicate,
    /// A comparison against an expectation.
    Exact,
}

/// The operator vocabulary a language uses to express a comparison.
///
/// Supplied by the caller because `==` in Python renders as `Eq` and `!=` in Go
/// renders as `!=`; the shared engine must not assume either.
pub struct Operators {
    /// Substrings that mark an exact comparison, longest first so a multi-word
    /// operator is matched before a prefix of it.
    pub comparisons: &'static [&'static str],
    /// Substrings that mark a leading unary operator, which wraps a subject
    /// rather than replacing it.
    pub unary: &'static [&'static str],
    /// Rendered forms that assert nothing.
    pub trivials: &'static [&'static str],
}

/// Analyze one file's change given both revisions' summaries.
///
/// `language` only affects wording; the rules are identical across languages.
pub fn analyze(
    path: &str,
    language: &str,
    operators: &Operators,
    base: Option<&FileSummary>,
    head: &FileSummary,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    // Additive rules: independent of whether the base could be analyzed.
    for function in &head.functions {
        let existing = base.and_then(|summary| {
            summary
                .functions
                .iter()
                .find(|candidate| candidate.name == function.name)
        });

        if function.skipped && !existing.is_some_and(|previous| previous.skipped) {
            findings.push(finding(
                Severity::High,
                path,
                function.line,
                "skipped_test",
                format!(
                    "test `{}` is skipped, so it no longer checks anything",
                    function.name
                ),
            ));
        }

        for assertion in &function.assertions {
            if strength_of(&assertion.test, operators) != Strength::Trivial {
                continue;
            }
            // A trivial assertion that already existed proves nothing about
            // this change, so only newly introduced ones are reported.
            let preexisting = existing.is_some_and(|previous| {
                previous
                    .assertions
                    .iter()
                    .any(|candidate| candidate.test == assertion.test)
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
                    "test `{}` contains an assertion that cannot fail (`{}`)",
                    function.name, assertion.test
                ),
            ));
        }

        // A test with no assertions at all is the language-neutral form of a
        // gutted test. Only reported when the base had some, so a test that
        // never asserted is not treated as a regression.
        if function.assertions.is_empty()
            && !function.body_is_empty
            && existing.is_some_and(|previous| !previous.assertions.is_empty())
        {
            findings.push(finding(
                Severity::High,
                path,
                function.line,
                "removed_assertion",
                format!(
                    "test `{}` no longer asserts anything; every check was removed",
                    function.name
                ),
            ));
        }
    }

    let Some(base) = base else {
        return findings;
    };

    // Comparative rules.
    for head_function in &head.functions {
        let Some(base_function) = base
            .functions
            .iter()
            .find(|candidate| candidate.name == head_function.name)
        else {
            continue;
        };

        for removed in missing_assertions(base_function, head_function) {
            // An assertion whose subject survived under a different form was
            // rewritten, not deleted. Without this, adding a negation — which
            // strengthens a test — is reported as a removal plus an addition.
            let rewritten = head_function
                .assertions
                .iter()
                .any(|candidate| same_subject(&removed, candidate, operators));
            if rewritten {
                continue;
            }
            findings.push(finding(
                Severity::High,
                path,
                removed.line,
                "removed_assertion",
                format!(
                    "an assertion was removed from test `{}` (base line {}: `{}`)",
                    head_function.name, removed.line, removed.test
                ),
            ));
        }

        findings.extend(weakening_findings(
            path,
            head_function,
            base_function,
            operators,
        ));
    }

    for base_function in &base.functions {
        if !head
            .functions
            .iter()
            .any(|candidate| candidate.name == base_function.name)
        {
            findings.push(finding(
                Severity::High,
                path,
                base_function.line,
                "removed_test",
                format!(
                    "{} test `{}` no longer exists (base line {}); its assertions are no longer checked",
                    language, base_function.name, base_function.line
                ),
            ));
        }
    }

    findings
}

/// Assertions present at base and absent at head, matched on normalized text.
fn missing_assertions(base: &TestFunction, head: &TestFunction) -> Vec<Assertion> {
    let mut unmatched: Vec<&Assertion> = head.assertions.iter().collect();
    let mut missing = Vec::new();
    for assertion in &base.assertions {
        match unmatched
            .iter()
            .position(|candidate| candidate.test == assertion.test)
        {
            Some(index) => {
                unmatched.remove(index);
            }
            None => missing.push(assertion.clone()),
        }
    }
    missing
}

/// Assertions that survived in weaker form, or whose expectation moved.
fn weakening_findings(
    path: &str,
    head_function: &TestFunction,
    base_function: &TestFunction,
    operators: &Operators,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    // Consume identical assertions first, so a newly added assertion whose
    // subject collides with an existing one is not mistaken for a rewrite.
    // This ordering bug was found by writing the test, not by inspection.
    let mut remaining_base: Vec<&Assertion> = base_function.assertions.iter().collect();
    let mut remaining_head: Vec<&Assertion> = Vec::new();
    for head_assertion in &head_function.assertions {
        match remaining_base
            .iter()
            .position(|candidate| candidate.test == head_assertion.test)
        {
            Some(index) => {
                remaining_base.remove(index);
            }
            None => remaining_head.push(head_assertion),
        }
    }

    for head_assertion in remaining_head {
        let Some(base_assertion) = remaining_base
            .iter()
            .find(|candidate| same_subject(candidate, head_assertion, operators))
        else {
            continue;
        };

        let base_strength = strength_of(&base_assertion.test, operators);
        let head_strength = strength_of(&head_assertion.test, operators);

        if base_strength > head_strength {
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "weakened_assertion",
                format!(
                    "test `{}` replaced `{}` with the weaker `{}`",
                    head_function.name, base_assertion.test, head_assertion.test
                ),
            ));
            continue;
        }

        if base_strength == Strength::Exact
            && head_strength == Strength::Exact
            && base_assertion.test != head_assertion.test
        {
            findings.push(finding(
                Severity::High,
                path,
                head_assertion.line,
                "changed_expected_value",
                format!(
                    "test `{}` changed the expectation from `{}` to `{}`",
                    head_function.name, base_assertion.test, head_assertion.test
                ),
            ));
        }
    }

    findings
}

/// Classify an assertion's strength.
pub fn strength_of(normalized: &str, operators: &Operators) -> Strength {
    if is_trivial(normalized, operators) {
        return Strength::Trivial;
    }
    if operators
        .comparisons
        .iter()
        .any(|operator| normalized.contains(operator))
    {
        Strength::Exact
    } else {
        Strength::Predicate
    }
}

fn is_trivial(normalized: &str, operators: &Operators) -> bool {
    let trimmed = normalized.trim();
    operators.trivials.contains(&trimmed)
}

/// Whether two assertions clearly concern the same subject expression.
///
/// Deliberately strict: a false pairing would invent a weakening the code does
/// not contain, which trains a reader to ignore the rule.
fn same_subject(a: &Assertion, b: &Assertion, operators: &Operators) -> bool {
    match (
        subject_of(&a.test, operators),
        subject_of(&b.test, operators),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The leading expression of a normalized assertion, with unary operators
/// stripped.
///
/// Stripping is what makes `is_even(3)` and `Not is_even(3)` share a subject.
/// Without it, adding a negation is reported as a removed assertion — a false
/// positive on a change that strengthens the test, which downgrades a genuinely
/// verified change because high-severity findings block verification.
pub fn subject_of(normalized: &str, operators: &Operators) -> Option<String> {
    if let Some(index) = operators
        .comparisons
        .iter()
        .filter_map(|operator| normalized.find(operator))
        .min()
    {
        return Some(strip_unary(normalized[..index].trim(), operators));
    }
    let trimmed = normalized.trim();
    (!trimmed.is_empty()).then(|| strip_unary(trimmed, operators))
}

fn strip_unary(expression: &str, operators: &Operators) -> String {
    let mut current = expression.trim();
    loop {
        let mut stripped = false;
        for prefix in operators.unary {
            if let Some(rest) = current.strip_prefix(prefix) {
                current = rest.trim();
                stripped = true;
                break;
            }
        }
        if !stripped {
            return current.to_owned();
        }
    }
}

/// Build an integrity finding.
pub fn finding(
    severity: Severity,
    path: &str,
    line: usize,
    rule: &str,
    message: String,
) -> IntegrityFinding {
    IntegrityFinding {
        severity,
        path: path.to_owned(),
        line: line.to_string(),
        rule: rule.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYTHON: Operators = Operators {
        comparisons: &[
            " Eq ", " NotEq ", " Lt ", " LtE ", " Gt ", " GtE ", " Is ", " IsNot ", " In ",
            " NotIn ",
        ],
        unary: &["Not ", "-", "+", "~"],
        trivials: &["True", "1", "None", "[]", "{}", "()", "''", "\"\\\"\""],
    };

    fn function(name: &str, assertions: &[&str]) -> TestFunction {
        TestFunction {
            name: name.to_owned(),
            line: 1,
            assertions: assertions
                .iter()
                .enumerate()
                .map(|(index, test)| Assertion {
                    line: index + 1,
                    test: (*test).to_owned(),
                    message: None,
                })
                .collect(),
            skipped: false,
            body_is_empty: false,
        }
    }

    fn summary(functions: Vec<TestFunction>) -> FileSummary {
        FileSummary {
            format: 1,
            error: None,
            functions,
        }
    }

    fn rules(findings: &[IntegrityFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    #[test]
    fn identical_revisions_produce_no_findings() {
        let source = summary(vec![function("t", &["f() Eq 1"])]);
        let findings = analyze("t.py", "Python", &PYTHON, Some(&source), &source);
        assert!(findings.is_empty(), "got {:?}", rules(&findings));
    }

    #[test]
    fn weakening_is_reported() {
        let base = summary(vec![function("t", &["f() Eq 1"])]);
        let head = summary(vec![function("t", &["f()"])]);
        let findings = analyze("t.py", "Python", &PYTHON, Some(&base), &head);
        assert!(rules(&findings).contains(&"weakened_assertion"));
    }

    /// The language-neutral version of "every check was deleted".
    #[test]
    fn removing_every_assertion_is_reported() {
        let base = summary(vec![function("t", &["f() Eq 1"])]);
        let head = summary(vec![function("t", &[])]);
        let findings = analyze("t.go", "Go", &PYTHON, Some(&base), &head);
        assert!(
            rules(&findings).contains(&"removed_assertion"),
            "a test that no longer asserts anything must be reported, got {:?}",
            rules(&findings)
        );
    }

    /// A test that never asserted anything is not a regression.
    #[test]
    fn a_test_that_never_asserted_is_not_reported() {
        let base = summary(vec![function("t", &[])]);
        let head = summary(vec![function("t", &[])]);
        let findings = analyze("t.go", "Go", &PYTHON, Some(&base), &head);
        assert!(findings.is_empty(), "got {:?}", rules(&findings));
    }

    #[test]
    fn a_new_skip_is_reported_and_an_existing_one_is_not() {
        let mut skipped = function("t", &["f() Eq 1"]);
        skipped.skipped = true;
        let active = function("t", &["f() Eq 1"]);

        let newly = analyze(
            "t.py",
            "Python",
            &PYTHON,
            Some(&summary(vec![active.clone()])),
            &summary(vec![skipped.clone()]),
        );
        assert!(rules(&newly).contains(&"skipped_test"));

        let already = analyze(
            "t.py",
            "Python",
            &PYTHON,
            Some(&summary(vec![skipped.clone()])),
            &summary(vec![skipped]),
        );
        assert!(
            !rules(&already).contains(&"skipped_test"),
            "a pre-existing skip is not a new weakening"
        );
    }

    #[test]
    fn removing_a_test_is_reported() {
        let base = summary(vec![function("a", &["x Eq 1"]), function("b", &["y Eq 2"])]);
        let head = summary(vec![function("a", &["x Eq 1"])]);
        let findings = analyze("t.py", "Python", &PYTHON, Some(&base), &head);
        assert!(rules(&findings).contains(&"removed_test"));
    }

    #[test]
    fn changing_an_expectation_is_reported() {
        let base = summary(vec![function("t", &["f() Eq 1"])]);
        let head = summary(vec![function("t", &["f() Eq 2"])]);
        let findings = analyze("t.py", "Python", &PYTHON, Some(&base), &head);
        assert!(rules(&findings).contains(&"changed_expected_value"));
    }

    /// The false positive found by running the Python analyzer on a legitimate
    /// fix, asserted here so it cannot return in any language.
    #[test]
    fn adding_a_negation_is_not_a_removal() {
        let base = summary(vec![function("t", &["is_even(3)"])]);
        let head = summary(vec![function("t", &["Not is_even(3)"])]);
        let findings = analyze("t.py", "Python", &PYTHON, Some(&base), &head);
        assert!(
            findings.is_empty(),
            "strengthening an assertion must produce no findings, got {:?}",
            rules(&findings)
        );
    }

    #[test]
    fn a_trivial_assertion_is_reported_only_when_new() {
        let trivial = function("t", &["True"]);
        let other = function("t", &["f() Eq 1"]);

        let newly = analyze(
            "t.py",
            "Python",
            &PYTHON,
            Some(&summary(vec![other.clone()])),
            &summary(vec![trivial.clone()]),
        );
        assert!(rules(&newly).contains(&"trivial_assertion"));

        let already = analyze(
            "t.py",
            "Python",
            &PYTHON,
            Some(&summary(vec![trivial.clone()])),
            &summary(vec![trivial]),
        );
        assert!(already.is_empty(), "got {:?}", rules(&already));
    }

    /// A new file cannot have weakened anything.
    #[test]
    fn a_new_file_has_no_comparative_findings() {
        let head = summary(vec![function("t", &["f() Eq 1"])]);
        let findings = analyze("t.py", "Python", &PYTHON, None, &head);
        assert!(findings.is_empty(), "got {:?}", rules(&findings));
    }

    #[test]
    fn subjects_strip_leading_unary_operators() {
        assert_eq!(subject_of("Not f()", &PYTHON).as_deref(), Some("f()"));
        assert_eq!(subject_of("Not Not f()", &PYTHON).as_deref(), Some("f()"));
        assert_eq!(subject_of("f() Eq 1", &PYTHON).as_deref(), Some("f()"));
        // A bare subject with no comparison is its own subject.
        assert_eq!(subject_of("f()", &PYTHON).as_deref(), Some("f()"));
    }
}
