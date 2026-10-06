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

/// `--to` and `--uninstall` must agree with each other.
///
/// They did not: `installed_path` returns `$DEST/witdiff` when `--to` is
/// given, and the `case` guarding removal rejected that same path as "outside
/// the known install locations". An install made with the two flags the README
/// documents could therefore not be undone with them, and the error told the
/// user to delete by hand a file this script had just created.
///
/// This **executes** the uninstall branch against a stub install rather than
/// grepping for strings. A first version of this test read the source for
/// `$DEST/witdiff` and passed with the fix reverted — the token appears in
/// `installed_path` whether or not the removal branch honours it, which is the
/// same "mentions it" versus "handles it" trap this repository has hit before.
#[test]
fn a_to_install_can_be_uninstalled() {
    let root = integrations().parent().expect("repository root").to_owned();
    let script = root.join("install.sh");
    let sandbox = tempfile::TempDir::new().expect("temp dir");
    let dest = sandbox.path().join("dest");
    std::fs::create_dir_all(&dest).expect("dest");

    // Stand in for an installed binary: the uninstall path only needs a file
    // at the resolved location.
    let binary = dest.join("witdiff");
    std::fs::write(&binary, "#!/bin/sh\necho stub\n").expect("stub");
    assert!(binary.exists());

    let output = std::process::Command::new("sh")
        .arg(&script)
        .arg("--to")
        .arg(&dest)
        .arg("--uninstall")
        .output()
        .expect("run install.sh --uninstall");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output.status.success(),
        "uninstalling a `--to` install must succeed; got {text}"
    );
    assert!(
        !binary.exists(),
        "the file the installer created must be gone; output was {text}"
    );
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
        // `$FETCHED` and `$work` are temporary directories this script created
        // and traps to remove; `$1`/`$path` are its own install locations.
        let targeted = line.contains("witdiff")
            || line.contains("$1")
            || line.contains("$path")
            || line.contains("$work")
            || line.contains("$FETCHED");
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

/// The installer must install only the harnesses that are present.
///
/// `integrations/README.md` promised this and the script did not do it: it wrote
/// rules for every harness unconditionally, so a user without Cursor found a
/// `.cursor/rules/` directory they had to clean up by hand.
#[test]
fn the_integrations_installer_copies_only_present_harnesses() {
    let dir = integrations();
    let script = std::fs::read_to_string(dir.join("install.sh")).expect("read installer");
    assert!(
        script.contains("WITDIFF_INSTALL_ALL"),
        "there must be a way to force every harness: PATH is not a reliable \
         signal, since a GUI launcher or an alias is invisible to `command -v`"
    );

    let project = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(project.path().join(".git")).expect("git dir");

    // Only `opencode` is reachable, so the others must be left alone. The stub
    // is a real executable on PATH rather than a mock of the check.
    let bin = project.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let stub = bin.join("opencode");
    std::fs::write(&stub, "#!/bin/sh\nexit 0\n").expect("write stub");
    // `std::os::unix` does not exist on Windows, so the permission bit is set
    // only where it means something. Without the guard this test fails to
    // compile against `x86_64-pc-windows-msvc`, which is a release target.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    // System tools must stay on PATH: the script needs `dirname` and `mktemp`
    // before it reaches any harness check, and a PATH of only the stub made it
    // fail with "dirname: command not found" rather than exercising detection.
    let path = format!("/usr/bin:/bin:{}", bin.display());
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("sh {} .", dir.join("install.sh").display()))
        .current_dir(project.path())
        .env("PATH", &path)
        .env_remove("WITDIFF_INSTALL_ALL")
        .output()
        .expect("run the installer");
    assert!(
        output.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        project
            .path()
            .join(".opencode/skills/witdiff/SKILL.md")
            .is_file(),
        "the harness on PATH must be installed"
    );
    for absent in [".claude", ".cursor"] {
        assert!(
            !project.path().join(absent).exists(),
            "{absent} was installed for a harness that is not on PATH; the \
             README says only present harnesses are installed"
        );
    }
}

