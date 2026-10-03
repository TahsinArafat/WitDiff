//! Changed-code mutation operators and mutant representation.
//!
//! See ADR-0011. Mutation answers a question the red/green experiment cannot: a
//! transplanted test can fail on the base revision for the right reason and
//! still barely constrain the changed code. Mutating that code and observing
//! whether the tests notice measures how tightly the contract is pinned.
//!
//! ## Not a proof
//!
//! Everything here produces *supplementary* evidence. A surviving mutant is a
//! question about test strength, not a verdict about correctness, and nothing
//! in this module can turn a status into `verified`. That is enforced by the
//! caller in [`crate::verify`], which never consults mutation results when
//! computing a status.
//!
//! ## Production code only
//!
//! A mutant is applied by rewriting a byte span in the head source. Test code is
//! never inside a mutation span, and that is enforced by construction rather
//! than by convention: candidates come from parsed expression nodes that lie
//! outside every `#[cfg(test)]` module. Mutating a test's own expected value
//! would change the oracle and make the result meaningless — observed while
//! designing this, where a naive search-and-replace rewrote an assertion's
//! literal alongside the production one and the mutant appeared to survive.

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::{spanned::Spanned, BinOp, Expr, ExprBinary, File, Item, Lit};

/// One mutation that can be applied to a source file.
///
/// The span is a byte range into the head source. Applying a mutant replaces
/// exactly that range, which is what makes "change one thing, and only in
/// production code" checkable rather than aspirational.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mutant {
    /// Deterministic identifier: stable for the same file, span and operator.
    pub id: String,
    /// Repository-relative path of the mutated file.
    pub path: String,
    /// Operator name, stable across runs.
    pub operator: String,
    /// 1-based line of the mutation, for reporting.
    pub line: usize,
    /// Byte range replaced in the head source.
    pub start: usize,
    pub end: usize,
    /// Original source text of the span.
    pub original: String,
    /// Replacement text.
    pub replacement: String,
    /// Enclosing function name, when one could be determined.
    pub function: Option<String>,
}

impl Mutant {
    /// Apply this mutant to `source`, returning the mutated text.
    ///
    /// Returns `None` when the span no longer matches the recorded original, so
    /// a stale mutant produces an error rather than a silently different edit.
    pub fn apply(&self, source: &str) -> Option<String> {
        let existing = source.get(self.start..self.end)?;
        if existing != self.original {
            return None;
        }
        let mut out = String::with_capacity(source.len() + self.replacement.len());
        out.push_str(&source[..self.start]);
        out.push_str(&self.replacement);
        out.push_str(&source[self.end..]);
        Some(out)
    }
}

/// A mutation operator, as a closed set.
///
/// A closed set rather than free-form names so the receipt's operator values are
/// stable and a consumer can switch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `==` <-> `!=`, and the same flip for reference equality.
    EqualityFlip,
    /// `<` -> `<=`, `<=` -> `<`, `>` -> `>=`, `>=` -> `>`.
    ComparisonBoundary,
    /// `&&` <-> `||`.
    LogicalFlip,
    /// `true` <-> `false`.
    BooleanFlip,
}

impl Operator {
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::EqualityFlip => "equality_flip",
            Operator::ComparisonBoundary => "comparison_boundary",
            Operator::LogicalFlip => "logical_flip",
            Operator::BooleanFlip => "boolean_flip",
        }
    }
}

/// Generate every mutant for one changed file.
///
/// `changed_lines` restricts generation to functions the diff touches, so cost
/// grows with the change rather than with the file. `test_regions` are byte
/// ranges that must never be mutated; they come from the inline-test locator
/// (ADR-0010) so a `#[cfg(test)] mod tests` in the same file is excluded.
pub fn mutants_for_file(
    path: &str,
    source: &str,
    changed_lines: &[usize],
    test_regions: &[(usize, usize)],
) -> Vec<Mutant> {
    let Ok(parsed) = syn::parse_file(source) else {
        return Vec::new();
    };

    let mut mutants = Vec::new();
    collect_from_items(
        &parsed.items,
        path,
        source,
        changed_lines,
        test_regions,
        &mut mutants,
    );

    // Deterministic order: the caller may truncate, and a truncated run must
    // truncate the same way every time.
    mutants.sort_by(|a, b| (a.start, a.operator.as_str()).cmp(&(b.start, b.operator.as_str())));
    mutants.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.operator == b.operator);
    mutants
}

