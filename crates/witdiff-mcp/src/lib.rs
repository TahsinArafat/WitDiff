//! Model Context Protocol adapter for the WitDiff verification engine.
//!
//! See ADR-0014. This is a protocol adapter and nothing more: every tool call
//! delegates to [`witdiff_core`], and the receipt is returned unchanged rather
//! than re-rendered, so an MCP consumer sees the same `witdiff.receipt.v1`
//! document the CLI writes.
//!
//! ## Scope
//!
//! Implemented: the stdio transport, `initialize`, `tools/list`, `tools/call`
//! and `ping`. Everything else is answered with a JSON-RPC `Method not found`,
//! so a client is told plainly rather than left waiting.
//!
//! ## Two details that are load-bearing
//!
//! 1. **A tool failure is a result, not a protocol error.** The specification
//!    requires errors originating from the tool to be reported inside the result
//!    with `isError: true`, because a protocol-level error is invisible to the
//!    model, which then cannot self-correct. A verification that exits 2 is
//!    therefore a *successful* tool call reporting a failed gate.
//! 2. **stdout carries protocol frames and nothing else.** A stray `println!`
//!    anywhere in the call path would corrupt the stream. Response bytes go
//!    through the writer passed to [`Server::serve`]; diagnostics go to stderr.

use std::io::{BufRead, Write};

use serde_json::{json, Map, Value};

use witdiff_core::{
    config::Config,
    model::InspectReport,
    verify::{inspect_repository, verify_repository, VerifyOptions},
    GitRepo, Receipt,
};

/// The protocol revision this adapter implements.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error code for a method the server does not implement.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error code for malformed input.
const INVALID_REQUEST: i64 = -32600;

/// A JSON-RPC response or `None` for a notification, which by definition has no
/// reply.
pub type Reply = Option<Value>;

/// The three tools this server exposes, as required by PG-501.
///
/// Deliberately no more: each additional tool is another way for an agent to
/// reach a verification conclusion, and the contract is that they all agree.
pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "witdiff_inspect",
            "description": "Show how WitDiff classifies the changed files against a base revision, without running any tests. Use this to diagnose a surprising verify result or to see which files count as tests.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {
                        "type": "string",
                        "description": "Path to the repository. Defaults to the server's working directory."
                    },
                    "base": {
                        "type": "string",
                        "description": "Git revision to compare against. Defaults to the configured base."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "witdiff_verify",
            "description": "Prove that the changed tests actually detect the change: they must pass on the workspace and fail on the base revision. Returns the full receipt including status. A non-verified status is a result, not a tool error.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {
                        "type": "string",
                        "description": "Path to the repository. Defaults to the server's working directory."
                    },
                    "base": {
                        "type": "string",
                        "description": "Git revision to compare against. Defaults to the configured base."
                    },
                    "command": {
                        "type": "string",
                        "description": "Override the configured test command, e.g. \"cargo test --all-targets\"."
                    },
                    "strict": {
                        "type": "boolean",
                        "description": "Require exactly `verified`, not `verified_with_warnings`."
                    },
                    "fail_on_no_changed_tests": {
                        "type": "boolean",
                        "description": "Treat a change with no tests as a gate failure."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "witdiff_receipt",
            "description": "Read the most recent receipt from disk without running anything. Returns null when no receipt exists yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": {
                        "type": "string",
                        "description": "Path to the repository. Defaults to the server's working directory."
                    }
                },
                "additionalProperties": false
            }
        }
    ])
}

/// The server's tool dispatcher.
pub struct Server {
    /// Directory used when a call does not name a repository.
    default_repo: std::path::PathBuf,
}

impl Server {
    pub fn new(default_repo: impl Into<std::path::PathBuf>) -> Self {
        Self {
            default_repo: default_repo.into(),
        }
    }

