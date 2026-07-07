//! Purpose: Model Context Protocol (MCP) server surface for keel.
//! Caller: `commands.rs` `mcp` arm — `keel mcp serve` reads JSON-RPC
//!   2.0 messages from stdin, writes responses to stdout, logs to stderr, and
//!   exits cleanly when stdin closes. Test harnesses also drive `dispatch`
//!   directly without spawning the binary.
//! Dependencies: serde_json for newline-delimited JSON-RPC framing,
//!   `utility::recall` for the FTS5-backed search/status surface,
//!   `utility::system_map` for the workspace map, `proxy::run` for the
//!   capture+compaction pipeline used by the `run_command` tool, and
//!   `runtime` for the harness home resolver.
//! Main Functions: `run_mcp_command`, `serve_stdio`, `dispatch`.
//! Side Effects: Reads stdin, writes JSON-RPC responses to stdout, writes
//!   diagnostics to stderr, opens (and on first call creates) the recall
//!   SQLite index under `<claude-home>/recall-index.sqlite3`, and may execute
//!   user-supplied shell commands via the proxy capture path when the
//!   `run_command` tool is invoked.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::memory::system_map_reference_directory;
use crate::utility::recall::recall_status_snapshot;
use crate::utility::system_map::render_system_map;

mod tools;

/// Maximum bytes accepted for a single newline-delimited JSON-RPC frame. A peer
/// that streams data without a terminating newline would otherwise grow the
/// read buffer without bound and exhaust memory; capping the per-frame read
/// bounds that to a generous-but-finite size (8 MiB — far above any real tool
/// call, well below a DoS). An over-cap frame is refused, not truncated.
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;

/// Default wire-protocol version advertised during `initialize` when the client
/// does not request one. Per the MCP lifecycle spec the server SHOULD respond
/// with the client's requested `protocolVersion` when it can support it, and
/// only fall back to its own latest supported version otherwise. We echo the
/// client's value in [`handle_initialize`] and use this constant as the
/// fallback, so the server stays compatible as the spec revises without needing
/// a constant bump each time. Current spec revision: 2025-11-25
/// (see code.claude.com/docs/en/mcp and modelcontextprotocol.io/specification).
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Server identity returned in the `initialize` response. The version mirrors
/// the workspace package version so plugin manifests and the server agree on
/// what the host is talking to.
const MCP_SERVER_NAME: &str = "keel";
const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON-RPC error codes the dispatcher returns. The numeric values are
/// stable per JSON-RPC 2.0 §5.1; using named constants here keeps the
/// `tools/call` and `resources/read` arms readable.
const JSON_RPC_PARSE_ERROR: i64 = -32700;
const JSON_RPC_INVALID_REQUEST: i64 = -32600;
const JSON_RPC_METHOD_NOT_FOUND: i64 = -32601;
pub(super) const JSON_RPC_INVALID_PARAMS: i64 = -32602;
const JSON_RPC_INTERNAL_ERROR: i64 = -32603;

/// Resource URIs this server publishes. Keep these as constants so the list
/// surface and the read surface stay in lockstep.
const SYSTEM_MAP_RESOURCE_URI: &str = "keel://system-map";
const RECALL_STATUS_RESOURCE_URI: &str = "keel://recall/status";

/// Entry point for `keel mcp <subcommand>`. The only subcommand
/// today is `serve`, which switches the binary into a JSON-RPC stdio loop.
pub fn run_mcp_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let subcommand = arguments.first().map(String::as_str).unwrap_or("");
    match subcommand {
        "serve" => serve_stdio(
            &mut std::io::stdin().lock(),
            standard_output,
            standard_error,
        ),
        "" | "help" | "--help" | "-h" => {
            render_mcp_help(standard_output);
            0
        }
        other => {
            let _ = writeln!(standard_error, "Unknown mcp subcommand: {other}");
            render_mcp_help(standard_error);
            1
        }
    }
}

