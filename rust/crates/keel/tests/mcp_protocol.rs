//! Purpose: End-to-end harness that spawns the `keel` binary in
//!   `mcp serve` mode and drives it across the JSON-RPC 2.0 wire to confirm
//!   the framing, dispatcher arms, and stdin EOF handling all line up with
//!   what the in-process unit tests verify under `src/mcp/mod.rs`.
//! Caller: `cargo test -p keel --test mcp_protocol`.
//! Dependencies: serde_json for request/response framing, Cargo's optional
//!   `CARGO_BIN_EXE_keel` path or the target/debug fallback for the binary under
//!   test, and stdlib `Command`/`BufReader` plumbing for stdio.
//! Main Functions: `mcp_serve_discovery_then_tools_list_round_trip`,
//!   `mcp_serve_tools_call_recall_status_returns_text_payload`,
//!   `mcp_serve_resources_list_includes_system_map_and_recall_status`,
//!   `mcp_serve_unknown_method_returns_method_not_found`,
//!   `mcp_serve_parse_error_returns_dash_32700`.
//! Side Effects: Spawns a child process per test, reads its stdout/stderr,
//!   isolates harness home under a per-test temp directory so the recall
//!   index never collides with a real install or with sibling tests.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const CARGO_BIN_EXE_KEEL: Option<&str> = option_env!("CARGO_BIN_EXE_keel");

fn keel_binary_path() -> PathBuf {
    if let Some(path) = CARGO_BIN_EXE_KEEL {
        return PathBuf::from(path);
    }
    let mut path = env::current_exe().expect("resolve integration test executable");
    path.pop(); // deps/
    path.pop(); // target/debug/
    path.push(if cfg!(windows) { "keel.exe" } else { "keel" });
    path
}

struct McpServerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpServerProcess {
    fn spawn(claude_home: &Path) -> Self {
        Self::spawn_with_profile(claude_home, None)
    }

    fn spawn_with_profile(claude_home: &Path, profile: Option<&str>) -> Self {
        let binary_path = keel_binary_path();
        let mut command = Command::new(binary_path);
        command.arg("mcp").arg("serve");
        command.env("CLAUDE_TARGET_OVERRIDE", claude_home);
        command.env("HOME", claude_home);
        command.env("USERPROFILE", claude_home);
        if let Some(profile) = profile {
            command.env("KEEL_MCP_CATALOG_PROFILE", profile);
        }
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

    fn send_modern(&mut self, mut request: Value) {
        if request.get("params").is_none() {
            request["params"] = json!({});
        }
        request["params"]["_meta"] = modern_meta();
        self.send(&request);
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

fn spawn_http_server(claude_home: &Path) -> (Child, SocketAddr) {
    let probe = TcpListener::bind("127.0.0.1:0").expect("reserve HTTP port");
    let address = probe.local_addr().expect("read HTTP port");
    drop(probe);

    let mut command = Command::new(keel_binary_path());
    let bind = address.to_string();
    command.args(["mcp", "serve-http", "--bind", &bind]);
    command
        .env("CLAUDE_TARGET_OVERRIDE", claude_home)
        .env("HOME", claude_home)
        .env("USERPROFILE", claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn keel HTTP MCP server");
    let stdout = BufReader::new(child.stdout.take().expect("capture HTTP server stdout"));
    let (ready_sender, ready_receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            if line.contains("listening on") {
                let _ = ready_sender.send(());
                break;
            }
        }
    });
    ready_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("HTTP MCP server readiness");
    (child, address)
}
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name":"keel-protocol-test","version":"1"}
    })
}

fn send_http_discovery(address: SocketAddr, request_id: usize) -> Result<(), String> {
    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "server/discover",
        "params": {"_meta": modern_meta()}
    }))
    .map_err(|error| format!("serialize request: {error}"))?;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: server/discover\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("connect: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("set read timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write request: {error}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("read response: {error}"))?;
    if !response.starts_with("HTTP/1.1 200")
        || !response.contains("\"supportedVersions\"")
        || response.contains("MCP-Session-Id")
    {
        return Err(format!("unexpected HTTP response: {response}"));
    }
    Ok(())
}

