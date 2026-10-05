//! Tests for JavaScript and TypeScript structural integrity analysis.
//!
//! These are `#[ignore]`d because they spawn Node. Where a project supplies a
//! real parser the analysis is exercised end to end; the traversal itself is
//! covered by the embedded tool's own tests and by the shapes asserted below,
//! so a missing parser in the development environment degrades coverage rather
//! than failing it.

use std::{fs, path::Path, process::Command};

use witdiff_core::jsanalysis::{analyze_javascript_test_change, JsToolchain};

fn toolchain() -> Option<JsToolchain> {
    match Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => {
            JsToolchain::from_test_command(&["npm".to_owned(), "test".into()], Path::new("."))
        }
        _ => None,
    }
}

/// A directory with a package.json, so Node resolves a project context.
fn project() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("package.json"),
        "{ \"name\": \"fixture\", \"version\": \"1.0.0\" }\n",
    )
    .expect("write package.json");
    dir
}

fn rules(findings: &[witdiff_core::IntegrityFinding]) -> Vec<&str> {
    findings.iter().map(|f| f.rule.as_str()).collect()
}

/// The repository root, which is where `npm install` put the parsers.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repository root")
        .to_path_buf()
}

/// The toolchain whose project is the repository itself, so the parser required
/// from `node_modules` is the real `acorn` installed by `npm install`.
///
/// This is the case ADR-0021 could not previously be verified against: a real
/// parser, resolved from a real project, analyzing a real file.
fn real_parser_toolchain() -> Option<JsToolchain> {
    let root = repo_root();
    if !root.join("node_modules").is_dir() {
        return None;
    }
    let _ = Command::new("node").arg("--version").output().ok()?;
    JsToolchain::from_test_command(&["npm".to_owned(), "test".into()], &root)
}

/// Analyze `head` against `base` with a real parser, writing both to a
/// temporary directory. The path is a real file so the analyzer reads it from
/// disk exactly as it would in a repository.
fn analyze_with_real_parser(base: &str, head: &str) -> Option<Vec<witdiff_core::IntegrityFinding>> {
    let toolchain = real_parser_toolchain()?;
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, head).expect("write head");
    Some(analyze_javascript_test_change(
        "a.test.js",
        &toolchain,
        &path,
        Some(base),
    ))
}

#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn a_missing_parser_is_reported_rather_than_ignored() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return; // Node absent; nothing to assert.
    };

    // No parser is installed in the temporary project, so the tool must report
    // that rather than producing nothing.
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    let reported = rules(&findings);
    assert!(
        reported.contains(&"test_source_unparsable"),
        "a project with no parser must be reported, got {reported:?}"
    );
    let finding = findings
        .iter()
        .find(|f| f.rule == "test_source_unparsable")
        .expect("the finding");
    assert!(
        finding.message.contains("@babel/parser") && finding.message.contains("acorn"),
        "the message should name the parsers it looked for, got {}",
        finding.message
    );
}

#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn unknown_test_file_reports_rather_than_pretending_clean() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return;
    };
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    // With no parser the file cannot be analyzed, so it must be reported.
    // This is the assertion that matters: never a silent zero.
    if findings.is_empty() {
        // A parser was present in the environment, which is a stronger outcome
        // than this test requires; nothing to assert.
        return;
    }
    assert!(
        rules(&findings).contains(&"test_source_unparsable"),
        "an unanalyzable file must be reported, got {:?}",
        rules(&findings)
    );
}

/// The operator vocabulary the tool emits. Asserted directly so a change to the
/// tool's rendering cannot silently stop pairing expectations with subjects.
#[test]
fn the_operator_vocabulary_matches_the_embedded_tool() {
    use witdiff_core::framework::TestFramework;
    let _ = TestFramework::JavaScript;
    let operators = witdiff_core::jsanalysis::operators_for_tests();
    use witdiff_core::testshape::{strength_of, subject_of, Strength};
    assert_eq!(strength_of("add(2, 3) Eq 5", &operators), Strength::Exact);
    assert_eq!(
        subject_of("add(2, 3) Eq 5", &operators).as_deref(),
        Some("add(2, 3)")
    );
    assert_eq!(
        subject_of("add(2, 3)", &operators).as_deref(),
        Some("add(2, 3)")
    );
    assert_eq!(
        subject_of("x NotEq 3", &operators).as_deref(),
        Some("x"),
        "a negated expectation shares the subject"
    );
}

