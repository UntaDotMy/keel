//! Purpose: Model Context Protocol (MCP) server surface for keel.
//! Caller: `commands.rs` `mcp` arm — `keel mcp serve` (stdio) and
//!   `keel mcp serve-http` (Streamable HTTP multi-client).
//! Dependencies: serde_json for JSON-RPC framing; `utility::recall` /
//!   `system_map` / proxy for tools; std::net for HTTP (no async runtime).
//! Main Functions: `run_mcp_command`, `serve_stdio_owned_stdin`, `dispatch`,
//!   `http::serve_http`.
//! Side Effects: stdio or TCP I/O; recall SQLite; optional child commands via
//!   `run_command`.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::memory::system_map_reference_directory;
use crate::utility::recall::recall_status_snapshot;
use crate::utility::system_map::render_system_map;

mod http;
mod tools;

/// Maximum bytes accepted for a single newline-delimited JSON-RPC frame. A peer
/// that streams data without a terminating newline would otherwise grow the
/// read buffer without bound and exhaust memory; capping the per-frame read
/// bounds that to a generous-but-finite size (8 MiB — far above any real tool
/// call, well below a DoS). An over-cap frame is refused, not truncated.
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;

/// Default max concurrent in-flight JSON-RPC requests per `mcp serve` process.
/// Hosts may stream multiple `tools/call`s before prior responses return; without
/// a cap a flood could spawn unbounded OS threads. Override with
/// `KEEL_MCP_MAX_INFLIGHT` (min 1, max 512). Multi-session scale (100+ harness
/// chats) is separate: each session is its own `keel mcp serve` process.
const DEFAULT_MAX_INFLIGHT: usize = 64;

/// Default wire-protocol version advertised during `initialize` when the client
/// does not request one. Per the MCP lifecycle spec the server SHOULD respond
/// with the client's requested `protocolVersion` when it can support it, and
/// only fall back to its own latest supported version otherwise. We echo the
/// client's value in [`handle_initialize`] and use this constant as the
/// fallback, so the server stays compatible as the spec revises without needing
/// a constant bump each time. Current spec revision: 2025-11-25
/// (see code.claude.com/docs/en/mcp and modelcontextprotocol.io/specification).
pub(super) const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Server identity returned in the `initialize` response. The version mirrors
/// the workspace package version so plugin manifests and the server agree on
/// what the host is talking to.
pub(super) const MCP_SERVER_NAME: &str = "keel";
pub(super) const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON-RPC error codes the dispatcher returns. The numeric values are
/// stable per JSON-RPC 2.0 §5.1; using named constants here keeps the
/// `tools/call` and `resources/read` arms readable.
pub(super) const JSON_RPC_PARSE_ERROR: i64 = -32700;
pub(super) const JSON_RPC_INVALID_REQUEST: i64 = -32600;
const JSON_RPC_METHOD_NOT_FOUND: i64 = -32601;
pub(super) const JSON_RPC_INVALID_PARAMS: i64 = -32602;
const JSON_RPC_INTERNAL_ERROR: i64 = -32603;

/// Resource URIs this server publishes. Keep these as constants so the list
/// surface and the read surface stay in lockstep.
const SYSTEM_MAP_RESOURCE_URI: &str = "keel://system-map";
const RECALL_STATUS_RESOURCE_URI: &str = "keel://recall/status";

