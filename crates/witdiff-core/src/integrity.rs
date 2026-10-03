use crate::model::{IntegrityFinding, Severity};

pub fn analyze_test_diff(path: &str, diff: &str) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();

    for raw in diff.lines() {
        if raw.starts_with("+++") || raw.starts_with("---") {
            continue;
        }
        let (added, removed, line) = if let Some(line) = raw.strip_prefix('+') {
            (true, false, line.trim())
        } else if let Some(line) = raw.strip_prefix('-') {
            (false, true, line.trim())
        } else {
            continue;
        };

        if removed && is_assertion(line) {
            findings.push(finding(
                Severity::High,
                path,
                raw,
                "removed_assertion",
                "an assertion was removed from a changed test file",
            ));
        }
        if removed && line.contains("#[test]") {
            findings.push(finding(
                Severity::High,
                path,
                raw,
                "removed_test_attribute",
                "a #[test] attribute was removed",
            ));
        }
        if added && line.contains("#[ignore") {
            findings.push(finding(
                Severity::High,
                path,
                raw,
                "ignored_test",
                "a test was marked ignored",
            ));
        }
        if added && line.contains("#[should_panic") {
            findings.push(finding(
                Severity::Warning,
                path,
                raw,
                "added_should_panic",
                "#[should_panic] was added; verify that this does not weaken the expected behavior",
            ));
        }
        if added && is_trivially_true_assertion(line) {
            findings.push(finding(
                Severity::High,
                path,
                raw,
                "trivial_assertion",
                "a trivially true assertion was added",
            ));
        }
    }

    findings
}

fn is_assertion(line: &str) -> bool {
    [
        "assert!(",
        "assert_eq!(",
        "assert_ne!(",
        "debug_assert!(",
        "debug_assert_eq!(",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

fn is_trivially_true_assertion(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("assert!(true)") || compact.contains("assert_eq!(true,true)")
}

fn finding(
    severity: Severity,
    path: &str,
    line: &str,
    rule: &str,
    message: &str,
) -> IntegrityFinding {
    IntegrityFinding {
        severity,
        path: path.to_owned(),
        line: line.to_owned(),
        rule: rule.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_removed_assertions_and_ignored_tests() {
        let diff = r#"
-    assert_eq!(status, 401);
+    #[ignore]
+    assert!(true);
"#;
        let findings = analyze_test_diff("tests/auth.rs", diff);
        assert_eq!(findings.len(), 3);
        assert!(findings.iter().any(|f| f.rule == "removed_assertion"));
        assert!(findings.iter().any(|f| f.rule == "ignored_test"));
        assert!(findings.iter().any(|f| f.rule == "trivial_assertion"));
    }
}
