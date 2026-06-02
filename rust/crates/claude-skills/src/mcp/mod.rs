//! Purpose: Model Context Protocol (MCP) server surface for claude_core.
//! Caller: `commands.rs` `mcp` arm — `claude-skills mcp serve` reads JSON-RPC
//!   2.0 messages from stdin, writes responses to stdout, logs to stderr, and
//!   exits cleanly when stdin closes. Test harnesses also drive `dispatch`
//!   directly without spawning the binary.
//! Dependencies: serde_json for newline-delimited JSON-RPC framing,
//!   `utility::recall` for the FTS5-backed search/status surface,
//!   `utility::system_map` for the workspace map, `proxy::run` for the
//!   capture+compaction pipeline used by the `run_command` tool, and
//!   `runtime` for the Claude home resolver.
//! Main Functions: `run_mcp_command`, `serve_stdio`, `dispatch`.
//! Side Effects: Reads stdin, writes JSON-RPC responses to stdout, writes
//!   diagnostics to stderr, opens (and on first call creates) the recall
//!   SQLite index under `<claude-home>/recall-index.sqlite3`, and may execute
//!   user-supplied shell commands via the proxy capture path when the
//!   `run_command` tool is invoked.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::recall::{recall_status_snapshot, search_recall_index, RecallSearchResult};
use crate::utility::system_map::{render_system_map, sanitize_key};

/// Wire-protocol version this server advertises during `initialize`. Matches
/// the version Claude Code probes for (see code.claude.com/docs/en/mcp).
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Server identity returned in the `initialize` response. The version mirrors
/// the workspace package version so plugin manifests and the server agree on
/// what the host is talking to.
const MCP_SERVER_NAME: &str = "claude_core";
const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default cap for `recall` matches when the caller does not supply one. The
/// CLI uses the same default (see `utility::recall::DEFAULT_RECALL_LIMIT`).
const DEFAULT_RECALL_LIMIT: usize = 20;
const MAX_RECALL_LIMIT: usize = 100;

/// JSON-RPC error codes the dispatcher returns. The numeric values are
/// stable per JSON-RPC 2.0 §5.1; using named constants here keeps the
/// `tools/call` and `resources/read` arms readable.
const JSON_RPC_PARSE_ERROR: i64 = -32700;
const JSON_RPC_INVALID_REQUEST: i64 = -32600;
const JSON_RPC_METHOD_NOT_FOUND: i64 = -32601;
const JSON_RPC_INVALID_PARAMS: i64 = -32602;
const JSON_RPC_INTERNAL_ERROR: i64 = -32603;

/// Resource URIs this server publishes. Keep these as constants so the list
/// surface and the read surface stay in lockstep.
const SYSTEM_MAP_RESOURCE_URI: &str = "claude_core://system-map";
const RECALL_STATUS_RESOURCE_URI: &str = "claude_core://recall/status";