fn collect_from_items(
    items: &[Item],
    path: &str,
    source: &str,
    changed_lines: &[usize],
    test_regions: &[(usize, usize)],
    mutants: &mut Vec<Mutant>,
) {
    for item in items {
        match item {
            Item::Fn(function_item) => {
                let name = function_item.sig.ident.to_string();
                collect_from_block(
                    &function_item.block,
                    path,
                    source,
                    changed_lines,
                    test_regions,
                    Some(&name),
                    mutants,
                );
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_from_items(items, path, source, changed_lines, test_regions, mutants);
                }
            }
            Item::Impl(implementation) => {
                for inner in &implementation.items {
                    if let syn::ImplItem::Fn(method) = inner {
                        let name = method.sig.ident.to_string();
                        collect_from_block(
                            &method.block,
                            path,
                            source,
                            changed_lines,
                            test_regions,
                            Some(&name),
                            mutants,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_from_block(
    block: &syn::Block,
    path: &str,
    source: &str,
    changed_lines: &[usize],
    test_regions: &[(usize, usize)],
    function: Option<&str>,
    mutants: &mut Vec<Mutant>,
) {
    let mut collector = ExprCollector {
        path,
        source,
        changed_lines,
        test_regions,
        function,
        mutants,
    };
    for statement in &block.stmts {
        collector.visit_statement(statement);
    }
}

/// Walks statements and expressions, recording a mutant for every operator
/// instance that sits inside the changed lines and outside test code.
struct ExprCollector<'a> {
    path: &'a str,
    source: &'a str,
    changed_lines: &'a [usize],
    test_regions: &'a [(usize, usize)],
    function: Option<&'a str>,
    mutants: &'a mut Vec<Mutant>,
}

impl ExprCollector<'_> {
    fn consider(&mut self, span: proc_macro2::Span, operator: Operator, replacement: String) {
        let Some((start, end)) = self.offset_range(span) else {
            return;
        };
        if !self.is_mutable(start, end, span) {
            return;
        }
        let original = self.source[start..end].to_owned();
        if original == replacement {
            return;
        }
        self.mutants.push(Mutant {
            id: mutant_id(self.path, start, end, operator),
            path: self.path.to_owned(),
            operator: operator.as_str().to_owned(),
            line: span.start().line,
            start,
            end,
            original,
            replacement,
            function: self.function.map(str::to_owned),
        });
    }

    /// Whether a span may be mutated: inside a changed line, outside test code.
    fn is_mutable(&self, start: usize, end: usize, span: proc_macro2::Span) -> bool {
        if !self.changed_lines.contains(&span.start().line) {
            return false;
        }
        // Never mutate inside a test module. This is the guarantee that makes a
        // mutation result meaningful: changing the oracle would change the
        // question rather than the answer.
        !self
            .test_regions
            .iter()
            .any(|(region_start, region_end)| start >= *region_start && end <= *region_end)
    }

    fn offset_range(&self, span: proc_macro2::Span) -> Option<(usize, usize)> {
        Some((
            line_col_to_offset(self.source, span.start())?,
            line_col_to_offset(self.source, span.end())?,
        ))
    }

    fn visit_statement(&mut self, statement: &syn::Stmt) {
        match statement {
            syn::Stmt::Expr(expression, _) => self.visit_expression(expression),
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    self.visit_expression(&init.expr);
                }
            }
            syn::Stmt::Item(item) => {
                if let Item::Fn(function) = item {
                    let name = function.sig.ident.to_string();
                    collect_from_block(
                        &function.block,
                        self.path,
                        self.source,
                        self.changed_lines,
                        self.test_regions,
                        Some(&name),
                        self.mutants,
                    );
                }
            }
            syn::Stmt::Macro(_) => {}
        }
    }

    fn visit_expression(&mut self, expression: &Expr) {
        match expression {
            Expr::Binary(binary) => self.visit_binary(binary),
            Expr::Lit(literal) => {
                if let Lit::Bool(value) = &literal.lit {
                    let replacement = if value.value { "false" } else { "true" };
                    self.consider(value.span(), Operator::BooleanFlip, replacement.to_owned());
                }
            }
            Expr::Unary(unary) => self.visit_expression(&unary.expr),
            Expr::Paren(paren) => self.visit_expression(&paren.expr),
            Expr::Call(call) => {
                for argument in &call.args {
                    self.visit_expression(argument);
                }
                self.visit_expression(&call.func);
            }
            Expr::MethodCall(method) => {
                for argument in &method.args {
                    self.visit_expression(argument);
                }
                self.visit_expression(&method.receiver);
            }
            Expr::If(branch) => {
                self.visit_expression(&branch.cond);
                for statement in &branch.then_branch.stmts {
                    self.visit_statement(statement);
                }
                if let Some((_, otherwise)) = &branch.else_branch {
                    self.visit_expression(otherwise);
                }
            }
            Expr::Match(matched) => {
                self.visit_expression(&matched.expr);
                for arm in &matched.arms {
                    self.visit_expression(&arm.body);
                }
            }
            Expr::Block(block) => {
                for statement in &block.block.stmts {
                    self.visit_statement(statement);
                }
            }
            Expr::Loop(looped) => {
                for statement in &looped.body.stmts {
                    self.visit_statement(statement);
                }
            }
            Expr::While(looped) => {
                self.visit_expression(&looped.cond);
                for statement in &looped.body.stmts {
                    self.visit_statement(statement);
                }
            }
            Expr::ForLoop(looped) => {
                self.visit_expression(&looped.expr);
                for statement in &looped.body.stmts {
                    self.visit_statement(statement);
                }
            }
            Expr::Return(returned) => {
                if let Some(value) = &returned.expr {
                    self.visit_expression(value);
                }
            }
            Expr::Assign(assign) => {
                self.visit_expression(&assign.left);
                self.visit_expression(&assign.right);
            }
            Expr::Reference(reference) => self.visit_expression(&reference.expr),
            Expr::Tuple(tuple) => {
                for element in &tuple.elems {
                    self.visit_expression(element);
                }
            }
            Expr::Array(array) => {
                for element in &array.elems {
                    self.visit_expression(element);
                }
            }
            Expr::Struct(structure) => {
                for field in &structure.fields {
                    self.visit_expression(&field.expr);
                }
            }
            Expr::Field(field) => self.visit_expression(&field.base),
            Expr::Index(index) => {
                self.visit_expression(&index.expr);
                self.visit_expression(&index.index);
            }
            Expr::Cast(cast) => self.visit_expression(&cast.expr),
            Expr::Try(tried) => self.visit_expression(&tried.expr),
            Expr::Await(awaited) => self.visit_expression(&awaited.base),
            Expr::Range(range) => {
                if let Some(start) = &range.start {
                    self.visit_expression(start);
                }
                if let Some(end) = &range.end {
                    self.visit_expression(end);
                }
            }
            _ => {}
        }
    }

    fn visit_binary(&mut self, binary: &ExprBinary) {
        self.visit_expression(&binary.left);
        self.visit_expression(&binary.right);

        let operator = match binary.op {
            BinOp::Eq(_) => Some((Operator::EqualityFlip, "!=")),
            BinOp::Ne(_) => Some((Operator::EqualityFlip, "==")),
            BinOp::Lt(_) => Some((Operator::ComparisonBoundary, "<=")),
            BinOp::Le(_) => Some((Operator::ComparisonBoundary, "<")),
            BinOp::Gt(_) => Some((Operator::ComparisonBoundary, ">=")),
            BinOp::Ge(_) => Some((Operator::ComparisonBoundary, ">")),
            BinOp::And(_) => Some((Operator::LogicalFlip, "||")),
            BinOp::Or(_) => Some((Operator::LogicalFlip, "&&")),
            _ => None,
        };

        if let Some((operator, replacement)) = operator {
            self.consider(binary.op.span(), operator, replacement.to_owned());
        }
    }
}

/// A deterministic identifier for a mutation site.
///
/// Derived from the path, span and operator rather than from a counter, so the
/// same revision pair yields the same IDs across runs and cached results stay
/// addressable without storing a mapping.
fn mutant_id(path: &str, start: usize, end: usize, operator: Operator) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    hasher.update(start.to_string().as_bytes());
    hasher.update(end.to_string().as_bytes());
    hasher.update(operator.as_str().as_bytes());
    let digest = hasher.finalize();
    // A short prefix is enough to be collision-free in practice and keeps the
    // receipt readable.
    hex::encode(&digest[..8])
}

use sha2::{Digest, Sha256};

/// Convert a `proc_macro2` position into a byte offset.
///
/// `LineColumn::column` counts UTF-8 characters rather than bytes, so a line
/// containing non-ASCII text would otherwise slice the wrong range. This is the
/// same conversion ADR-0010 required, for the same reason.
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
                    None => return Some(line_start + text.len()),
                }
            }
        }
        line_start += text.len() + 1;
    }
    None
}

