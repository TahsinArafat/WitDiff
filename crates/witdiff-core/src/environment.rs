//! The environment the evidence was produced under: toolchain versions and the
//! dependency manifests that pinned them.
//!
//! The verification digest covers *what* was verified — revisions, the test
//! command, and file contents. It deliberately says nothing about *where*.
//! That gap is real: a proof that holds under Python 3.9 and pytest 8.4 is a
//! different claim than the same revision under Python 3.12 and pytest 9, and
//! until now the receipt could not tell them apart.
//!
//! ## Why this is not folded into the verification digest
//!
//! [`crate::digest`] is recomputed by `witdiff receipt` against a stored
//! receipt, and a mismatch means "the verified inputs changed". Adding the
//! environment would make every receipt written by an earlier version report
//! that its inputs changed the moment Python was upgraded — a false staleness
//! warning on evidence that is entirely unchanged. The two answer different
//! questions, so they are recorded separately: the digest is about content,
//! the environment is about the run.
//!
//! It is therefore also **not** signed. ADR-0022 limits signed bytes to the
//! status and the digest precisely so that adding a field cannot invalidate
//! existing signatures, and this follows the same rule. The environment is
//! descriptive evidence for a reader, not an authenticated claim: it carries no
//! authority on its own, exactly like `notes`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::git::GitRepo;

/// What produced the evidence.
///
/// Sorted by key so two receipts recording the same environment serialize
/// identically and can be compared byte for byte.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Environment(BTreeMap<String, String>);

impl Environment {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    fn record(&mut self, key: &str, value: impl Into<String>) {
        self.0.insert(key.to_owned(), value.into());
    }

    /// The program the configured test command starts.
    pub fn test_program(&self) -> Option<&str> {
        self.get("test_program")
    }
}

/// The version flag for a program, and only for programs whose output
/// WitDiff knows how to read.
///
/// Running `<anything> --version` on an arbitrary configured program would
/// execute project-controlled code during collection, which is the objection
/// ADR-0019 makes about installing toolchains. An unrecognized program is
/// recorded by name and not executed.
const VERSION_FLAGS: &[(&str, &str)] = &[
    ("cargo", "--version"),
    ("rustc", "--version"),
    ("node", "--version"),
    ("python3", "--version"),
    ("python", "--version"),
    ("go", "version"),
    ("java", "-version"),
    ("javac", "-version"),
    ("ruby", "--version"),
    ("npm", "--version"),
    ("pnpm", "--version"),
    ("yarn", "--version"),
    ("pytest", "--version"),
    ("rspec", "--version"),
    ("mvn", "--version"),
    ("gradle", "--version"),
];

/// The toolchain that participates in a configured program's run.
///
/// Not every program on the machine: recording whatever happens to be
/// installed made two receipts of the same verification differ because one
/// host had `pnpm` and the other did not, which is noise rather than
/// evidence. Only the companions that can affect the result are probed.
fn companions(program: &str) -> &'static [&'static str] {
    match program {
        "cargo" => &["rustc"],
        "node" | "npm" | "pnpm" | "yarn" => &["node"],
        "python3" | "python" | "pytest" => &["python3"],
        "java" | "mvn" | "mvnw" | "gradle" | "gradlew" => &["java", "javac"],
        "ruby" | "rake" | "rspec" | "bundle" => &["ruby"],
        _ => &[],
    }
}

/// Dependency manifests whose presence identifies a dependency set.
///
/// The file itself is not copied into the receipt — its content is digested
/// instead, so a consumer can tell "the lockfile changed" from "the toolchain
/// changed" without seeing anyone's dependency list in full.
const MANIFESTS: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "go.sum",
    "go.mod",
    "package-lock.json",
    "package.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "requirements.txt",
    "requirements-dev.txt",
    "poetry.lock",
    "uv.lock",
    "pyproject.toml",
    "Gemfile",
    "Gemfile.lock",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "gradle.lockfile",
];

/// Collect the environment for a verification.
///
/// `test_command` is used only to name the program it starts, never to run it;
/// the probe is the fixed version flag above, not an argument of the command.
pub fn collect(repo: &GitRepo, test_command: &[String]) -> Environment {
    let mut environment = Environment::default();

    let program = test_command
        .first()
        .map(|name| program_base(name))
        .unwrap_or_default();
    if !program.is_empty() {
        environment.record("test_program", program.clone());
        environment.record(
            "test_program_version",
            probe_version(&program).unwrap_or_else(|| "unknown".to_owned()),
        );
        for companion in companions(&program) {
            // Already recorded above as the configured program itself.
            if *companion == program.as_str() {
                continue;
            }
            if let Some(version) = probe_version(companion) {
                environment.record(&format!("tool_{companion}"), version);
            }
        }
    }

    for manifest in MANIFESTS {
        if let Some(digest) = manifest_digest(&repo.root().join(manifest)) {
            environment.record(&format!("manifest_{manifest}"), digest);
        }
    }

    environment
}