/// Entry point for `claude-skills mcp <subcommand>`. The only subcommand
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
    let _ = writeln!(standard_output, "Usage: claude-skills mcp serve");
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Runs the claude_core Model Context Protocol server over stdio."
    );
    let _ = writeln!(
        standard_output,
        "Reads newline-delimited JSON-RPC 2.0 messages from stdin, writes responses to stdout."
    );
    let _ = writeln!(
        standard_output,
        "Tools: recall, system_map, run_command, recall_status."
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
    let mut line = String::new();
    loop {
        line.clear();
        let read_result = reader.read_line(&mut line);
        match read_result {
            Ok(0) => return 0,
            Ok(_) => {}
            Err(error) => {
                let _ = writeln!(standard_error, "[claude_core mcp] read stdin: {error}");
                return 1;
            }
        }
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
                let _ = writeln!(
                    standard_error,
                    "[claude_core mcp] write stdout: {write_error}"
                );
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
                "[claude_core mcp] serialize response: {serialize_error}"
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
    // response is sent. We still execute side-effect-free notifications
    // (`notifications/initialized`) for protocol completeness.
    let is_notification = id.is_none() || matches!(id.as_ref(), Some(Value::Null));

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
        "initialize" => Ok(handle_initialize()),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(params),
        "resources/list" => Ok(handle_resources_list()),
        "resources/read" => handle_resources_read(params),
        other => Err(MethodError {
            code: JSON_RPC_METHOD_NOT_FOUND,
            message: format!("Method not found: {other}"),
        }),
    }
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
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

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "recall",
                "description": "Call this BEFORE claiming what you remember or previously learned — search your durable memory instead of relying on conversation alone. Full-text search over Markdown under <claude-home>/{memories,memoriesv2,working-briefs}. Auto-syncs the index before querying.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms; punctuation is stripped and tokens are AND-ed with prefix match." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RECALL_LIMIT, "description": "Maximum hits (default 20)." }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "system_map",
                "description": "Call this BEFORE any claim about the current repository's structure or layout (\"what is this project\", \"how is this organized\", \"where does X live\") instead of guessing or reading files blind. Returns the workspace SYSTEM_MAP.md (the auto-refreshed copy under ~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md, falling back to a freshly rendered map).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_root": { "type": "string", "description": "Absolute workspace root. Defaults to the server process working directory." }
                    }
                }
            },
            {
                "name": "run_command",
                "description": "Prefer this over a raw shell call for noisy commands (test, build, lint, logs, search): it runs the command through the compaction proxy so compacted high-signal output enters context instead of the raw stream.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command line to execute (joined with the platform shell when shell metacharacters are present)." }
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "recall_status",
                "description": "Check recall index health before trusting or after rebuilding the memory index: document count, schema version, last-sync timestamp, and on-disk index path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

fn handle_tools_call(params: &Value) -> Result<Value, MethodError> {
    let object = params.as_object().ok_or_else(|| MethodError {
        code: JSON_RPC_INVALID_PARAMS,
        message: "tools/call params must be an object".to_string(),
    })?;
    let tool_name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| MethodError {
            code: JSON_RPC_INVALID_PARAMS,
            message: "tools/call params.name is required".to_string(),
        })?;
    let arguments = object.get("arguments").cloned().unwrap_or(Value::Null);

    let outcome = match tool_name {
        "recall" => tool_recall(&arguments),
        "system_map" => tool_system_map(&arguments),
        "run_command" => tool_run_command(&arguments),
        "recall_status" => tool_recall_status(&arguments),
        other => {
            return Err(MethodError {
                code: JSON_RPC_INVALID_PARAMS,
                message: format!("Unknown tool: {other}"),
            });
        }
    };

    match outcome {
        Ok(text) => Ok(json!({
            "content": [
                { "type": "text", "text": text }
            ],
            "isError": false,
        })),
        Err(message) => Ok(json!({
            "content": [
                { "type": "text", "text": message }
            ],
            "isError": true,
        })),
    }
}