/// With no harness reachable, install everything rather than nothing.
///
/// "Not found" is not evidence of "not used" — a GUI launcher or a shell alias
/// is invisible to `command -v`. A checker that refuses to install anything is
/// worse than one that installs too much.
///
/// This caught a real disagreement: the fallback set a display string while the
/// install guards still consulted the PATH check, so the script announced "all
/// integrations will be installed" and then installed none.
#[test]
fn an_empty_path_installs_every_harness() {
    let dir = integrations();
    let project = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(project.path().join(".git")).expect("git dir");

    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("sh {} .", dir.join("install.sh").display()))
        .current_dir(project.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("WITDIFF_INSTALL_ALL")
        .output()
        .expect("run the installer");
    assert!(output.status.success(), "installer failed");

    for installed in [
        ".claude/skills/witdiff/SKILL.md",
        ".opencode/skills/witdiff/SKILL.md",
        ".cursor/rules/witdiff.mdc",
    ] {
        assert!(
            project.path().join(installed).is_file(),
            "with nothing on PATH the installer must still install {installed}"
        );
    }
}

/// Uninstall must not be restricted by which harnesses are present.
///
/// Detection decides what to install; it must never decide what to remove. A
/// user uninstalling on a machine where a harness is no longer on PATH would
/// otherwise be left with files the script refuses to acknowledge.
#[test]
fn uninstall_ignores_harness_detection() {
    let dir = integrations();
    let script = dir.join("install.sh");
    let project = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(project.path().join(".git")).expect("git dir");

    let run = |args: &str| {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sh {} {args}", script.display()))
            .current_dir(project.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run the installer")
    };

    // Installed with everything forced, then removed with nothing on PATH.
    assert!(std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("sh {} .", script.display()))
        .current_dir(project.path())
        .env("PATH", "/usr/bin:/bin")
        .env("WITDIFF_INSTALL_ALL", "1")
        .output()
        .expect("install")
        .status
        .success());
    assert!(project
        .path()
        .join(".claude/skills/witdiff/SKILL.md")
        .is_file());

    assert!(run("--uninstall .").status.success(), "uninstall failed");
    assert!(
        !project.path().join(".claude/skills/witdiff").exists(),
        "uninstall left files behind because no harness was on PATH"
    );
}