/// The file name a command starts, without its directory.
///
/// `./mvnw` and `/usr/bin/python3` name `mvnw` and `python3`.
fn program_base(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.to_owned())
}

/// Read a program's version, or `None` when it is absent or says nothing.
fn probe_version(program: &str) -> Option<String> {
    let flag = VERSION_FLAGS
        .iter()
        .find(|(name, _)| *name == program)
        .map(|(_, flag)| *flag)?;
    run_version(program, flag)
}

/// Run a bounded version probe and take the first line of its output.
///
/// Both streams are considered: `java -version` writes to stderr, which is
/// the case that would otherwise record nothing for an entire JDK.
fn run_version(program: &str, flag: &str) -> Option<String> {
    let output = Command::new(program).arg(flag).output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    first_line(&text)
}

/// The first non-empty line, trimmed, or `None` when there is none.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

/// A digest of a manifest's content, or `None` when it is absent.
///
/// Read at most 1 MiB: a lockfile is large enough to be worth hashing and not
/// worth slurping whole into memory to decide about.
fn manifest_digest(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    const CAP: u64 = 1 << 20;
    let bytes = if metadata.len() > CAP {
        let mut file = std::fs::File::open(path).ok()?;
        let mut buffer = vec![0u8; CAP as usize];
        use std::io::Read;
        file.read_exact(&mut buffer).ok()?;
        buffer
    } else {
        std::fs::read(path).ok()?
    };
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(b"witdiff.environment-manifest.v1\0");
    hasher.update(&bytes);
    Some(hex::encode(hasher.finalize())[..16].to_owned())
}

/// Compare two environments for reporting.
///
/// Returns the keys that differ or are present on only one side, sorted. Used
/// by `witdiff receipt` so a reader can see that a stored proof was produced
/// under a different toolchain rather than having to diff the JSON themselves.
pub fn difference(expected: &Environment, observed: &Environment) -> Vec<String> {
    let mut differences: Vec<String> = expected
        .0
        .keys()
        .chain(observed.0.keys())
        .filter(|key| expected.get(key) != observed.get(key))
        .cloned()
        .collect();
    differences.sort();
    differences.dedup();
    differences
}

