use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::framework::TestFramework;
use crate::model::{FailureKind, RunResult};

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn from_vec(parts: Vec<String>) -> Result<Self> {
        let mut iter = parts.into_iter();
        let program = iter.next().filter(|s| !s.is_empty()).ok_or_else(|| {
            anyhow::anyhow!("test command is empty; configure verification.test_command")
        })?;
        Ok(Self {
            program,
            args: iter.collect(),
        })
    }

    pub fn as_vec(&self) -> Vec<String> {
        let mut result = Vec::with_capacity(self.args.len() + 1);
        result.push(self.program.clone());
        result.extend(self.args.clone());
        result
    }
}

/// Run a command inside a container rather than on the host.
///
/// The container is named, because killing `docker run` kills the *client*
/// and not the container it started. On a timeout that would leave the
/// container running forever, which is a resource leak and a long-lived
/// instance of the very code the sandbox exists to contain. The name is
/// therefore kept so the timeout path can remove it.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// The image to run in, as an operator would name it.
    pub image: String,
    /// Host environment variables to forward, by name.
    ///
    /// An explicit allow-list: a sandboxed run does not inherit the operator's
    /// shell by default.
    pub env: Vec<String>,
    /// Whether the container may reach the network.
    pub network: bool,
}

impl Sandbox {
    /// Build the sandbox a configuration asks for, or `None` when it does not
    /// ask for one.
    ///
    /// An image that is configured but empty is treated as no sandbox: a typo
    /// that silently ran commands on the host would be worse than a typo that
    /// failed loudly, so an empty value is refused by the caller instead of
    /// being used as an image name.
    pub fn from_config(config: &crate::config::VerificationConfig) -> Option<Self> {
        let image = config.sandbox_image.as_ref()?.trim();
        if image.is_empty() {
            return None;
        }
        Some(Self {
            image: image.to_owned(),
            env: config.sandbox_env.clone(),
            network: config.sandbox_network,
        })
    }

    /// Rewrite a command so it runs inside this sandbox.
    ///
    /// The workspace is mounted at its own absolute path, so paths inside it
    /// resolve identically in and out of the container. Paths *outside* the
    /// workspace do not: the host's toolchain, its cargo registry, and any
    /// absolute path configured above the workspace are not there. That is the
    /// point of the isolation, and it is why the image must carry the
    /// toolchain the command needs.
    pub fn wrap(&self, spec: &CommandSpec, cwd: &Path, container: &str) -> CommandSpec {
        let path = cwd.to_string_lossy().into_owned();
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            container.to_owned(),
            "--workdir".into(),
            path.clone(),
            // The same absolute path inside, so output and any path recorded
            // in the workspace keep working.
            "--volume".into(),
            format!("{path}:{path}"),
            "--env".into(),
            "WITDIFF=1".into(),
        ];
        if !self.network {
            args.push("--network".into());
            args.push("none".into());
        }
        for name in &self.env {
            args.push("--env".into());
            args.push(name.clone());
        }
        args.push(self.image.clone());
        args.push(spec.program.clone());
        args.extend(spec.args.iter().cloned());
        CommandSpec {
            program: "docker".into(),
            args,
        }
    }
}

/// A container name unique to one run.
///
/// Not derived from the workspace path: two fixtures can share a temporary
/// directory, and a colliding name would let one run remove the other's
/// container.
fn container_name() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("witdiff-{}-{sequence}", std::process::id())
}

/// Remove a container if it still exists.
///
/// Best-effort: a `docker` that is absent or refusing to run means the sandbox
/// was never usable, and failing to clean up must not turn a timeout into a
/// tool error.
fn remove_container(name: &str) {
    if name.is_empty() {
        return;
    }
    let _ = Command::new("docker")
        .args(["rm", "--force", name])
        .output();
}