/// Running the integrations installer through a pipe must actually install.
///
/// This was broken: `$0` is the interpreter (`sh`) when a script is piped into a
/// shell, so the installer looked for its packages beside `/bin`, found none,
/// copied nothing, and **exited 0** — a success report for an install that did
/// not happen.
///
/// The earlier version of this test searched the script for identifiers, and
/// passed while the detection condition was replaced with `if false`. It now
/// runs the script through a pipe and checks the files exist, which is the only
/// thing that distinguishes a working install from the reported failure.
#[test]
fn the_integrations_installer_installs_when_piped() {
    let dir = integrations();
    let path = dir.join("install.sh");
    let script = std::fs::read_to_string(&path).expect("read integrations/install.sh");

    let project = tempfile::TempDir::new().expect("temp dir");
    // A checkout is present, so the download path is not taken and the test
    // stays offline; what is exercised is that the packages are found and
    // copied at all.
    std::fs::create_dir_all(project.path().join(".git")).expect("git dir");

    // Run the script's text through a shell, the way the documented one-liner
    // does, so `$0` is the interpreter exactly as in the failure.
    let mut child = std::process::Command::new("sh")
        .arg("-s")
        .current_dir(project.path())
        .env("WITDIFF_INSTALL_DIR", dir.display().to_string())
        .env("WITDIFF_INSTALL_ALL", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run the installer through a pipe");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(script.as_bytes())
            .expect("write the script to the shell");
    }
    let output = child.wait_with_output().expect("wait for the installer");

    assert!(
        output.status.success(),
        "the piped installer must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Which files appear depends on which harnesses are on PATH, so the check
    // is that *something* was installed rather than a fixed set: forcing
    // WITDIFF_INSTALL_ALL makes the set deterministic.
    for installed in [
        ".claude/skills/witdiff/SKILL.md",
        ".opencode/plugins/witdiff.ts",
        ".cursor/rules/witdiff.mdc",
    ] {
        assert!(
            project.path().join(installed).is_file(),
            "piping the installer produced no {installed}; it reported success \
             without installing, which is the bug this guards"
        );
    }
}

/// The platform-detection branch and the archive mapping must agree.
///
/// `detect_target` decides which triple this machine is; the mapping below it
/// decides which archive that triple names. A target present in one but not the
/// other produces an installer that identifies the platform and then refuses it.
#[test]
fn every_detected_target_has_an_archive() {
    let dir = integrations();
    let root = dir.parent().expect("repository root");
    let text = std::fs::read_to_string(root.join("install.sh")).expect("read install.sh");

    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        // The triple must be produced by platform detection *and* accepted by
        // the archive mapping. The two regions are located by slicing between
        // the `case` that starts them and the next line that closes it, because
        // the blocks are indented to different depths and a search for `esac`
        // at column zero ran past the end of the first one entirely — which made
        // this test pass while the Windows branch was deleted.
        // Sliced between the `case` under the marker and the `esac` that closes
        // it. Nesting is counted, because these blocks contain inner `case`
        // statements: stopping at the first `esac` truncated the detection block
        // at the Linux branch, so the test reported that macOS was undetected in
        // a script that detects it.
        let region = |marker: &str| -> String {
            let start = text
                .find(marker)
                .unwrap_or_else(|| panic!("install.sh has no `{marker}`"));
            let rest = &text[start..];
            let mut depth = 0i32;
            let mut opened = false;
            let mut offset = 0;
            for line in rest.split_inclusive('\n') {
                let word = line.trim();
                if word.ends_with("in") && word.starts_with("case ") || word == "case" {
                    depth += 1;
                    opened = true;
                } else if word == "esac" {
                    depth -= 1;
                    if opened && depth == 0 {
                        return rest[..offset + line.len()].to_owned();
                    }
                }
                offset += line.len();
            }
            panic!("the `{marker}` block is never closed");
        };

        let detection = region("detect_target()");
        let mapping = region("case \"$target\" in");

        assert_ne!(
            detection, mapping,
            "the two regions must be found separately, or one test covers both"
        );
        assert!(
            detection.contains(target),
            "install.sh does not detect {target}; a platform the release publishes \
             for would be refused with \"no prebuilt binary\""
        );
        assert!(
            mapping.contains(target),
            "install.sh maps no archive for {target}"
        );
    }
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

/// `witdiff uninstall` must work without any copy of the installer.
///
/// The failure it exists for: the installer printed `install.sh --uninstall`,
/// but the documented install path is `curl ... | sh`, which saves no copy.
/// Measured from a real shell, a user ran exactly that and got
/// `zsh: command not found: install.sh`. The binary is the one thing an install
/// always leaves behind, so it has to be able to remove itself.
#[test]
fn the_binary_can_uninstall_itself_without_a_script() {
    let binary = env!("CARGO_BIN_EXE_witdiff");
    let sandbox = tempfile::TempDir::new().expect("temp dir");
    let bin_dir = sandbox.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let installed = bin_dir.join(if cfg!(windows) {
        "witdiff.exe"
    } else {
        "witdiff"
    });
    std::fs::copy(binary, &installed).expect("copy the binary");

    // A HOME with one integration of ours and one of the user's own.
    let home = sandbox.path().join("home");
    std::fs::create_dir_all(home.join(".claude/skills/witdiff")).expect("ours");
    std::fs::create_dir_all(home.join(".claude/skills/my-own-skill")).expect("theirs");
    std::fs::write(home.join(".claude/skills/my-own-skill/notes.md"), "mine").expect("write");

    let output = std::process::Command::new(&installed)
        .arg("uninstall")
        .env("HOME", &home)
        .output()
        .expect("run uninstall");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // On Windows a running executable cannot delete itself; the command is
    // expected to report that rather than pretend. Everywhere else the file
    // must be gone.
    #[cfg(not(windows))]
    assert!(
        !installed.exists(),
        "the binary must remove itself; output={text}"
    );
    #[cfg(windows)]
    assert!(
        !output.status.success(),
        "on Windows the failure to self-delete must be reported, not hidden; output={text}"
    );

    assert!(
        !home.join(".claude/skills/witdiff").exists(),
        "our own integration must be removed; output={text}"
    );
    assert!(
        home.join(".claude/skills/my-own-skill/notes.md").exists(),
        "a skill WitDiff did not install must survive; output={text}"
    );
}

/// A dry run must change nothing.
///
/// An uninstaller that cannot be previewed is one a cautious user will not run,
/// which leaves them stuck with a tool they wanted to remove.
#[test]
fn uninstall_dry_run_removes_nothing() {
    let binary = env!("CARGO_BIN_EXE_witdiff");
    let sandbox = tempfile::TempDir::new().expect("temp dir");
    let bin_dir = sandbox.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let installed = bin_dir.join(if cfg!(windows) {
        "witdiff.exe"
    } else {
        "witdiff"
    });
    std::fs::copy(binary, &installed).expect("copy");

    let home = sandbox.path().join("home");
    std::fs::create_dir_all(home.join(".claude/skills/witdiff")).expect("ours");

    let output = std::process::Command::new(&installed)
        .args(["uninstall", "--dry-run"])
        .env("HOME", &home)
        .output()
        .expect("run");
    assert!(output.status.success());

    assert!(installed.exists(), "a dry run must not remove the binary");
    assert!(
        home.join(".claude/skills/witdiff").exists(),
        "a dry run must not remove integrations"
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Nothing was changed"), "{text}");
}

/// `--keep-integrations` removes only the binary.
#[test]
fn uninstall_can_keep_the_integrations() {
    let binary = env!("CARGO_BIN_EXE_witdiff");
    let sandbox = tempfile::TempDir::new().expect("temp dir");
    let bin_dir = sandbox.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let installed = bin_dir.join(if cfg!(windows) {
        "witdiff.exe"
    } else {
        "witdiff"
    });
    std::fs::copy(binary, &installed).expect("copy");

    let home = sandbox.path().join("home");
    std::fs::create_dir_all(home.join(".claude/skills/witdiff")).expect("ours");

    let _ = std::process::Command::new(&installed)
        .args(["uninstall", "--keep-integrations"])
        .env("HOME", &home)
        .output()
        .expect("run");

    assert!(
        home.join(".claude/skills/witdiff").exists(),
        "--keep-integrations must leave them alone"
    );
}

/// No printed instruction may name a file the reader is not guaranteed to have.
///
/// A property rather than a string check, because the last version of this
/// mistake was the literal `install.sh --uninstall` and the next would be a
/// different literal. `To remove:` must name the `witdiff` command; a script
/// reference may appear only as a clearly-labelled fallback with a URL.
#[test]
fn the_installer_never_requires_a_local_script() {
    let root = integrations().parent().expect("repository root").to_owned();
    let text = std::fs::read_to_string(root.join("install.sh")).expect("read install.sh");

    assert!(
        text.contains("To remove: witdiff uninstall"),
        "the removal hint must name the binary, which always exists"
    );
    // Every bare `install.sh --flag` mention must be inside a URL or inside a
    // line that also offers the URL form.
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        // Lines that explicitly scope themselves to a checkout are offering
        // the local form *in addition to* the URL, which is the point.
        let scoped_local = trimmed.to_ascii_lowercase();
        if scoped_local.contains("no local copy of this script")
            || scoped_local.contains("from a checkout")
        {
            continue;
        }
        if trimmed.contains("install.sh --") && !trimmed.contains("http") {
            // A usage line inside the script's own help is fine: the reader
            // already has the file at that point.
            let is_own_usage = trimmed.starts_with("Usage:") || trimmed.starts_with("--");
            assert!(
                is_own_usage || trimmed.contains("$0"),
                "line {} names a bare installer path in output the user reads: {trimmed}",
                index + 1
            );
        }
    }
}

/// The README must not tell a user to run a script they do not have.
///
/// Reported twice from a real shell — `zsh: command not found: install.sh` —
/// and both times the installer's own output was fixed while the README kept
/// the same mistake. A bare `install.sh` only resolves inside a checkout, and
/// the documented install path is `curl ... | sh`, which saves no copy.
///
/// Asserted over the whole file rather than for known strings: the first
/// instance was `./install.sh --version`, the second `integrations/install.sh
/// --uninstall`, and a third would be a different literal.
#[test]
fn the_readme_never_names_a_script_the_user_is_not_guaranteed_to_have() {
    let root = integrations().parent().expect("repository root").to_owned();
    let text = std::fs::read_to_string(root.join("README.md")).expect("read README");

    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        // Prose about the repository's own layout is not an instruction.
        if trimmed.starts_with("Cloning the repository") {
            continue;
        }
        // A URL always resolves; anything containing `http` is fine.
        if trimmed.contains("http") {
            continue;
        }
        // A shell command is a line starting with the script name, with or
        // without a path prefix. Running one requires a local copy.
        let starts_with_script = trimmed.starts_with("./install.sh")
            || trimmed.starts_with("install.sh")
            || trimmed.starts_with("./integrations/install.sh")
            || trimmed.starts_with("integrations/install.sh")
            || trimmed.starts_with("./integrations/install.sh");
        assert!(
            !starts_with_script,
            "README line {} runs a script the reader may not have: {trimmed}",
            index + 1
        );
    }
}