/// The `#[cfg(test)]` module byte ranges of a file, for mutation exclusion.
///
/// Delegates to the inline-test locator so both features agree on what counts
/// as test code.
pub fn test_regions(source: &str) -> Vec<(usize, usize)> {
    crate::inline::test_module_regions(source)
}

/// Render an expression, used by tests and by the receipt's detail rendering.
pub fn render<T: ToTokens>(node: &T) -> String {
    node.to_token_stream().to_string()
}

/// The set of parse failures worth surfacing to a caller.
pub fn parses(source: &str) -> bool {
    syn::parse_file(source).is_ok()
}

/// Exposed for tests: the count of items in a parsed file.
pub fn item_count(source: &str) -> usize {
    syn::parse_file(source)
        .map(|parsed: File| parsed.items.len())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All lines changed, for tests that are not about scoping.
    fn all_lines() -> Vec<usize> {
        (1..=200).collect()
    }

    fn mutants(source: &str) -> Vec<Mutant> {
        mutants_for_file("src/lib.rs", source, &all_lines(), &test_regions(source))
    }

    fn operators(mutants: &[Mutant]) -> Vec<&str> {
        mutants.iter().map(|m| m.operator.as_str()).collect()
    }

    #[test]
    fn equality_flip_is_generated() {
        let found = mutants("pub fn f(a: i32) -> bool { a == 1 }\n");
        assert_eq!(operators(&found), vec!["equality_flip"]);
        assert_eq!(found[0].original, "==");
        assert_eq!(found[0].replacement, "!=");
    }

    #[test]
    fn comparison_boundary_is_generated_both_ways() {
        let less = mutants("pub fn f(a: i32) -> bool { a < 1 }\n");
        assert_eq!(less[0].original, "<");
        assert_eq!(less[0].replacement, "<=");

        let less_equal = mutants("pub fn f(a: i32) -> bool { a <= 1 }\n");
        assert_eq!(less_equal[0].original, "<=");
        assert_eq!(less_equal[0].replacement, "<");
    }

    #[test]
    fn logical_and_boolean_flips_are_generated() {
        let logical = mutants("pub fn f(a: bool, b: bool) -> bool { a && b }\n");
        assert_eq!(operators(&logical), vec!["logical_flip"]);
        assert_eq!(logical[0].replacement, "||");

        let boolean = mutants("pub fn f() -> bool { true }\n");
        assert_eq!(operators(&boolean), vec!["boolean_flip"]);
        assert_eq!(boolean[0].replacement, "false");
    }

    /// The guarantee that makes a mutation result meaningful: the oracle is
    /// never edited. Mutating a test's own literal would change the question
    /// rather than the answer.
    #[test]
    fn test_code_is_never_mutated() {
        let source = "\
pub fn f(a: i32) -> bool { a == 1 }

#[cfg(test)]
mod tests {
    #[test]
    fn t() { assert!(f(1) == true); }
}
";
        let found = mutants(source);
        let regions = test_regions(source);
        assert_eq!(regions.len(), 1);
        for mutant in &found {
            assert!(
                !(mutant.start >= regions[0].0 && mutant.end <= regions[0].1),
                "mutant {:?} in {}..{} falls inside the test module {}..{}",
                mutant.operator,
                mutant.start,
                mutant.end,
                regions[0].0,
                regions[0].1
            );
        }
        // The production operator is still found.
        assert!(operators(&found).contains(&"equality_flip"));
    }

    /// Only changed functions are mutated, so cost tracks the change rather
    /// than the file.
    #[test]
    fn only_changed_lines_are_mutated() {
        let source = "pub fn untouched(a: i32) -> bool { a == 1 }\n\npub fn changed(a: i32) -> bool { a != 2 }\n";
        let found = mutants_for_file("src/lib.rs", source, &[3], &[]);
        assert_eq!(
            found.len(),
            1,
            "only the changed line should produce mutants, got {found:?}"
        );
        assert_eq!(found[0].function.as_deref(), Some("changed"));
    }

    #[test]
    fn mutant_ids_are_deterministic_and_distinct() {
        let source = "pub fn f(a: i32, b: i32) -> bool { a == 1 && b == 2 }\n";
        let first = mutants(source);
        let second = mutants(source);
        assert_eq!(
            first.iter().map(|m| &m.id).collect::<Vec<_>>(),
            second.iter().map(|m| &m.id).collect::<Vec<_>>(),
            "the same source must produce the same ids"
        );
        let mut ids: Vec<&str> = first.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), first.len(), "ids must be distinct");
    }

    #[test]
    fn applying_a_mutant_replaces_exactly_the_span() {
        let source = "pub fn f(a: i32) -> bool { a == 1 }\n";
        let mutant = mutants(source).remove(0);
        let mutated = mutant.apply(source).expect("span should match");
        assert_eq!(mutated, "pub fn f(a: i32) -> bool { a != 1 }\n");
        assert_eq!(mutated.len(), source.len());
    }

    /// A stale mutant must not silently apply a different edit.
    ///
    /// The replacement source has to differ *at the recorded span* for this to
    /// test anything: `a === 1` still has `==` at the same offsets, so it would
    /// legitimately apply. Prefixing the line moves the operator.
    #[test]
    fn applying_a_stale_mutant_is_refused() {
        let source = "pub fn f(a: i32) -> bool { a == 1 }\n";
        let mutant = mutants(source).remove(0);
        let different = "// shifted\npub fn f(a: i32) -> bool { a == 1 }\n";
        assert_ne!(
            &different[mutant.start..mutant.end],
            mutant.original,
            "the fixture must actually differ at the recorded span"
        );
        assert!(
            mutant.apply(different).is_none(),
            "a mismatched span must be refused rather than applied blindly"
        );
    }

    #[test]
    fn unparsable_source_yields_no_mutants() {
        assert!(mutants("fn oops( {").is_empty());
    }

    /// Non-ASCII text before an operator must not shift the span, because
    /// `proc_macro2` columns count characters rather than bytes.
    #[test]
    fn mutation_after_multibyte_text_uses_the_right_span() {
        let source = "// café ✓\npub fn f(a: i32) -> bool { a == 1 }\n";
        let found = mutants(source);
        assert_eq!(found.len(), 1, "got {found:?}");
        let mutated = found[0].apply(source).expect("span should match");
        assert_eq!(
            mutated, "// café ✓\npub fn f(a: i32) -> bool { a != 1 }\n",
            "the mutation must land on the operator, not inside the comment"
        );
    }
}