#[test]
fn mcp_serve_discovery_then_tools_list_round_trip() {
    for profile in ["core", "full"] {
        let home = unique_temp_directory("discovery-tools");
        let mut server = McpServerProcess::spawn_with_profile(&home, Some(profile));
        server.send_modern(json!({"jsonrpc":"2.0","id":1,"method":"server/discover"}));
        let discovery = server.recv();
        assert_eq!(
            discovery["result"]["supportedVersions"],
            json!(["2026-07-28"])
        );
        assert!(discovery["result"]["capabilities"]["tools"].is_object());
        assert!(discovery["result"].get("tools").is_none());
        let mut cursor = None;
        let mut names = std::collections::BTreeSet::new();
        for id in 2..100 {
            let mut params = json!({});
            if let Some(value) = cursor.take() {
                params["cursor"] = value;
            }
            server.send_modern(
                json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":params}),
            );
            let response = server.recv();
            let tools = response["result"]["tools"].as_array().expect("tool page");
            for tool in tools {
                assert!(
                    names.insert(tool["name"].as_str().unwrap().to_string()),
                    "duplicate tool"
                );
                assert_eq!(tool["inputSchema"]["type"], "object");
                if let Some(required) = tool["inputSchema"]["required"].as_array() {
                    for field in required {
                        assert!(tool["inputSchema"]["properties"]
                            .get(field.as_str().unwrap())
                            .is_some());
                    }
                }
            }
            cursor = response["result"].get("nextCursor").cloned();
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none(), "catalog walk did not terminate");
        for name in ["recall", "run_command", "context_brief", "anvil"] {
            assert!(names.contains(name));
        }
        if profile == "full" {
            assert!(names.contains("rewrite"));
        }
        server.close();
        let _ = std::fs::remove_dir_all(home);
    }
}