/// Entry point for `keel mcp <subcommand>`.
pub fn run_mcp_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let subcommand = arguments.first().map(String::as_str).unwrap_or("");
    match subcommand {
        // Owned stdin reader thread so responses flush while stdin is blocked
        // waiting for the next frame (true full-duplex on the JSON-RPC pipe).
        "serve" => serve_stdio_owned_stdin(standard_output, standard_error),
        "serve-http" => http::serve_http(&arguments[1..], standard_output, standard_error),
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
    let _ = writeln!(standard_output, "Usage:");
    let _ = writeln!(standard_output, "  keel mcp serve");
    let _ = writeln!(
        standard_output,
        "  keel mcp serve-http [--bind HOST:PORT]"
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "stdio (serve): one process per host session. The harness launches a separate"
    );
    let _ = writeln!(
        standard_output,
        "  `keel mcp serve` per chat/session — 100 sessions ≈ 100 processes, not one."
    );
    let _ = writeln!(
        standard_output,
        "  Within a process: concurrent JSON-RPC workers (KEEL_MCP_MAX_INFLIGHT, default 64)."
    );
    let _ = writeln!(
        standard_output,
        "  Responses may complete out of order; clients match by id. JSON-RPC batch arrays"
    );
    let _ = writeln!(
        standard_output,
        "  return one newline-framed JSON array response (JSON-RPC 2.0 §6)."
    );
    let _ = writeln!(
        standard_output,
        "  Shared recall DB uses SQLite WAL + busy_timeout across processes."
    );
    let _ = writeln!(
        standard_output,
        "  Tool wall-clock: KEEL_MCP_TOOL_TIMEOUT_SECS (default 90)."
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "HTTP (serve-http): Streamable HTTP multi-client on one process (default"
    );
    let _ = writeln!(
        standard_output,
        "  127.0.0.1:3920). Origin checked against DNS-rebinding rules; concurrent"
    );
    let _ = writeln!(
        standard_output,
        "  connections each run request workers. Endpoint: POST/GET/DELETE /mcp."
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Tools: recall, system_map, run_command, recall_status, skill_route, skill_get, skill_list, memory_status, brief_list, brief_get, brief_create, system_map_refresh, …"
    );
    let _ = writeln!(
        standard_output,
        "Resources: {SYSTEM_MAP_RESOURCE_URI}, {RECALL_STATUS_RESOURCE_URI}."
    );
}

/// Events on the serve loop's multi-producer channel.
///
/// The stdin reader and request workers all send into one queue so the main
/// thread can write responses as they complete (JSON-RPC allows out-of-order
/// replies matched by `id`) while still accepting new frames — no head-of-line
/// block when one slow `tools/call` is in flight.
enum ServeEvent {
    /// One newline-delimited frame body (trim empty before enqueue).
    Frame(String),
    /// Stdin hit an oversized frame; main must reply and tear down.
    OversizedFrame,
    /// Reader finished (EOF). Drain in-flight work, then exit.
    ReaderEof,
    /// Unrecoverable stdin error.
    ReaderError(String),
    /// A worker finished a request that needs a response written to stdout.
    /// `batch` is set when the request was part of a JSON-RPC batch array.
    Response {
        value: Value,
        batch: Option<BatchMember>,
    },
    /// A worker finished a notification (no response). Frees an in-flight slot.
    WorkerDone { batch: Option<BatchMember> },
}

/// Identity of one element inside an open JSON-RPC batch.
#[derive(Clone, Copy)]
struct BatchMember {
    batch_id: u64,
    index: usize,
}

/// Collects per-index results until every batch element has finished, then
/// emits one JSON array (JSON-RPC 2.0 §6). Notification slots contribute no
/// response object.
struct BatchCollector {
    slots: Vec<BatchSlot>,
    remaining: usize,
}

enum BatchSlot {
    Pending,
    Notification,
    Response(Value),
}

/// One scheduled unit of work (single-line request or batch element).
struct PendingJob {
    request: Value,
    batch: Option<BatchMember>,
}

