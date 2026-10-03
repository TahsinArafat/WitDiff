//! The Python summary script, embedded so there is no separate file to
//! locate at runtime.
//!
//! WitDiff writes this to a temporary file and runs it with the interpreter
//! named by the project's own test command (ADR-0016). Keeping it here rather
//! than in a data directory means the analyzer and its script cannot drift
//! apart during installation.

/// The script's format version. WitDiff writes and reads both sides, so a
/// mismatch is detected rather than misparsed.
pub const SUMMARY_FORMAT: u32 = 1;

/// Emit a normalized structural summary of one Python file as JSON.
pub const SUMMARY_SCRIPT: &str = concat!(
    "import ast\n",
    "import json\n",
    "import sys\n",
    "\n",
    "\n",
    "def normalize(node):\n",
    "    \"\"\"Render an expression to a stable, formatting-insensitive string.\n",
    "\n",
    "    Deliberately not `ast.dump`: its output embeds `ctx=Load()` nodes and has\n",
    "    changed across Python releases, so two interpreters could disagree about\n",
    "    identical code. This form is ours, and WitDiff versions both sides.\n",
    "    \"\"\"\n",
    "    if node is None:\n",
    "        return None\n",
    "    if isinstance(node, ast.Constant):\n",
    "        return repr(node.value)\n",
    "    if isinstance(node, ast.Name):\n",
    "        return node.id\n",
    "    if isinstance(node, ast.Attribute):\n",
    "        return \"%s.%s\" % (normalize(node.value), node.attr)\n",
    "    if isinstance(node, ast.Subscript):\n",
    "        return \"%s[%s]\" % (normalize(node.value), normalize(node.slice))\n",
    "    if isinstance(node, ast.Call):\n",
    "        parts = [normalize(a) for a in node.args]\n",
    "        parts += [\"%s=%s\" % (k.arg, normalize(k.value)) for k in node.keywords]\n",
    "        return \"%s(%s)\" % (normalize(node.func), \", \".join(parts))\n",
    "    if isinstance(node, ast.BoolOp):\n",
    "        op = type(node.op).__name__\n",
    "        return (\" %s \" % op).join(normalize(v) for v in node.values)\n",
    "    if isinstance(node, ast.UnaryOp):\n",
    "        return \"%s %s\" % (type(node.op).__name__, normalize(node.operand))\n",
    "    if isinstance(node, ast.BinOp):\n",
    "        return \"%s %s %s\" % (\n",
    "            normalize(node.left),\n",
    "            type(node.op).__name__,\n",
    "            normalize(node.right),\n",
    "        )\n",
    "    if isinstance(node, ast.Compare):\n",
    "        parts = [normalize(node.left)]\n",
    "        for op, comparator in zip(node.ops, node.comparators):\n",
    "            parts.append(type(op).__name__)\n",
    "            parts.append(normalize(comparator))\n",
    "        return \" \".join(parts)\n",
    "    if isinstance(node, (ast.Tuple, ast.List, ast.Set)):\n",
    "        return \"[%s]\" % \", \".join(normalize(e) for e in node.elts)\n",
    "    if isinstance(node, ast.Dict):\n",
    "        return \"{%s}\" % \", \".join(\n",
    "            \"%s: %s\" % (normalize(k), normalize(v))\n",
    "            for k, v in zip(node.keys, node.values)\n",
    "        )\n",
    "    if isinstance(node, ast.Lambda):\n",
    "        return \"lambda: %s\" % normalize(node.body)\n",
    "    if isinstance(node, ast.IfExp):\n",
    "        return \"%s if %s else %s\" % (\n",
    "            normalize(node.body),\n",
    "            normalize(node.test),\n",
    "            normalize(node.orelse),\n",
    "        )\n",
    "    if isinstance(node, ast.Starred):\n",
    "        return \"*%s\" % normalize(node.value)\n",
    "    # Anything else is named so it participates in comparison without\n",
    "    # pretending to be understood.\n",
    "    return type(node).__name__\n",
    "\n",
    "\n",
    "def test_functions(tree):\n",
    "    \"\"\"Yield functions that pytest would collect.\"\"\"\n",
    "    for node in tree.body:\n",
    "        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):\n",
    "            decorators = [normalize(d) for d in node.decorator_list]\n",
    "            is_test = node.name.startswith(\"test_\") or any(\n",
    "                d and \"fixture\" not in d for d in decorators\n",
    "            )\n",
    "            if is_test:\n",
    "                yield node, decorators\n",
    "        elif isinstance(node, ast.ClassDef):\n",
    "            if node.name.startswith(\"Test\"):\n",
    "                for inner in node.body:\n",
    "                    if isinstance(inner, (ast.FunctionDef, ast.AsyncFunctionDef)):\n",
    "                        decorators = [normalize(d) for d in inner.decorator_list]\n",
    "                        yield inner, decorators\n",
    "\n",
    "\n",
    "def summary(path):\n",
    "    with open(path, \"r\", encoding=\"utf-8\") as handle:\n",
    "        source = handle.read()\n",
    "    tree = ast.parse(source, filename=path)\n",
    "\n",
    "    functions = []\n",
    "    for node, decorators in test_functions(tree):\n",
    "        assertions = []\n",
    "        for child in ast.walk(node):\n",
    "            if isinstance(child, ast.Assert):\n",
    "                assertions.append(\n",
    "                    {\n",
    "                        \"line\": child.lineno,\n",
    "                        \"test\": normalize(child.test),\n",
    "                        \"message\": normalize(child.msg),\n",
    "                    }\n",
    "                )\n",
    "        skipped = any(\n",
    "            d in (\"pytest.mark.skip\", \"pytest.mark.skipif\", \"unittest.skip\")\n",
    "            for d in decorators\n",
    "        )\n",
    "        body_is_empty = all(\n",
    "            isinstance(s, ast.Pass)\n",
    "            or (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant))\n",
    "            for s in node.body\n",
    "        )\n",
    "        functions.append(\n",
    "            {\n",
    "                \"name\": node.name,\n",
    "                \"line\": node.lineno,\n",
    "                \"assertions\": assertions,\n",
    "                \"skipped\": skipped,\n",
    "                \"body_is_empty\": body_is_empty,\n",
    "            }\n",
    "        )\n",
    "\n",
    "    return {\n",
    "        \"format\": 1,\n",
    "        \"functions\": functions,\n",
    "    }\n",
    "\n",
    "\n",
    "def main():\n",
    "    if len(sys.argv) != 2:\n",
    "        print(\"usage: summarizer.py <path>\", file=sys.stderr)\n",
    "        return 2\n",
    "    try:\n",
    "        result = summary(sys.argv[1])\n",
    "    except SyntaxError as error:\n",
    "        json.dump({\"format\": 1, \"error\": \"syntax_error: %s\" % error}, sys.stdout)\n",
    "        return 0\n",
    "    except OSError as error:\n",
    "        json.dump({\"format\": 1, \"error\": \"io_error: %s\" % error}, sys.stdout)\n",
    "        return 0\n",
    "    json.dump(result, sys.stdout)\n",
    "    return 0\n",
    "\n",
    "\n",
    "if __name__ == \"__main__\":\n",
    "    sys.exit(main())\n",
    "\n",
);