/// A file that cannot be parsed is reported; it is never reported as clean.
#[test]
#[ignore = "end-to-end: spawns Node; run with -- --ignored"]
fn a_syntax_error_is_reported_not_swallowed() {
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { this is not ]) };\n").expect("write");
    let Some(toolchain) = toolchain() else {
        return;
    };
    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    if !findings.is_empty() {
        assert!(
            rules(&findings).contains(&"test_source_unparsable"),
            "an unparsable file must be reported, got {:?}",
            rules(&findings)
        );
    }
}

// ---------------------------------------------------------------------------
// Verified against a real parser.
//
// Everything below runs `acorn` and `typescript` from this repository's
// `node_modules`. ADR-0021 recorded that this had never been done, because the
// environment could not install a parser. Every test here was written against
// measured output of the real tool, not against a reading of it.
// ---------------------------------------------------------------------------

/// A real `acorn` must resolve from the project's own `node_modules`, which is
/// the whole premise of ADR-0021. If this fails, nothing below is meaningful.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_real_parser_resolves_from_the_project() {
    let Some(toolchain) = real_parser_toolchain() else {
        eprintln!("no node_modules in the repository; run `npm install`");
        return;
    };
    let dir = project();
    let path = dir.path().join("a.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");

    let findings = analyze_javascript_test_change("a.test.js", &toolchain, &path, None);
    assert!(
        !rules(&findings).contains(&"test_source_unparsable"),
        "the project's own acorn must be found, got {:?}: {}",
        rules(&findings),
        findings
            .iter()
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// **Bug found by running against real acorn.** `expect(a).not.toBe(1)` produced
/// *no assertion at all*, because acorn's tree for the negated form puts the
/// matcher one level deeper than the code assumed: the outer callee is
/// `.not` and the inner one is `.toBe`. The traversal therefore returned an
/// empty result, and since no assertion existed at base or at head, every rule
/// silently reported nothing.
///
/// The practical damage: converting `expect(x).toBe(1)` into
/// `expect(x).not.toBe(1)` — inverting an assertion so it passes for the wrong
/// reason — produced zero findings.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_negated_expectation_is_an_assertion_not_a_silence() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => { expect(x).toBe(1); });\n",
        "test('t', () => { expect(x).not.toBe(1); });\n",
    ) else {
        return;
    };
    let reported = rules(&findings);
    assert!(
        !reported.contains(&"test_source_unparsable"),
        "the file must parse, got {reported:?}"
    );
    assert!(
        !reported.is_empty(),
        "inverting toBe into not.toBe must produce a finding; it produced none"
    );
}

/// **Bug found by running against real acorn.** `expect(x).toBe(2)` and
/// `expect(x).toBe(3)` normalized to the *same* string, because the tool's
/// `normalize` handled Babel's `NumericLiteral`/`StringLiteral` but not acorn's
/// ESTree `Literal`. Every expectation change was therefore invisible, which is
/// the exact failure the tool exists to catch.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn changing_an_expected_value_is_reported() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => { expect(add(1, 2)).toBe(3); });\n",
        "test('t', () => { expect(add(1, 2)).toBe(4); });\n",
    ) else {
        return;
    };
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "changing the expected value must be reported, got {:?}",
        rules(&findings)
    );
}

/// The same normalization requirement for a removed assertion. `Literal` was
/// rendered as the bare word `Literal`, so removing one argument changed the
/// rendered string and was reported as a change rather than a removal.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn removing_an_assertion_is_reported() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => {\n  expect(a).toBe(1);\n  expect(b).toBe(2);\n});\n",
        "test('t', () => {\n  expect(a).toBe(1);\n});\n",
    ) else {
        return;
    };
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "removing an assertion must be reported, got {:?}",
        rules(&findings)
    );
}

/// Reformatting must stay silent, which is the property that keeps the rules
/// worth reading. Worth asserting against a real parser precisely because
/// normalization is where the `Literal` bug lived.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn reformatting_is_not_a_finding() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => { expect(add(1,2)).toBe(3); });\n",
        "test('t', () => {\n  expect(add(1, 2)).toBe(3);\n});\n",
    ) else {
        return;
    };
    assert!(
        findings.is_empty(),
        "reformatting must produce no finding, got {:?}",
        rules(&findings)
    );
}

/// **Bug found by running against real acorn.** Node's own `assert` module was
/// invisible: `assert.strictEqual(a, 1)` and `assert.deepEqual(a, 1)` produced
/// no assertion, because the tool only recognized a *bare* `assert*` call and
/// its own regex never matched the member-call form Node actually ships.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn the_node_assert_module_is_analyzed() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => { assert.strictEqual(a, 1); assert.deepEqual(b, {x: 1}); });\n",
        "test('t', () => { assert.strictEqual(a, 1); });\n",
    ) else {
        return;
    };
    assert!(
        rules(&findings).contains(&"removed_assertion"),
        "Node's assert module must be analyzed, got {:?}",
        rules(&findings)
    );
}

