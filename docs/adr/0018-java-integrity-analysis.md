# ADR-0018: Java integrity analysis via the JDK's own parser

Status: accepted

## Context

ADR-0017 added Go, leaving Java as the next language per the support matrix
assessment: the JDK ships a parser, so the analysis needs no dependency and no
extra install step, the same property that made Python and Go tractable.

Java adds a hazard the previous languages did not have, and it is the kind that
fails silently rather than loudly.

## Evidence

- **`com.sun.source` is public API.** `JavacTask` exposes a parser and `Trees`
  supplies positions, so no `--add-exports` flag and no internal
  `com.sun.tools.javac` package is required. Verified: an attempt using the
  internal `JCTree` failed to compile without `--add-exports`, while the public
  `Trees.instance(task)` approach worked with no flags.
- **`java Summarize.java <target>` works.** Single-file source launch compiles
  in memory, at 0.115s warm. It requires a **JDK**, not a bare JRE, because it
  invokes `javac` internally. The tool detects a missing compiler and reports it.
- **JUnit reverses the argument order.** `assertEquals(expected, actual)` puts
  the expectation first, unlike Python's `assert actual == expected` and Go's
  `if actual != want`. Confirmed against the JUnit 5 API.

That last point is the whole reason this ADR exists. Feeding
`assertEquals(2, Add(1,1))` to a subject-first engine without swapping would
compare the expectation as the subject and the subject as the expectation. The
result is not a crash or a missing finding: `removed_assertion` fires for the
old form and an addition appears for the new one, so every expectation change
would be reported as a deletion. That is a plausible-looking wrong answer.

## Decision

1. **Analyze Java through the JDK's public parser**, mirroring Go. The toolchain
   is discovered from the project's test command, recognizing `java`, `mvn`,
   `mvnw`, `gradle` and `gradlew`, which is what a Java project actually
   configures.

2. **Swap JUnit's arguments in the tool.** The normalization emits the
   subject-first form (`Add(1, 1) Eq 2`) that the shared engine expects, so the
   language-specific argument order is resolved at the boundary rather than
   leaking into the rules.

3. **The operator vocabulary must list the names the tool emits, not the Java
   spellings.** This was learned by failing: the first version listed `" == "`
   while the tool emits `Eq`, so `subject_of` found no comparison operator,
   took the whole string as the subject, and every expectation change was
   reported as `removed_assertion` followed by an addition. A test now asserts
   that the canonical `Eq` form is classified `Exact` and yields a subject.

4. **A JRE-only environment is reported, not guessed at.** The tool checks
   `ToolProvider.getSystemJavaCompiler()` and emits an explicit error, which
   surfaces as `test_source_unparsable` with the reason.

## Consequences

- Java gains the same six rules as Rust, Python and Go.
- `changed_expected_value` fires when an equality is replaced by a comparison
  over the same subject (`assertEquals(2, x)` becoming `assertTrue(x > 0)`).
  Verified that Python reports the same rule for the equivalent change, so the
  languages agree. A comparison still names a specific boundary, so it is an
  exact constraint under the shared strength model; the genuinely weaker form
  is a bare truthiness check.
- The analysis needs a JDK. A project testing with a JRE alone gets an explicit
  inability-to-analyze finding rather than silence.
- No new dependency, and no `--add-exports` flag, so it works on any JDK.
- Recognizing `mvn`/`gradle` as the Java marker means the framework classifier
  gained a `java` variant; verified against captured Maven Surefire and Gradle
  failure output, including a passing run that must not classify as a failure.

## Alternatives considered

- **The internal `com.sun.tools.javac` API.** Rejected: it needs
  `--add-exports`, which not every JDK configuration accepts, and it is not a
  supported interface.
- **Regex over Java source.** Rejected for the same reason as every other
  language here, and worse in Java's case: distinguishing
  `assertEquals(2, x)` from `assertEquals(x, 2)` — which have opposite
  meanings — is exactly the kind of thing a pattern cannot do reliably.
- **Compare the failure message.** Rejected: a reworded message would be a false
  `changed_expected_value` and an inverted expectation would be missed.
- **Require the user to add a parser dependency.** Rejected: the JDK already has
  one, so requiring an install would add a step whose absence silently degrades
  the analysis.
- **Support JUnit 4 separately.** Not needed yet: `@Test` and `@Ignore` are
  handled by the same code path, and the assertion methods share names. A real
  JUnit 4 project would confirm whether anything else differs.