fn render_mcp_help(standard_output: &mut dyn Write) {
    let _ = writeln!(standard_output, "Usage: keel mcp serve");
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Runs the keel Model Context Protocol server over stdio."
    );
    let _ = writeln!(
        standard_output,
        "Reads newline-delimited JSON-RPC 2.0 messages from stdin, writes responses to stdout."
    );
    let _ = writeln!(
        standard_output,
        "Tools: recall, system_map, run_command, recall_status, skill_route, skill_get, skill_list, memory_status, brief_list, brief_get, brief_create, system_map_refresh."
    );
    let _ = writeln!(
        standard_output,
        "Resources: {SYSTEM_MAP_RESOURCE_URI}, {RECALL_STATUS_RESOURCE_URI}."
    );
}

/// Read newline-delimited JSON-RPC messages from `input` and write framed
/// responses to `standard_output`. Notifications produce no response. Returns
/// 0 on clean stdin EOF, non-zero only when an unrecoverable I/O error
/// prevents reading or writing the stream.
pub fn serve_stdio(
    input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut reader = BufReader::new(input);
    let mut raw_line: Vec<u8> = Vec::new();
    loop {
        raw_line.clear();
        // Cap a single frame so a peer that never sends a newline cannot grow
        // the buffer without bound and OOM the subprocess. `take` limits this
        // read to MAX_FRAME_BYTES; if the cap is hit with no terminating
        // newline the frame is oversized — refuse it and drop the connection
        // rather than parse a truncated payload.
        let read_result = reader
            .by_ref()
            .take(MAX_FRAME_BYTES)
            .read_until(b'\n', &mut raw_line);
        match read_result {
            Ok(0) => return 0,
            Ok(_) => {}
            Err(error) => {
                let _ = writeln!(standard_error, "[keel mcp] read stdin: {error}");
                return 1;
            }
        }
        if raw_line.len() as u64 >= MAX_FRAME_BYTES && raw_line.last() != Some(&b'\n') {
            let oversized = error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: frame exceeds maximum size",
            );
            let _ = write_framed_response(standard_output, standard_error, &oversized);
            return 1;
        }
        let line = String::from_utf8_lossy(&raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Result<Value, serde_json::Error> = serde_json::from_str(trimmed);
        let response = match parsed {
            Ok(request) => dispatch(&request),
            Err(parse_error) => Some(error_response(
                Value::Null,
                JSON_RPC_PARSE_ERROR,
                &format!("Parse error: {parse_error}"),
            )),
        };
        if let Some(response_value) = response {
            if let Err(write_error) =
                write_framed_response(standard_output, standard_error, &response_value)
            {
                let _ = writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                return 1;
            }
        }
    }
}

fn write_framed_response(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    response: &Value,
) -> std::io::Result<()> {
    let serialized = match serde_json::to_string(response) {
        Ok(text) => text,
        Err(serialize_error) => {
            let _ = writeln!(
                standard_error,
                "[keel mcp] serialize response: {serialize_error}"
            );
            // Fall back to a hand-built error envelope so the peer sees a
            // valid JSON-RPC frame even when the original payload could not
            // be serialized for some reason.
            "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"internal serialization error\"}}".to_string()
        }
    };
    standard_output.write_all(serialized.as_bytes())?;
    standard_output.write_all(b"\n")?;
    standard_output.flush()
}