fn handle_resources_list() -> Value {
    json!({
        "resources": [
            {
                "uri": SYSTEM_MAP_RESOURCE_URI,
                "name": "claude_core SYSTEM_MAP.md",
                "description": "Workspace structural map (auto-refreshed under ~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md, falling back to a freshly rendered map).",
                "mimeType": "text/markdown"
            },
            {
                "uri": RECALL_STATUS_RESOURCE_URI,
                "name": "claude_core recall index status",
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

fn tool_recall(arguments: &Value) -> Result<String, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("recall: missing query".to_string());
    }
    let limit_value = arguments.get("limit");
    let limit = match limit_value {
        Some(Value::Number(number)) => match number.as_u64() {
            Some(parsed) if parsed > 0 => (parsed as usize).min(MAX_RECALL_LIMIT),
            _ => {
                return Err(format!(
                    "recall: limit must be a positive integer, got {number}"
                ))
            }
        },
        Some(Value::Null) | None => DEFAULT_RECALL_LIMIT,
        Some(other) => {
            return Err(format!(
                "recall: limit must be a positive integer, got {other}"
            ));
        }
    };
    let claude_home = resolve_claude_home("").map_err(|error| format!("recall: {error}"))?;
    let result = search_recall_index(&claude_home, &query, limit)
        .map_err(|error| format!("recall: {error}"))?;
    let payload = render_recall_payload(&claude_home, &query, limit, result);
    serde_json::to_string_pretty(&payload).map_err(|error| format!("recall: serialize: {error}"))
}

fn render_recall_payload(
    claude_home: &Path,
    query: &str,
    limit: usize,
    result: Option<RecallSearchResult>,
) -> Value {
    let (fts_query, hits) = match result {
        Some(search_result) => (search_result.fts_query, search_result.hits),
        None => (String::new(), Vec::new()),
    };
    let matches: Vec<Value> = hits
        .iter()
        .map(|hit| {
            let relative = relative_to_home(claude_home, Path::new(&hit.absolute_path));
            json!({
                "path": relative,
                "absolutePath": hit.absolute_path,
                "score": format!("{:.4}", hit.score),
                "line": hit.line,
                "snippet": hit.snippet,
            })
        })
        .collect();
    json!({
        "query": query,
        "ftsQuery": fts_query,
        "limit": limit,
        "claudeHome": display_path(claude_home),
        "count": matches.len(),
        "matches": matches,
    })
}

fn relative_to_home(claude_home: &Path, absolute_path: &Path) -> String {
    match absolute_path.strip_prefix(claude_home) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => display_path(absolute_path),
    }
}

fn tool_system_map(arguments: &Value) -> Result<String, String> {
    let workspace_override = arguments
        .get("workspace_root")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    system_map_text(workspace_override.as_deref())
}

fn system_map_text(workspace_override: Option<&Path>) -> Result<String, String> {
    let workspace_root = match workspace_override {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?,
    };
    let claude_home =
        resolve_claude_home("").map_err(|error| format!("resolve claude home: {error}"))?;
    let workspace_slug = sanitize_key(&workspace_root.to_string_lossy());
    let cached_map = claude_home
        .join("memories")
        .join("workspaces")
        .join(&workspace_slug)
        .join("reference")
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

fn tool_run_command(arguments: &Value) -> Result<String, String> {
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if command.is_empty() {
        return Err("run_command: missing command".to_string());
    }
    // Shell out to a fresh `claude-skills run -- <command>` so the proxy
    // pipeline (capture, compaction, raw-store, gain analytics) runs in its
    // intended configuration. Setting `CLAUDE_SKILLS_HOOK` flips the proxy's
    // capture gate on for this child even when the parent MCP server was
    // launched from a plain shell.
    let executable = env::current_exe().map_err(|error| format!("locate self: {error}"))?;
    let mut child = Command::new(&executable);
    child.arg("run");
    child.arg("--");
    let (program, args) = crate::runtime::platform_shell_command_parts(&command);
    child.arg(program);
    for argument in args {
        child.arg(argument);
    }
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let output = child
        .output()
        .map_err(|error| format!("run_command: spawn: {error}"))?;
    let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    // Return a plain-text report rather than a JSON object. Embedding multi-line
    // stdout/stderr as JSON string values escapes every newline as a literal
    // `\n`, which turns a build/test log into an unreadable single line in the
    // MCP tool-result view. A text report keeps real newlines so the output is
    // legible to both the human reading the transcript and the model consuming
    // the result; the exit code stays on its own labeled line for easy parsing.
    Ok(render_run_command_report(
        &command,
        output.status.code().unwrap_or(-1),
        &stdout_text,
        &stderr_text,
    ))
}

/// Build the human-readable `run_command` report. Pure (no IO) so the framing
/// is unit-testable. Sections with no content are omitted; a command that
/// produced nothing on either stream still reports its exit code plus an
/// explicit `(no output)` marker so the result is never ambiguously empty.
fn render_run_command_report(command: &str, exit_code: i32, stdout: &str, stderr: &str) -> String {
    let mut report = String::new();
    report.push_str(&format!("$ {command}\n"));
    report.push_str(&format!("exit code: {exit_code}\n"));

    let stdout_body = stdout.trim_end_matches(['\n', '\r']);
    let stderr_body = stderr.trim_end_matches(['\n', '\r']);

    if !stdout_body.trim().is_empty() {
        report.push_str("\n--- stdout ---\n");
        report.push_str(stdout_body);
        report.push('\n');
    }
    if !stderr_body.trim().is_empty() {
        report.push_str("\n--- stderr ---\n");
        report.push_str(stderr_body);
        report.push('\n');
    }
    if stdout_body.trim().is_empty() && stderr_body.trim().is_empty() {
        report.push_str("\n(no output)\n");
    }
    report
}

fn tool_recall_status(_arguments: &Value) -> Result<String, String> {
    let payload = recall_status_payload().map_err(|message| format!("recall_status: {message}"))?;
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("recall_status: serialize: {error}"))
}

fn recall_status_payload() -> Result<Value, String> {
    let claude_home = resolve_claude_home("")?;
    let snapshot = recall_status_snapshot(&claude_home)?;
    Ok(json!({
        "claudeHome": display_path(&snapshot.claude_home),
        "indexPath": display_path(&snapshot.index_path),
        "schemaVersion": snapshot.schema_version,
        "documents": snapshot.document_count,
        "lastIndexedAtMillis": snapshot.last_indexed_at_millis.to_string(),
        "addedSinceLastSync": snapshot.added_since_last_sync,
        "updatedSinceLastSync": snapshot.updated_since_last_sync,
        "removedSinceLastSync": snapshot.removed_since_last_sync,
    }))
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
struct MethodError {
    code: i64,
    message: String,
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
    fn tools_list_advertises_four_tools() {
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
        assert!(names.contains(&"recall"), "names: {names:?}");
        assert!(names.contains(&"system_map"), "names: {names:?}");
        assert!(names.contains(&"run_command"), "names: {names:?}");
        assert!(names.contains(&"recall_status"), "names: {names:?}");
        assert_eq!(names.len(), 4);
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
    fn run_command_report_keeps_real_newlines() {
        // Regression: stdout/stderr used to be embedded as JSON string values,
        // which escaped every newline as a literal `\n` and produced an
        // unreadable single-line wall. The text report must carry real
        // newlines and label each stream.
        let report = render_run_command_report(
            "cargo test",
            0,
            "running 2 tests\ntest a ... ok\ntest b ... ok",
            "",
        );
        assert!(report.contains("$ cargo test\n"));
        assert!(report.contains("exit code: 0\n"));
        assert!(report.contains("--- stdout ---\n"));
        // Real newline present, no literal backslash-n escape.
        assert!(report.contains("test a ... ok\ntest b ... ok"));
        assert!(
            !report.contains("\\n"),
            "report must not contain escaped newlines"
        );
        // No stderr section when stderr is empty.
        assert!(!report.contains("--- stderr ---"));
    }

    #[test]
    fn run_command_report_includes_stderr_and_nonzero_exit() {
        let report = render_run_command_report("false", 1, "", "boom: it failed\n");
        assert!(report.contains("exit code: 1\n"));
        assert!(report.contains("--- stderr ---\n"));
        assert!(report.contains("boom: it failed"));
        assert!(!report.contains("--- stdout ---"));
    }

    #[test]
    fn run_command_report_marks_empty_output() {
        let report = render_run_command_report("true", 0, "", "");
        assert!(report.contains("exit code: 0\n"));
        assert!(report.contains("(no output)"));
    }
}
