//! Purpose: Streamable HTTP transport for the keel MCP server — multi-client
//!   concurrent connections on one process (MCP 2025-11-25 transports).
//! Caller: `run_mcp_command` `serve-http` arm.
//! Dependencies: std::net only (no async runtime); reuses `super::dispatch` /
//!   `dispatch_body` for JSON-RPC semantics.
//! Side Effects: Binds a TCP listener (default 127.0.0.1:3920), accepts bounded
//!   concurrent clients, writes responses, and tracks sessions/cancellation.

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{
    DispatchBodyResult, JSON_RPC_INTERNAL_ERROR, JSON_RPC_INVALID_REQUEST, JSON_RPC_PARSE_ERROR,
};

const DEFAULT_BIND: &str = "127.0.0.1:3920";
const MAX_HTTP_BODY: usize = 8 * 1024 * 1024;
const MAX_HTTP_HEADER: usize = 16 * 1024;
const MAX_HTTP_BATCH_ITEMS: usize = 64;
const HTTP_INFLIGHT_WAIT: Duration = Duration::from_secs(2);
const HTTP_BATCH_WALL_BUDGET: Duration = Duration::from_secs(30);

/// `keel mcp serve-http [--bind HOST:PORT]`
pub(super) fn serve_http(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let bind = parse_bind(arguments).unwrap_or_else(|| DEFAULT_BIND.to_string());
    let addr: SocketAddr = match bind.parse() {
        Ok(addr) => addr,
        Err(error) => {
            let _ = writeln!(standard_error, "serve-http: invalid --bind {bind}: {error}");
            return 1;
        }
    };
    if !addr.ip().is_loopback() && !allow_remote_bind() {
        let _ = writeln!(
            standard_error,
            "serve-http: refusing non-loopback bind {addr} (set KEEL_MCP_HTTP_ALLOW_REMOTE=1 to override)"
        );
        return 1;
    }
    if allow_remote_bind() && configured_auth_token().is_none() {
        let _ = writeln!(
            standard_error,
            "serve-http: KEEL_MCP_HTTP_AUTH_TOKEN is required when remote HTTP is enabled"
        );
        return 1;
    }

    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = writeln!(standard_error, "serve-http: bind {addr}: {error}");
            return 1;
        }
    };
    let _ = writeln!(
        standard_output,
        "keel mcp serve-http listening on http://{addr}/mcp (Streamable HTTP; multi-client)"
    );
    let _ = standard_output.flush();

    let state = Arc::new(HttpState::default());
    // Mirror the stdio loop's KEEL_MCP_MAX_INFLIGHT contract (mod.rs):
    // bound concurrent in-flight request handling on HTTP too.
    let max_inflight = super::max_inflight();
    let inflight = Arc::new(InflightGuard::new(max_inflight));
    let connections = Arc::new(InflightGuard::new(max_inflight.saturating_add(8)));

    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                let Some(connection_permit) = connections.try_acquire() else {
                    let _ = write_http(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        None,
                        b"server busy",
                    );
                    continue;
                };
                let state = Arc::clone(&state);
                let inflight = Arc::clone(&inflight);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
                let spawn = thread::Builder::new()
                    .name("keel-mcp-http".into())
                    .spawn(move || {
                        let _connection_permit = connection_permit;
                        if let Err(error) = handle_connection(stream, state, inflight) {
                            // Connection-level errors stay local; no shared stderr.
                            let _ = error;
                        }
                    });
                if let Err(error) = spawn {
                    let _ = writeln!(standard_error, "serve-http: spawn worker: {error}");
                }
            }
            Err(error) => {
                let _ = writeln!(standard_error, "serve-http: accept: {error}");
            }
        }
    }
    0
}

fn parse_bind(arguments: &[String]) -> Option<String> {
    let mut i = 0;
    while i < arguments.len() {
        if arguments[i] == "--bind" {
            return arguments.get(i + 1).cloned();
        }
        if let Some(value) = arguments[i].strip_prefix("--bind=") {
            return Some(value.to_string());
        }
        i += 1;
    }
    None
}