/// Execute a command under an optional wall-clock deadline.
///
/// ## Why the pipes are drained on separate threads
///
/// A child that writes more than one pipe buffer (64 KiB on Linux and macOS)
/// blocks in `write` until its parent reads. If the parent instead waits for
/// exit first — as `Command::output()` does internally, but only while also
/// reading — a naive "spawn, wait with deadline, then collect" implementation
/// deadlocks: the child cannot exit until drained, and the parent will not
/// drain until it exits. Measured empirically: exactly 65536 bytes are
/// delivered before the child parks forever.
///
/// Therefore stdout and stderr are drained by dedicated threads from the moment
/// the child starts, and only the waiting thread observes the deadline.
///
/// ## Timeout semantics
///
/// On deadline expiry the child is killed, its output is still collected, and
/// the result is returned with `timed_out = true` rather than an `Err`. A
/// timeout is a fact about the candidate's test suite, not a WitDiff
/// malfunction, so it must be representable in the receipt. It is always
/// classified as `FailureKind::Timeout`, which no proof path accepts.
pub fn run(
    spec: &CommandSpec,
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Option<Duration>,
    framework: TestFramework,
    sandbox: Option<&Sandbox>,
) -> Result<RunResult> {
    if !cwd.is_dir() {
        bail!("test working directory does not exist: {}", cwd.display());
    }
    // The container is named before anything starts so the timeout path can
    // remove it; see `Sandbox` for why the name matters.
    let container = sandbox.map(|_| container_name());
    let effective = match (sandbox, &container) {
        (Some(sandbox), Some(name)) => sandbox.wrap(spec, cwd, name),
        _ => spec.clone(),
    };
    run_inner(
        &effective,
        cwd,
        max_output_bytes,
        timeout,
        framework,
        container.as_deref().unwrap_or_default(),
    )
}

fn run_inner(
    spec: &CommandSpec,
    cwd: &Path,
    max_output_bytes: usize,
    timeout: Option<Duration>,
    framework: TestFramework,
    container: &str,
) -> Result<RunResult> {
    let started = Instant::now();
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(cwd)
        .env("WITDIFF", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute test command: {}", spec.program))?;

    // Start draining before waiting, so a chatty child can never fill a pipe.
    let stdout_pipe = child
        .stdout
        .take()
        .context("failed to capture test command stdout")?;
    let stderr_pipe = child
        .stderr
        .take()
        .context("failed to capture test command stderr")?;
    let stdout_reader = std::thread::spawn(move || read_all(stdout_pipe));
    let stderr_reader = std::thread::spawn(move || read_all(stderr_pipe));

    // A dedicated waiter reports exit without the deadline racing the reaping.
    let (exit_tx, exit_rx) = mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = exit_tx.send(child.wait());
    });

    let mut timed_out = false;
    let status = match timeout {
        Some(limit) => match exit_rx.recv_timeout(limit) {
            Ok(status) => status.context("failed waiting for test command")?,
            Err(_) => {
                timed_out = true;
                terminate(pid);
                // Killing the client does not stop the container, so remove it
                // explicitly. Otherwise a timed-out run leaves the code the
                // sandbox is meant to contain running on the host indefinitely.
                remove_container(container);
                // Reap the killed child so it does not linger as a zombie. The
                // wait already succeeded on the kill signal; if this second
                // receive also times out the process group is wedged beyond
                // what WitDiff can clean up, so degrade to no exit code.
                match exit_rx.recv_timeout(REAP_GRACE) {
                    Ok(status) => status.context("failed reaping timed-out test command")?,
                    Err(_) => {
                        return finish(
                            spec,
                            cwd,
                            started,
                            stdout_reader,
                            stderr_reader,
                            max_output_bytes,
                            timed_out,
                            None,
                            false,
                            framework,
                        );
                    }
                }
            }
        },
        None => exit_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("test command waiter terminated unexpectedly"))?
            .context("failed waiting for test command")?,
    };

    finish(
        spec,
        cwd,
        started,
        stdout_reader,
        stderr_reader,
        max_output_bytes,
        timed_out,
        status.code(),
        status.success(),
        framework,
    )
}

/// Grace period for a killed child to be reaped.
const REAP_GRACE: Duration = Duration::from_secs(10);

/// Assemble the [`RunResult`] once the child has exited (or been killed).
#[allow(clippy::too_many_arguments)]
fn finish(
    spec: &CommandSpec,
    cwd: &Path,
    started: Instant,
    stdout_reader: std::thread::JoinHandle<Vec<u8>>,
    stderr_reader: std::thread::JoinHandle<Vec<u8>>,
    max_output_bytes: usize,
    timed_out: bool,
    exit_code: Option<i32>,
    success: bool,
    framework: TestFramework,
) -> Result<RunResult> {
    // The readers finish once the child's pipes close, which happens on exit.
    let stdout_raw = stdout_reader.join().unwrap_or_default();
    let stderr_raw = stderr_reader.join().unwrap_or_default();
    let stdout = bounded_tail(&String::from_utf8_lossy(&stdout_raw), max_output_bytes);
    let stderr = bounded_tail(&String::from_utf8_lossy(&stderr_raw), max_output_bytes);

    // A timeout is never a success, whatever partial output looked like.
    let success = success && !timed_out;
    let failure_kind = if success {
        None
    } else if timed_out {
        Some(FailureKind::Timeout)
    } else {
        Some(framework.classify(&stdout, &stderr, exit_code))
    };

    Ok(RunResult {
        command: spec.as_vec(),
        cwd: cwd.display().to_string(),
        success,
        exit_code,
        duration_ms: started.elapsed().as_millis(),
        stdout,
        stderr,
        failure_kind,
        timed_out,
        // `run` takes one sample. `crate::run::run_repeating` is what sets
        // these to something else, so a direct caller gets the honest default.
        stability: crate::run::Stability::SingleRun,
        repeats: 1,
    })
}