#[test]
fn mcp_serve_tools_call_recall_status_returns_text_payload() {
    let claude_home = unique_temp_directory("recall-status");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send_modern(json!({
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

    server.send_modern(json!({
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

    server.send_modern(json!({
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
fn mcp_serve_ping_returns_complete_result() {
    let claude_home = unique_temp_directory("ping");
    let mut server = McpServerProcess::spawn(&claude_home);

    server.send_modern(json!({
        "jsonrpc": "2.0",
        "id": "ping-token",
        "method": "ping"
    }));
    let response = server.recv();
    assert_eq!(response["id"], json!("ping-token"));
    assert_eq!(response["result"], json!({"resultType": "complete"}));

    server.close();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_serve_request_with_null_id_is_rejected() {
    let home = unique_temp_directory("id-null");
    let mut server = McpServerProcess::spawn(&home);
    server.send_modern(json!({"jsonrpc":"2.0","id":null,"method":"ping"}));
    let response = server.recv();
    assert_eq!(response["error"]["code"], -32600);
    assert!(response.get("result").is_none());
    server.close();
    let _ = std::fs::remove_dir_all(home);
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

#[test]
fn mcp_http_discovery_handles_parallel_clients() {
    const CLIENT_COUNT: usize = 32;
    let claude_home = unique_temp_directory("http-parallel");
    let (mut server, address) = spawn_http_server(&claude_home);
    let barrier = Arc::new(Barrier::new(CLIENT_COUNT));
    let handles: Vec<_> = (0..CLIENT_COUNT)
        .map(|request_id| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                send_http_discovery(address, request_id)
            })
        })
        .collect();

    for handle in handles {
        handle
            .join()
            .expect("parallel HTTP client thread")
            .expect("parallel HTTP discovery response");
    }

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&claude_home);
}

fn send_http_json(
    address: SocketAddr,
    body: &Value,
    extra_headers: &[(&str, &str)],
) -> Result<(u16, Value), String> {
    let body_bytes = serde_json::to_string(body).map_err(|error| format!("serialize: {error}"))?;
    let mut header_block = String::new();
    for (name, value) in extra_headers {
        header_block.push_str(&format!("{name}: {value}\r\n"));
    }
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n{header_block}\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body_bytes}",
        body_bytes.len()
    );
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("connect: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("set read timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write: {error}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("read: {error}"))?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("missing status: {response}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim())
        .filter(|body| !body.is_empty())
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|error| format!("parse body: {error}"))?
        .unwrap_or(Value::Null);
    Ok((status, body))
}

#[test]
fn mcp_http_mixed_classic_and_modern_clients_do_not_contaminate() {
    // Process-global wire era previously let Classic initialize and Modern discover
    // on one serve-http listener cross-contaminate. Mixed clients must stay isolated.
    let claude_home = unique_temp_directory("http-mixed-era");
    let (mut server, address) = spawn_http_server(&claude_home);
    let barrier = Arc::new(Barrier::new(2));

    let classic_barrier = Arc::clone(&barrier);
    let classic = thread::spawn(move || {
        classic_barrier.wait();
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "classic-peer", "version": "1"}
            }
        });
        send_http_json(address, &body, &[])
    });

    let modern_barrier = Arc::clone(&barrier);
    let modern = thread::spawn(move || {
        modern_barrier.wait();
        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "server/discover",
            "params": {"_meta": modern_meta()}
        });
        send_http_json(
            address,
            &body,
            &[
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "server/discover"),
            ],
        )
    });

    let (classic_status, classic_body) = classic.join().unwrap().expect("classic HTTP client");
    let (modern_status, modern_body) = modern.join().unwrap().expect("modern HTTP client");
    assert_eq!(classic_status, 200, "classic={classic_body}");
    assert!(
        classic_body.get("error").is_none(),
        "classic={classic_body}"
    );
    assert_eq!(classic_body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(modern_status, 200, "modern={modern_body}");
    assert!(modern_body.get("result").is_some(), "modern={modern_body}");
    assert_eq!(
        modern_body["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );

    // After both handshakes, each era still works on the same listener.
    let classic_again = send_http_json(
        address,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "classic-again", "version": "1"}
            }
        }),
        &[],
    )
    .expect("classic follow-up");
    assert_eq!(
        classic_again.0, 200,
        "classic follow-up={:?}",
        classic_again.1
    );
    assert!(classic_again.1.get("error").is_none());

    let modern_again = send_http_json(
        address,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "server/discover",
            "params": {"_meta": modern_meta()}
        }),
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
    )
    .expect("modern follow-up");
    assert_eq!(modern_again.0, 200, "modern follow-up={:?}", modern_again.1);
    assert!(modern_again.1.get("result").is_some());

    // Modern still rejects bare params on this listener (no classic soft-default bleed).
    let modern_bare = send_http_json(
        address,
        &json!({"jsonrpc":"2.0","id":5,"method":"server/discover","params":{}}),
        &[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
    )
    .expect("modern bare discover");
    assert_eq!(modern_bare.1["error"]["code"], -32602);

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_http_classic_2025_initialize_succeeds() {
    let claude_home = unique_temp_directory("http-classic-init");
    let (mut server, address) = spawn_http_server(&claude_home);

    let body = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "http-classic", "version": "1"}
        }
    }))
    .unwrap();

    // Classic initialize does not require modern routing headers.
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    assert!(response.starts_with("HTTP/1.1 200"), "response={response}");
    let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(body.get("error").is_none(), "body={body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    assert!(body["result"]["capabilities"]["tools"].is_object());
    assert_eq!(body["result"]["serverInfo"]["name"], "keel");

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_http_legacy_client_handshake_and_tools_list_succeeds() {
    let claude_home = unique_temp_directory("http-legacy-client");
    let (mut server, address) = spawn_http_server(&claude_home);

    let send_request = |body_val: &Value, session_id: Option<&str>| -> (u16, Value) {
        let body = serde_json::to_string(body_val).unwrap();
        let session_header = session_id
            .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             {session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        let json_part = response
            .split_once("\r\n\r\n")
            .map(|(_, b)| b)
            .unwrap_or("");
        let val: Value = serde_json::from_str(json_part).unwrap_or(Value::Null);
        (status, val)
    };

    let (status, init) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "legacy",
                "capabilities": {},
                "clientInfo": {"name": "legacy-mcp-client", "version": "1"}
            }
        }),
        Some("session-123"),
    );
    assert_eq!(status, 200, "init failed: {init}");
    assert_eq!(init["result"]["protocolVersion"], "legacy");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let (status, _) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
        Some("session-123"),
    );
    assert_eq!(status, 202);

    let (status, listed) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some("session-123"),
    );
    assert_eq!(status, 200, "tools/list failed: {listed}");
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "tools list must not be empty");

    let (status, called) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "recall_status",
                "arguments": {}
            }
        }),
        Some("session-123"),
    );
    assert_eq!(status, 200, "tools/call failed: {called}");
    assert!(called["result"].get("content").is_some());

    let (status, ping) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "ping"
        }),
        Some("session-123"),
    );
    assert_eq!(status, 200, "ping failed: {ping}");

    let (status, prompts) = send_request(
        &json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "prompts/list"
        }),
        Some("session-123"),
    );
    assert_eq!(status, 200, "prompts/list failed: {prompts}");
    assert_eq!(prompts["result"]["prompts"], json!([]));

    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn mcp_stdio_classic_initialize_handshake_succeeds() {
    let home = unique_temp_directory("classic-stdio");
    let mut server = McpServerProcess::spawn(&home);
    server.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"legacy","version":"1"}}}));
    let init = server.recv();
    assert!(init.get("error").is_none(), "classic initialize: {init}");
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert_eq!(init["result"]["serverInfo"]["name"], "keel");
    server.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    // Classic path: tools/list and ping without `_meta` after initialize.
    server.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let listed = server.recv();
    assert!(
        listed.get("result").is_some(),
        "classic tools/list: {listed}"
    );
    assert!(listed["result"]["tools"].is_array());
    server.send(&json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    assert_eq!(server.recv()["result"]["resultType"], "complete");
    server.close();
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn mcp_stdio_discover_then_initialize_fallback() {
    let home = unique_temp_directory("discover-fallback");
    let mut server = McpServerProcess::spawn(&home);
    // Antigravity-like: try modern discover without `_meta`, then fall back to initialize.
    server.send(&json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}));
    let discover = server.recv();
    assert_eq!(discover["error"]["code"], -32602);
    server.send(&json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"antigravity","version":"1"}}}));
    let init = server.recv();
    assert!(init.get("result").is_some(), "fallback initialize: {init}");
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    server.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    server.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"recall_status","arguments":{}}}));
    let call = server.recv();
    assert!(
        call.get("result").is_some(),
        "tools/call after fallback: {call}"
    );
    server.close();
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn mcp_stdio_modern_minimal_meta_tools_after_discover() {
    let home = unique_temp_directory("modern-minimal-meta");
    let mut server = McpServerProcess::spawn(&home);
    let minimal = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    server.send(
        &json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":minimal}}),
    );
    let discovery = server.recv();
    assert_eq!(
        discovery["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    server.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":minimal}}));
    let listed = server.recv();
    assert!(
        listed.get("result").is_some(),
        "minimal-meta tools/list: {listed}"
    );
    server.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"recall_status","arguments":{},"_meta":minimal}}));
    let call = server.recv();
    assert!(
        call.get("result").is_some(),
        "minimal-meta tools/call: {call}"
    );
    server.close();
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn mcp_stdio_unsupported_initialize_lists_both_eras() {
    let home = unique_temp_directory("unsupported-init");
    let mut server = McpServerProcess::spawn(&home);
    server.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}));
    let rejected = server.recv();
    assert_eq!(rejected["error"]["code"], -32022);
    assert_eq!(rejected["error"]["data"]["requested"], "1999-01-01");
    let supported = rejected["error"]["data"]["supported"]
        .as_array()
        .expect("supported list");
    for version in ["2024-11-05", "2025-03-26", "2025-11-25", "2026-07-28"] {
        assert!(
            supported
                .iter()
                .any(|entry| entry.as_str() == Some(version)),
            "missing {version} in {supported:?}"
        );
    }
    server.close();
    let _ = std::fs::remove_dir_all(home);
}