/// Read newline-delimited JSON-RPC messages from `input` and write framed
/// responses to `standard_output`. Notifications produce no response.
///
/// **Concurrency:** each request is dispatched on a worker thread (bounded by
/// `KEEL_MCP_MAX_INFLIGHT`, default 64). Responses may complete out of order;
/// clients match on JSON-RPC `id`. Stdout writes stay single-threaded so frames
/// never interleave mid-line.
///
/// Test/harness path: frames are read to EOF first, then workers run
/// concurrently (proves no serial tool blocking). Production `keel mcp serve`
/// uses [`serve_stdio_owned_stdin`] so stdin is read on a side thread and
/// responses flush while the client is still streaming requests.
///
/// Returns 0 on clean stdin EOF (after draining in-flight work), non-zero only
/// when an unrecoverable I/O error prevents reading or writing the stream.
///
/// Production `keel mcp serve` uses [`serve_stdio_owned_stdin`]. This entry is
/// for unit tests that feed a byte buffer (not a live stdin handle).
#[cfg(test)]
pub fn serve_stdio(
    input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let max_inflight = max_inflight();
    let (event_tx, event_rx) = mpsc::channel::<ServeEvent>();

    let mut reader = BufReader::new(input);
    let mut raw_line: Vec<u8> = Vec::new();
    let mut preloaded: VecDeque<ServeEvent> = VecDeque::new();
    loop {
        raw_line.clear();
        let read_result = reader
            .by_ref()
            .take(MAX_FRAME_BYTES)
            .read_until(b'\n', &mut raw_line);
        match read_result {
            Ok(0) => {
                preloaded.push_back(ServeEvent::ReaderEof);
                break;
            }
            Ok(_) => {}
            Err(error) => {
                preloaded.push_back(ServeEvent::ReaderError(error.to_string()));
                break;
            }
        }
        if raw_line.len() as u64 >= MAX_FRAME_BYTES && raw_line.last() != Some(&b'\n') {
            preloaded.push_back(ServeEvent::OversizedFrame);
            break;
        }
        let line = String::from_utf8_lossy(&raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        preloaded.push_back(ServeEvent::Frame(trimmed.to_string()));
    }
    for event in preloaded {
        let _ = event_tx.send(event);
    }
    run_serve_event_loop(
        event_tx,
        event_rx,
        standard_output,
        standard_error,
        max_inflight,
    )
}

/// Production entry: stdin reader runs on its own thread so the event loop can
/// write tool responses while still accepting new frames (true full-duplex).
fn serve_stdio_owned_stdin(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let max_inflight = max_inflight();
    let (event_tx, event_rx) = mpsc::channel::<ServeEvent>();
    let reader_tx = event_tx.clone();
    let reader_handle = thread::Builder::new()
        .name("keel-mcp-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut locked = stdin.lock();
            read_frames_into(&mut locked, reader_tx);
        });
    if let Err(error) = reader_handle {
        let _ = writeln!(standard_error, "[keel mcp] spawn stdin reader: {error}");
        return 1;
    }
    run_serve_event_loop(
        event_tx,
        event_rx,
        standard_output,
        standard_error,
        max_inflight,
    )
}