fn allow_remote_bind() -> bool {
    matches!(
        env::var("KEEL_MCP_HTTP_ALLOW_REMOTE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}
fn configured_auth_token() -> Option<String> {
    env::var("KEEL_MCP_HTTP_AUTH_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn authorization_header_matches(header: Option<&str>, expected: &str) -> bool {
    let Some(provided) = header
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
    else {
        return false;
    };
    if provided.is_empty() || expected.is_empty() {
        return false;
    }
    let provided_bytes = provided.as_bytes();
    let expected_bytes = expected.as_bytes();
    let mut difference = (provided_bytes.len() ^ expected_bytes.len()) as u8;
    for index in 0..provided_bytes.len().max(expected_bytes.len()) {
        difference |= provided_bytes.get(index).copied().unwrap_or_default()
            ^ expected_bytes.get(index).copied().unwrap_or_default();
    }
    difference == 0
}

fn remote_http_authorized(header: Option<&str>) -> bool {
    configured_auth_token()
        .map(|expected| authorization_header_matches(header, &expected))
        .unwrap_or(false)
}

#[derive(Default)]
struct HttpState {
    sessions: Mutex<HashSet<String>>,
    cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

fn handle_connection(
    mut stream: TcpStream,
    state: Arc<HttpState>,
    inflight: Arc<InflightGuard>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if find_header_end(&buffer).is_none() && buffer.len() > MAX_HTTP_HEADER {
            write_http(
                &mut stream,
                431,
                "text/plain; charset=utf-8",
                None,
                b"request headers too large",
            )?;
            return Ok(());
        }
        if let Some(header_end) = find_header_end(&buffer) {
            if header_end > MAX_HTTP_HEADER {
                write_http(
                    &mut stream,
                    431,
                    "text/plain; charset=utf-8",
                    None,
                    b"request headers too large",
                )?;
                return Ok(());
            }
            let header_text = String::from_utf8_lossy(&buffer[..header_end]);
            let headers = parse_headers(&header_text);
            if !headers.content_length_valid {
                write_http(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    None,
                    b"invalid Content-Length",
                )?;
                return Ok(());
            }
            let content_length = headers.content_length.unwrap_or(0);
            if content_length > MAX_HTTP_BODY {
                write_http(
                    &mut stream,
                    413,
                    "text/plain; charset=utf-8",
                    None,
                    b"payload too large",
                )?;
                return Ok(());
            }
            let total_needed = header_end + content_length;
            while buffer.len() < total_needed {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() > total_needed {
                    break;
                }
            }
            if buffer.len() < total_needed {
                write_http(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    None,
                    b"incomplete request body",
                )?;
                return Ok(());
            }
            let body = if content_length == 0 {
                Vec::new()
            } else {
                buffer
                    .get(header_end..header_end + content_length)
                    .unwrap_or(&[])
                    .to_vec()
            };
            // Bound in-flight request handling; the permit releases on drop
            // when this connection's handler returns.
            if body_contains_only_cancellation_notifications(&body) {
                respond(&mut stream, &headers, &body, &state)?;
                return Ok(());
            }
            let Some(_permit) = inflight.acquire_timeout(HTTP_INFLIGHT_WAIT) else {
                write_http(
                    &mut stream,
                    503,
                    "text/plain; charset=utf-8",
                    None,
                    b"server busy",
                )?;
                return Ok(());
            };
            respond(&mut stream, &headers, &body, &state)?;
            return Ok(());
        }
    }
    Ok(())
}

/// Bounds concurrent in-flight request handling on the HTTP transport,
/// mirroring the stdio loop's `KEEL_MCP_MAX_INFLIGHT` contract (mod.rs,
/// default 64). Over-cap waiters park on the condvar until a slot frees;
/// connection socket timeouts bound how long a waiter can stay parked.
struct InflightGuard {
    capacity: usize,
    in_flight: Mutex<usize>,
    slot_freed: Condvar,
}

impl InflightGuard {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            in_flight: Mutex::new(0),
            slot_freed: Condvar::new(),
        }
    }

    /// Block until an in-flight slot is free, then take it. The returned
    /// permit releases the slot on drop and wakes one waiter.
    #[cfg(test)]
    fn acquire(self: &Arc<Self>) -> InflightPermit {
        let mut current = self.lock();
        while *current >= self.capacity {
            current = self
                .slot_freed
                .wait(current)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *current += 1;
        InflightPermit {
            guard: Arc::clone(self),
        }
    }

    fn acquire_timeout(self: &Arc<Self>, timeout: Duration) -> Option<InflightPermit> {
        let deadline = Instant::now() + timeout;
        let mut current = self.lock();
        while *current >= self.capacity {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let waited = self
                .slot_freed
                .wait_timeout(current, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            current = waited.0;
            if waited.1.timed_out() && *current >= self.capacity {
                return None;
            }
        }
        *current += 1;
        Some(InflightPermit {
            guard: Arc::clone(self),
        })
    }

    fn try_acquire(self: &Arc<Self>) -> Option<InflightPermit> {
        let mut current = self.lock();
        if *current >= self.capacity {
            return None;
        }
        *current += 1;
        Some(InflightPermit {
            guard: Arc::clone(self),
        })
    }

    /// Test observation only; production paths rely on acquire/permit drop.
    #[cfg(test)]
    fn in_flight(&self) -> usize {
        *self.lock()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, usize> {
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct InflightPermit {
    guard: Arc<InflightGuard>,
}

impl Drop for InflightPermit {
    fn drop(&mut self) {
        let mut current = self.guard.lock();
        *current -= 1;
        self.guard.slot_freed.notify_one();
    }
}

struct HttpHeaders {
    method: String,
    path: String,
    origin: Option<String>,
    content_length: Option<usize>,
    content_length_valid: bool,
    accept: String,
    authorization: Option<String>,
    protocol_version: Option<String>,
    session_id: Option<String>,
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

fn parse_headers(text: &str) -> HttpHeaders {
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut origin = None;
    let mut content_length = None;
    let mut content_length_valid = true;
    let mut accept = String::new();
    let mut authorization = None;
    let mut protocol_version = None;
    let mut session_id = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "origin" => origin = Some(value.to_string()),
                "content-length" => {
                    if content_length.is_some() {
                        content_length_valid = false;
                    } else {
                        match value.parse() {
                            Ok(parsed) => content_length = Some(parsed),
                            Err(_) => content_length_valid = false,
                        }
                    }
                }
                "accept" => accept = value.to_string(),
                "authorization" => authorization = Some(value.to_string()),
                "mcp-protocol-version" => protocol_version = Some(value.to_string()),
                "mcp-session-id" => session_id = Some(value.to_string()),
                _ => {}
            }
        }
    }
    HttpHeaders {
        method,
        path,
        origin,
        content_length,
        content_length_valid,
        accept,
        authorization,
        protocol_version,
        session_id,
    }
}

fn accepts_streamable_http(accept: &str) -> bool {
    let mut accepts_json = false;
    let mut accepts_sse = false;
    for media_type in accept.split(',') {
        match media_type.trim().split(';').next().unwrap_or("").trim() {
            "application/json" => accepts_json = true,
            "text/event-stream" => accepts_sse = true,
            _ => {}
        }
    }
    accepts_json && accepts_sse
}

fn supported_http_protocol_version(version: &str) -> bool {
    matches!(version, "2025-03-26" | "2025-11-25")
}

fn exact_origin_host(origin: &str) -> Option<String> {
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    if authority.is_empty()
        || authority
            .chars()
            .any(|character| matches!(character, '/' | '?' | '#' | '@'))
    {
        return None;
    }

    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let end = bracketed.find(']')?;
        let host = &bracketed[..end];
        let suffix = &bracketed[end + 1..];
        if !suffix.is_empty() {
            let port = suffix.strip_prefix(':')?;
            if port.is_empty() || port.parse::<u16>().is_err() {
                return None;
            }
        }
        host
    } else {
        if authority.matches(':').count() > 1 {
            return None;
        }
        match authority.rsplit_once(':') {
            Some((host, port)) => {
                if port.is_empty() || port.parse::<u16>().is_err() {
                    return None;
                }
                host
            }
            None => authority,
        }
    };

    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn local_origin_allowed(origin: &str) -> bool {
    matches!(
        exact_origin_host(origin).as_deref(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    )
}

fn remote_origin_allowed(origin: &str) -> bool {
    if !allow_remote_bind() {
        return false;
    }
    env::var("KEEL_MCP_HTTP_ALLOWED_ORIGINS")
        .ok()
        .map(|allowed| allowed.split(',').any(|value| value.trim() == origin))
        .unwrap_or(false)
}

fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some("null") => true,
        Some(value) if local_origin_allowed(value) => true,
        Some(value) if remote_origin_allowed(value) => true,
        Some(_) => false,
    }
}

fn body_contains_only_cancellation_notifications(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let messages: Vec<&Value> = match &value {
        Value::Array(items) if !items.is_empty() => items.iter().collect(),
        Value::Array(_) => return false,
        other => vec![other],
    };
    messages.iter().all(|message| {
        message.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
            && message.get("id").is_none()
    })
}

fn respond(
    stream: &mut TcpStream,
    headers: &HttpHeaders,
    body: &[u8],
    state: &Arc<HttpState>,
) -> std::io::Result<()> {
    if allow_remote_bind() && !remote_http_authorized(headers.authorization.as_deref()) {
        return write_http(
            stream,
            401,
            "text/plain; charset=utf-8",
            None,
            b"Unauthorized",
        );
    }
    if !origin_allowed(headers.origin.as_deref()) {
        let err = json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": -32000, "message": "Origin not allowed" }
        });
        let bytes = serde_json::to_vec(&err).unwrap_or_default();
        return write_http(stream, 403, "application/json", None, &bytes);
    }

    let path = headers.path.split('?').next().unwrap_or("/");
    if path != "/mcp" && path != "/mcp/" {
        return write_http(stream, 404, "text/plain; charset=utf-8", None, b"not found");
    }

    match headers.method.to_ascii_uppercase().as_str() {
        "GET" => {
            // Optional SSE listen; we offer a minimal open stream then close.
            if headers.accept.contains("text/event-stream") {
                let priming = "id: 0\ndata: \n\n";
                return write_http(
                    stream,
                    200,
                    "text/event-stream",
                    headers.session_id.as_deref(),
                    priming.as_bytes(),
                );
            }
            write_http(
                stream,
                405,
                "text/plain; charset=utf-8",
                None,
                b"Method Not Allowed",
            )
        }
        "DELETE" => {
            if let Some(id) = headers.session_id.as_ref() {
                if let Ok(mut guard) = state.sessions.lock() {
                    guard.remove(id);
                }
            }
            write_http(stream, 200, "text/plain; charset=utf-8", None, b"")
        }
        "POST" => handle_post(stream, headers, body, state),
        _ => write_http(
            stream,
            405,
            "text/plain; charset=utf-8",
            None,
            b"Method Not Allowed",
        ),
    }
}

fn handle_post(
    stream: &mut TcpStream,
    headers: &HttpHeaders,
    body: &[u8],
    state: &Arc<HttpState>,
) -> std::io::Result<()> {
    if !accepts_streamable_http(&headers.accept) {
        return write_http(
            stream,
            406,
            "text/plain; charset=utf-8",
            None,
            b"Accept must include application/json and text/event-stream",
        );
    }
    if let Some(version) = headers.protocol_version.as_deref() {
        if !supported_http_protocol_version(version) {
            return write_http(
                stream,
                400,
                "text/plain; charset=utf-8",
                None,
                b"Unsupported MCP-Protocol-Version",
            );
        }
    }
    if let Some(id) = headers.session_id.as_ref() {
        let known = state
            .sessions
            .lock()
            .map(|guard| guard.contains(id))
            .unwrap_or(false);
        if !known {
            return write_http(
                stream,
                404,
                "text/plain; charset=utf-8",
                None,
                b"Unknown MCP-Session-Id",
            );
        }
    }

    if body.is_empty() {
        return write_http(
            stream,
            400,
            "application/json",
            None,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"empty body"}}"#,
        );
    }

    let parsed: Result<Value, _> = serde_json::from_slice(body);
    let value = match parsed {
        Ok(value) => value,
        Err(error) => {
            let err = super::error_response(
                Value::Null,
                JSON_RPC_PARSE_ERROR,
                &format!("Parse error: {error}"),
            );
            let bytes = serde_json::to_vec(&err).unwrap_or_default();
            return write_http(stream, 400, "application/json", None, &bytes);
        }
    };

    // Client responses posted to the server: accept with 202.
    if (value.get("result").is_some() || value.get("error").is_some())
        && value.get("method").is_none()
    {
        return write_http(stream, 202, "text/plain; charset=utf-8", None, b"");
    }

    let is_initialize = value.get("method").and_then(Value::as_str) == Some("initialize");
    apply_http_cancellations(&value, state, headers.session_id.as_deref());

    // Batch members stay in this connection worker. Connections are already
    // concurrent and globally bounded; per-item threads would multiply the
    // connection limit by the batch-size limit.
    let outcome = if value.is_array() {
        dispatch_body_bounded(&value, state, headers.session_id.as_deref())
    } else {
        dispatch_http_value(&value, state, headers.session_id.as_deref())
    };

    let mut new_session: Option<String> = None;
    if is_initialize {
        let session = format!("keel-{}", generate_session_token());
        if let Ok(mut guard) = state.sessions.lock() {
            guard.insert(session.clone());
        }
        new_session = Some(session);
    }

    match outcome {
        DispatchBodyResult::Accepted => write_http(
            stream,
            202,
            "text/plain; charset=utf-8",
            new_session.as_deref().or(headers.session_id.as_deref()),
            b"",
        ),
        DispatchBodyResult::Json(response) => {
            let bytes = serde_json::to_vec(&response).unwrap_or_default();
            write_http(
                stream,
                200,
                "application/json",
                new_session.as_deref().or(headers.session_id.as_deref()),
                &bytes,
            )
        }
    }
}