#[cfg(test)]
mod tests {
    use super::*;

    /// A truncated or mis-escaped embedded script would fail at runtime in a
    /// way that is easy to mistake for a project problem. Checking it here
    /// makes such a mistake a build failure instead.
    #[test]
    fn the_script_has_the_expected_shape() {
        assert!(SUMMARY_SCRIPT.contains("import ast"));
        assert!(SUMMARY_SCRIPT.contains("import json"));
        assert!(
            SUMMARY_SCRIPT.contains("def normalize(node)"),
            "the normalizer is the part that must not go missing"
        );
        assert!(
            SUMMARY_SCRIPT.contains(r#"if __name__ == "__main__":"#),
            "the entry point must survive escaping"
        );
    }

    /// The script must contain no Rust escape sequences that leaked through
    /// and no raw characters that would break the JSON it writes.
    #[test]
    fn the_script_has_no_escaping_damage() {
        assert!(
            !SUMMARY_SCRIPT.contains("\\\""),
            "a stray backslash-quote means the escaping is wrong: {:?}",
            &SUMMARY_SCRIPT[..200.min(SUMMARY_SCRIPT.len())]
        );
        assert!(
            !SUMMARY_SCRIPT.contains('\t'),
            "tabs should have been spaces for a predictable script"
        );
    }

    /// `ast.dump` is deliberately not used: its output is version-sensitive.
    ///
    /// The check is for a *call*, not for the name: the script's docstring
    /// explains why `ast.dump` is avoided, and a naive substring test would
    /// fail on its own documentation.
    #[test]
    fn the_script_does_not_call_ast_dump() {
        assert!(
            !SUMMARY_SCRIPT.contains("ast.dump("),
            "ast.dump output embeds ctx= nodes and varies across releases"
        );
    }
}