/// Dispatch a single parsed JSON-RPC request. Returns `Some(response)` for
/// requests (objects with an `id`) and `None` for notifications. Tests drive
/// this function directly to avoid spawning the binary; the stdio loop also
/// uses it after framing.
pub fn dispatch(request: &Value) -> Option<Value> {
    let object = match request.as_object() {
        Some(object) => object,
        None => {
            return Some(error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: expected JSON object",
            ));
        }
    };

    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        return Some(error_response(
            id,
            JSON_RPC_INVALID_REQUEST,
            "Invalid Request: jsonrpc must be \"2.0\"",
        ));
    }

    let method = match object.get("method").and_then(Value::as_str) {
        Some(method) => method.to_string(),
        None => {
            let id = object.get("id").cloned().unwrap_or(Value::Null);
            return Some(error_response(
                id,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: missing method",
            ));
        }
    };

    let id = object.get("id").cloned();
    let params = object.get("params").cloned().unwrap_or(Value::Null);

    // JSON-RPC 2.0 §4.1: a request without `id` is a notification — no
    // An id:null is a valid request and must receive a response.
    let is_notification = id.is_none();

    if is_notification {
        // Currently the only meaningful incoming notification is
        // `notifications/initialized`. Other notifications are ignored
        // silently per the spec — they must never produce a response.
        let _ = handle_method(&method, &params);
        return None;
    }

    let request_id = id.unwrap_or(Value::Null);
    Some(match handle_method(&method, &params) {
        Ok(result) => success_response(request_id, result),
        Err(MethodError { code, message }) => error_response(request_id, code, &message),
    })
}

/// Dispatcher for a single MCP method. Kept method-keyed (rather than
/// argument-keyed) so the protocol surface is greppable and each handler
/// stays small.
fn handle_method(method: &str, params: &Value) -> Result<Value, MethodError> {
    match method {
        "initialize" => Ok(handle_initialize(params)),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::handle_tools_list()),
        "tools/call" => tools::handle_tools_call(params),
        "resources/list" => Ok(handle_resources_list()),
        "resources/read" => handle_resources_read(params),
        other => Err(MethodError {
            code: JSON_RPC_METHOD_NOT_FOUND,
            message: format!("Method not found: {other}"),
        }),
    }
}

fn handle_initialize(params: &Value) -> Value {
    // Per the MCP lifecycle spec, echo the client's requested protocolVersion
    // when present so the negotiated session version matches what the client
    // asked for; fall back to our latest supported version when the client
    // omits it. A non-string value is ignored in favor of the fallback.
    let negotiated = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(MCP_PROTOCOL_VERSION);
    json!({
        "protocolVersion": negotiated,
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": MCP_SERVER_VERSION,
        },
        "capabilities": {
            "tools": {},
            "resources": {},
        },
    })
}

fn handle_resources_list() -> Value {
    json!({
        "resources": [
            {
                "uri": SYSTEM_MAP_RESOURCE_URI,
                "name": "keel SYSTEM_MAP.md",
                "description": "Workspace structural map (auto-refreshed under ~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md, falling back to a freshly rendered map).",
                "mimeType": "text/markdown"
            },
            {
                "uri": RECALL_STATUS_RESOURCE_URI,
                "name": "keel recall index status",
                "description": "JSON snapshot of the recall FTS5 index health (document count, schema version, last-sync timestamp).",
                "mimeType": "application/json"
            }
        ]
    })
}

fn handle_resources_read(params: &Value) -> Result<Value, MethodError> {
    let object = params.as_object().ok_or_else(|| MethodError {
        code: JSON_RPC_INVALID_PARAMS,
        message: "resources/read params must be an object".to_string(),
    })?;
    let uri = object
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| MethodError {
            code: JSON_RPC_INVALID_PARAMS,
            message: "resources/read params.uri is required".to_string(),
        })?;
    match uri {
        SYSTEM_MAP_RESOURCE_URI => {
            let text = system_map_text(None).map_err(|message| MethodError {
                code: JSON_RPC_INTERNAL_ERROR,
                message,
            })?;
            Ok(json!({
                "contents": [
                    {
                        "uri": SYSTEM_MAP_RESOURCE_URI,
                        "mimeType": "text/markdown",
                        "text": text,
                    }
                ]
            }))
        }
        RECALL_STATUS_RESOURCE_URI => {
            let payload = recall_status_payload().map_err(|message| MethodError {
                code: JSON_RPC_INTERNAL_ERROR,
                message,
            })?;
            let text = serde_json::to_string_pretty(&payload).map_err(|error| MethodError {
                code: JSON_RPC_INTERNAL_ERROR,
                message: format!("serialize recall status: {error}"),
            })?;
            Ok(json!({
                "contents": [
                    {
                        "uri": RECALL_STATUS_RESOURCE_URI,
                        "mimeType": "application/json",
                        "text": text,
                    }
                ]
            }))
        }
        other => Err(MethodError {
            code: JSON_RPC_INVALID_PARAMS,
            message: format!("Unknown resource URI: {other}"),
        }),
    }
}

