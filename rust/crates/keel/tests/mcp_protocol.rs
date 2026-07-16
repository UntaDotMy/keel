//! Purpose: End-to-end harness that spawns the `keel` binary in
//!   `mcp serve` mode and drives it across the JSON-RPC 2.0 wire to confirm
//!   the framing, dispatcher arms, and stdin EOF handling all line up with
//!   what the in-process unit tests verify under `src/mcp/mod.rs`.
//! Caller: `cargo test -p keel --test mcp_protocol`.
//! Dependencies: serde_json for request/response framing, the cargo-injected
//!   `CARGO_BIN_EXE_keel` env var for the binary under test, the
//!   stdlib `Command`/`BufReader` plumbing for stdio.
//! Main Functions: `mcp_serve_initialize_then_tools_list_round_trip`,
//!   `mcp_serve_tools_call_recall_status_returns_text_payload`,
//!   `mcp_serve_resources_list_includes_system_map_and_recall_status`,
//!   `mcp_serve_unknown_method_returns_method_not_found`,
//!   `mcp_serve_parse_error_returns_dash_32700`.
//! Side Effects: Spawns a child process per test, reads its stdout/stderr,
//!   isolates harness home under a per-test temp directory so the recall
//!   index never collides with a real install or with sibling tests.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const BINARY_ENV_VAR: &str = "CARGO_BIN_EXE_keel";

