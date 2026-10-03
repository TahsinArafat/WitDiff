//! Transplantation of inline `#[cfg(test)]` test modules onto a base revision.
//!
//! See ADR-0010. The red/green experiment needs a base worktree that contains
//! the changed *tests* and nothing else. For a dedicated test file that is a
//! whole-file copy. For an inline `#[cfg(test)] mod tests` block the test and
//! the code it exercises share a file, so only part of the file may be moved.
//!
//! ## Why spans rather than hunks
//!
//! A patch-based approach cannot express this. Git forms hunks by proximity, not
//! by meaning, so a production edit and a test edit within one context window
//! land in the same hunk; keeping the hunk transplants the production change and
//! dropping it discards the test change. Measured, not assumed — see ADR-0010.
//!
//! ## The shape of the operation
//!
//! Head is parsed with `syn` to find the byte spans of its `#[cfg(test)]`
//! modules. Each such span replaces the span of the module with the same
//! identity in the base source. The head revision's own bytes are moved
//! verbatim, so the author's formatting survives and no formatter is involved.
//!
//! ## Conservative by construction
//!
//! Every precondition is checked before anything is written, and a file that
//! fails any of them is *refused by name* rather than spliced partially. A
//! partial splice would produce a worktree that is neither revision, and the
//! resulting red or green would be attributed to a code state that never
//! existed.

use std::collections::BTreeMap;

use syn::{spanned::Spanned, File, Item, ItemMod};

/// Why a file could not be spliced.
///
/// Each variant is reported to the operator, because "WitDiff declined" and
/// "WitDiff could not" call for different follow-up work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The base or head revision did not parse as Rust.
    Unparsable,
    /// Head has no `#[cfg(test)]` module, so there is nothing to transplant.
    NoTestModuleInHead,
    /// A test module in head has no counterpart at the same path in base, so
    /// there is no span to replace. A module new in head cannot be spliced.
    NoCounterpartInBase,
    /// A changed `#[test]` function lies outside every test module.
    TestOutsideTestModule,
    /// The file changed outside every test module, so it is not a test-only
    /// change and transplanting the modules alone would misrepresent it.
    ChangeOutsideTestModule,
}

impl RefusalReason {
    /// A stable token for the receipt.
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalReason::Unparsable => "unparsable",
            RefusalReason::NoTestModuleInHead => "no_test_module_in_head",
            RefusalReason::NoCounterpartInBase => "no_counterpart_in_base",
            RefusalReason::TestOutsideTestModule => "test_outside_test_module",
            RefusalReason::ChangeOutsideTestModule => "change_outside_test_module",
        }
    }

    /// Operator-facing explanation of what to do next.
    pub fn explanation(self) -> &'static str {
        match self {
            RefusalReason::Unparsable => {
                "the base or head revision could not be parsed as Rust"
            }
            RefusalReason::NoTestModuleInHead => {
                "the head revision contains no #[cfg(test)] module to transplant"
            }
            RefusalReason::NoCounterpartInBase => {
                "a #[cfg(test)] module is new in this revision, so it has no base counterpart to replace"
            }
            RefusalReason::TestOutsideTestModule => {
                "a changed #[test] function lies outside every #[cfg(test)] module"
            }
            RefusalReason::ChangeOutsideTestModule => {
                "the file also changed outside its #[cfg(test)] modules, so it is not a test-only change"
            }
        }
    }
}

/// The outcome of attempting to splice one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpliceOutcome {
    /// The spliced source, ready to be written into the base worktree.
    Spliced(String),
    /// The file carries no inline test change; nothing to do.
    NoInlineTestChange,
    /// The file could not be spliced safely.
    Refused(RefusalReason),
}

/// Byte span of a `#[cfg(test)]` module in one revision.
///
/// The module's identity (`mod tests`, or `outer::inner::tests` when nested) is
/// the key it is stored under rather than a field, so the two cannot disagree.
struct ModuleSpan {
    /// Byte range of the whole item, attributes included.
    start: usize,
    end: usize,
}

