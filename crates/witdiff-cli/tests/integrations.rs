//! The agent integrations in `integrations/` must stay loadable.
//!
//! Each harness discovers these files by convention — a directory name that has
//! to match the skill's `name`, a `.mdc` extension, a `pi` key in the manifest.
//! A file that violates the convention is not rejected loudly: the harness
//! simply does not load it, which looks exactly like a working integration with
//! nothing to say. These tests make that failure visible in CI instead.
//!
//! The rules asserted here are taken from each harness's own documentation:
//!
//! - **Agent Skills spec** (Claude Code, Pi, OpenCode): `name` matches
//!   `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 characters, equal to the enclosing
//!   directory name; `description` between 1 and 1024 characters.
//! - **OpenCode** additionally recognizes only `name`, `description`, `license`,
//!   `compatibility` and `metadata`, and ignores anything else.
//! - **Cursor** requires a `.mdc` extension and frontmatter with
//!   `description`/`globs`/`alwaysApply`, or the file is ignored outright.
//! - **Pi** discovers a package's skills from conventional directories or the
//!   `pi` key in `package.json`, and expects the `pi-package` keyword for the
//!   gallery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn integrations() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|workspace| workspace.parent())
        .expect("repository root")
        .join("integrations")
}

/// Parse the YAML frontmatter of a `SKILL.md`, without a YAML dependency.
///
/// Only the top-level `key: value` pairs are needed, and the values in these
/// files are single-line. A nested block (`metadata:`) is skipped rather than
/// mis-parsed, so a real YAML library is not required to check the fields the
/// harnesses actually validate.
fn frontmatter(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{} must start with frontmatter, or every harness ignores the file",
        path.display()
    );

    let mut fields = BTreeMap::new();
    let mut in_nested_block = false;
    for line in lines {
        if line == "---" {
            return fields;
        }
        // A key with no value opens a nested block; its indented children are
        // not top-level fields and must not be reported as unknown keys.
        if in_nested_block {
            if line.starts_with(' ') || line.starts_with('\t') {
                continue;
            }
            in_nested_block = false;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            fields.insert(key.trim().to_owned(), String::new());
            in_nested_block = true;
            continue;
        }
        fields.insert(key.trim().to_owned(), value.trim_matches('"').to_owned());
    }
    panic!("{}: unterminated frontmatter", path.display());
}

/// Every `SKILL.md` under `integrations/`, with the harness it targets.
fn skills() -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for harness in ["claude", "opencode", "pi"] {
        let dir = integrations().join(harness).join("skills");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let skill = entry.path().join("SKILL.md");
            if skill.is_file() {
                found.push((harness.to_owned(), skill));
            }
        }
    }
    found
}

#[test]
fn every_harness_ships_a_skill() {
    let found = skills();
    for harness in ["claude", "opencode", "pi"] {
        assert!(
            found.iter().any(|(name, _)| name == harness),
            "{harness} has no skills/*/SKILL.md, so nothing would load"
        );
    }
}

/// The name must match the directory. OpenCode enforces this, and Pi warns that
/// other implementations may — so a mismatch is a file that loads in one harness
/// and silently does not in another.
#[test]
fn a_skill_name_matches_its_directory() {
    for (harness, path) in skills() {
        let fields = frontmatter(&path);
        let name = fields
            .get("name")
            .unwrap_or_else(|| panic!("{harness} {} has no name", path.display()));
        let directory = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(
            *name,
            directory,
            "{}: `name: {name}` must equal the directory `{directory}`, or the \
             harness will not load it",
            path.display()
        );
    }
}

#[test]
fn a_skill_name_satisfies_the_agent_skills_pattern() {
    for (harness, path) in skills() {
        let name = frontmatter(&path).remove("name").expect("checked above");
        assert!(
            !name.is_empty() && name.len() <= 64,
            "{harness} {}: name must be 1-64 characters, got {}",
            path.display(),
            name.len()
        );
        let valid = name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
        assert!(
            valid && !name.starts_with('-') && !name.ends_with('-'),
            "{harness} {}: `{name}` must match ^[a-z0-9]+(-[a-z0-9]+)*$",
            path.display()
        );
    }
}

/// The description is what the model routes on. Too long is truncated, and
/// empty means the skill is never selected.
#[test]
fn a_skill_description_is_usable_for_routing() {
    for (harness, path) in skills() {
        let description = frontmatter(&path)
            .remove("description")
            .unwrap_or_else(|| panic!("{harness} {} has no description", path.display()));
        assert!(
            !description.is_empty() && description.len() <= 1024,
            "{harness} {}: description must be 1-1024 characters, got {}",
            path.display(),
            description.len()
        );
        // A description that never says when to use the skill routes poorly.
        assert!(
            description.to_lowercase().contains("use "),
            "{harness} {}: the description should say when to use the skill, got {description:?}",
            path.display()
        );
    }
}