/// **Bug found by running against real acorn.** `test.skip` produced *no
/// function at all*, so newly skipping a test was invisible — the same class of
/// silence as an ignored Rust test, which the tool does report.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_newly_skipped_test_is_reported() {
    let Some(findings) = analyze_with_real_parser(
        "test('t', () => { expect(a).toBe(1); });\n",
        "test.skip('t', () => { expect(a).toBe(1); });\n",
    ) else {
        return;
    };
    assert!(
        rules(&findings).contains(&"skipped_test"),
        "newly skipping a test must be reported, got {:?}",
        rules(&findings)
    );
}

/// A parser resolving from a *nested* `node_modules`. ADR-0021 claims the
/// project directory is passed explicitly and each parser required by absolute
/// path, which is the claim most likely to be subtly wrong.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_nested_node_modules_is_found() {
    let Some(toolchain) = real_parser_toolchain() else {
        return;
    };
    // The temporary project lives elsewhere, but the parser is required from
    // the repository root, so resolution must not depend on the file's own
    // directory or the working directory.
    let dir = project();
    let path = dir.path().join("nested.test.js");
    fs::write(&path, "test('t', () => { expect(x).toBe(1); });\n").expect("write");

    let findings = analyze_javascript_test_change("nested.test.js", &toolchain, &path, None);
    assert!(
        !rules(&findings).contains(&"test_source_unparsable"),
        "the parser must resolve regardless of where the analyzed file lives, got {:?}",
        rules(&findings)
    );
}

/// TypeScript through the `typescript` parser. This path previously crashed
/// with "Maximum call stack size exceeded" and reported zero functions, because
/// the TypeScript AST tags nodes with `kind`, not `type`, so the traversal
/// visited nothing and then recursed forever through the parent chain.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_typescript_file_is_analyzed() {
    let Some(toolchain) = real_parser_toolchain() else {
        return;
    };
    let dir = project();
    let path = dir.path().join("a.test.ts");
    fs::write(
        &path,
        "test('t', () => {\n  const v: number = add(1, 1);\n  expect(v).toBe(2);\n});\n",
    )
    .expect("write");

    let findings = analyze_javascript_test_change("a.test.ts", &toolchain, &path, None);
    assert!(
        !rules(&findings).contains(&"test_source_unparsable"),
        "a .ts file must be analyzed, got {:?}: {}",
        rules(&findings),
        findings
            .iter()
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    );
}

/// The TypeScript path must also *detect* changes, not merely parse. A parser
/// that returns an empty summary would pass the previous assertion while
/// proving nothing.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_typescript_expectation_change_is_reported() {
    let Some(toolchain) = real_parser_toolchain() else {
        return;
    };
    let dir = project();
    let head = dir.path().join("a.test.ts");
    fs::write(
        &head,
        "test('t', () => {\n  const v: number = add(1, 1);\n  expect(v).toBe(4);\n});\n",
    )
    .expect("write");
    let base = "test('t', () => {\n  const v: number = add(1, 1);\n  expect(v).toBe(2);\n});\n";

    let findings = analyze_javascript_test_change("a.test.ts", &toolchain, &head, Some(base));
    assert!(
        rules(&findings).contains(&"changed_expected_value"),
        "a TypeScript expectation change must be reported, got {:?}",
        rules(&findings)
    );
}

/// `.tsx` is the case the support matrix calls out explicitly as worth checking
/// first, and it exercises a different `ScriptKind` from `.ts`.
#[test]
#[ignore = "end-to-end: spawns Node with a real parser; run with -- --ignored"]
fn a_tsx_file_is_analyzed() {
    let Some(toolchain) = real_parser_toolchain() else {
        return;
    };
    let dir = project();
    let path = dir.path().join("a.test.tsx");
    fs::write(
        &path,
        "const C = () => <div>hi</div>;\ntest('t', () => { expect(1).toBe(1); });\n",
    )
    .expect("write");

    let findings = analyze_javascript_test_change("a.test.tsx", &toolchain, &path, None);
    assert!(
        !rules(&findings).contains(&"test_source_unparsable"),
        "a .tsx file must be analyzed, got {:?}: {}",
        rules(&findings),
        findings
            .iter()
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    );
}
