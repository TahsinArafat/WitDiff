//! Notify when a newer release exists.
//!
//! ## Design constraints, and why the shape is this one
//!
//! Three shipped invariants constrain what this module may do:
//!
//! 1. **`AGENTS.md` #4**: core verification must remain usable without network
//!    access.
//! 2. **README principle #4**: core verification must work offline.
//! 3. **A signed receipt must describe a tool that still exists.** A check that
//!    ran during `verify` could, in principle, replace the binary mid-run and
//!    leave a receipt referring to a version no longer installed.
//!
//! So this module never downloads, never installs, never replaces the binary,
//! and **never runs during verification**. It prints one line at most, on a
//! command whose job is already informational (`doctor`), and everything about
//! it is best-effort:
//!
//! - **Offline is not an error.** No network, no `curl`, a timeout, malformed
//!   JSON — every failure is silent. A tool that announced "could not check for
//!   updates" on every air-gapped run would be worse than one that never
//!   checked.
//! - **Cached for a day.** A notice that appears on every invocation is noise,
//!   and a network round trip on every invocation is rude. The cache lives
//!   beside the config, not in the repository, so it cannot affect a receipt.
//! - **Opt-out is explicit.** `WITDIFF_NO_UPDATE_CHECK=1` in the environment,
//!   or `[updates] check = false` in `witdiff.toml`. The environment variable
//!   wins, because CI sets the environment and the repository may be someone
//!   else's.
//! - **The channel is inherited.** An alpha install is offered alpha updates; a
//!   stable install is offered stable updates. Upgrading a user off the channel
//!   they chose is not a courtesy.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The repository releases are read from.
const REPO: &str = "TahsinArafat/WitDiff";

/// Where the installer lives.
///
/// A full URL, not a relative path. Someone running an installed binary does
/// not have the repository checked out, so `./install.sh` is not a command they
/// can run — and `install.sh --force` is worse, because it looks runnable and
/// fails with "command not found" for the exact user the notice is addressed
/// to. The notice must name something that works from any directory.
const INSTALL_URL: &str = "https://raw.githubusercontent.com/TahsinArafat/WitDiff/main/install.sh";

/// How long a cached answer stays usable.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the network is given before the check is abandoned.
///
/// Short on purpose. This is a courtesy notice; it must never be something a
/// user waits for, and a slow network must degrade to "no notice" rather than
/// to a hang.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The outcome of a check.
#[derive(Debug, PartialEq, Eq)]
pub enum Notice {
    /// A newer release exists on the same channel.
    Available { installed: String, latest: String },
    /// Already current, or newer than anything published.
    Current,
    /// The check did not happen, and that is fine.
    Unavailable,
}

/// Whether the user or the project asked for no check.
pub fn disabled(project_root: &Path) -> bool {
    if std::env::var_os("WITDIFF_NO_UPDATE_CHECK").is_some() {
        return true;
    }
    // A project-level opt-out, so a repository can turn this off for everyone
    // who works in it without each developer exporting a variable.
    let config = project_root.join("witdiff.toml");
    match std::fs::read_to_string(config) {
        Ok(text) => text.lines().any(|line| {
            let line = line.trim();
            line == "check = false" || line == "check=false"
        }),
        Err(_) => false,
    }
}

/// Check for a newer release, using the cache when it is fresh.
pub fn check(project_root: &Path, installed: &str) -> Notice {
    if disabled(project_root) {
        return Notice::Unavailable;
    }
    let cache = cache_path();
    if let Some(cached) = read_cache(&cache) {
        return classify(installed, &cached);
    }
    let Some(latest) = fetch_latest(installed) else {
        return Notice::Unavailable;
    };
    write_cache(&cache, &latest);
    classify(installed, &latest)
}