/// OpenCode recognizes exactly five frontmatter fields and ignores the rest.
/// A field it silently drops is a promise the file appears to make and does not.
#[test]
fn opencode_skills_use_only_recognized_frontmatter() {
    const RECOGNIZED: [&str; 5] = [
        "name",
        "description",
        "license",
        "compatibility",
        "metadata",
    ];
    for (harness, path) in skills() {
        if harness != "opencode" {
            continue;
        }
        for field in frontmatter(&path).keys() {
            assert!(
                RECOGNIZED.contains(&field.as_str()),
                "{}: `{field}` is not recognized by OpenCode and would be ignored",
                path.display()
            );
        }
    }
}

/// Cursor ignores a `.mdc` file without frontmatter, and a `.md` file
/// altogether, so both the extension and the fields are load-bearing.
#[test]
fn the_cursor_rule_is_loadable() {
    let path = integrations()
        .join("cursor")
        .join("rules")
        .join("witdiff.mdc");
    assert!(path.is_file(), "{} must exist", path.display());

    let fields = frontmatter(&path);
    assert!(
        matches!(fields.get("alwaysApply").map(String::as_str), Some("true")),
        "the rule must set `alwaysApply: true`; otherwise it is only applied when \
         the model decides it is relevant, and the verification step is exactly \
         the thing an agent skips"
    );
    assert!(
        fields.contains_key("description"),
        "Cursor requires a description to route a non-always rule, and it \
         documents one for every rule"
    );
}

/// A package that declares a path must have it. Pi loads what it is told to
/// load; a typo yields no skill and no error.
#[test]
fn the_manifests_point_at_files_that_exist() {
    let pi = integrations().join("pi");
    let manifest = pi.join("package.json");
    assert!(manifest.is_file(), "{} must exist", manifest.display());

    let text = std::fs::read_to_string(&manifest).expect("read");
    assert!(
        text.contains("\"pi\"") && text.contains("./skills"),
        "the Pi package must declare `pi.skills`, or Pi falls back to \
         conventional discovery and this manifest is decoration"
    );
    assert!(
        text.contains("pi-package"),
        "the `pi-package` keyword makes the package discoverable in the gallery"
    );
    assert!(
        pi.join("skills").is_dir(),
        "the declared skills path must exist"
    );

    let opencode = integrations().join("opencode");
    let manifest = opencode.join("package.json");
    let text = std::fs::read_to_string(&manifest).expect("read");
    assert!(
        text.contains("@opencode-ai/plugin"),
        "the plugin imports the host-provided package, which must be declared"
    );
    assert!(
        opencode.join("plugins").join("witdiff.ts").is_file(),
        "the plugin file must exist"
    );
}
/// The installer must know how to remove what it installed.
///
/// Without this, a user who tries the tool has no documented way back to a
/// clean state. Asserted rather than assumed because both installers gained
/// their uninstall path after the fact, and a regression would be invisible
/// until someone needed it.
#[test]
fn both_installers_support_uninstall() {
    let dir = integrations();
    let root = dir.parent().expect("repository root");
    for path in [root.join("install.sh"), dir.join("install.sh")] {
        let text = std::fs::read_to_string(&path).expect("read installer");
        assert!(
            text.contains("--uninstall"),
            "{}: must accept --uninstall, or a user cannot undo the install",
            path.display()
        );
        assert!(
            text.contains("removed"),
            "{}: must report what it removed",
            path.display()
        );
    }
}

/// Every installer must handle every platform the release publishes.
///
/// This executes the installer's own archive-selection logic rather than
/// pattern-matching its source. A string search passed while the mapping was
/// broken, because the target triples also appear in the platform-detection
/// branch: it could not tell "handles this target" from "mentions this target",
/// which is the whole distinction the test exists to make.
#[test]
fn the_binary_installer_covers_every_published_platform() {
    let dir = integrations();
    let root = dir.parent().expect("repository root");
    let workflow = std::fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("read release workflow");
    let script = std::fs::read_to_string(root.join("install.sh")).expect("read install.sh");

    // Lift the archive-selection `case` out of the installer, so what runs is
    // the shipped logic rather than a copy of it that could drift.
    let case_start = script
        .find("case \"$target\" in")
        .expect("install.sh must select an archive by target");
    let case_end = script[case_start..]
        .find("esac\n")
        .expect("the archive case must terminate")
        + case_start
        + "esac\n".len();
    let mapping = &script[case_start..case_end];

    let targets = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ];

    for target in targets {
        assert!(
            workflow.contains(target),
            "{target} must be in the release matrix"
        );
        let expected = if target.contains("windows") {
            format!("witdiff-{target}.zip")
        } else {
            format!("witdiff-{target}.tar.gz")
        };

        let probe = format!(
            "target={target}\narchive=\n{mapping}\n[ -n \"$archive\" ] || exit 3\nprintf '%s' \"$archive\""
        );
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&probe)
            .output()
            .expect("run the installer's mapping");
        assert!(
            output.status.success(),
            "install.sh refuses the target {target}, which the release publishes: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "install.sh maps {target} to the wrong archive"
        );
    }
}