/// Build the spliced source: base revision, with each of its `#[cfg(test)]`
/// modules replaced by the head revision's module of the same identity.
///
/// Returns [`SpliceOutcome::NoInlineTestChange`] when head contains no test
/// module, so a caller can distinguish "nothing to do" from "refused".
pub fn splice_inline_tests(base_source: &str, head_source: &str) -> SpliceOutcome {
    let Ok(head_parsed) = syn::parse_file(head_source) else {
        return SpliceOutcome::Refused(RefusalReason::Unparsable);
    };
    let Ok(base_parsed) = syn::parse_file(base_source) else {
        return SpliceOutcome::Refused(RefusalReason::Unparsable);
    };

    let head_modules = collect_test_modules(&head_parsed, head_source);
    if head_modules.is_empty() {
        return SpliceOutcome::NoInlineTestChange;
    }

    let base_modules = collect_test_modules(&base_parsed, base_source);

    // Precondition 2: every head test module needs a base counterpart.
    if head_modules
        .keys()
        .any(|identity| !base_modules.contains_key(identity))
    {
        return SpliceOutcome::Refused(RefusalReason::NoCounterpartInBase);
    }

    // Precondition 3: every test module must have changed, or there is nothing
    // to transplant; a module that is byte-identical at base already exists
    // there and needs no splicing.
    let changed: Vec<(&String, &ModuleSpan)> = head_modules
        .iter()
        .filter(|(identity, module)| {
            let base_module = &base_modules[identity.as_str()];
            base_source[base_module.start..base_module.end] != head_source[module.start..module.end]
        })
        .collect();

    if changed.is_empty() {
        return SpliceOutcome::NoInlineTestChange;
    }

    // Precondition 3: no changed test function may sit outside a test module.
    if test_outside_modules(&head_parsed, head_source, &head_modules) {
        return SpliceOutcome::Refused(RefusalReason::TestOutsideTestModule);
    }

    // Replace from the end so earlier byte offsets stay valid.
    let mut replacements: Vec<(&ModuleSpan, &ModuleSpan)> = changed
        .iter()
        .map(|(identity, head_module)| (&base_modules[*identity], *head_module))
        .collect();
    replacements.sort_by_key(|(base_module, _)| std::cmp::Reverse(base_module.start));

    let mut spliced = base_source.to_owned();
    for (base_module, head_module) in replacements {
        // Precondition 5 is enforced by the caller, which is the only place
        // that can see the diff: see `verify`'s change-outside-module check.
        spliced.replace_range(
            base_module.start..base_module.end,
            &head_source[head_module.start..head_module.end],
        );
    }

    SpliceOutcome::Spliced(spliced)
}

/// The byte ranges of every `#[cfg(test)]` module in a revision.
///
/// Shared with the mutation module, which must never mutate test code: the two
/// features agreeing on what counts as a test is the point.
pub fn test_module_regions(source: &str) -> Vec<(usize, usize)> {
    let Ok(parsed) = syn::parse_file(source) else {
        return Vec::new();
    };
    let mut regions: Vec<(usize, usize)> = collect_test_modules(&parsed, source)
        .into_values()
        .map(|module| (module.start, module.end))
        .collect();
    regions.sort_unstable();
    regions
}

/// The identities of the `#[cfg(test)]` modules in a revision.
///
/// Recorded in the receipt so a reader knows which modules were transplanted,
/// not merely that the file was touched.
pub fn test_module_identities(source: &str) -> Vec<String> {
    let Ok(parsed) = syn::parse_file(source) else {
        return Vec::new();
    };
    collect_test_modules(&parsed, source).into_keys().collect()
}

/// Whether a byte offset falls inside any of `regions`.
fn in_regions(regions: &[(usize, usize)], offset: usize) -> bool {
    regions
        .iter()
        .any(|(start, end)| offset >= *start && offset < *end)
}

/// Collect `#[cfg(test)]` modules, keyed by module path.
fn collect_test_modules(parsed: &File, source: &str) -> BTreeMap<String, ModuleSpan> {
    let mut modules = BTreeMap::new();
    collect_from_items(&parsed.items, "", source, &mut modules);
    modules
}