/// Resolve the workspace SYSTEM_MAP.md text — the cached copy under the
/// workspace reference lane when present and non-empty, else a freshly rendered
/// map. Shared by the `system_map` tool and the `keel://system-map`
/// resource so both surfaces return the same content.
pub(super) fn system_map_text(workspace_override: Option<&Path>) -> Result<String, String> {
    let workspace_root = match workspace_override {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?,
    };
    let claude_home =
        resolve_claude_home("").map_err(|error| format!("resolve claude home: {error}"))?;
    let cached_map = system_map_reference_directory(&claude_home, "memory", &workspace_root)
        .join("SYSTEM_MAP.md");
    if cached_map.is_file() {
        if let Ok(text) = std::fs::read_to_string(&cached_map) {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }
    Ok(render_system_map(&workspace_root))
}

/// Build the recall index health snapshot payload. Shared by the
/// `recall_status` / `memory_status` tools and the `keel://recall/status`
/// resource so all surfaces report the same shape.
#[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
pub(super) fn recall_status_payload() -> Result<Value, String> {
    let claude_home = resolve_claude_home("")?;
    let snapshot = recall_status_snapshot(&claude_home)?;
    let mut payload = json!({
        "claudeHome": display_path(&snapshot.claude_home),
        "indexPath": display_path(&snapshot.index_path),
        "schemaVersion": snapshot.schema_version,
        "documents": snapshot.document_count,
        "lastIndexedAtMillis": snapshot.last_indexed_at_millis.to_string(),
        "addedSinceLastSync": snapshot.added_since_last_sync,
        "updatedSinceLastSync": snapshot.updated_since_last_sync,
        "removedSinceLastSync": snapshot.removed_since_last_sync,
    });
    #[cfg(feature = "semantic")]
    {
        if let Some(object) = payload.as_object_mut() {
            object.insert("vectors".to_string(), json!(snapshot.vector_count));
        }
    }
    Ok(payload)
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

#[derive(Debug, Clone)]
pub(super) struct MethodError {
    pub(super) code: i64,
    pub(super) message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_returns_protocol_and_server_info() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], json!(1));
        let result = &response["result"];
        assert_eq!(result["protocolVersion"], json!(MCP_PROTOCOL_VERSION));
        assert_eq!(result["serverInfo"]["name"], json!(MCP_SERVER_NAME));
        assert_eq!(result["serverInfo"]["version"], json!(MCP_SERVER_VERSION));
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
    }

    #[test]
    fn initialize_echoes_client_requested_protocol_version() {
        // Per the MCP lifecycle spec the server responds with the client's
        // requested protocolVersion when it can support it, rather than forcing
        // its own. This keeps the server compatible as the spec revises without a
        // constant bump. A client asking for an older revision gets that revision
        // back; omitting it falls back to MCP_PROTOCOL_VERSION (covered above).
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(
            response["result"]["protocolVersion"],
            json!("2024-11-05"),
            "server must echo the client's requested protocol version"
        );

        // A non-string protocolVersion is ignored in favor of the fallback.
        let bad = json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "initialize",
            "params": { "protocolVersion": 1234 }
        });
        let bad_response = dispatch(&bad).expect("response present");
        assert_eq!(
            bad_response["result"]["protocolVersion"],
            json!(MCP_PROTOCOL_VERSION),
            "a non-string protocolVersion must fall back to the server default"
        );

        // An explicit null protocolVersion also falls back (as_str on Null → None).
        let null_version = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "initialize",
            "params": { "protocolVersion": null }
        });
        let null_response = dispatch(&null_version).expect("response present");
        assert_eq!(
            null_response["result"]["protocolVersion"],
            json!(MCP_PROTOCOL_VERSION),
            "a null protocolVersion must fall back to the server default"
        );
    }

    #[test]
    fn notifications_initialized_produces_no_response() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        assert!(dispatch(&request).is_none());
    }

    #[test]
    fn ping_returns_empty_object_result() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "ping-1",
            "method": "ping"
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["id"], json!("ping-1"));
        assert_eq!(response["result"], json!({}));
    }

    #[test]
    fn tools_list_advertises_all_tools() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list"
        });
        let response = dispatch(&request).expect("response present");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .collect();
        for expected in [
            "recall",
            "system_map",
            "run_command",
            "recall_status",
            "skill_route",
            "skill_get",
            "skill_list",
            "memory_status",
            "brief_list",
            "brief_get",
            "brief_create",
            "system_map_refresh",
            "context_brief",
            "cli",
            "sprint",
            "user_story_lint",
            "review",
            "workflow",
            "git_workflow",
            "memory",
            "gain",
            "raw",
            "config_audit",
            "skill_lint",
            "telemetry",
            "orchestration",
            "checkpoint",
            "session",
            "doctor",
            "code_search",
            "user_story",
            "flow",
            "work",
            "code_graph",
            "learn",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        assert!(
            !names.is_empty()
                && names
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == names.len(),
            "tool names must be unique: {names:?}"
        );
    }

    #[test]
    fn resources_list_advertises_system_map_and_recall_status() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "resources/list"
        });
        let response = dispatch(&request).expect("response present");
        let resources = response["result"]["resources"]
            .as_array()
            .expect("resources array");
        let uris: Vec<&str> = resources
            .iter()
            .filter_map(|entry| entry.get("uri").and_then(Value::as_str))
            .collect();
        assert!(uris.contains(&SYSTEM_MAP_RESOURCE_URI));
        assert!(uris.contains(&RECALL_STATUS_RESOURCE_URI));
        assert_eq!(uris.len(), 2);
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "tools/teleport"
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["error"]["code"], json!(JSON_RPC_METHOD_NOT_FOUND));
    }

    #[test]
    fn missing_jsonrpc_version_returns_invalid_request() {
        let request = json!({
            "id": 1,
            "method": "initialize"
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_REQUEST));
    }

    #[test]
    fn parse_error_response_uses_dash_32700() {
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        // Trailing newline lets the loop process the malformed line then EOF.
        let mut input: &[u8] = b"not-json\n";
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("\"code\":-32700"), "rendered: {rendered}");
    }

    #[test]
    fn tools_call_unknown_tool_reports_invalid_params() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "definitely-not-a-real-tool",
                "arguments": {}
            }
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_PARAMS));
    }

    #[test]
    fn serve_stdio_handles_request_then_eof() {
        let request = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping"
        }))
        .expect("serialize");
        let mut input_bytes = request.into_bytes();
        input_bytes.push(b'\n');
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("\"id\":1"), "rendered: {rendered}");
        assert!(rendered.contains("\"result\":{}"), "rendered: {rendered}");
        assert!(rendered.ends_with('\n'), "rendered: {rendered}");
    }

    #[test]
    fn request_with_id_null_receives_response() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "method": "ping"
        });
        let response = dispatch(&request).expect("response present");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(
            response["id"],
            json!(Value::Null),
            "id must be null as per request"
        );
        assert_eq!(
            response["result"],
            json!({}),
            "ping must return empty object"
        );
    }
}
