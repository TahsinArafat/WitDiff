use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

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
) -> Result<RunResult> {
    if !cwd.is_dir() {
        bail!("test working directory does not exist: {}", cwd.display());
    }
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
        Some(classify_failure(&stdout, &stderr))
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
    let combined = format!("{stdout}\n{stderr}").to_lowercase();
    if combined.contains("test result: failed")
        || combined.contains("failures:")
        || combined.contains("tests failed")
    {
        FailureKind::TestFailure
    } else if combined.contains("could not compile")
        || combined.contains("error[e")
        || combined.contains("error: could not compile")
    {
        FailureKind::CompileError
    } else {
        FailureKind::CommandFailure
    }
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
        let result = run(&spec("true", &[]), &cwd, 4096, None).expect("run should succeed");
        assert!(result.success);
        assert!(!result.timed_out);
    }
}