fn read_all(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    // A read error on the pipe means the child died mid-write; the partial
    // output is still the best evidence available, so keep it.
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}

/// Best-effort termination of a child that overran its deadline.
fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: `kill` is async-signal-safe and takes no pointers. `pid` is a
        // live child of this process, so the only failure mode is the child
        // having exited between the timeout and this call, which is benign.
        unsafe {
            libc_kill(pid as i32, SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        // No portable way to signal an arbitrary child without a process
        // handle; the waiter's reap grace keeps this from hanging forever.
        let _ = pid;
    }
}

#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

pub fn classify_failure(stdout: &str, stderr: &str) -> FailureKind {
    TestFramework::Cargo.classify(stdout, stderr, None)
}

fn bounded_tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let start = text.len().saturating_sub(max_bytes);
    let mut boundary = start;
    while boundary < text.len() && !text.is_char_boundary(boundary) {
        boundary += 1;
    }
    format!(
        "[output truncated; showing final {max_bytes} bytes]\n{}",
        &text[boundary..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_cargo_test_failure() {
        let kind = classify_failure("test result: FAILED. 2 passed; 1 failed", "");
        assert_eq!(kind, FailureKind::TestFailure);
    }

    #[test]
    fn recognizes_compile_failure() {
        let kind = classify_failure("", "error: could not compile `demo`");
        assert_eq!(kind, FailureKind::CompileError);
    }

    fn spec(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    #[test]
    fn completed_command_reports_success_and_no_timeout() {
        let cwd = std::env::temp_dir();
        let result = run(
            &spec("true", &[]),
            &cwd,
            4096,
            Some(Duration::from_secs(30)),
            TestFramework::Cargo,
            None,
        )
        .expect("run should succeed");
        assert!(result.success);
        assert!(!result.timed_out);
        assert_eq!(result.failure_kind, None);
    }

    #[test]
    fn failing_command_is_classified_without_timeout() {
        let cwd = std::env::temp_dir();
        let result = run(
            &spec("false", &[]),
            &cwd,
            4096,
            Some(Duration::from_secs(30)),
            TestFramework::Cargo,
            None,
        )
        .expect("run should complete");
        assert!(!result.success);
        assert!(!result.timed_out);
        assert_eq!(result.failure_kind, Some(FailureKind::CommandFailure));
    }

    #[test]
    fn missing_program_is_an_error_not_a_timeout() {
        let cwd = std::env::temp_dir();
        let error = run(
            &spec("witdiff-definitely-not-a-real-program", &[]),
            &cwd,
            4096,
            Some(Duration::from_secs(5)),
            TestFramework::Cargo,
            None,
        )
        .expect_err("a missing program must be an explicit error");
        assert!(
            format!("{error:#}").contains("failed to execute test command"),
            "error should name the spawn failure, got: {error:#}"
        );
    }

    /// A suite that never terminates must be bounded, not allowed to hang.
    ///
    /// This is the core regression for the pipe-deadlock hazard: a naive
    /// implementation that waits for exit before reading the pipes blocks
    /// forever, because the child cannot exit while its pipe buffer is full.
    #[test]
    fn runaway_command_is_killed_at_the_deadline() {
        let cwd = std::env::temp_dir();
        let started = Instant::now();
        let result = run(
            &spec("sleep", &["60"]),
            &cwd,
            4096,
            Some(Duration::from_millis(700)),
            TestFramework::Cargo,
            None,
        )
        .expect("a timeout is reported, not raised as a tool error");

        assert!(result.timed_out, "the run must be marked as timed out");
        assert!(!result.success, "a timed-out run is never a success");
        assert_eq!(result.failure_kind, Some(FailureKind::Timeout));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the deadline must actually bound execution, took {:?}",
            started.elapsed()
        );
    }

    /// Output larger than one pipe buffer (64 KiB) must not deadlock, even when
    /// the producer is slow relative to the reader.
    #[test]
    fn large_output_does_not_deadlock_the_reader() {
        let cwd = std::env::temp_dir();
        // ~1 MiB of output, far beyond a single pipe buffer.
        let result = run(
            &spec("head", &["-c", "1048576", "/dev/zero"]),
            &cwd,
            4096,
            Some(Duration::from_secs(60)),
            TestFramework::Cargo,
            None,
        )
        .expect("large output must be drained, not deadlock");

        assert!(result.stdout.len() <= 4096 + 128);
        assert!(
            result.stdout.contains("output truncated"),
            "the retained output should be marked as truncated"
        );
    }

    #[test]
    fn no_timeout_configured_still_completes() {
        let cwd = std::env::temp_dir();
        let result = run(
            &spec("true", &[]),
            &cwd,
            4096,
            None,
            TestFramework::Cargo,
            None,
        )
        .expect("run should succeed");
        assert!(result.success);
        assert!(!result.timed_out);
    }

    // ---------------------------------------------------------------------
    // Sandboxed execution
    // ---------------------------------------------------------------------

    fn sandbox_image() -> Option<String> {
        let image =
            std::env::var("WITDIFF_TEST_IMAGE").unwrap_or_else(|_| "alpine:3.19".to_owned());
        // A missing docker or a missing image means the assertion cannot be
        // made, not that it passed.
        match Command::new("docker")
            .args(["image", "inspect", &image])
            .output()
        {
            Ok(output) if output.status.success() => Some(image),
            _ => {
                eprintln!("docker or the image {image} is unavailable; skipping");
                None
            }
        }
    }

    /// The command must become a container run. Asserted without docker,
    /// because the shape is what the configuration promises.
    #[test]
    fn a_sandbox_rewrites_the_command_into_a_container_run() {
        let sandbox = Sandbox {
            image: "alpine:3.19".to_owned(),
            env: vec!["DATABASE_URL".to_owned()],
            network: false,
        };
        let cwd = std::path::Path::new("/workspace/project");
        let spec = CommandSpec::from_vec(vec!["cargo".into(), "test".into()]).expect("spec");
        let wrapped = sandbox.wrap(&spec, cwd, "witdiff-test-1");

        assert_eq!(wrapped.program, "docker");
        assert_eq!(wrapped.args[0], "run");
        // Named, so a timeout can remove it rather than leaving it running.
        let name_at = wrapped
            .args
            .iter()
            .position(|a| a.as_str() == "--name")
            .expect("--name");
        assert_eq!(wrapped.args[name_at + 1], "witdiff-test-1");
        // Mounted at its own absolute path, so paths in output still resolve.
        let volume_at = wrapped
            .args
            .iter()
            .position(|a| a.as_str() == "--volume")
            .expect("--volume");
        assert_eq!(
            wrapped.args[volume_at + 1],
            "/workspace/project:/workspace/project"
        );
        let workdir_at = wrapped
            .args
            .iter()
            .position(|a| a.as_str() == "--workdir")
            .expect("--workdir");
        assert_eq!(wrapped.args[workdir_at + 1], "/workspace/project");
        // The allow-listed variable is forwarded alongside the marker.
        let envs = wrapped
            .args
            .iter()
            .filter(|a| a.as_str() == "--env")
            .count();
        assert_eq!(envs, 2, "WITDIFF plus DATABASE_URL");
        assert!(wrapped.args.iter().any(|a| a.as_str() == "DATABASE_URL"));
        assert!(wrapped.args.iter().any(|a| a.as_str() == "none"));
        // The original command follows the image, unmodified.
        assert_eq!(wrapped.args[wrapped.args.len() - 2], "cargo");
        assert_eq!(wrapped.args[wrapped.args.len() - 1], "test");
    }

    /// Network on by default so a project that fetches dependencies keeps
    /// working, and off only when asked.
    #[test]
    fn network_is_on_by_default_and_off_only_when_asked() {
        let cwd = std::path::Path::new("/w");
        let spec = CommandSpec::from_vec(vec!["true".into()]).expect("spec");

        let open = Sandbox {
            image: "a".into(),
            env: Vec::new(),
            network: true,
        };
        let wrapped = open.wrap(&spec, cwd, "c1");
        assert!(!wrapped.args.iter().any(|a| a.as_str() == "--network"));

        let closed = Sandbox {
            image: "a".into(),
            env: Vec::new(),
            network: false,
        };
        let wrapped = closed.wrap(&spec, cwd, "c2");
        assert!(wrapped.args.iter().any(|a| a.as_str() == "--network"));
    }

    /// An image is required. Configuring a sandbox that silently does nothing
    /// would be the worst outcome: isolation that reports success.
    #[test]
    fn no_image_means_no_sandbox() {
        let mut config = crate::config::VerificationConfig::default();
        assert!(Sandbox::from_config(&config).is_none());

        config.sandbox_image = Some(String::new());
        assert!(
            Sandbox::from_config(&config).is_none(),
            "an empty image must not be treated as a sandbox"
        );

        config.sandbox_image = Some("  ".to_owned());
        assert!(Sandbox::from_config(&config).is_none());

        config.sandbox_image = Some("alpine:3.19".to_owned());
        config.sandbox_env = vec!["A".to_owned()];
        config.sandbox_network = false;
        let sandbox = Sandbox::from_config(&config).expect("configured image");
        assert_eq!(sandbox.image, "alpine:3.19");
        assert_eq!(sandbox.env, vec!["A".to_owned()]);
        assert!(!sandbox.network);
    }

    /// Two fixtures can share a temporary directory, so a name derived from the
    /// path could collide and let one run remove the other's container.
    #[test]
    fn container_names_do_not_collide() {
        let names: Vec<String> = (0..50).map(|_| container_name()).collect();
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        for name in &names {
            assert!(name.starts_with("witdiff-"), "identifiable prefix: {name}");
        }
    }

    /// A sandboxed command really runs in the container rather than the host.
    #[test]
    #[ignore = "end-to-end: spawns docker; run with -- --ignored"]
    fn a_sandboxed_command_runs_inside_the_container() {
        let Some(image) = sandbox_image() else { return };
        let cwd = std::env::temp_dir();
        let sandbox = Sandbox {
            image,
            env: Vec::new(),
            network: true,
        };
        let result = run(
            &spec("sh", &["-c", "test -f /etc/os-release || exit 7"]),
            &cwd,
            4096,
            Some(Duration::from_secs(60)),
            TestFramework::Cargo,
            Some(&sandbox),
        )
        .expect("the docker client should run");

        assert!(
            result.success,
            "the command must run in the container, got: {}",
            result.stderr
        );
        assert_eq!(
            result.command[0], "docker",
            "the receipt must record what actually ran"
        );
    }

    /// A timed-out run must not leave a container behind. Killing `docker run`
    /// stops the client, not the container, so the code the sandbox exists to
    /// contain would otherwise keep running on the host indefinitely.
    #[test]
    #[ignore = "end-to-end: spawns docker; run with -- --ignored"]
    fn a_timed_out_sandboxed_run_leaves_no_container_behind() {
        let Some(image) = sandbox_image() else { return };
        let cwd = std::env::temp_dir();
        let sandbox = Sandbox {
            image,
            env: Vec::new(),
            network: true,
        };

        let before = container_ids();

        let result = run(
            &spec("sleep", &["60"]),
            &cwd,
            4096,
            Some(Duration::from_millis(700)),
            TestFramework::Cargo,
            Some(&sandbox),
        )
        .expect("a timeout is reported, not raised as a tool error");
        assert!(result.timed_out, "the run must be marked timed out");

        // Removal is not instantaneous; give it a moment to disappear.
        let mut leftover = container_ids();
        for _ in 0..20 {
            if leftover.difference(&before).count() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            leftover = container_ids();
        }
        let leaked: Vec<_> = leftover.difference(&before).collect();
        assert!(
            leaked.is_empty(),
            "a timed-out sandboxed run must remove its container; leaked {leaked:?}"
        );
    }

    /// `--network none` really removes egress.
    #[test]
    #[ignore = "end-to-end: spawns docker; run with -- --ignored"]
    fn a_closed_sandbox_has_no_network() {
        let Some(image) = sandbox_image() else { return };
        let cwd = std::env::temp_dir();
        let sandbox = Sandbox {
            image,
            env: Vec::new(),
            network: false,
        };

        let result = run(
            &spec(
                "sh",
                &[
                    "-c",
                    "wget -q -T 5 -O - https://example.com >/dev/null 2>&1",
                ],
            ),
            &cwd,
            4096,
            Some(Duration::from_secs(30)),
            TestFramework::Cargo,
            Some(&sandbox),
        )
        .expect("the docker client should run");

        assert!(
            !result.success,
            "network access must be refused when disabled; got exit {:?}",
            result.exit_code
        );
    }

    /// Ids of witdiff containers that currently exist, so a leak is visible.
    fn container_ids() -> std::collections::HashSet<String> {
        let Ok(output) = Command::new("docker")
            .args(["ps", "-q", "-a", "--filter", "name=witdiff-"])
            .output()
        else {
            return std::collections::HashSet::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }
}
