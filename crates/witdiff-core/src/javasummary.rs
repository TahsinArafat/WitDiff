//! The Java summary tool, embedded so there is no separate file to locate at
//! runtime.
//!
//! WitDiff writes this to a temporary directory and runs it with `java`
//! (ADR-0018). Only the public `com.sun.source` API is used, so no
//! `--add-exports` flag or internal `com.sun.tools.javac` package is needed and
//! the tool works on any JDK.

/// The tool's format version.
pub const SUMMARY_FORMAT: u32 = 1;

/// The class name the file must be written as, because Java requires a public
/// class to match its filename.
pub const SUMMARY_CLASS: &str = "Summarize";

/// Emit a normalized structural summary of one Java test file as JSON.
pub const SUMMARY_SCRIPT: &str = concat!(
    "// Summary tool for Java test files. Compiled and run by the JDK that the\n",
    "// project already uses to build (ADR-0018).\n",
    "//\n",
    "// Uses only the public com.sun.source API - no internal com.sun.tools.javac\n",
    "// packages and no --add-exports - so it works on any JDK.\n",
    "\n",
    "import com.sun.source.tree.*;\n",
    "import com.sun.source.util.*;\n",
    "import javax.tools.*;\n",
    "import java.util.*;\n",
    "\n",
    "public class Summarize {\n",
    "    // Normalize an expression to a stable, formatting-insensitive form. This\n",
    "    // form is ours, and both sides are versioned together, so a compiler\n",
    "    // change cannot alter the comparison.\n",
    "    static String render(ExpressionTree e) {\n",
    "        if (e == null) return \"null\";\n",
    "        if (e instanceof ParenthesizedTree p) return render(p.getExpression());\n",
    "        if (e instanceof MethodInvocationTree m) {\n",
    "            List<String> parts = new ArrayList<>();\n",
    "            for (ExpressionTree a : m.getArguments()) parts.add(render(a));\n",
    "            return render(m.getMethodSelect()) + \"(\" + String.join(\", \", parts) + \")\";\n",
    "        }\n",
    "        if (e instanceof IdentifierTree i) return i.getName().toString();\n",
    "        if (e instanceof MemberSelectTree s) return render(s.getExpression()) + \".\" + s.getIdentifier();\n",
    "        if (e instanceof LiteralTree l) return String.valueOf(l.getValue());\n",
    "        if (e instanceof BinaryTree b) {\n",
    "            return render(b.getLeftOperand()) + \" \" + opOf(b.getKind()) + \" \" + render(b.getRightOperand());\n",
    "        }\n",
    "        if (e instanceof UnaryTree u) return opOf(u.getKind()) + \" \" + render(u.getExpression());\n",
    "        if (e instanceof TypeCastTree c) return \"(\" + c.getType() + \") \" + render(c.getExpression());\n",
    "        if (e instanceof NewClassTree n) return \"new \" + n.getIdentifier();\n",
    "        return e.getKind().toString();\n",
    "    }\n",
    "\n",
    "    static String opOf(Tree.Kind kind) {\n",
    "        switch (kind) {\n",
    "            case EQUAL_TO: return \"==\";\n",
    "            case NOT_EQUAL_TO: return \"!=\";\n",
    "            case LESS_THAN: return \"<\";\n",
    "            case LESS_THAN_EQUAL: return \"<=\";\n",
    "            case GREATER_THAN: return \">\";\n",
    "            case GREATER_THAN_EQUAL: return \">=\";\n",
    "            case CONDITIONAL_AND: return \"&&\";\n",
    "            case CONDITIONAL_OR: return \"||\";\n",
    "            case LOGICAL_COMPLEMENT: return \"!\";\n",
    "            case PLUS: return \"+\";\n",
    "            case MINUS: return \"-\";\n",
    "            default: return kind.toString();\n",
    "        }\n",
    "    }\n",
    "\n",
    "    // JUnit declares assertEquals(expected, actual), so the expectation comes\n",
    "    // FIRST. Python and Go put the subject first, so this must swap to match\n",
    "    // the shared engine's subject-first form. Getting this wrong would invert\n",
    "    // every comparison rather than failing loudly.\n",
    "    static String assertionOf(String name, List<? extends ExpressionTree> args) {\n",
    "        List<String> rendered = new ArrayList<>();\n",
    "        for (ExpressionTree a : args) rendered.add(render(a));\n",
    "        if (name.equals(\"assertEquals\") || name.equals(\"assertNotEquals\")\n",
    "            || name.equals(\"assertSame\") || name.equals(\"assertNotSame\")) {\n",
    "            if (rendered.size() >= 2) {\n",
    "                String op = name.equals(\"assertEquals\") || name.equals(\"assertSame\") ? \" Eq \" : \" NotEq \";\n",
    "                return rendered.get(1) + op + rendered.get(0);\n",
    "            }\n",
    "        }\n",
    "        if (name.equals(\"assertTrue\") || name.equals(\"assertFalse\")) {\n",
    "            // Only one meaningful argument; any trailing argument is a message.\n",
    "            return rendered.isEmpty() ? \"\" : rendered.get(0);\n",
    "        }\n",
    "        if (name.equals(\"assertNull\") || name.equals(\"assertNotNull\")) {\n",
    "            return (rendered.isEmpty() ? \"\" : rendered.get(0))\n",
    "                + (name.equals(\"assertNull\") ? \" Is null\" : \" IsNot null\");\n",
    "        }\n",
    "        if (name.equals(\"assertArrayEquals\") || name.equals(\"assertIterableEquals\")) {\n",
    "            if (rendered.size() >= 2) return rendered.get(1) + \" Eq \" + rendered.get(0);\n",
    "        }\n",
    "        // fail(...) and anything unrecognized: the message is not the assertion.\n",
    "        return \"\";\n",
    "    }\n",
    "\n",
    "    static boolean isAssertionName(String name) {\n",
    "        return name.startsWith(\"assert\") || name.equals(\"fail\");\n",
    "    }\n",
    "\n",
    "    static String simpleName(String select) {\n",
    "        int dot = select.lastIndexOf('.');\n",
    "        return dot < 0 ? select : select.substring(dot + 1);\n",
    "    }\n",
    "\n",
    "    static String escape(String s) {\n",
    "        StringBuilder b = new StringBuilder();\n",
    "        for (char c : s.toCharArray()) {\n",
    "            switch (c) {\n",
    "                case '\"': b.append(\"\\\\\\\"\"); break;\n",
    "                case '\\\\': b.append(\"\\\\\\\\\"); break;\n",
    "                case '\\n': b.append(\"\\\\n\"); break;\n",
    "                case '\\r': b.append(\"\\\\r\"); break;\n",
    "                case '\\t': b.append(\"\\\\t\"); break;\n",
    "                default:\n",
    "                    if (c < 0x20) b.append(String.format(\"\\\\u%04x\", (int) c));\n",
    "                    else b.append(c);\n",
    "            }\n",
    "        }\n",
    "        return b.toString();\n",
    "    }\n",
    "\n",
    "    public static void main(String[] args) {\n",
    "        try {\n",
    "            run(args);\n",
    "        } catch (Exception error) {\n",
    "            System.out.println(\"{\\\"format\\\": 1, \\\"error\\\": \\\"\" + escape(String.valueOf(error.getMessage())) + \"\\\"}\");\n",
    "        }\n",
    "    }\n",
    "\n",
    "    static void run(String[] args) throws Exception {\n",
    "        if (args.length != 1) {\n",
    "            System.out.println(\"{\\\"format\\\": 1, \\\"error\\\": \\\"usage: Summarize <path>\\\"}\");\n",
    "            return;\n",
    "        }\n",
    "        JavaCompiler compiler = ToolProvider.getSystemJavaCompiler();\n",
    "        if (compiler == null) {\n",
    "            System.out.println(\"{\\\"format\\\": 1, \\\"error\\\": \\\"no system Java compiler; a JRE alone cannot analyze\\\"}\");\n",
    "            return;\n",
    "        }\n",
    "        DiagnosticCollector<JavaFileObject> diagnostics = new DiagnosticCollector<>();\n",
    "        StandardJavaFileManager fm = compiler.getStandardFileManager(diagnostics, null, null);\n",
    "        JavacTask task = (JavacTask) compiler.getTask(null, fm, diagnostics, List.of(), null,\n",
    "            fm.getJavaFileObjects(args[0]));\n",
    "\n",
    "        Trees trees = Trees.instance(task);\n",
    "        StringBuilder out = new StringBuilder();\n",
    "        out.append(\"{\\\"format\\\": 1, \\\"functions\\\": [\");\n",
    "        boolean firstFunction = true;\n",
    "\n",
    "        for (CompilationUnitTree unit : task.parse()) {\n",
    "            SourcePositions pos = trees.getSourcePositions();\n",
    "            LineMap lines = unit.getLineMap();\n",
    "\n",
    "            for (Tree decl : unit.getTypeDecls()) {\n",
    "                if (!(decl instanceof ClassTree cls)) continue;\n",
    "                for (Tree member : cls.getMembers()) {\n",
    "                    if (!(member instanceof MethodTree method)) continue;\n",
    "                    if (method.getBody() == null) continue;\n",
    "\n",
    "                    // JUnit 5 uses @Test; JUnit 4 does too; a method named\n",
    "                    // testXxx is the JUnit 3 convention.\n",
    "                    boolean isTest = method.getName().toString().startsWith(\"test\");\n",
    "                    boolean skipped = false;\n",
    "                    for (AnnotationTree annotation : method.getModifiers().getAnnotations()) {\n",
    "                        // getAnnotationType returns Tree, not ExpressionTree, so\n",
    "                        // the name is taken from its source form.\n",
    "                        String annotationName = simpleName(annotation.getAnnotationType().toString());\n",
    "                        if (annotationName.equals(\"Test\")) isTest = true;\n",
    "                        if (annotationName.equals(\"Disabled\") || annotationName.equals(\"Ignore\")) skipped = true;\n",
    "                    }\n",
    "                    if (!isTest) continue;\n",
    "\n",
    "                    List<String> tests = new ArrayList<>();\n",
    "                    List<Long> testLines = new ArrayList<>();\n",
    "                    new TreeScanner<Void, Void>() {\n",
    "                        public Void visitMethodInvocation(MethodInvocationTree node, Void p) {\n",
    "                            String select = render(node.getMethodSelect());\n",
    "                            String simple = simpleName(select);\n",
    "                            if (isAssertionName(simple)) {\n",
    "                                String rendered = assertionOf(simple, node.getArguments());\n",
    "                                if (!rendered.isEmpty()) {\n",
    "                                    tests.add(rendered);\n",
    "                                    testLines.add(lines.getLineNumber(pos.getStartPosition(unit, node)));\n",
    "                                }\n",
    "                            }\n",
    "                            return super.visitMethodInvocation(node, p);\n",
    "                        }\n",
    "\n",
    "                        // `if (cond) fail(...)` is the other common shape, and\n",
    "                        // the condition is the assertion.\n",
    "                        public Void visitIf(IfTree node, Void p) {\n",
    "                            boolean reportsFailure = false;\n",
    "                            for (StatementTree s : node.getThenStatement() instanceof BlockTree b\n",
    "                                    ? b.getStatements() : List.of(node.getThenStatement())) {\n",
    "                                if (s instanceof ExpressionStatementTree e\n",
    "                                        && e.getExpression() instanceof MethodInvocationTree call) {\n",
    "                                    String simple = simpleName(render(call.getMethodSelect()));\n",
    "                                    if (simple.equals(\"fail\") || simple.equals(\"assertTrue\")) {\n",
    "                                        reportsFailure = true;\n",
    "                                    }\n",
    "                                }\n",
    "                            }\n",
    "                            if (reportsFailure) {\n",
    "                                tests.add(render(node.getCondition()));\n",
    "                                testLines.add(lines.getLineNumber(pos.getStartPosition(unit, node)));\n",
    "                            }\n",
    "                            return super.visitIf(node, p);\n",
    "                        }\n",
    "                    }.scan(method.getBody(), null);\n",
    "\n",
    "                    if (!firstFunction) out.append(\", \");\n",
    "                    firstFunction = false;\n",
    "                    out.append(\"{\\\"name\\\": \\\"\").append(escape(method.getName().toString())).append(\"\\\"\");\n",
    "                    out.append(\", \\\"line\\\": \").append(lines.getLineNumber(pos.getStartPosition(unit, method)));\n",
    "                    out.append(\", \\\"skipped\\\": \").append(skipped);\n",
    "                    out.append(\", \\\"body_is_empty\\\": \").append(method.getBody().getStatements().isEmpty());\n",
    "                    out.append(\", \\\"assertions\\\": [\");\n",
    "                    for (int i = 0; i < tests.size(); i++) {\n",
    "                        if (i > 0) out.append(\", \");\n",
    "                        out.append(\"{\\\"line\\\": \").append(testLines.get(i))\n",
    "                           .append(\", \\\"test\\\": \\\"\").append(escape(tests.get(i))).append(\"\\\"}\");\n",
    "                    }\n",
    "                    out.append(\"]}\");\n",
    "                }\n",
    "            }\n",
    "        }\n",
    "        out.append(\"]}\");\n",
    "        System.out.println(out);\n",
    "    }\n",
    "}\n",
    "\n",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_has_the_expected_shape() {
        assert!(SUMMARY_SCRIPT.contains("public class Summarize"));
        assert!(SUMMARY_SCRIPT.contains("public static void main"));
        assert!(SUMMARY_SCRIPT.contains("JavacTask"));
    }

    /// Only the public API may be used: an internal import would require
    /// `--add-exports` and break on some JDKs.
    ///
    /// The check is for an *import*, not for the name: the tool's comment
    /// explains why the internal package is avoided, and a naive substring test
    /// would fail on its own documentation.
    #[test]
    fn the_tool_uses_only_the_public_api() {
        assert!(SUMMARY_SCRIPT.contains("import com.sun.source"));
        assert!(
            !SUMMARY_SCRIPT.contains("import com.sun.tools.javac"),
            "internal javac packages need --add-exports, which not every JDK accepts"
        );
    }

    /// JUnit puts the expectation first, unlike Python and Go. The swap is what
    /// makes the shared engine's subject-first comparison work, and getting it
    /// wrong would invert every expectation rather than fail loudly.
    #[test]
    fn the_tool_swaps_junit_argument_order() {
        assert!(SUMMARY_SCRIPT.contains("rendered.get(1)"));
        assert!(SUMMARY_SCRIPT.contains("assertEquals"));
    }

    /// A JRE without a compiler must be reported, not treated as a parse error.
    #[test]
    fn the_tool_reports_a_missing_compiler() {
        assert!(SUMMARY_SCRIPT.contains("no system Java compiler"));
    }
}