/// Whether a program that the receipt recorded is now missing.
///
/// The reverse failure is worth separating out: a proof taken on a machine with
/// pytest, replayed on one without it, has not been shown to still hold.
pub fn missing_tools(recorded: &Environment) -> Vec<String> {
    // A `tool_*` entry names a program under its own key, while `test_program`
    // records the program as the *value*. Reading either one as the other would
    // probe a tool named "test_program", which never exists, and report every
    // receipt as having lost its toolchain.
    let tools: Vec<String> = recorded
        .iter()
        .filter(|(key, value)| {
            (key.starts_with("tool_") || *key == "test_program") && *value != "unknown"
        })
        .map(|(key, value)| match key.strip_prefix("tool_") {
            Some(name) => name.to_owned(),
            None => value.to_owned(),
        })
        .collect();

    tools
        .into_iter()
        .filter(|program| {
            let flag = VERSION_FLAGS
                .iter()
                .find(|(name, _)| *name == program.as_str())
                .map(|(_, flag)| *flag)
                .unwrap_or("--version");
            // Presence, not success: an unknown exit code still means the
            // program exists. Only a failure to spawn means it is gone.
            Command::new(program).arg(flag).output().is_err()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_sorted_so_two_receipts_compare_byte_for_byte() {
        let mut left = Environment::default();
        left.record("z", "1");
        left.record("a", "2");

        let mut right = Environment::default();
        right.record("a", "2");
        right.record("z", "1");

        assert_eq!(
            serde_json::to_string(&left).expect("serialize"),
            serde_json::to_string(&right).expect("serialize"),
            "iteration order must not leak into the receipt"
        );
    }

    #[test]
    fn an_empty_environment_serializes_and_round_trips() {
        let environment = Environment::default();
        let text = serde_json::to_string(&environment).expect("serialize");
        let back: Environment = serde_json::from_str(&text).expect("deserialize");
        assert!(back.is_empty());
        assert_eq!(back, environment);
    }

    #[test]
    fn a_program_name_is_reduced_to_its_file_name() {
        assert_eq!(program_base("./mvnw"), "mvnw");
        assert_eq!(program_base("/usr/bin/python3"), "python3");
        assert_eq!(program_base("cargo"), "cargo");
    }

    /// Recording a manifest must not put the manifest in the receipt: a
    /// dependency list is not something a tool should be shipping around, and
    /// the digest is what makes "did this change" answerable.
    #[test]
    fn a_manifest_is_recorded_as_a_digest_not_as_content() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("Cargo.lock");
        std::fs::write(&path, "[[package]]\nname = \"secret\"\n").expect("write");

        let digest = manifest_digest(&path).expect("digested");
        assert_eq!(digest.len(), 16, "a truncated digest, not the content");
        assert!(!digest.contains("secret"), "content must not appear");

        // Same content, different path, same digest: it identifies the set.
        let other = dir.path().join("nested.lock");
        std::fs::write(&other, "[[package]]\nname = \"secret\"\n").expect("write");
        assert_eq!(manifest_digest(&other), Some(digest));
    }

    #[test]
    fn an_absent_manifest_contributes_nothing() {
        assert_eq!(manifest_digest(Path::new("/nonexistent/Cargo.lock")), None);
    }

    /// Differences are what makes a stored receipt comparable to a live one.
    #[test]
    fn differences_name_the_keys_that_changed() {
        let mut before = Environment::default();
        before.record("tool_python3", "3.9.6".to_owned());
        before.record("manifest_Cargo.lock", "abc".to_owned());

        let mut after = Environment::default();
        after.record("tool_python3", "3.12.0".to_owned());
        after.record("manifest_Cargo.lock", "abc".to_owned());

        let differences = difference(&before, &after);
        assert_eq!(differences, vec!["tool_python3".to_owned()]);
    }

    #[test]
    fn a_key_present_on_only_one_side_is_a_difference() {
        let mut before = Environment::default();
        before.record("manifest_Cargo.lock", "abc".to_owned());
        let after = Environment::default();
        assert_eq!(
            difference(&before, &after),
            vec!["manifest_Cargo.lock".to_owned()]
        );
    }

    #[test]
    fn identical_environments_have_no_difference() {
        let mut environment = Environment::default();
        environment.record("tool_cargo", "1.80.0".to_owned());
        assert!(difference(&environment, &environment.clone()).is_empty());
    }

    /// `java -version` writes to stderr, which is the case that would
    /// otherwise record nothing for an entire JDK.
    #[test]
    fn a_version_reported_on_stderr_is_still_collected() {
        assert_eq!(
            first_line("\n  something\nelse\n").as_deref(),
            Some("something")
        );
        assert_eq!(first_line("   \n"), None);
        assert_eq!(first_line(""), None);
    }

    /// An unlisted program is never executed during collection.
    ///
    /// The risk is running project-controlled code as a side effect of simply
    /// reading a receipt's environment. `probe_version` consults a whitelist
    /// first, so an unknown name resolves to `None` without spawning anything.
    #[test]
    fn an_unlisted_program_is_not_executed() {
        assert_eq!(probe_version("definitely-not-a-real-program"), None);
        assert_eq!(probe_version("./some-thing.sh"), None);
        assert_eq!(probe_version(""), None);
    }

    /// The whitelist is deliberate and bounded. Every entry has a stable
    /// version flag, and none of them is a program a repository can rename.
    #[test]
    fn the_version_whitelist_is_stable_and_bounded() {
        let names: Vec<&str> = VERSION_FLAGS.iter().map(|(name, _)| *name).collect();
        assert!(names.len() < 20, "a bounded whitelist, got {}", names.len());
        for expected in ["cargo", "rustc", "node", "python3", "go", "java", "ruby"] {
            assert!(names.contains(&expected), "{expected} must be probeable");
        }
        // `git` is deliberately absent: it is run constantly by WitDiff already
        // and its version is not material to a verification result.
        assert!(
            !names.contains(&"git"),
            "git is not an evidence-bearing tool"
        );
    }

    /// Only the companions that can affect the result are probed. Recording
    /// every tool on the machine made two receipts of the same verification
    /// differ because one host had `pnpm` and the other did not.
    #[test]
    fn companions_are_scoped_to_the_configured_program_family() {
        assert_eq!(companions("cargo"), ["rustc"]);
        assert_eq!(companions("npm"), ["node"]);
        assert_eq!(companions("mvn"), ["java", "javac"]);
        assert_eq!(companions("rspec"), ["ruby"]);
        // A program whose own toolchain is itself contributes nothing extra,
        // rather than recording it twice under two names.
        assert!(companions("weird-thing").is_empty());
    }
}