pub(super) fn max_inflight() -> usize {
    env::var("KEEL_MCP_MAX_INFLIGHT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|n| n.clamp(1, 512))
        .unwrap_or(DEFAULT_MAX_INFLIGHT)
}

fn read_frames_into(input: &mut dyn Read, event_tx: mpsc::Sender<ServeEvent>) {
    let mut reader = BufReader::new(input);
    let mut raw_line: Vec<u8> = Vec::new();
    loop {
        raw_line.clear();
        let read_result = reader
            .by_ref()
            .take(MAX_FRAME_BYTES)
            .read_until(b'\n', &mut raw_line);
        match read_result {
            Ok(0) => {
                let _ = event_tx.send(ServeEvent::ReaderEof);
                return;
            }
            Ok(_) => {}
            Err(error) => {
                let _ = event_tx.send(ServeEvent::ReaderError(error.to_string()));
                return;
            }
        }
        if raw_line.len() as u64 >= MAX_FRAME_BYTES && raw_line.last() != Some(&b'\n') {
            let _ = event_tx.send(ServeEvent::OversizedFrame);
            return;
        }
        let line = String::from_utf8_lossy(&raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if event_tx.send(ServeEvent::Frame(trimmed.to_string())).is_err() {
            return;
        }
    }
}

fn run_serve_event_loop(
    event_tx: mpsc::Sender<ServeEvent>,
    event_rx: mpsc::Receiver<ServeEvent>,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    max_inflight: usize,
) -> u8 {
    let mut in_flight: usize = 0;
    let mut pending: VecDeque<PendingJob> = VecDeque::new();
    let mut batches: HashMap<u64, BatchCollector> = HashMap::new();
    let mut next_batch_id: u64 = 1;
    let mut reader_done = false;
    let mut exit_code: u8 = 0;

    loop {
        let event = match event_rx.recv() {
            Ok(event) => event,
            Err(_) => break,
        };

        match event {
            ServeEvent::Frame(frame) => match parse_frame(&frame) {
                FrameParse::Single(request) => {
                    pending.push_back(PendingJob {
                        request,
                        batch: None,
                    });
                }
                FrameParse::Batch(items) => {
                    let batch_id = next_batch_id;
                    next_batch_id = next_batch_id.saturating_add(1);
                    let len = items.len();
                    batches.insert(
                        batch_id,
                        BatchCollector {
                            slots: (0..len).map(|_| BatchSlot::Pending).collect(),
                            remaining: len,
                        },
                    );
                    for (index, request) in items.into_iter().enumerate() {
                        pending.push_back(PendingJob {
                            request,
                            batch: Some(BatchMember { batch_id, index }),
                        });
                    }
                }
                FrameParse::Immediate(response) => {
                    if let Err(write_error) =
                        write_framed_response(standard_output, standard_error, &response)
                    {
                        let _ = writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                        exit_code = 1;
                        reader_done = true;
                        pending.clear();
                    }
                }
            },
            ServeEvent::OversizedFrame => {
                let oversized = error_response(
                    Value::Null,
                    JSON_RPC_INVALID_REQUEST,
                    "Invalid Request: frame exceeds maximum size",
                );
                let _ = write_framed_response(standard_output, standard_error, &oversized);
                exit_code = 1;
                reader_done = true;
                pending.clear();
            }
            ServeEvent::ReaderEof => {
                reader_done = true;
            }
            ServeEvent::ReaderError(message) => {
                let _ = writeln!(standard_error, "[keel mcp] read stdin: {message}");
                exit_code = 1;
                reader_done = true;
                pending.clear();
            }
            ServeEvent::Response { value, batch } => {
                in_flight = in_flight.saturating_sub(1);
                if let Some(member) = batch {
                    if let Some(finished) = complete_batch_slot(
                        &mut batches,
                        member,
                        BatchSlot::Response(value),
                    ) {
                        if let Err(write_error) =
                            write_framed_response(standard_output, standard_error, &finished)
                        {
                            let _ =
                                writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                            exit_code = 1;
                            reader_done = true;
                            pending.clear();
                        }
                    }
                } else if let Err(write_error) =
                    write_framed_response(standard_output, standard_error, &value)
                {
                    let _ = writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                    exit_code = 1;
                    reader_done = true;
                    pending.clear();
                }
            }
            ServeEvent::WorkerDone { batch } => {
                in_flight = in_flight.saturating_sub(1);
                if let Some(member) = batch {
                    if let Some(finished) =
                        complete_batch_slot(&mut batches, member, BatchSlot::Notification)
                    {
                        if let Err(write_error) =
                            write_framed_response(standard_output, standard_error, &finished)
                        {
                            let _ =
                                writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                            exit_code = 1;
                            reader_done = true;
                            pending.clear();
                        }
                    }
                }
            }
        }

        while in_flight < max_inflight {
            let Some(job) = pending.pop_front() else {
                break;
            };
            in_flight += 1;
            let worker_tx = event_tx.clone();
            let request_id = job.request.get("id").cloned().unwrap_or(Value::Null);
            let batch = job.batch;
            let request = job.request;
            let spawn_result = thread::Builder::new()
                .name("keel-mcp-req".into())
                .spawn(move || match dispatch(&request) {
                    Some(response) => {
                        let _ = worker_tx.send(ServeEvent::Response {
                            value: response,
                            batch,
                        });
                    }
                    None => {
                        let _ = worker_tx.send(ServeEvent::WorkerDone { batch });
                    }
                });
            if let Err(error) = spawn_result {
                in_flight = in_flight.saturating_sub(1);
                let _ = writeln!(standard_error, "[keel mcp] spawn request worker: {error}");
                let response = error_response(
                    request_id,
                    JSON_RPC_INTERNAL_ERROR,
                    &format!("failed to spawn request worker: {error}"),
                );
                if let Some(member) = batch {
                    if let Some(finished) = complete_batch_slot(
                        &mut batches,
                        member,
                        BatchSlot::Response(response),
                    ) {
                        if write_framed_response(standard_output, standard_error, &finished)
                            .is_err()
                        {
                            exit_code = 1;
                            reader_done = true;
                            pending.clear();
                            break;
                        }
                    }
                } else if write_framed_response(standard_output, standard_error, &response)
                    .is_err()
                {
                    exit_code = 1;
                    reader_done = true;
                    pending.clear();
                    break;
                }
            }
        }

        if reader_done && pending.is_empty() && in_flight == 0 && batches.is_empty() {
            break;
        }
    }

    exit_code
}

/// Record one finished batch element. When the last slot completes, remove the
/// collector and return the JSON-RPC batch response array (or `None` when the
/// batch was notifications-only — JSON-RPC §6: return nothing).
fn complete_batch_slot(
    batches: &mut HashMap<u64, BatchCollector>,
    member: BatchMember,
    slot: BatchSlot,
) -> Option<Value> {
    let collector = batches.get_mut(&member.batch_id)?;
    if member.index >= collector.slots.len() {
        return None;
    }
    if matches!(collector.slots[member.index], BatchSlot::Pending) {
        collector.slots[member.index] = slot;
        collector.remaining = collector.remaining.saturating_sub(1);
    }
    if collector.remaining > 0 {
        return None;
    }
    let finished = batches.remove(&member.batch_id)?;
    let responses: Vec<Value> = finished
        .slots
        .into_iter()
        .filter_map(|s| match s {
            BatchSlot::Response(value) => Some(value),
            BatchSlot::Notification | BatchSlot::Pending => None,
        })
        .collect();
    if responses.is_empty() {
        None
    } else {
        Some(Value::Array(responses))
    }
}

enum FrameParse {
    /// Single JSON-RPC object (normal stdio line).
    Single(Value),
    /// JSON-RPC batch array — response must be one array when complete.
    Batch(Vec<Value>),
    /// Parse/shape error that must be answered immediately without a worker.
    Immediate(Value),
}

/// Parse a single newline-delimited frame.
///
/// - Object → single request/notification (MCP 2025-11-25 stdio default).
/// - Array → JSON-RPC 2.0 batch (also older MCP 2025-03-26 receive-batch).
fn parse_frame(frame: &str) -> FrameParse {
    let parsed: Result<Value, serde_json::Error> = serde_json::from_str(frame);
    match parsed {
        Err(parse_error) => FrameParse::Immediate(error_response(
            Value::Null,
            JSON_RPC_PARSE_ERROR,
            &format!("Parse error: {parse_error}"),
        )),
        Ok(Value::Array(items)) => {
            if items.is_empty() {
                return FrameParse::Immediate(error_response(
                    Value::Null,
                    JSON_RPC_INVALID_REQUEST,
                    "Invalid Request: empty batch",
                ));
            }
            FrameParse::Batch(items)
        }
        Ok(other) => FrameParse::Single(other),
    }
}

/// Dispatch a JSON-RPC body (object or batch array) for HTTP / tests.
/// Returns framed outcomes without I/O.
pub(crate) fn dispatch_body(body: &Value) -> DispatchBodyResult {
    match body {
        Value::Array(items) if items.is_empty() => DispatchBodyResult::Json(error_response(
            Value::Null,
            JSON_RPC_INVALID_REQUEST,
            "Invalid Request: empty batch",
        )),
        Value::Array(items) => {
            let mut responses = Vec::new();
            for item in items {
                if let Some(response) = dispatch(item) {
                    responses.push(response);
                }
            }
            if responses.is_empty() {
                DispatchBodyResult::Accepted
            } else {
                DispatchBodyResult::Json(Value::Array(responses))
            }
        }
        other => match dispatch(other) {
            Some(response) => DispatchBodyResult::Json(response),
            None => DispatchBodyResult::Accepted,
        },
    }
}

pub(crate) enum DispatchBodyResult {
    /// JSON-RPC response object or batch array.
    Json(Value),
    /// Notification-only / no response body (HTTP 202).
    Accepted,
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
        // Test-only in-process delay — never ships in non-test binaries.
        // Used to prove concurrent workers without spawning OS hang children.
        #[cfg(test)]
        "keel/test_delay_ms" => {
            let ms = params
                .get("ms")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .min(500);
            thread::sleep(std::time::Duration::from_millis(ms));
            Ok(json!({ "slept_ms": ms }))
        }
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

pub(super) fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

pub(super) fn error_response(id: Value, code: i64, message: &str) -> Value {
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

    #[test]
    fn serve_stdio_handles_jsonrpc_batch_as_one_array_response() {
        let batch = serde_json::to_string(&json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "id": 2, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"}
        ]))
        .expect("serialize");
        let mut input_bytes = batch.into_bytes();
        input_bytes.push(b'\n');
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        let line = rendered
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("one batch response line");
        let parsed: Value = serde_json::from_str(line).expect("batch response is JSON");
        let arr = parsed.as_array().expect("JSON-RPC batch returns an array");
        assert_eq!(arr.len(), 2, "notification omitted; two pings remain: {line}");
        let ids: Vec<_> = arr.iter().map(|r| r.get("id").cloned()).collect();
        assert!(ids.contains(&Some(json!(1))), "line={line}");
        assert!(ids.contains(&Some(json!(2))), "line={line}");
    }

    #[test]
    fn serve_stdio_handles_many_inflight_pings_without_shell() {
        // why: prove multi-request scheduling without spawning OS children.
        // A previous wall-clock test used hanging `run_command` (ping/sleep) and
        // could freeze a developer machine under full suite load — do not restore it.
        let mut input_bytes = Vec::new();
        for id in 1u64..=32 {
            let request = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
            input_bytes.extend(serde_json::to_vec(&request).expect("serialize"));
            input_bytes.push(b'\n');
        }
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        let response_lines = rendered
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        assert_eq!(
            response_lines, 32,
            "expected 32 ping responses; got: {rendered}"
        );
        for id in 1u64..=32 {
            assert!(
                rendered.contains(&format!("\"id\":{id}")),
                "missing id {id} in {rendered}"
            );
        }
    }

    #[test]
    fn parse_frame_empty_batch_is_invalid_request() {
        match parse_frame("[]") {
            FrameParse::Immediate(response) => {
                assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_REQUEST));
            }
            FrameParse::Single(_) | FrameParse::Batch(_) => {
                panic!("empty batch must not schedule workers")
            }
        }
    }

    #[test]
    fn serve_stdio_concurrent_in_process_delays_share_wall_clock() {
        // why: prove workers overlap without OS hang children (user ban 2026-07-15).
        // Two 80ms in-process delays + one ping. Serial ≈ 160ms+; concurrent ≈ 80ms.
        let delay = |id: u64| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "keel/test_delay_ms",
                "params": { "ms": 80 }
            })
        };
        let ping = json!({"jsonrpc": "2.0", "id": 99, "method": "ping"});
        let mut input_bytes = Vec::new();
        for value in [delay(1), delay(2), ping] {
            input_bytes.extend(serde_json::to_vec(&value).expect("serialize"));
            input_bytes.push(b'\n');
        }
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        let started = std::time::Instant::now();
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        let elapsed = started.elapsed();
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("\"id\":1"), "{rendered}");
        assert!(rendered.contains("\"id\":2"), "{rendered}");
        assert!(rendered.contains("\"id\":99"), "{rendered}");
        // Ping must not wait behind both delays.
        let ping_pos = rendered.find("\"id\":99").expect("ping");
        let d1 = rendered.find("\"id\":1").expect("d1");
        let d2 = rendered.find("\"id\":2").expect("d2");
        assert!(
            ping_pos < d1 && ping_pos < d2,
            "ping should complete first; {rendered}"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "expected concurrent ~80ms, took {elapsed:?}; serial would be ≥160ms"
        );
    }

    #[test]
    fn dispatch_body_batch_returns_array() {
        let body = json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "id": 2, "method": "ping"}
        ]);
        match dispatch_body(&body) {
            DispatchBodyResult::Json(Value::Array(items)) => {
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected batch array"),
        }
    }
}