/// The installer must run on a machine without `shasum`.
///
/// `shasum` is macOS-only and `sha256sum` is Linux-only, so an installer that
/// depends on either fails for half its users. There is a fallback.
#[test]
fn the_binary_installer_does_not_depend_on_one_hash_tool() {
    let dir = integrations();
    let root = dir.parent().expect("repository root");
    let text = std::fs::read_to_string(root.join("install.sh")).expect("read install.sh");
    for tool in ["sha256sum", "shasum", "openssl"] {
        assert!(
            text.contains(tool),
            "install.sh must fall back across hash tools; {tool} is missing"
        );
    }
}

/// A checksum failure must refuse, not warn.
///
/// This is the only thing between a user and an arbitrary download, so a
/// regression that turned it into a warning would be the worst kind.
#[test]
fn a_checksum_mismatch_refuses_to_install() {
    let dir = integrations();
    let root = dir.parent().expect("repository root");
    let text = std::fs::read_to_string(root.join("install.sh")).expect("read install.sh");
    assert!(
        text.contains("Not installing"),
        "a mismatched checksum must stop the install explicitly"
    );
    assert!(
        !text.contains("return 0\n  fi\n  echo \"checksum verified"),
        "the mismatch path must not fall through to success"
    );
}

/// An installer that deletes must delete only what it installed.
///
/// The integrations installer originally deleted nothing, and a test asserted
/// that. It then gained an `--uninstall` path, which needs to remove files — so
/// "no `rm` at all" became the wrong property. What matters is that every
/// removal is aimed at a path this installer created.
#[test]
fn removals_are_aimed_at_installed_paths_only() {
    let dir = integrations();
    let path = dir.join("install.sh");
    let text = std::fs::read_to_string(&path).expect("read installer");
    let code: String = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    for line in code.lines().filter(|l| l.contains("rm ")) {
        let targeted = line.contains("witdiff")
            || line.contains("$1")
            || line.contains("$path")
            || line.contains("$work");
        assert!(
            targeted,
            "{}: this removal is not aimed at an installed path: {line}",
            path.display()
        );
    }
    assert!(
        code.contains("rmdir"),
        "{}: shared directories must be removed with rmdir so a non-empty one is left alone",
        path.display()
    );
    assert!(
        text.contains("left in place") || text.contains("left alone"),
        "{}: the installer must say what it declined to remove",
        path.display()
    );
}

/// The same skill body is copied to several places because each harness reads
/// its own path. Copies that can drift will, and the failure is silent: one
/// harness loads a stale instruction set while another loads the current one.
///
/// `integrations/skill/SKILL.md` is the source. The in-repo copies under
/// `skills/` and `.claude/skills/` are what this repository's own agents load,
/// so they must match it exactly.
#[test]
fn the_skill_copies_do_not_drift() {
    let root = integrations();
    let root = root.parent().expect("repository root");
    let source = std::fs::read_to_string(integrations().join("skill").join("SKILL.md"))
        .expect("read the canonical skill");

    for copy in [
        "skills/witdiff/SKILL.md",
        ".claude/skills/witdiff/SKILL.md",
        "integrations/claude/skills/witdiff/SKILL.md",
        "integrations/opencode/skills/witdiff/SKILL.md",
        "integrations/pi/skills/witdiff/SKILL.md",
    ] {
        let path = root.join(copy);
        assert!(path.is_file(), "{copy} must exist");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            source,
            "{copy} differs from integrations/skill/SKILL.md; update the copies \
             with `integrations/sync-skill.sh`"
        );
    }
}

/// Every integration must carry the exit-code table, including the fact that
/// code 2 is a failed gate rather than a tool error. This is the one instruction
/// that stops a failed verification being reported as success, so a harness
/// missing it is worse than not shipping that harness.
#[test]
fn every_integration_tells_the_agent_how_to_read_exit_codes() {
    let files = [
        integrations().join("generic").join("AGENTS.md"),
        integrations()
            .join("cursor")
            .join("rules")
            .join("witdiff.mdc"),
    ];
    let mut checked = files.to_vec();
    checked.extend(skills().into_iter().map(|(_, path)| path));

    for path in checked {
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("exit") && text.contains('2'),
            "{}: must explain the exit codes, or a failed gate reads as success",
            path.display()
        );
        assert!(
            text.contains("verified"),
            "{}: must name the `verified` status so the agent reads the receipt \
             rather than trusting the exit code alone",
            path.display()
        );
    }
}