fn http_cancellation_key(session_id: Option<&str>, request_id: &Value) -> Option<String> {
    super::cancellation_key(request_id)
        .map(|request_key| format!("{}\0{request_key}", session_id.unwrap_or("")))
}

fn apply_http_cancellations(value: &Value, state: &HttpState, session_id: Option<&str>) {
    let messages: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let cancellations = state
        .cancellations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for message in messages {
        if message.get("method").and_then(Value::as_str) != Some("notifications/cancelled") {
            continue;
        }
        let Some(key) = message
            .get("params")
            .and_then(|params| params.get("requestId"))
            .and_then(|request_id| http_cancellation_key(session_id, request_id))
        else {
            continue;
        };
        if let Some(cancellation) = cancellations.get(&key) {
            cancellation.store(true, Ordering::Release);
        }
    }
}

fn dispatch_http_value(
    value: &Value,
    state: &HttpState,
    session_id: Option<&str>,
) -> DispatchBodyResult {
    let cancellation = Arc::new(AtomicBool::new(false));
    dispatch_http_value_with_cancellation(value, state, session_id, cancellation)
}

fn dispatch_http_value_with_cancellation(
    value: &Value,
    state: &HttpState,
    session_id: Option<&str>,
    cancellation: Arc<AtomicBool>,
) -> DispatchBodyResult {
    let key = value
        .get("id")
        .and_then(|request_id| http_cancellation_key(session_id, request_id));
    if let Some(key) = key.as_ref() {
        state
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.clone(), Arc::clone(&cancellation));
    }
    let response = super::dispatch_cancellable(value, &cancellation);
    if let Some(key) = key.as_ref() {
        let mut registrations = state
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if registrations
            .get(key)
            .map(|registered| Arc::ptr_eq(registered, &cancellation))
            .unwrap_or(false)
        {
            registrations.remove(key);
        }
    }
    if cancellation.load(Ordering::Acquire) {
        DispatchBodyResult::Accepted
    } else {
        match response {
            Some(response) => DispatchBodyResult::Json(response),
            None => DispatchBodyResult::Accepted,
        }
    }
}