fn collect_from_items(
    items: &[Item],
    prefix: &str,
    source: &str,
    modules: &mut BTreeMap<String, ModuleSpan>,
) {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };
        let identity = if prefix.is_empty() {
            module.ident.to_string()
        } else {
            format!("{prefix}::{}", module.ident)
        };
        if is_test_module(module) {
            let span = module.span();
            if let (Some(start), Some(end)) = (
                line_col_to_offset(source, span.start()),
                line_col_to_offset(source, span.end()),
            ) {
                modules.insert(identity.clone(), ModuleSpan { start, end });
            }
        }
        // A test module may itself contain modules; recurse so a nested
        // `#[cfg(test)] mod outer { mod inner; }` is found by its full path.
        if let Some((_, items)) = &module.content {
            collect_from_items(items, &identity, source, modules);
        }
    }
}

/// Convert a `proc_macro2` line/column position into a byte offset.
///
/// `LineColumn::column` counts UTF-8 *characters*, not bytes, while Rust string
/// slicing needs byte offsets. On a line containing any non-ASCII character the
/// two differ, so adding the column to a byte offset silently slices the wrong
/// range. Verified against a source whose identifier contained multi-byte
/// characters before relying on it.
///
/// The reported end position is inclusive of the final character, so the end
/// offset is taken one character past it.
fn line_col_to_offset(source: &str, position: proc_macro2::LineColumn) -> Option<usize> {
    let mut line_start = 0usize;
    for (index, text) in source.split('\n').enumerate() {
        if index + 1 == position.line {
            let mut chars = text.char_indices();
            let mut seen = 0usize;
            loop {
                match chars.next() {
                    Some((byte, _)) if seen == position.column => {
                        return Some(line_start + byte);
                    }
                    Some(_) => seen += 1,
                    // The column is at or past the end of the line: clamp to
                    // the line's end rather than rejecting the span.
                    None => return Some(line_start + text.len()),
                }
            }
        }
        line_start += text.len() + 1;
    }
    None
}

/// Whether a module is gated on `cfg(test)`.
fn is_test_module(module: &ItemMod) -> bool {
    module.attrs.iter().any(|attr| {
        let path = attr.path();
        if !path.is_ident("cfg") {
            return false;
        }
        // Match `#[cfg(test)]` exactly, not `#[cfg(not(test))]`, which would
        // select the *non-test* build.
        let rendered = attr
            .meta
            .require_list()
            .map(|list| list.tokens.to_string())
            .unwrap_or_default();
        let compact: String = rendered.chars().filter(|c| !c.is_whitespace()).collect();
        compact == "test"
    })
}

/// Whether any `#[test]` function in head lies outside every test module.
fn test_outside_modules(
    parsed: &File,
    source: &str,
    modules: &BTreeMap<String, ModuleSpan>,
) -> bool {
    let regions: Vec<(usize, usize)> = modules
        .values()
        .map(|module| (module.start, module.end))
        .collect();

    let mut found_outside = false;
    walk_items(&parsed.items, source, &regions, &mut found_outside);
    found_outside
}