    /// Serve newline-delimited JSON-RPC until the input ends.
    ///
    /// Returns the number of frames handled, which is what the transport tests
    /// assert on.
    pub fn serve<R: BufRead, W: Write>(&self, input: R, mut output: W) -> anyhow::Result<usize> {
        let mut handled = 0usize;
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Some(reply) = self.handle_line(&line) else {
                // A notification: correctly answered with silence.
                continue;
            };
            // One frame per line. `serde_json::to_string` never emits a raw
            // newline because it escapes control characters inside strings.
            writeln!(output, "{}", serde_json::to_string(&reply)?)?;
            output.flush()?;
            handled += 1;
        }
        Ok(handled)
    }

    /// Handle one frame. Returns `None` for a notification.
    pub fn handle_line(&self, line: &str) -> Reply {
        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                // Without a parseable id there is nothing to correlate a reply
                // with, so the id is null as the specification requires.
                return Some(error_response(
                    Value::Null,
                    INVALID_REQUEST,
                    &format!("could not parse JSON-RPC frame: {error}"),
                ));
            }
        };

        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        // A notification is a request without an id. `notifications/initialized`
        // is the only one this server expects, and it requires no reply.
        let is_notification = id.is_none();
        if is_notification {
            if method == "notifications/initialized" {
                return None;
            }
            // An unknown notification is ignored per JSON-RPC, since replying to
            // a notification would itself be a protocol violation.
            return None;
        }

        let id = id.unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        match method.as_str() {
            "initialize" => Some(success(id, initialize_result())),
            "ping" => Some(success(id, json!({}))),
            "tools/list" => Some(success(id, json!({ "tools": tool_definitions() }))),
            "tools/call" => Some(self.call_tool(id, params)),
            other => Some(error_response(
                id,
                METHOD_NOT_FOUND,
                &format!("method `{other}` is not implemented by witdiff-mcp"),
            )),
        }
    }

    /// Dispatch `tools/call`.
    ///
    /// Every failure path returns a *result* with `isError`, never a JSON-RPC
    /// error, because the model must be able to see that the call failed.
    fn call_tool(&self, id: Value, params: Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let outcome = match name {
            "witdiff_inspect" => self.inspect(&arguments),
            "witdiff_verify" => self.verify(&arguments),
            "witdiff_receipt" => self.receipt(&arguments),
            other => {
                // An unknown tool is an error in *finding* the tool, which the
                // specification says belongs at the protocol level.
                return error_response(id, METHOD_NOT_FOUND, &format!("unknown tool `{other}`"));
            }
        };

        match outcome {
            Ok(content) => success(id, tool_result(content, false)),
            Err(error) => success(
                id,
                tool_result(
                    json!({
                        "error": format!("{error:#}"),
                        "hint": "WitDiff could not complete this call. Check the repository path and that it is a Git repository.",
                    }),
                    true,
                ),
            ),
        }
    }

    fn inspect(&self, arguments: &Value) -> anyhow::Result<Value> {
        let repo = self.open_repo(arguments)?;
        let config = Config::load(repo.root())?;
        let base = string_arg(arguments, "base");
        let report: InspectReport = inspect_repository(&repo, &config, base.as_deref())?;
        Ok(serde_json::to_value(report)?)
    }

    fn verify(&self, arguments: &Value) -> anyhow::Result<Value> {
        let repo = self.open_repo(arguments)?;
        let config = Config::load(repo.root())?;
        let command = match string_arg(arguments, "command") {
            Some(text) => Some(witdiff_core::CommandSpec::from_vec(
                shell_words::split(&text).map_err(|error| {
                    anyhow::anyhow!("could not parse the command argument: {error}")
                })?,
            )?),
            None => None,
        };
        let strict = bool_arg(arguments, "strict");
        let fail_on_no_changed_tests = bool_arg(arguments, "fail_on_no_changed_tests");

        let receipt: Receipt = verify_repository(
            &repo,
            &config,
            VerifyOptions {
                requested_base: string_arg(arguments, "base"),
                command,
                keep_worktree: false,
            },
        )?;

        // The gate verdict comes from the same function the CLI and CI use, so
        // MCP cannot disagree with them (ADR-0013). The committed policy is a
        // floor: flags passed here may tighten it and can never relax it, or a
        // client could sidestep what the repository enforces.
        let policy = config
            .gate
            .clone()
            .tightened_by(strict, fail_on_no_changed_tests);
        let gate = receipt.gate(&policy);
        Ok(json!({
            "gate": gate.as_str(),
            "passed": gate.passes(),
            "status": receipt.status.as_str(),
            // The machine-readable why, so a client branches on a token rather
            // than parsing the prose in `notes` to work out which of several
            // unrelated causes applied.
            "reason": receipt.reason.as_str(),
            "reason_is_actionable_by_author": receipt.reason.is_actionable_by_author(),
            "remediation": receipt.status.remediation(),
            "gate_reason": receipt.gate_reason(&policy),
            "receipt": receipt,
        }))
    }

    fn receipt(&self, arguments: &Value) -> anyhow::Result<Value> {
        let repo = self.open_repo(arguments)?;
        let path = repo.root().join(".witdiff").join("receipt.json");
        if !path.is_file() {
            // Absence is a fact, not an error: no verification has been run.
            return Ok(json!({ "receipt": Value::Null, "path": path.display().to_string() }));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| anyhow::anyhow!("could not read {}: {error}", path.display()))?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|error| anyhow::anyhow!("the stored receipt is not valid JSON: {error}"))?;
        Ok(json!({ "receipt": parsed, "path": path.display().to_string() }))
    }

    fn open_repo(&self, arguments: &Value) -> anyhow::Result<GitRepo> {
        let path = string_arg(arguments, "repo")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| self.default_repo.clone());
        GitRepo::discover(&path)
    }
}

/// Build a `CallToolResult`.
///
/// `content` is always present, because the schema requires it; the structured
/// payload rides alongside it so a client can read either.
fn tool_result(payload: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": is_error,
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "witdiff",
            "title": "WitDiff deterministic verification",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn success(id: Value, result: Value) -> Value {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), json!("2.0"));
    object.insert("id".into(), id);
    object.insert("result".into(), result);
    Value::Object(object)
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn string_arg(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn bool_arg(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}
