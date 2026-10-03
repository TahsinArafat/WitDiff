//! Protocol-level tests for the MCP adapter.
//!
//! These drive [`Server::serve`] over in-memory buffers rather than spawning a
//! process, so framing, routing and result shapes are exercised directly. The
//! tool *bodies* are thin delegations to `witdiff-core`, which has its own
//! end-to-end coverage, so what matters here is the protocol contract:
//! what is answered, with which shape, and — for failures — at which level.

use std::io::Cursor;

use serde_json::{json, Value};
use witdiff_mcp::{Server, PROTOCOL_VERSION};

/// Send frames, return the parsed replies in order.
fn exchange(server: &Server, frames: &[Value]) -> Vec<Value> {
    let mut input = String::new();
    for frame in frames {
        input.push_str(&serde_json::to_string(frame).expect("frame should serialize"));
        input.push('\n');
    }
    let mut output = Vec::new();
    server
        .serve(Cursor::new(input), &mut output)
        .expect("serve should not fail");
    String::from_utf8(output)
        .expect("output should be utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each reply must be one JSON frame"))
        .collect()
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn server() -> Server {
    // A directory that is unlikely to be a Git repository keeps the tests
    // hermetic: no tool call here depends on a real repository succeeding.
    Server::new(std::env::temp_dir())
}

#[test]
fn initialize_negotiates_the_protocol_version_and_declares_tools() {
    let replies = exchange(
        &server(),
        &[request(
            1,
            "initialize",
            json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }),
        )],
    );
    assert_eq!(replies.len(), 1);
    let result = &replies[0]["result"];
    assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(result["serverInfo"]["name"], "witdiff");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "the server must declare the tools capability, got {}",
        result["capabilities"]
    );
}

/// A notification must be answered with silence. Replying to one is itself a
/// protocol violation, and a stray frame would desynchronize the client.
#[test]
fn notifications_receive_no_reply() {
    let mut input = String::new();
    input.push_str(
        &serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap(),
    );
    input.push('\n');
    input.push_str(&serde_json::to_string(&request(2, "ping", json!({}))).unwrap());
    input.push('\n');

    let mut output = Vec::new();
    let handled = server()
        .serve(Cursor::new(input), &mut output)
        .expect("serve should not fail");

    let text = String::from_utf8(output).expect("utf-8");
    assert_eq!(
        handled, 1,
        "only the request should produce a reply, got: {text:?}"
    );
    assert_eq!(text.lines().count(), 1, "exactly one frame: {text:?}");
    let reply: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(reply["id"], 2);
}

#[test]
fn tools_list_exposes_exactly_the_permitted_tools() {
    let replies = exchange(&server(), &[request(1, "tools/list", json!({}))]);
    let tools = replies[0]["result"]["tools"]
        .as_array()
        .expect("tools must be an array");

    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        vec!["witdiff_inspect", "witdiff_verify", "witdiff_receipt"],
        "PG-501 permits exactly inspect, verify and receipt"
    );

    for tool in tools {
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{} needs a description for the model to choose it",
            tool["name"]
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "{} needs an object input schema",
            tool["name"]
        );
    }
}

/// The requirement that a tool failure is a *result* with `isError`, not a
/// JSON-RPC error: a protocol-level error is invisible to the model, which then
/// cannot self-correct.
#[test]
fn a_failing_tool_call_is_a_result_not_a_protocol_error() {
    let replies = exchange(
        &server(),
        &[request(
            1,
            "tools/call",
            json!({ "name": "witdiff_inspect", "arguments": { "repo": "/definitely/not/a/repo" } }),
        )],
    );
    let reply = &replies[0];
    assert!(
        reply.get("error").is_none(),
        "a tool failure must not be a protocol error, got {reply}"
    );
    assert_eq!(reply["result"]["isError"], true);
    assert!(
        reply["result"]["content"].is_array(),
        "content is required by the schema even on failure"
    );
}

/// An unknown *tool* is an error in finding the tool, which the specification
/// places at the protocol level.
#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let replies = exchange(
        &server(),
        &[request(1, "tools/call", json!({ "name": "no_such_tool" }))],
    );
    assert_eq!(replies[0]["error"]["code"], -32601);
}

#[test]
fn an_unimplemented_method_is_reported_rather_than_ignored() {
    let replies = exchange(&server(), &[request(1, "resources/list", json!({}))]);
    assert_eq!(replies[0]["error"]["code"], -32601);
    let message = replies[0]["error"]["message"]
        .as_str()
        .expect("an error needs a message");
    assert!(
        message.contains("resources/list"),
        "the error should name the method: {message}"
    );
}

#[test]
fn malformed_input_produces_a_parse_error_with_a_null_id() {
    let mut output = Vec::new();
    server()
        .serve(Cursor::new(b"this is not json\n".to_vec()), &mut output)
        .expect("serve should not fail on bad input");
    let reply: Value = serde_json::from_str(String::from_utf8(output).unwrap().trim()).unwrap();
    assert_eq!(reply["error"]["code"], -32600);
    assert_eq!(reply["id"], Value::Null);
}

/// Blank lines are not frames. Treating them as input would produce a parse
/// error for a harmless trailing newline.
#[test]
fn blank_lines_are_ignored() {
    let mut output = Vec::new();
    let handled = server()
        .serve(Cursor::new(b"\n\n\n".to_vec()), &mut output)
        .expect("serve should not fail");
    assert_eq!(handled, 0);
    assert!(output.is_empty(), "no frame should be produced");
}

/// Framing: every reply must be exactly one line, or the client's reader
/// desynchronizes. `serde_json` escapes control characters, so a message
/// containing a newline must not become two frames.
#[test]
fn each_reply_is_exactly_one_line() {
    let replies = exchange(
        &server(),
        &[
            request(1, "initialize", json!({})),
            request(2, "tools/list", json!({})),
            request(3, "ping", json!({})),
        ],
    );
    assert_eq!(replies.len(), 3);
    for (index, reply) in replies.iter().enumerate() {
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(
            reply["id"],
            json!(index as i64 + 1),
            "ids must be echoed in order"
        );
    }
}

#[test]
fn ping_is_answered() {
    let replies = exchange(&server(), &[request(7, "ping", json!({}))]);
    assert_eq!(replies[0]["id"], 7);
    assert!(replies[0]["result"].is_object());
}

/// The tool definitions and the dispatcher must agree: a tool that is listed
/// but not callable, or callable but not listed, is a contract violation.
#[test]
fn every_listed_tool_dispatches_to_a_handler() {
    let definitions = witdiff_mcp::tool_definitions();
    for tool in definitions.as_array().expect("array") {
        let name = tool["name"].as_str().expect("name");
        let replies = exchange(
            &server(),
            &[request(
                1,
                "tools/call",
                json!({ "name": name, "arguments": {} }),
            )],
        );
        assert!(
            replies[0].get("error").is_none(),
            "{name} is listed but not dispatched: {}",
            replies[0]
        );
    }
}