/// Dispatch a bounded batch in its already-bounded connection worker.
fn dispatch_body_bounded(
    body: &Value,
    state: &Arc<HttpState>,
    session_id: Option<&str>,
) -> DispatchBodyResult {
    dispatch_body_with_budget(body, state, session_id, HTTP_BATCH_WALL_BUDGET)
}

fn dispatch_body_with_budget(
    body: &Value,
    state: &Arc<HttpState>,
    session_id: Option<&str>,
    wall_budget: Duration,
) -> DispatchBodyResult {
    let items = match body.as_array() {
        Some(items) if items.is_empty() => {
            return DispatchBodyResult::Json(super::error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: empty batch",
            ));
        }
        Some(items) if items.len() > MAX_HTTP_BATCH_ITEMS => {
            return DispatchBodyResult::Json(super::error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: batch exceeds maximum item count",
            ));
        }
        Some(items) => items,
        None => return dispatch_http_value(body, state, session_id),
    };

    let current_cancellation = Arc::new(Mutex::new(None::<Arc<AtomicBool>>));
    let deadline_expired = Arc::new(AtomicBool::new(false));
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let timer_current = Arc::clone(&current_cancellation);
    let timer_expired = Arc::clone(&deadline_expired);
    let timer = match thread::Builder::new()
        .name("keel-mcp-http-batch-deadline".into())
        .spawn(move || {
            if finished_rx.recv_timeout(wall_budget).is_err() {
                timer_expired.store(true, Ordering::Release);
                if let Some(cancellation) = timer_current
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                {
                    cancellation.store(true, Ordering::Release);
                }
            }
        }) {
        Ok(timer) => timer,
        Err(error) => {
            return DispatchBodyResult::Json(super::error_response(
                Value::Null,
                JSON_RPC_INTERNAL_ERROR,
                &format!("batch deadline worker unavailable: {error}"),
            ));
        }
    };

    let batch_cancellations: Vec<Arc<AtomicBool>> = items
        .iter()
        .map(|_| Arc::new(AtomicBool::new(false)))
        .collect();
    {
        let mut registrations = state
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (item, cancellation) in items.iter().zip(&batch_cancellations) {
            if let Some(key) = item
                .get("id")
                .and_then(|request_id| http_cancellation_key(session_id, request_id))
            {
                registrations.insert(key, Arc::clone(cancellation));
            }
        }
    }
    // The caller scans cancellations before batch registration. Scan once more
    // so a cancellation notification in the same batch reaches a later item.
    apply_http_cancellations(body, state, session_id);

    let mut responses = Vec::with_capacity(items.len());
    for (item, cancellation) in items.iter().zip(&batch_cancellations) {
        if deadline_expired.load(Ordering::Acquire) {
            if let Some(id) = item.get("id") {
                responses.push(super::error_response(
                    id.clone(),
                    JSON_RPC_INTERNAL_ERROR,
                    "batch wall-clock budget exceeded",
                ));
            }
            continue;
        }
        *current_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(cancellation));
        let outcome = dispatch_http_value_with_cancellation(
            item,
            state,
            session_id,
            Arc::clone(cancellation),
        );
        let mut current = current_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, cancellation))
        {
            *current = None;
        }
        drop(current);
        match outcome {
            DispatchBodyResult::Json(response) => responses.push(response),
            DispatchBodyResult::Accepted
                if deadline_expired.load(Ordering::Acquire) && item.get("id").is_some() =>
            {
                responses.push(super::error_response(
                    item.get("id").cloned().unwrap_or(Value::Null),
                    JSON_RPC_INTERNAL_ERROR,
                    "batch wall-clock budget exceeded",
                ));
            }
            DispatchBodyResult::Accepted => {}
        }
    }
    let _ = finished_tx.send(());
    let _ = timer.join();
    {
        let mut registrations = state
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (item, cancellation) in items.iter().zip(&batch_cancellations) {
            if let Some(key) = item
                .get("id")
                .and_then(|request_id| http_cancellation_key(session_id, request_id))
            {
                if registrations
                    .get(&key)
                    .is_some_and(|registered| Arc::ptr_eq(registered, cancellation))
                {
                    registrations.remove(&key);
                }
            }
        }
    }
    if responses.is_empty() {
        DispatchBodyResult::Accepted
    } else {
        DispatchBodyResult::Json(Value::Array(responses))
    }
}