/// Compare the installed version with the latest tag on the same channel.
///
/// The comparison is on the numeric components, then on the pre-release
/// identifier, so `1.0.0-alpha.10` sorts after `1.0.0-alpha.9` — a string
/// comparison would call the tenth alpha older than the ninth and quietly stop
/// offering updates at alpha.9.
fn classify(installed: &str, latest: &str) -> Notice {
    let installed = installed.trim_start_matches('v');
    let latest = latest.trim_start_matches('v');
    if !is_parseable(installed) || !is_parseable(latest) {
        return Notice::Unavailable;
    }
    if version_key(installed) >= version_key(latest) {
        return Notice::Current;
    }
    Notice::Available {
        installed: installed.to_owned(),
        latest: latest.to_owned(),
    }
}

/// A sortable key for a version, including its pre-release identifier.
///
/// `(major, minor, patch, channel_rank, prerelease_number)`, where channel rank
/// is 0 for a pre-release and 1 for a release so a full release outranks its own
/// alpha.
///
/// An earlier version ranked a release 0 and a pre-release 1, which made `1.0.0`
/// sort *older* than `1.0.0-alpha.3`. The direction matters: the reverse would
/// tell every alpha user they were up to date on the day `v1.0.0` shipped.
/// Caught by the tests, not by reading.
fn version_key(version: &str) -> (u64, u64, u64, u8, u64) {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    match pre {
        Some(pre) if !pre.is_empty() => {
            let number = pre
                .rsplit('.')
                .next()
                .and_then(|last| last.parse::<u64>().ok())
                .unwrap_or(0);
            (major, minor, patch, 0, number)
        }
        _ => (major, minor, patch, 1, 0),
    }
}

/// Whether a version string is one this module can reason about.
///
/// A tag that does not begin with digits is not compared at all. Treating an
/// unparseable string as `0.0.0` would make `witdiff doctor` announce an update
/// to anyone whose version contained a typo.
fn is_parseable(version: &str) -> bool {
    let core = version.split('-').next().unwrap_or_default();
    let head = core.split('.').next().unwrap_or_default();
    !head.is_empty() && head.chars().all(|c| c.is_ascii_digit())
}

/// Read the newest release tag on the channel `installed` is on.
///
/// `curl` rather than an HTTP client crate: the installer already requires it,
/// and adding a TLS stack to the binary for a courtesy notice would enlarge the
/// dependency surface of a tool whose value is that it has almost none.
fn fetch_latest(installed: &str) -> Option<String> {
    let installed = installed.trim_start_matches('v');
    let wants_prerelease = installed.contains('-');

    // `gh` is not assumed either; the plain REST endpoint needs no token for a
    // public repository.
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time"])
        .arg(TIMEOUT.as_secs().to_string())
        .args(["-H", "Accept: application/vnd.github+json"])
        .arg(format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=30"
        ))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let releases: Vec<serde_json::Value> = serde_json::from_str(&body).ok()?;
    releases
        .iter()
        .filter(|release| release["draft"].as_bool() != Some(true))
        .filter_map(|release| release["tag_name"].as_str())
        .map(|tag| tag.trim_start_matches('v').to_owned())
        .filter(|tag| tag.contains('-') == wants_prerelease)
        .max_by_key(|tag| version_key(tag))
}

fn cache_path() -> PathBuf {
    // Beside the user's config, never inside the repository: a file written
    // into the working tree changes the workspace fingerprint, and evidence
    // freshness exists precisely to catch that.
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("witdiff").join("update-check")
}

fn read_cache(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let stamp: u64 = lines.next()?.trim().parse().ok()?;
    let version = lines.next()?.trim().to_owned();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if now.saturating_sub(stamp) > CACHE_TTL.as_secs() {
        return None;
    }
    Some(version)
}

fn write_cache(path: &Path, version: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Best-effort: a read-only cache directory must not affect the command.
    let _ = std::fs::write(path, format!("{now}\n{version}\n"));
}