struct McpServerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpServerProcess {
    fn spawn(claude_home: &Path) -> Self {
        let binary_path = env::var(BINARY_ENV_VAR)
            .unwrap_or_else(|_| panic!("{BINARY_ENV_VAR} env var should be set by cargo test"));
        let mut command = Command::new(binary_path);
        command.arg("mcp").arg("serve");
        command.env("CLAUDE_TARGET_OVERRIDE", claude_home);
        command.env("HOME", claude_home);
        command.env("USERPROFILE", claude_home);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn keel mcp serve");
        let stdin = child.stdin.take().expect("capture child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("capture child stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, request: &Value) {
        let mut serialized = serde_json::to_string(request).expect("serialize request");
        serialized.push('\n');
        self.stdin
            .write_all(serialized.as_bytes())
            .expect("write request to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .expect("read response line from child stdout");
        assert!(bytes > 0, "child closed stdout before responding");
        serde_json::from_str(line.trim()).expect("parse response JSON")
    }

    fn close(mut self) {
        // Dropping stdin closes the pipe, which is the EOF signal the server
        // loops on. We then wait so the test fails loudly if the binary hangs
        // or exits non-zero on a clean shutdown.
        drop(self.stdin);
        let status = self.child.wait().expect("wait for child to exit");
        assert!(
            status.success(),
            "keel mcp serve exited with status {status:?}"
        );
    }
}

fn unique_temp_directory(label: &str) -> PathBuf {
    let unique_suffix: u128 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let candidate = env::temp_dir().join(format!("keel-mcp-{label}-{unique_suffix}"));
    std::fs::create_dir_all(&candidate).expect("create temp claude home");
    candidate
}

#[test]
fn mcp_serve_initialize_then_tools_list_round_trip() {
    let claude_home = unique_temp_directory("init-tools");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }));
    let initialize_response = server.recv();
    assert_eq!(initialize_response["jsonrpc"], "2.0");
    assert_eq!(initialize_response["id"], json!(1));
    // Empty params → server falls back to its latest supported revision. (When
    // the client requests a protocolVersion the server echoes it instead; that
    // negotiation path is unit-tested in mcp/mod.rs.)
    assert_eq!(
        initialize_response["result"]["protocolVersion"],
        json!("2025-11-25")
    );
    assert_eq!(
        initialize_response["result"]["serverInfo"]["name"],
        json!("keel")
    );

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }));
    let tools_response = server.recv();
    let tools = tools_response["result"]["tools"]
        .as_array()
        .expect("tools array");
    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(tool_names.contains(&"recall".to_string()));
    assert!(tool_names.contains(&"system_map".to_string()));
    assert!(tool_names.contains(&"run_command".to_string()));
    assert!(tool_names.contains(&"recall_status".to_string()));
    assert!(tool_names.contains(&"observe".to_string()));
    assert!(tool_names.contains(&"rewrite".to_string()));
    assert!(tool_names.contains(&"skill_eval".to_string()));
    assert!(tool_names.contains(&"dispatch".to_string()));
    assert!(tool_names.contains(&"design_intelligence".to_string()));
    assert!(
        tools.len() >= 40,
        "expected full MCP catalog (>=40 tools), got {}: {tool_names:?}",
        tools.len()
    );
    for tool in tools {
        assert_eq!(
            tool["inputSchema"]["type"],
            json!("object"),
            "inputSchema.type must be object for {:?}",
            tool.get("name")
        );
    }

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_tools_call_recall_status_returns_text_payload() {
    let claude_home = unique_temp_directory("recall-status");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "recall_status",
            "arguments": {}
        }
    }));
    let response = server.recv();
    let content = response["result"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 1, "response: {response}");
    assert_eq!(content[0]["type"], json!("text"));
    let text = content[0]["text"].as_str().expect("text field");
    let payload: Value = serde_json::from_str(text).expect("parse recall_status payload");
    assert!(payload["schemaVersion"].is_string());
    assert!(payload["documents"].is_number());
    assert!(payload["claudeHome"].is_string());

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_resources_list_includes_system_map_and_recall_status() {
    let claude_home = unique_temp_directory("resources-list");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list"
    }));
    let response = server.recv();
    let uris: Vec<String> = response["result"]["resources"]
        .as_array()
        .expect("resources array")
        .iter()
        .filter_map(|entry| entry.get("uri").and_then(Value::as_str).map(str::to_string))
        .collect();
    assert!(uris.contains(&"keel://system-map".to_string()));
    assert!(uris.contains(&"keel://recall/status".to_string()));

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_unknown_method_returns_method_not_found() {
    let claude_home = unique_temp_directory("unknown-method");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 17,
        "method": "tools/teleport"
    }));
    let response = server.recv();
    assert_eq!(response["id"], json!(17));
    assert_eq!(response["error"]["code"], json!(-32601));

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_parse_error_returns_dash_32700() {
    let claude_home = unique_temp_directory("parse-error");
    let mut server = McpServerProcess::spawn(&claude_home);

    server
        .stdin
        .write_all(b"not-valid-json\n")
        .expect("write malformed line");
    server.stdin.flush().expect("flush malformed line");
    let response = server.recv();
    assert_eq!(response["error"]["code"], json!(-32700));

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_ping_returns_empty_object() {
    let claude_home = unique_temp_directory("ping");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": "ping-token",
        "method": "ping"
    }));
    let response = server.recv();
    assert_eq!(response["id"], json!("ping-token"));
    assert_eq!(response["result"], json!({}));

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_request_with_null_id_receives_response() {
    // Per JSON-RPC 2.0 section 4.1, a request with id:null is a valid request,
    // not a notification. It must receive a response.
    let claude_home = unique_temp_directory("id-null");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "method": "ping"
    }));
    let response = server.recv();
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(
        response["id"],
        json!(Value::Null),
        "id must be null as sent"
    );
    assert_eq!(
        response["result"],
        json!({}),
        "ping must return empty object"
    );

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_tools_call_with_array_params_returns_invalid_params() {
    // The MCP `tools/call` method requires a structured params object carrying
    // `name` and `arguments`. JSON-RPC permits array-form params in general, but
    // tools/call has no positional contract — an array cannot name a tool — so
    // rejecting it with -32602 (Invalid params) is correct, not a gap. This test
    // pins that contract so a future "accept arrays" change is a conscious one.
    let claude_home = unique_temp_directory("array-params");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "tools/call",
        "params": ["recall_status", {}]
    }));
    let response = server.recv();
    assert_eq!(response["id"], json!(21));
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "array params for tools/call must be Invalid params: {response}"
    );

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_tools_call_with_omitted_params_returns_invalid_params() {
    // Omitted params default to null; tools/call still needs a name, so this is
    // Invalid params rather than a panic or a silent default tool.
    let claude_home = unique_temp_directory("omitted-params");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "tools/call"
    }));
    let response = server.recv();
    assert_eq!(response["id"], json!(22));
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "missing params for tools/call must be Invalid params: {response}"
    );

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}
