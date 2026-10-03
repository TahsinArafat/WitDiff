//! Wire-format test that drives the real `witdiff-mcp` binary.
//!
//! This runs as a separate process on purpose. An in-process test passes a
//! `Cursor` as the server's writer, so a child process that inherits *stdout*
//! writes to the test harness's captured stdout instead — invisible to the
//! assertion. Only a real subprocess can observe the bytes that actually reach
//! the protocol stream.
//!
//! The bug this guards against was found live: `git rev-parse --verify` prints
//! the resolved commit hash, and a git invocation using `.status()` inherits
//! stdout, so a bare hash appeared between JSON-RPC frames and desynchronized
//! the client.

use std::{
    io::Write,
    process::{Command, Stdio},
};

fn binary() -> std::path::PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests of a crate
    // that defines a binary target.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_witdiff-mcp"))
}

/// Create a committed Git repository, so ref resolution actually runs.
fn repository() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "Test"],
    ] {
        assert!(
            Command::new("git")
                .args(&args)
                .current_dir(dir.path())
                .status()
                .expect("git should run")
                .success(),
            "git {args:?} failed"
        );
    }
    std::fs::write(dir.path().join("README.md"), "# t\n").expect("write");
    for args in [vec!["add", "."], vec!["commit", "-qm", "base"]] {
        assert!(
            Command::new("git")
                .args(&args)
                .current_dir(dir.path())
                .status()
                .expect("git should run")
                .success(),
            "git {args:?} failed"
        );
    }
    dir
}

/// Two requests must produce exactly two lines on stdout.
#[test]
fn the_server_emits_exactly_one_frame_per_request() {
    let repo = repository();
    let mut child = Command::new(binary())
        .arg(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        // `witdiff_inspect` resolves the base ref, which is what leaked.
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .expect("write");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"witdiff_inspect","arguments":{{"base":"HEAD"}}}}}}"#
        )
        .expect("write");
    }
    // Closing stdin ends the serve loop.
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("the server should exit");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");

    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "two requests must produce exactly two frames; stray output from an inherited \
         stdout would add more. Raw stdout was:\n{stdout}"
    );
    for line in &lines {
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|error| {
            panic!("every line must be a complete JSON frame ({error}): {line:?}")
        });
        assert_eq!(parsed["jsonrpc"], "2.0", "got {parsed}");
    }
}