/// The line to print, or `None` when there is nothing to say.
pub fn message(notice: &Notice) -> Option<String> {
    match notice {
        Notice::Available { installed, latest } => Some(format!(
            "note: witdiff {latest} is available (installed {installed}).\n\
             \x20       Update with:\n\
             \x20         curl -fsSL {INSTALL_URL} | sh -s -- --force\n\
             \x20       Set WITDIFF_NO_UPDATE_CHECK=1 to silence this."
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_patch_is_available() {
        assert_eq!(
            classify("1.0.0", "1.0.1"),
            Notice::Available {
                installed: "1.0.0".into(),
                latest: "1.0.1".into()
            }
        );
    }

    #[test]
    fn the_same_version_is_current() {
        assert_eq!(classify("1.0.0", "1.0.0"), Notice::Current);
    }

    #[test]
    fn a_leading_v_is_ignored_on_both_sides() {
        assert_eq!(classify("v1.0.0", "v1.0.0"), Notice::Current);
        assert_eq!(
            classify("1.0.0", "v1.0.1"),
            Notice::Available {
                installed: "1.0.0".into(),
                latest: "1.0.1".into()
            }
        );
    }

    /// The bug a string comparison would produce: `alpha.10` must be newer than
    /// `alpha.9`, or updates stop being offered at the ninth alpha.
    #[test]
    fn a_two_digit_prerelease_number_sorts_correctly() {
        assert_eq!(
            classify("1.0.0-alpha.9", "1.0.0-alpha.10"),
            Notice::Available {
                installed: "1.0.0-alpha.9".into(),
                latest: "1.0.0-alpha.10".into()
            }
        );
        assert_eq!(classify("1.0.0-alpha.10", "1.0.0-alpha.9"), Notice::Current);
    }

    /// A full release outranks a pre-release of the same version.
    #[test]
    fn a_release_outranks_its_own_prerelease() {
        assert_eq!(
            classify("1.0.0-alpha.3", "1.0.0"),
            Notice::Available {
                installed: "1.0.0-alpha.3".into(),
                latest: "1.0.0".into()
            }
        );
        assert_eq!(classify("1.0.0", "1.0.0-alpha.3"), Notice::Current);
    }

    /// A version that cannot be parsed is not a basis for telling someone to
    /// update. `Unavailable` is the honest answer: nothing was learned.
    #[test]
    fn a_malformed_version_is_not_an_update_offer() {
        assert_eq!(classify("", "1.0.0"), Notice::Unavailable);
        assert_eq!(classify("nonsense", "1.0.0"), Notice::Unavailable);
        assert_eq!(classify("1.0.0", "nonsense"), Notice::Unavailable);
        assert_eq!(classify("v", "1.0.0"), Notice::Unavailable);
    }

    #[test]
    fn the_environment_variable_disables_the_check() {
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: single-threaded test process control of a variable this
        // module reads; no other test touches it.
        unsafe { std::env::set_var("WITDIFF_NO_UPDATE_CHECK", "1") };
        assert!(disabled(dir.path()));
        unsafe { std::env::remove_var("WITDIFF_NO_UPDATE_CHECK") };
        assert!(!disabled(dir.path()));
    }

    #[test]
    fn the_project_can_opt_out() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(!disabled(dir.path()));
        std::fs::write(
            dir.path().join("witdiff.toml"),
            "[updates]\ncheck = false\n",
        )
        .unwrap();
        assert!(disabled(dir.path()));
    }

    /// An offline machine must produce no notice and no error. Every failure
    /// path returns `Unavailable`, and `message` renders nothing for it.
    #[test]
    fn being_unavailable_produces_no_output() {
        assert!(message(&Notice::Unavailable).is_none());
        assert!(message(&Notice::Current).is_none());
    }

    #[test]
    fn an_available_update_names_both_versions_and_the_way_out() {
        let text = message(&Notice::Available {
            installed: "1.0.0-alpha.2".into(),
            latest: "1.0.0-alpha.3".into(),
        })
        .expect("a notice");
        assert!(text.contains("1.0.0-alpha.3"), "{text}");
        assert!(text.contains("1.0.0-alpha.2"), "{text}");
        assert!(text.contains("WITDIFF_NO_UPDATE_CHECK"), "{text}");
    }

    /// A stale cache must not be believed.
    #[test]
    fn an_expired_cache_entry_is_ignored() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("update-check");
        std::fs::write(&path, "1\n1.0.0\n").unwrap();
        assert!(read_cache(&path).is_none(), "a stamp from 1970 is stale");
    }

    #[test]
    fn a_fresh_cache_entry_is_used() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("update-check");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        std::fs::write(&path, format!("{now}\n1.0.0-alpha.3\n")).unwrap();
        assert_eq!(read_cache(&path).as_deref(), Some("1.0.0-alpha.3"));
    }

    /// The cache must live outside any repository. A file written into the
    /// working tree changes the workspace fingerprint, and evidence freshness
    /// exists specifically to catch that — an update check that invalidated its
    /// own run's evidence would be a self-inflicted stale receipt.
    #[test]
    fn the_cache_is_not_inside_a_repository() {
        let path = cache_path();
        assert!(
            !path.starts_with(std::env::current_dir().unwrap_or_default()),
            "the cache must not sit under the working directory: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains("witdiff"),
            "the cache must be namespaced to this tool: {}",
            path.display()
        );
    }

    /// Every failure mode must be silent. A tool that announced "could not
    /// check for updates" on an air-gapped CI machine would be worse than one
    /// that never checked at all.
    #[test]
    fn no_failure_path_produces_output() {
        for notice in [Notice::Unavailable, Notice::Current] {
            assert!(message(&notice).is_none(), "{notice:?} must print nothing");
        }
    }

    /// The notice must name the remedy. A user told an update exists, with no
    /// documented way to take it, is being told about a problem rather than a
    /// solution — the same gap found in the installer's uninstall path.
    /// The notice must name a command that works from any directory.
    ///
    /// It said `install.sh --force`, which is a relative path. A user running
    /// an installed binary has no repository checked out, so that command fails
    /// with "command not found" for precisely the person being told to update.
    #[test]
    fn the_notice_names_the_command_that_updates() {
        let text = message(&Notice::Available {
            installed: "1.0.0-alpha.1".into(),
            latest: "1.0.0-alpha.3".into(),
        })
        .expect("a notice");
        assert!(text.contains(INSTALL_URL), "{text}");
    }

    /// No relative path may appear in the notice.
    ///
    /// Asserted as a property rather than for one known string: the failure was
    /// `install.sh --force` being a path that only resolves inside a checkout,
    /// and the next such mistake would be a different literal. Every command in
    /// the notice must be reachable by someone who has only the binary.
    #[test]
    fn the_notice_never_names_a_path_that_needs_a_checkout() {
        let text = message(&Notice::Available {
            installed: "1.0.0-alpha.1".into(),
            latest: "1.0.0-alpha.3".into(),
        })
        .expect("a notice");

        for line in text.lines() {
            let line = line.trim();
            // A relative invocation: a bare word ending in `.sh`, or a `./`
            // prefix, or a path that is not part of a URL.
            assert!(
                !line.starts_with("./"),
                "a relative path cannot be run by someone without the repository: {line}"
            );
        }
        // Every `.sh` mention must be inside an absolute URL.
        for token in text.split_whitespace() {
            if token.contains(".sh") {
                assert!(
                    token.starts_with("https://"),
                    "`{token}` is a bare script reference; give the full URL"
                );
            }
        }
    }

    /// Running the notice's own command must be possible from any directory.
    #[test]
    fn the_notice_command_is_absolute_and_forceful() {
        let text = message(&Notice::Available {
            installed: "1.0.0-alpha.1".into(),
            latest: "1.0.0-alpha.3".into(),
        })
        .expect("a notice");
        assert!(text.contains("curl -fsSL https://"), "{text}");
        assert!(text.contains("| sh -s -- --force"), "{text}");
    }
}