fn walk_items(items: &[Item], source: &str, regions: &[(usize, usize)], found_outside: &mut bool) {
    for item in items {
        match item {
            Item::Fn(function) => {
                let is_test = function
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("test"));
                if is_test {
                    if let Some(offset) =
                        line_col_to_offset(source, function.sig.ident.span().start())
                    {
                        if !in_regions(regions, offset) {
                            *found_outside = true;
                        }
                    }
                }
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    walk_items(items, source, regions, found_outside);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"pub fn prod() -> i32 { 1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t() { assert_eq!(prod(), 1); }
}
"#;

    const HEAD: &str = r#"pub fn prod() -> i32 { 2 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t() { assert_eq!(prod(), 3); }
}
"#;

    fn spliced(base: &str, head: &str) -> String {
        match splice_inline_tests(base, head) {
            SpliceOutcome::Spliced(source) => source,
            other => panic!("expected a splice, got {other:?}"),
        }
    }

    /// The core guarantee: base production code survives, head test module is
    /// transplanted.
    #[test]
    fn splice_keeps_base_production_and_takes_head_tests() {
        let result = spliced(BASE, HEAD);
        assert!(
            result.contains("pub fn prod() -> i32 { 1 }"),
            "base production code must survive: {result}"
        );
        assert!(
            !result.contains("pub fn prod() -> i32 { 2 }"),
            "the production change must not be transplanted: {result}"
        );
        assert!(
            result.contains("assert_eq!(prod(), 3)"),
            "the head test must be transplanted: {result}"
        );
    }

    /// The result must still parse, or the experiment would fail for a reason
    /// that has nothing to do with the change.
    #[test]
    fn spliced_source_still_parses() {
        let result = spliced(BASE, HEAD);
        assert!(
            syn::parse_file(&result).is_ok(),
            "spliced output must be valid Rust: {result}"
        );
    }

    /// Text outside the modules is carried over byte for byte.
    #[test]
    fn splice_does_not_reformat_the_base() {
        let base = "fn a() {}\n\n\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(false); }\n}\n";
        let head = "fn a() {}\n\n\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        let result = spliced(base, head);
        assert!(
            result.starts_with("fn a() {}\n\n\n\n"),
            "base bytes outside the module must be untouched: {result:?}"
        );
    }

    /// An unchanged module needs no transplant.
    #[test]
    fn unchanged_module_is_not_a_splice() {
        assert_eq!(
            splice_inline_tests(BASE, BASE),
            SpliceOutcome::NoInlineTestChange
        );
    }

    /// A module new in head has no base span to replace.
    #[test]
    fn module_new_in_head_is_refused() {
        let base = "pub fn prod() -> i32 { 1 }\n";
        let head = "pub fn prod() -> i32 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        assert_eq!(
            splice_inline_tests(base, head),
            SpliceOutcome::Refused(RefusalReason::NoCounterpartInBase)
        );
    }

    /// An unparsable revision is refused rather than guessed at.
    #[test]
    fn unparsable_revision_is_refused() {
        assert_eq!(
            splice_inline_tests(BASE, "fn oops( {"),
            SpliceOutcome::Refused(RefusalReason::Unparsable)
        );
        assert_eq!(
            splice_inline_tests("fn oops( {", BASE),
            SpliceOutcome::Refused(RefusalReason::Unparsable)
        );
    }

    /// A changed `#[test]` outside any test module is not an inline-test-only
    /// change and must be refused.
    #[test]
    fn test_outside_module_is_refused() {
        // The module exists in both revisions, so the outside-test check is
        // reached rather than a missing-counterpart refusal firing first.
        let base = "pub fn prod() -> i32 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(false); }\n}\n";
        let head = "#[test]\nfn top_level() { assert!(true); }\n\npub fn prod() -> i32 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        assert_eq!(
            splice_inline_tests(base, head),
            SpliceOutcome::Refused(RefusalReason::TestOutsideTestModule)
        );
    }

    /// `#[cfg(not(test))]` selects the non-test build and must not be treated
    /// as a test module.
    #[test]
    fn cfg_not_test_is_not_a_test_module() {
        let base = "pub fn prod() -> i32 { 1 }\n";
        let head =
            "pub fn prod() -> i32 { 1 }\n\n#[cfg(not(test))]\nmod helper {\n    fn h() {}\n}\n";
        assert_eq!(
            splice_inline_tests(base, head),
            SpliceOutcome::NoInlineTestChange
        );
    }

    /// Multiple test modules in one file are all spliced.
    #[test]
    fn multiple_test_modules_are_spliced() {
        let base = "#[cfg(test)]\nmod a {\n    #[test]\n    fn t() { assert!(false); }\n}\n#[cfg(test)]\nmod b {\n    #[test]\n    fn t() { assert!(false); }\n}\n";
        let head = "#[cfg(test)]\nmod a {\n    #[test]\n    fn t() { assert!(true); }\n}\n#[cfg(test)]\nmod b {\n    #[test]\n    fn t() { assert!(true); }\n}\n";
        let result = spliced(base, head);
        assert_eq!(result.matches("assert!(true)").count(), 2, "{result}");
    }
}