fn generate_session_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{}", std::process::id())
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    session_id: Option<&str>,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(session) = session_id {
        header.push_str(&format!("MCP-Session-Id: {session}\r\n"));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn read_http_response(client: &mut TcpStream) -> String {
        client.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match client.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if find_header_end(&response).is_some() {
                let header_end = find_header_end(&response).unwrap();
                let header_text = String::from_utf8_lossy(&response[..header_end]);
                let cl = header_text
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if response.len() >= header_end + cl {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&response).into_owned()
    }

    #[test]
    fn streamable_http_header_contract_is_explicit() {
        assert!(accepts_streamable_http(
            "application/json, text/event-stream"
        ));
        assert!(accepts_streamable_http(
            "text/event-stream; charset=utf-8, application/json"
        ));
        assert!(!accepts_streamable_http("application/json"));
        assert!(!accepts_streamable_http("text/event-stream"));
        assert!(!accepts_streamable_http(""));
        assert!(supported_http_protocol_version("2025-03-26"));
        assert!(supported_http_protocol_version("2025-11-25"));
        assert!(!supported_http_protocol_version("2099-01-01"));
    }

    #[test]
    fn bearer_authorization_requires_exact_token() {
        assert!(authorization_header_matches(
            Some("Bearer secret"),
            "secret"
        ));
        assert!(!authorization_header_matches(None, "secret"));
        assert!(!authorization_header_matches(
            Some("Basic secret"),
            "secret"
        ));
        assert!(!authorization_header_matches(
            Some("Bearer other"),
            "secret"
        ));
        assert!(authorization_header_matches(
            Some("Bearer secret "),
            "secret"
        ));
    }

    #[test]
    fn origin_allows_localhost_and_rejects_foreign() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://127.0.0.1:3000")));
        assert!(origin_allowed(Some("https://localhost:8443")));
        assert!(origin_allowed(Some("http://[::1]:3920")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("http://localhost.evil")));
        assert!(!origin_allowed(Some("http://127.0.0.1.evil")));
        assert!(!origin_allowed(Some("http://localhost/path")));
        assert!(!origin_allowed(Some("http://user@localhost")));
    }

    #[test]
    fn http_post_ping_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(HttpState::default());
        let state_accept = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, state_accept, Arc::new(InflightGuard::new(8)))
                .expect("handle");
        });

        thread::sleep(Duration::from_millis(20));
        let mut client = TcpStream::connect(addr).expect("connect");
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nOrigin: http://127.0.0.1\r\nAccept: application/json, text/event-stream\r\n\r\n",
            body.len()
        );
        client.write_all(request.as_bytes()).expect("write head");
        client.write_all(body).expect("write body");
        client.flush().expect("flush");

        let text = read_http_response(&mut client);
        assert!(
            text.contains("200") && text.contains("\"id\":1"),
            "response={text}"
        );
        let _ = server.join();
    }

    #[test]
    fn foreign_origin_is_forbidden() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(HttpState::default());
        let state_accept = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, state_accept, Arc::new(InflightGuard::new(8)))
                .expect("handle");
        });
        thread::sleep(Duration::from_millis(20));
        let mut client = TcpStream::connect(addr).expect("connect");
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nOrigin: https://evil.example\r\nAccept: application/json\r\n\r\n",
            body.len()
        );
        client.write_all(request.as_bytes()).expect("write");
        client.write_all(body).expect("body");
        let text = read_http_response(&mut client);
        assert!(text.contains("403"), "response={text}");
        let _ = server.join();
    }

    #[test]
    fn initialize_returns_server_info() {
        let result = super::super::dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": super::super::MCP_PROTOCOL_VERSION }
        }))
        .expect("response");
        assert_eq!(
            result["result"]["serverInfo"]["name"],
            json!(super::super::MCP_SERVER_NAME)
        );
        assert_eq!(
            result["result"]["serverInfo"]["version"],
            json!(super::super::MCP_SERVER_VERSION)
        );
    }

    #[test]
    fn inflight_guard_counts_acquires_and_releases() {
        let guard = Arc::new(InflightGuard::new(2));
        let first = guard.acquire();
        let second = guard.acquire();
        assert_eq!(guard.in_flight(), 2);
        drop(second);
        assert_eq!(guard.in_flight(), 1);
        drop(first);
        assert_eq!(guard.in_flight(), 0);
    }

    #[test]
    fn inflight_guard_blocks_at_capacity_then_wakes() {
        let guard = Arc::new(InflightGuard::new(1));
        let held = guard.acquire();

        let waiter = {
            let guard = Arc::clone(&guard);
            thread::spawn(move || {
                let permit = guard.acquire();
                (permit, guard.in_flight())
            })
        };
        // The waiter remains parked while the slot is held.
        // Capacity must not be exceeded.
        thread::sleep(Duration::from_millis(50));
        assert_eq!(guard.in_flight(), 1);

        drop(held);
        let (_, observed) = waiter.join().expect("waiter");
        assert_eq!(observed, 1, "waiter woke and took exactly one slot");
    }

    #[test]
    fn inflight_guard_times_out_without_exceeding_capacity() {
        let guard = Arc::new(InflightGuard::new(1));
        let _held = guard.acquire();
        let started = Instant::now();
        let permit = guard.acquire_timeout(Duration::from_millis(40));
        assert!(permit.is_none(), "over-cap waiter must time out");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(guard.in_flight(), 1);
    }

    #[test]
    fn inflight_guard_refuses_excess_connection_without_waiting() {
        let guard = Arc::new(InflightGuard::new(1));
        let _held = guard.try_acquire().expect("first permit");
        assert!(guard.try_acquire().is_none());
        assert_eq!(guard.in_flight(), 1);
    }

    #[test]
    fn oversized_declared_body_is_rejected_before_body_read() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(
                stream,
                Arc::new(HttpState::default()),
                Arc::new(InflightGuard::new(1)),
            )
            .expect("handle");
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n",
            MAX_HTTP_BODY + 1
        );
        client.write_all(request.as_bytes()).expect("write headers");
        client.flush().expect("flush");
        let text = read_http_response(&mut client);
        assert!(text.contains("413"), "response={text}");
        server.join().expect("server");
    }

    #[test]
    fn oversized_headers_are_rejected_before_unbounded_buffering() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(
                stream,
                Arc::new(HttpState::default()),
                Arc::new(InflightGuard::new(1)),
            )
            .expect("handle");
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        let request = format!(
            "GET /mcp HTTP/1.1\r\nX-Fill: {}",
            "x".repeat(MAX_HTTP_HEADER)
        );
        client.write_all(request.as_bytes()).expect("write headers");
        client.flush().expect("flush");
        let text = read_http_response(&mut client);
        assert!(text.contains("431"), "response={text}");
        server.join().expect("server");
    }

    #[test]
    fn duplicate_content_length_is_rejected_to_prevent_request_smuggling() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(
                stream,
                Arc::new(HttpState::default()),
                Arc::new(InflightGuard::new(1)),
            )
            .expect("handle");
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .write_all(
                b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n",
            )
            .expect("write headers");
        client.flush().expect("flush");
        let text = read_http_response(&mut client);
        assert!(text.contains("400"), "response={text}");
        server.join().expect("server");
    }

    #[test]
    fn oversized_json_rpc_batch_is_rejected_without_thread_fanout() {
        let body = Value::Array(
            (0..=MAX_HTTP_BATCH_ITEMS)
                .map(|id| json!({"jsonrpc":"2.0","id":id,"method":"ping"}))
                .collect(),
        );
        let DispatchBodyResult::Json(response) =
            dispatch_body_bounded(&body, &Arc::new(HttpState::default()), None)
        else {
            panic!("oversized batch must return an error response");
        };
        assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_REQUEST));
    }

    #[test]
    fn maximum_batch_returns_every_response_in_request_order() {
        let body = Value::Array(
            (0..MAX_HTTP_BATCH_ITEMS)
                .map(|id| json!({"jsonrpc":"2.0","id":id,"method":"ping"}))
                .collect(),
        );
        let DispatchBodyResult::Json(Value::Array(responses)) =
            dispatch_body_bounded(&body, &Arc::new(HttpState::default()), None)
        else {
            panic!("maximum batch must return an array");
        };
        assert_eq!(responses.len(), MAX_HTTP_BATCH_ITEMS);
        for (id, response) in responses.iter().enumerate() {
            assert_eq!(response["id"], json!(id));
        }
    }

    #[test]
    fn batch_deadline_cancels_current_member_and_errors_remaining_requests() {
        let body = json!([
            {"jsonrpc":"2.0","id":"slow","method":"keel/test_delay_ms","params":{"ms":500}},
            {"jsonrpc":"2.0","id":"later","method":"ping"}
        ]);
        let started = Instant::now();
        let DispatchBodyResult::Json(Value::Array(responses)) = dispatch_body_with_budget(
            &body,
            &Arc::new(HttpState::default()),
            Some("deadline-session"),
            Duration::from_millis(50),
        ) else {
            panic!("deadline batch must return errors");
        };
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(responses.len(), 2);
        assert!(responses
            .iter()
            .all(|response| response["error"]["code"] == json!(JSON_RPC_INTERNAL_ERROR)));
    }

    #[test]
    fn cancellation_of_preregistered_later_batch_member_prevents_execution() {
        let state = Arc::new(HttpState::default());
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            dispatch_body_with_budget(
                &json!([
                    {"jsonrpc":"2.0","id":"first","method":"keel/test_delay_ms","params":{"ms":200}},
                    {"jsonrpc":"2.0","id":"later","method":"keel/test_delay_ms","params":{"ms":500}}
                ]),
                &worker_state,
                Some("batch-session"),
                Duration::from_secs(2),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !state
            .cancellations
            .lock()
            .unwrap()
            .contains_key("batch-session\0\"later\"")
        {
            assert!(
                Instant::now() < deadline,
                "later member was not preregistered"
            );
            thread::yield_now();
        }
        apply_http_cancellations(
            &json!({
                "jsonrpc":"2.0",
                "method":"notifications/cancelled",
                "params":{"requestId":"later"}
            }),
            &state,
            Some("batch-session"),
        );
        let DispatchBodyResult::Json(Value::Array(responses)) = worker.join().unwrap() else {
            panic!("first batch member must respond");
        };
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!("first"));
    }

    #[test]
    fn http_cancellation_reaches_request_running_on_another_connection() {
        let state = Arc::new(HttpState::default());
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            dispatch_http_value(
                &json!({
                    "jsonrpc": "2.0",
                    "id": "http-slow",
                    "method": "keel/test_delay_ms",
                    "params": { "ms": 500 }
                }),
                &worker_state,
                Some("session-a"),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !state
            .cancellations
            .lock()
            .unwrap()
            .contains_key("session-a\0\"http-slow\"")
        {
            assert!(Instant::now() < deadline, "request registration timed out");
            thread::yield_now();
        }

        apply_http_cancellations(
            &json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": "http-slow" }
            }),
            &state,
            Some("session-b"),
        );
        assert!(!state
            .cancellations
            .lock()
            .unwrap()
            .get("session-a\0\"http-slow\"")
            .unwrap()
            .load(Ordering::Acquire));

        apply_http_cancellations(
            &json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": "http-slow" }
            }),
            &state,
            Some("session-a"),
        );

        assert!(matches!(
            worker.join().unwrap(),
            DispatchBodyResult::Accepted
        ));
        assert!(state.cancellations.lock().unwrap().is_empty());
    }
}
