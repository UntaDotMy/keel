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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::recall::recall_status_snapshot;
use crate::utility::workspace_index;

mod http;
mod tools;

pub(crate) fn tools_list_context_snapshot() -> (usize, usize) {
    let list = tools::handle_tools_list();
    let tool_count = list
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let serialized = serde_json::to_string(&list).unwrap_or_default();
    (
        tool_count,
        crate::proxy::token_meter::TokenMeter::count_text(&serialized),
    )
}

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
pub(super) const MCP_LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";
pub(super) const MCP_PREVIOUS_PROTOCOL_VERSION: &str = "2025-03-26";

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
pub(super) const JSON_RPC_INTERNAL_ERROR: i64 = -32603;

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
    let _ = writeln!(standard_output, "  keel mcp serve-http [--bind HOST:PORT]");
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
        "  Tool wall-clock: KEEL_MCP_TOOL_TIMEOUT_SECS (default 25, under typical host ~30s)."
    );
    let _ = writeln!(
        standard_output,
        "  Text size: KEEL_MCP_MAX_TEXT_CHARS (default 12000); stdio frame cap 24KB."
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
        cancellation_key: Option<String>,
        cancellation: Arc<AtomicBool>,
    },
    /// A worker finished a notification (no response). Frees an in-flight slot.
    WorkerDone {
        batch: Option<BatchMember>,
        cancellation_key: Option<String>,
        cancellation: Arc<AtomicBool>,
    },
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
    cancellation_key: Option<String>,
    cancellation: Arc<AtomicBool>,
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
fn serve_stdio_owned_stdin(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
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

/// Default idle self-reap budget for stdio `mcp serve`. A harness session that
/// drops without closing stdin leaves the server blocked on `recv()` forever,
/// holding the recall SQLite WAL and disk, which is the root cause of the
/// intermittent host tool timeouts. Idle alone is NOT an orphan, though: hosts
/// like Codex keep one server per session and go quiet between tool calls, so
/// the loop only exits after this budget when the SPAWNING harness process is
/// dead; with a live parent it resets the clock and keeps serving. Override
/// with `KEEL_MCP_IDLE_TIMEOUT_SECS` (min 30, max 86400; 0 disables).
const DEFAULT_MCP_IDLE_TIMEOUT_SECS: u64 = 300;

/// Resolve the idle self-reap budget. `0` disables (legacy never-exit behavior,
/// kept for hosts that hold a long-lived server intentionally). HTTP serve is
/// multi-client and does not use this; only the per-session stdio loop does.
pub(super) fn idle_timeout() -> Option<std::time::Duration> {
    let secs = env::var("KEEL_MCP_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_MCP_IDLE_TIMEOUT_SECS);
    if secs == 0 {
        return None;
    }
    Some(std::time::Duration::from_secs(secs.clamp(30, 86_400)))
}

/// Snapshot of the process that spawned this `mcp serve`, used to distinguish
/// an abandoned session (parent dead → reap) from a quiet-but-live one (parent
/// alive → keep serving). Captured once at startup; `alive()` re-probes the OS.
/// `None` (probe failed) is treated as alive: killing a live session's
/// transport is far worse than the rare unreaped orphan, which `doctor --fix`
/// still sweeps.
struct ParentWatch {
    ppid: u32,
}

impl ParentWatch {
    #[cfg(windows)]
    fn capture() -> Option<Self> {
        crate::runtime::parent_process_id(std::process::id()).map(|ppid| Self { ppid })
    }

    #[cfg(unix)]
    fn capture() -> Option<Self> {
        extern "C" {
            fn getppid() -> i32;
        }
        let ppid = unsafe { getppid() };
        if ppid <= 0 {
            return None;
        }
        Some(Self { ppid: ppid as u32 })
    }

    #[cfg(windows)]
    fn alive(&self) -> bool {
        crate::runtime::process_is_alive(self.ppid).unwrap_or(true)
    }

    #[cfg(unix)]
    fn alive(&self) -> bool {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // sig 0 = existence/permission check only.
        unsafe { kill(self.ppid as i32, 0) == 0 }
    }
}

/// Idle expiry only reaps when the harness that spawned this server is gone.
/// A live parent means the session is merely between tool calls (Codex keeps
/// one stdio server per session for hours), so the transport must stay up.
fn idle_reap_parent_gone(
    parent: &Option<ParentWatch>,
    standard_error: &mut dyn Write,
    budget: std::time::Duration,
) -> bool {
    let gone = !parent.as_ref().map(ParentWatch::alive).unwrap_or(true);
    if gone {
        let _ = writeln!(
            standard_error,
            "[keel mcp] idle for {}s and the spawning process is gone; self-reaping orphan (set KEEL_MCP_IDLE_TIMEOUT_SECS=0 to disable)",
            budget.as_secs()
        );
    }
    gone
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
        if event_tx
            .send(ServeEvent::Frame(trimmed.to_string()))
            .is_err()
        {
            return;
        }
    }
}

pub(super) fn cancellation_key(request_id: &Value) -> Option<String> {
    match request_id {
        Value::Null | Value::String(_) | Value::Number(_) => serde_json::to_string(request_id).ok(),
        _ => None,
    }
}

fn new_pending_job(
    request: Value,
    batch: Option<BatchMember>,
    cancellations: &mut HashMap<String, Arc<AtomicBool>>,
) -> PendingJob {
    let key = request.get("id").and_then(cancellation_key);
    let cancellation = Arc::new(AtomicBool::new(false));
    if let Some(key) = key.as_ref() {
        cancellations.insert(key.clone(), Arc::clone(&cancellation));
    }
    PendingJob {
        request,
        batch,
        cancellation_key: key,
        cancellation,
    }
}

fn apply_cancellation_notifications(frame: &str, cancellations: &HashMap<String, Arc<AtomicBool>>) {
    let Ok(value) = serde_json::from_str::<Value>(frame) else {
        return;
    };
    let messages: Vec<&Value> = match &value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for message in messages {
        if message.get("method").and_then(Value::as_str) != Some("notifications/cancelled") {
            continue;
        }
        let Some(key) = message
            .get("params")
            .and_then(|params| params.get("requestId"))
            .and_then(cancellation_key)
        else {
            continue;
        };
        if let Some(cancellation) = cancellations.get(&key) {
            cancellation.store(true, Ordering::Release);
        }
    }
}

fn remove_cancellation_registration(
    cancellations: &mut HashMap<String, Arc<AtomicBool>>,
    key: Option<&str>,
    cancellation: &Arc<AtomicBool>,
) {
    let Some(key) = key else {
        return;
    };
    if cancellations
        .get(key)
        .map(|registered| Arc::ptr_eq(registered, cancellation))
        .unwrap_or(false)
    {
        cancellations.remove(key);
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
    let mut cancellations: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    let mut next_batch_id: u64 = 1;
    let mut reader_done = false;
    let mut exit_code: u8 = 0;
    let idle_budget = idle_timeout();
    let parent = ParentWatch::capture();
    let mut last_frame_at = std::time::Instant::now();

    loop {
        // why: block only up to the remaining idle budget so an abandoned session
        // (orphan) exits instead of holding the recall WAL; frames reset the clock.
        let event = match idle_budget {
            Some(budget) => {
                let elapsed = last_frame_at.elapsed();
                if elapsed >= budget && in_flight == 0 && pending.is_empty() {
                    if idle_reap_parent_gone(&parent, standard_error, budget) {
                        break;
                    }
                    // Parent alive: the session is merely quiet — keep serving.
                    last_frame_at = std::time::Instant::now();
                }
                let remaining = budget
                    .saturating_sub(elapsed)
                    .max(std::time::Duration::from_millis(50));
                match event_rx.recv_timeout(remaining) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if in_flight == 0
                            && pending.is_empty()
                            && idle_reap_parent_gone(&parent, standard_error, budget)
                        {
                            break;
                        }
                        if in_flight == 0 && pending.is_empty() {
                            last_frame_at = std::time::Instant::now();
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match event_rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            },
        };

        match event {
            ServeEvent::Frame(frame) => {
                last_frame_at = std::time::Instant::now();
                apply_cancellation_notifications(&frame, &cancellations);
                match parse_frame(&frame) {
                    FrameParse::Single(request) => {
                        pending.push_back(new_pending_job(request, None, &mut cancellations));
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
                            pending.push_back(new_pending_job(
                                request,
                                Some(BatchMember { batch_id, index }),
                                &mut cancellations,
                            ));
                        }
                    }
                    FrameParse::Immediate(response) => {
                        if let Err(write_error) =
                            write_framed_response(standard_output, standard_error, &response)
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
            ServeEvent::Response {
                value,
                batch,
                cancellation_key,
                cancellation,
            } => {
                in_flight = in_flight.saturating_sub(1);
                remove_cancellation_registration(
                    &mut cancellations,
                    cancellation_key.as_deref(),
                    &cancellation,
                );
                if cancellation.load(Ordering::Acquire) {
                    if let Some(member) = batch {
                        if let Some(finished) =
                            complete_batch_slot(&mut batches, member, BatchSlot::Notification)
                        {
                            let _ =
                                write_framed_response(standard_output, standard_error, &finished);
                        }
                    }
                } else if let Some(member) = batch {
                    if let Some(finished) =
                        complete_batch_slot(&mut batches, member, BatchSlot::Response(value))
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
                } else if let Err(write_error) =
                    write_framed_response(standard_output, standard_error, &value)
                {
                    let _ = writeln!(standard_error, "[keel mcp] write stdout: {write_error}");
                    exit_code = 1;
                    reader_done = true;
                    pending.clear();
                }
            }
            ServeEvent::WorkerDone {
                batch,
                cancellation_key,
                cancellation,
            } => {
                in_flight = in_flight.saturating_sub(1);
                remove_cancellation_registration(
                    &mut cancellations,
                    cancellation_key.as_deref(),
                    &cancellation,
                );
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
            let cancellation_key = job.cancellation_key;
            let cancellation = job.cancellation;
            let worker_cancellation_key = cancellation_key.clone();
            let worker_cancellation = Arc::clone(&cancellation);
            let request = job.request;
            let spawn_result =
                thread::Builder::new()
                    .name("keel-mcp-req".into())
                    .spawn(
                        move || match dispatch_cancellable(&request, &worker_cancellation) {
                            Some(response) => {
                                let _ = worker_tx.send(ServeEvent::Response {
                                    value: response,
                                    batch,
                                    cancellation_key: worker_cancellation_key,
                                    cancellation: worker_cancellation,
                                });
                            }
                            None => {
                                let _ = worker_tx.send(ServeEvent::WorkerDone {
                                    batch,
                                    cancellation_key: worker_cancellation_key,
                                    cancellation: worker_cancellation,
                                });
                            }
                        },
                    );
            if let Err(error) = spawn_result {
                in_flight = in_flight.saturating_sub(1);
                remove_cancellation_registration(
                    &mut cancellations,
                    cancellation_key.as_deref(),
                    &cancellation,
                );
                let _ = writeln!(standard_error, "[keel mcp] spawn request worker: {error}");
                let response = error_response(
                    request_id,
                    JSON_RPC_INTERNAL_ERROR,
                    &format!("failed to spawn request worker: {error}"),
                );
                if let Some(member) = batch {
                    if let Some(finished) =
                        complete_batch_slot(&mut batches, member, BatchSlot::Response(response))
                    {
                        if write_framed_response(standard_output, standard_error, &finished)
                            .is_err()
                        {
                            exit_code = 1;
                            reader_done = true;
                            pending.clear();
                            break;
                        }
                    }
                } else if write_framed_response(standard_output, standard_error, &response).is_err()
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
#[cfg(test)]
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

/// Soft ceiling for one newline-delimited JSON-RPC response frame on stdio.
/// Hosts (notably Grok) have been observed to drop/desync on very large single
/// frames and then wait out the full tool timeout. Prefer truncating tool text
/// earlier; this is a last-resort guard so the peer always sees a parseable line.
const MAX_STDIO_FRAME_BYTES: usize = 24_000;

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
    // NDJSON framing: the frame body must not contain raw newlines. serde_json
    // escapes them; if that ever changes, fail closed with a short error frame
    // rather than desyncing the host's line reader for every subsequent call.
    let id = response.get("id").cloned().unwrap_or(Value::Null);
    let fallback = |value: Value| -> String {
        match serde_json::to_string(&value) {
            Ok(text)
                if !text.as_bytes().contains(&b'\n')
                    && !text.as_bytes().contains(&b'\r')
                    && text.len() <= MAX_STDIO_FRAME_BYTES =>
            {
                text
            }
            _ => {
                "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"internal framing error\"}}".to_string()
            }
        }
    };
    let frame = if serialized.as_bytes().contains(&b'\n') || serialized.as_bytes().contains(&b'\r')
    {
        let _ = writeln!(
            standard_error,
            "[keel mcp] refusing frame with interior newline ({} bytes)",
            serialized.len()
        );
        fallback(error_response(
            id,
            JSON_RPC_INTERNAL_ERROR,
            "internal error: response frame contained a raw newline",
        ))
    } else if serialized.len() > MAX_STDIO_FRAME_BYTES {
        let _ = writeln!(
            standard_error,
            "[keel mcp] response frame {} bytes exceeds {MAX_STDIO_FRAME_BYTES}; returning error frame",
            serialized.len()
        );
        // Prefer a tools/call-shaped error so hosts complete the pending call
        // instead of waiting out a full tool timeout on a dropped frame.
        if response.get("result").is_some() {
            fallback(success_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "MCP response truncated: frame was {} bytes (limit {MAX_STDIO_FRAME_BYTES}). Prefer skill_route / narrower tools, or CLI for full output.",
                            serialized.len()
                        )
                    }],
                    "isError": true,
                }),
            ))
        } else {
            fallback(error_response(
                id,
                JSON_RPC_INTERNAL_ERROR,
                &format!(
                    "response frame too large ({} bytes; limit {MAX_STDIO_FRAME_BYTES})",
                    serialized.len()
                ),
            ))
        }
    } else {
        serialized
    };
    standard_output.write_all(frame.as_bytes())?;
    standard_output.write_all(b"\n")?;
    standard_output.flush()
}

/// Dispatch a single parsed JSON-RPC request. Returns `Some(response)` for
/// requests (objects with an `id`) and `None` for notifications. Tests drive
/// this function directly to avoid spawning the binary; the stdio loop also
/// uses it after framing.
#[cfg(test)]
pub fn dispatch(request: &Value) -> Option<Value> {
    dispatch_cancellable(request, &Arc::new(AtomicBool::new(false)))
}

pub(super) fn dispatch_cancellable(
    request: &Value,
    cancellation: &Arc<AtomicBool>,
) -> Option<Value> {
    if cancellation.load(Ordering::Acquire) {
        return None;
    }
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
        let _ = handle_method_cancellable(&method, &params, cancellation);
        return None;
    }

    let request_id = id.unwrap_or(Value::Null);
    Some(
        match handle_method_cancellable(&method, &params, cancellation) {
            Ok(result) => success_response(request_id, result),
            Err(MethodError { code, message }) => error_response(request_id, code, &message),
        },
    )
}

/// Dispatcher for a single MCP method. Kept method-keyed (rather than
/// argument-keyed) so the protocol surface is greppable and each handler
/// stays small.
fn handle_method_cancellable(
    method: &str,
    params: &Value,
    cancellation: &Arc<AtomicBool>,
) -> Result<Value, MethodError> {
    match method {
        "initialize" => Ok(handle_initialize(params)),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::handle_tools_list()),
        "tools/call" => {
            tools::handle_tools_call_cancellable(params, Some(Arc::clone(cancellation)))
        }
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
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < deadline {
                if cancellation.load(Ordering::Acquire) {
                    return Ok(Value::Null);
                }
                thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(json!({ "slept_ms": ms }))
        }
        other => Err(MethodError {
            code: JSON_RPC_METHOD_NOT_FOUND,
            message: format!("Method not found: {other}"),
        }),
    }
}

fn handle_initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let negotiated = match requested {
        Some(version)
            if matches!(
                version,
                MCP_LEGACY_PROTOCOL_VERSION | MCP_PREVIOUS_PROTOCOL_VERSION | MCP_PROTOCOL_VERSION
            ) =>
        {
            version
        }
        _ => MCP_PROTOCOL_VERSION,
    };
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
                "description": "Indexed workspace map generated from the persistent code-index generation and commit evidence.",
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
            // why: Grok auto-reads this resource on session start. A live
            // walk of a large tree, or an untruncated 16KB+ frame, makes the
            // host drop the line and wait out 30–60s. Deadline + truncate.
            let text = match tools::run_tool_with_deadline(
                tools::mcp_child_timeout(),
                "resources/read system_map",
                || system_map_text(None),
            ) {
                Ok(text) => tools::truncate_mcp_text(&text),
                Err(message) => message,
            };
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
            let text = match tools::run_tool_with_deadline(
                tools::mcp_child_timeout(),
                "resources/read recall_status",
                || {
                    let payload = recall_status_payload()?;
                    // Compact JSON: pretty multi-line resource text bloats the stdio frame.
                    serde_json::to_string(&payload)
                        .map_err(|error| format!("serialize recall status: {error}"))
                },
            ) {
                Ok(text) => tools::truncate_mcp_text(&text),
                Err(message) => message,
            };
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

/// Resolve the indexed workspace map for the `system_map` tool and resource.
/// Index refresh failures are returned directly; no stale text or filesystem
/// fallback is permitted.
pub(super) fn system_map_text(workspace_override: Option<&Path>) -> Result<String, String> {
    let workspace_root = match workspace_override {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?,
    };
    let claude_home = resolve_claude_home("")?;
    workspace_index::render_map(&workspace_root, &claude_home.to_string_lossy())
}

/// Build the recall index health snapshot payload. Shared by the
/// `recall_status` / `memory_status` tools and the `keel://recall/status`
/// resource so all surfaces report the same shape.
pub(super) fn recall_status_payload() -> Result<Value, String> {
    let claude_home = resolve_claude_home("")?;
    let snapshot = recall_status_snapshot(&claude_home)?;
    let payload = json!({
        "claudeHome": display_path(&snapshot.claude_home),
        "indexPath": display_path(&snapshot.index_path),
        "schemaVersion": snapshot.schema_version,
        "documents": snapshot.document_count,
        "lastIndexedAtMillis": snapshot.last_indexed_at_millis.to_string(),
        "addedSinceLastSync": snapshot.added_since_last_sync,
        "updatedSinceLastSync": snapshot.updated_since_last_sync,
        "removedSinceLastSync": snapshot.removed_since_last_sync,
    });
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
    fn idle_timeout_defaults_to_bounded_reap_window() {
        // why: the default must be a real budget so orphans self-reap, and it
        // must exceed any single tool deadline so an active call is not idle.
        if std::env::var_os("KEEL_MCP_IDLE_TIMEOUT_SECS").is_none() {
            let budget = idle_timeout().expect("default idle reap enabled");
            assert!(budget.as_secs() >= 60, "idle window too tight: {budget:?}");
            assert!(
                budget.as_secs() > crate::mcp::tools::DEFAULT_MCP_CHILD_TIMEOUT_SECS_FOR_TEST,
                "idle window must exceed the per-tool deadline"
            );
        }
    }

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
            "anvil",
            "review",
            "git_workflow",
            "memory",
            "gain",
            "raw",
            "config_audit",
            "skill_lint",
            "telemetry",
            "session",
            "doctor",
            "code_search",
            "flow",
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
        assert_eq!(
            arr.len(),
            2,
            "notification omitted; two pings remain: {line}"
        );
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
        // Load-proof by COMPLETION ORDER, not wall clock (fixed ceilings flake on
        // saturated CI/dev boxes): a 300ms delay, an 80ms delay, and a ping.
        // Serial FIFO execution renders d1, d2, ping; concurrent workers render
        // ping, d2, d1. Asserting that order proves overlap regardless of how
        // slow the machine is.
        let delay = |id: u64, ms: u64| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "keel/test_delay_ms",
                "params": { "ms": ms }
            })
        };
        let ping = json!({"jsonrpc": "2.0", "id": 99, "method": "ping"});
        let mut input_bytes = Vec::new();
        for value in [delay(1, 300), delay(2, 80), ping] {
            input_bytes.extend(serde_json::to_vec(&value).expect("serialize"));
            input_bytes.push(b'\n');
        }
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        let exit = serve_stdio(&mut input, &mut output, &mut error_output);
        assert_eq!(exit, 0);
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("\"id\":1"), "{rendered}");
        assert!(rendered.contains("\"id\":2"), "{rendered}");
        assert!(rendered.contains("\"id\":99"), "{rendered}");
        let ping_pos = rendered.find("\"id\":99").expect("ping");
        let d1 = rendered.find("\"id\":1").expect("d1");
        let d2 = rendered.find("\"id\":2").expect("d2");
        assert!(
            ping_pos < d2 && d2 < d1,
            "fast jobs must overtake the slow one (ping, then 80ms, then 300ms); \
             serial FIFO would render d1 first: {rendered}"
        );
    }

    #[test]
    fn cancellation_notification_suppresses_and_stops_inflight_request() {
        let mut input_bytes = Vec::new();
        for value in [
            json!({
                "jsonrpc": "2.0",
                "id": "slow-request",
                "method": "keel/test_delay_ms",
                "params": { "ms": 500 }
            }),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": "slow-request", "reason": "test" }
            }),
            json!({"jsonrpc": "2.0", "id": "still-live", "method": "ping"}),
        ] {
            input_bytes.extend(serde_json::to_vec(&value).expect("serialize"));
            input_bytes.push(b'\n');
        }
        let mut input: &[u8] = &input_bytes;
        let mut output = Vec::new();
        let mut error_output = Vec::new();

        assert_eq!(serve_stdio(&mut input, &mut output, &mut error_output), 0);

        let rendered = String::from_utf8_lossy(&output);
        assert!(!rendered.contains("slow-request"), "{rendered}");
        assert!(rendered.contains("still-live"), "{rendered}");
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

    #[test]
    fn tools_list_framed_response_under_stdio_ceiling_with_headroom() {
        // Drive real dispatch → handle_tools_list (slimmed). Pins the wire
        // budget so discovery cannot fill the 24KB frame and trip hosts.
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list"
        });
        let response = dispatch(&request).expect("response present");
        let serialized = serde_json::to_string(&response).expect("serialize");
        assert!(
            serialized.len() <= MAX_STDIO_FRAME_BYTES,
            "framed tools/list {} exceeds {}",
            serialized.len(),
            MAX_STDIO_FRAME_BYTES
        );
        assert!(
            serialized.len() <= MAX_STDIO_FRAME_BYTES.saturating_sub(4_000),
            "framed tools/list {} needs ≥4KB headroom under {}",
            serialized.len(),
            MAX_STDIO_FRAME_BYTES
        );
        assert!(
            !serialized.contains('\n'),
            "tools/list must be one NDJSON line"
        );
    }

    #[test]
    fn write_framed_response_replaces_oversized_result_with_is_error() {
        // Oversized tools/call-shaped result must become isError text, not a
        // silent drop that leaves the host waiting out its full timeout.
        let mut huge_text = String::from("pad-");
        while huge_text.len() < MAX_STDIO_FRAME_BYTES + 2_000 {
            huge_text.push('x');
        }
        let response = success_response(
            json!(42),
            json!({
                "content": [{ "type": "text", "text": huge_text }],
                "isError": false,
            }),
        );
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        write_framed_response(&mut output, &mut error_output, &response).expect("write");
        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.ends_with('\n'));
        let line = rendered.trim_end();
        assert!(
            line.len() <= MAX_STDIO_FRAME_BYTES,
            "fallback frame {} must fit ceiling",
            line.len()
        );
        let parsed: Value = serde_json::from_str(line).expect("json");
        assert_eq!(parsed["id"], json!(42));
        assert_eq!(parsed["result"]["isError"], json!(true));
        let text = parsed["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            text.contains("truncated") || text.contains("frame"),
            "text={text}"
        );
    }
}
