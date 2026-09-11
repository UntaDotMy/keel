//! Purpose: Streamable HTTP transport for the keel MCP server — multi-client
//!   concurrent connections on one process (MCP 2026-07-28 transports).
//! Caller: `run_mcp_command` `serve-http` arm.
//! Dependencies: std::net only (no async runtime); reuses `super::dispatch` /
//!   `dispatch_body` for JSON-RPC semantics.
//! Side Effects: Binds a TCP listener (default 127.0.0.1:3920), accepts bounded
//!   concurrent clients, writes responses, and bounds request-scoped cancellation.

use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{JSON_RPC_INVALID_REQUEST, JSON_RPC_PARSE_ERROR};

const DEFAULT_BIND: &str = "127.0.0.1:3920";
const MAX_HTTP_BODY: usize = 8 * 1024 * 1024;
const MAX_HTTP_HEADER: usize = 16 * 1024;
const HTTP_INFLIGHT_WAIT: Duration = Duration::from_secs(2);

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

    let state = Arc::new(HttpState);
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
    // Constant-time comparison: iterate through expected_bytes so the loop
    // duration reveals nothing about the provided token's length.
    let mut difference = provided_bytes.len() ^ expected_bytes.len();
    for (index, &expected_byte) in expected_bytes.iter().enumerate() {
        difference |=
            usize::from(provided_bytes.get(index).copied().unwrap_or_default() ^ expected_byte);
    }
    difference == 0
}

fn remote_http_authorized(header: Option<&str>) -> bool {
    configured_auth_token()
        .map(|expected| authorization_header_matches(header, &expected))
        .unwrap_or(false)
}

#[derive(Default)]
struct HttpState;

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
    content_type: Option<String>,
    content_length: Option<usize>,
    content_length_valid: bool,
    accept: String,
    authorization: Option<String>,
    protocol_version: Option<String>,
    mcp_method: Option<String>,
    mcp_name: Option<String>,
    singleton_headers_valid: bool,
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
    let mut content_type = None;
    let mut content_length = None;
    let mut content_length_valid = true;
    let mut accept = String::new();
    let mut authorization = None;
    let mut protocol_version = None;
    let mut mcp_method = None;
    let mut mcp_name = None;
    let mut singleton_headers_valid = true;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "origin" => {
                    singleton_headers_valid &= origin.replace(value.to_string()).is_none();
                }
                "content-type" => {
                    singleton_headers_valid &= content_type.replace(value.to_string()).is_none();
                }
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
                "accept" => {
                    if !accept.is_empty() {
                        accept.push(',');
                    }
                    accept.push_str(value);
                }
                "authorization" => {
                    singleton_headers_valid &= authorization.replace(value.to_string()).is_none();
                }
                "mcp-protocol-version" => {
                    singleton_headers_valid &=
                        protocol_version.replace(value.to_string()).is_none();
                }
                "mcp-method" => {
                    singleton_headers_valid &= mcp_method.replace(value.to_string()).is_none();
                }
                "mcp-name" => {
                    singleton_headers_valid &= mcp_name.replace(value.to_string()).is_none();
                }
                "transfer-encoding" => content_length_valid = false,
                _ => {}
            }
        }
    }
    HttpHeaders {
        method,
        path,
        origin,
        content_type,
        content_length,
        content_length_valid,
        accept,
        authorization,
        protocol_version,
        mcp_method,
        mcp_name,
        singleton_headers_valid,
    }
}

fn media_type_list_contains(header: &str, expected: &str) -> bool {
    header.split(',').any(|value| {
        value
            .trim()
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(expected))
    })
}

fn accepts_streamable_http(accept: &str) -> bool {
    media_type_list_contains(accept, "application/json")
        && media_type_list_contains(accept, "text/event-stream")
}

fn is_json_content_type(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
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
        // Non-browser clients omit Origin; cross-origin browser requests attach it.
        // Bearer token authentication gates non-browser clients.
        None => true,
        Some("null") => false,
        Some(value) if local_origin_allowed(value) => true,
        Some(value) if remote_origin_allowed(value) => true,
        Some(_) => false,
    }
}

fn respond(
    stream: &mut TcpStream,
    headers: &HttpHeaders,
    body: &[u8],
    state: &Arc<HttpState>,
) -> std::io::Result<()> {
    if !headers.singleton_headers_valid {
        return write_protocol_error(
            stream,
            super::error_response(Value::Null, -32020, "Duplicate singleton HTTP header"),
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
    if (allow_remote_bind() || configured_auth_token().is_some())
        && !remote_http_authorized(headers.authorization.as_deref())
    {
        return write_http(
            stream,
            401,
            "text/plain; charset=utf-8",
            None,
            b"Unauthorized",
        );
    }
    let path = headers.path.split('?').next().unwrap_or("/");
    if path != "/mcp" && path != "/mcp/" {
        return write_http(stream, 404, "text/plain; charset=utf-8", None, b"not found");
    }

    match headers.method.to_ascii_uppercase().as_str() {
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
    _state: &Arc<HttpState>,
) -> std::io::Result<()> {
    if !headers
        .content_type
        .as_deref()
        .is_some_and(is_json_content_type)
    {
        return write_http(
            stream,
            415,
            "text/plain; charset=utf-8",
            None,
            b"Content-Type must be application/json",
        );
    }
    if !accepts_streamable_http(&headers.accept) {
        return write_http(
            stream,
            406,
            "text/plain; charset=utf-8",
            None,
            b"Accept must include application/json and text/event-stream",
        );
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return write_protocol_error(
                stream,
                super::error_response(Value::Null, JSON_RPC_PARSE_ERROR, "Parse error"),
            )
        }
    };
    if !value.is_object() || value.get("method").and_then(Value::as_str).is_none() {
        return write_protocol_error(
            stream,
            super::error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Each HTTP POST must contain one JSON-RPC request or notification",
            ),
        );
    }
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let method = value["method"].as_str().unwrap_or_default();
    if method == "initialize" {
        return write_protocol_error(
            stream,
            super::unsupported_protocol_response(id, value["params"]["protocolVersion"].as_str()),
        );
    }
    if let Err(message) = validate_routing_headers(headers, &value) {
        return write_protocol_error(stream, super::error_response(id, -32020, message));
    }
    if value.get("id").is_some() {
        if let Err(response) = super::validate_request_metadata(&value["params"], &id) {
            return write_protocol_error(stream, response);
        }
    }
    if method.starts_with("notifications/") && value.get("id").is_some() {
        return write_protocol_error(
            stream,
            super::error_response(
                id,
                JSON_RPC_INVALID_REQUEST,
                "MCP notifications must not include an id",
            ),
        );
    }
    // HTTP cancellation belongs to this response stream, never a reusable
    // client-supplied request id or a protocol session.
    let cancellation = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let peer = stream.try_clone()?;
    peer.set_read_timeout(Some(Duration::from_millis(50)))?;
    let watch_cancel = Arc::clone(&cancellation);
    let watch_finished = Arc::clone(&finished);
    let watcher = thread::spawn(move || {
        let mut byte = [0u8; 1];
        while !watch_finished.load(Ordering::Acquire) {
            match peer.peek(&mut byte) {
                Ok(0) => {
                    watch_cancel.store(true, Ordering::Release);
                    break;
                }
                Err(error)
                    if !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    watch_cancel.store(true, Ordering::Release);
                    break;
                }
                _ => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    let context = super::McpRequestContext::authoritative(None);
    let response = super::dispatch_cancellable_with_context(&value, &cancellation, &context);
    finished.store(true, Ordering::Release);
    // why: the watcher only observes cancellation; a join failure cannot
    // change the already-bounded response and must not leak a panic.
    let _ = watcher.join();
    match response {
        Some(response) => {
            let code = response["error"]["code"].as_i64();
            let status = if matches!(
                code,
                Some(-32600 | -32601 | -32602 | -32020 | -32021 | -32022)
            ) {
                400
            } else {
                200
            };
            let bytes = serde_json::to_vec(&response)?;
            write_http(stream, status, "application/json", None, &bytes)
        }
        None => write_http(stream, 202, "text/plain; charset=utf-8", None, b""),
    }
}

fn write_protocol_error(stream: &mut TcpStream, response: Value) -> std::io::Result<()> {
    write_http(
        stream,
        400,
        "application/json",
        None,
        &serde_json::to_vec(&response)?,
    )
}

fn validate_routing_headers(headers: &HttpHeaders, value: &Value) -> Result<(), &'static str> {
    let version = headers
        .protocol_version
        .as_deref()
        .ok_or("Missing MCP-Protocol-Version")?;
    let method = headers.mcp_method.as_deref().ok_or("Missing Mcp-Method")?;
    if method.is_empty()
        || !method.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        || Some(method) != value.get("method").and_then(Value::as_str)
    {
        return Err("Mcp-Method does not match request body");
    }
    let body_version = value["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"].as_str();
    if Some(version) != body_version {
        return Err("MCP-Protocol-Version does not match request body");
    }
    let body_name = match method {
        "tools/call" | "prompts/get" => Some(&value["params"]["name"]),
        "resources/read" => Some(&value["params"]["uri"]),
        _ => None,
    };
    if let Some(body_name) = body_name {
        let name = headers
            .mcp_name
            .as_deref()
            .and_then(decode_header_value)
            .ok_or("Missing or malformed Mcp-Name")?;
        if Some(name.as_str()) != body_name.as_str() {
            return Err("Mcp-Name does not match request body");
        }
    }
    Ok(())
}

fn decode_header_value(value: &str) -> Option<String> {
    if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|v| v.strip_suffix("?="))
    {
        if encoded.len() % 4 != 0 {
            return None;
        }
        let mut decoded = Vec::new();
        for (index, chunk) in encoded.as_bytes().chunks_exact(4).enumerate() {
            let digit = |byte| match byte {
                b'A'..=b'Z' => Some(byte - b'A'),
                b'a'..=b'z' => Some(byte - b'a' + 26),
                b'0'..=b'9' => Some(byte - b'0' + 52),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            };
            let a = digit(chunk[0])?;
            let b = digit(chunk[1])?;
            let final_chunk = (index + 1) * 4 == encoded.len();
            decoded.push((a << 2) | (b >> 4));
            if chunk[2] == b'=' {
                if !final_chunk || chunk[3] != b'=' || b & 15 != 0 {
                    return None;
                }
            } else {
                let c = digit(chunk[2])?;
                decoded.push((b << 4) | (c >> 2));
                if chunk[3] == b'=' {
                    if !final_chunk || c & 3 != 0 {
                        return None;
                    }
                } else {
                    decoded.push((c << 6) | digit(chunk[3])?);
                }
            }
        }
        String::from_utf8(decoded).ok()
    } else if value.trim() == value
        && value
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) || byte == b'\t')
    {
        Some(value.to_string())
    } else {
        None
    }
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    _session_id: Option<&str>,
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
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
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

    fn http_round_trip(state: Arc<HttpState>, request: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, state, Arc::new(InflightGuard::new(8))).expect("handle");
        });
        let mut client = TcpStream::connect(addr).expect("connect");
        client.write_all(request).expect("write request");
        client.flush().expect("flush request");
        let response = read_http_response(&mut client);
        server.join().expect("server worker");
        response
    }

    /// Build one complete POST /mcp request so a test can vary only the body
    /// and the headers it means to exercise.
    fn http_post_request(body: &[u8], extra_headers: &[(&str, &str)]) -> Vec<u8> {
        let mut request = String::from(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n",
        );
        for (name, value) in extra_headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut bytes = request.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn modern_post(method: &str, mut params: Value) -> Vec<u8> {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": super::super::MCP_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        let value = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params});
        let body = serde_json::to_vec(&value).unwrap();
        let mut headers = vec![
            ("MCP-Protocol-Version", super::super::MCP_PROTOCOL_VERSION),
            ("Mcp-Method", method),
        ];
        if let Some(name) = value["params"]["name"]
            .as_str()
            .or(value["params"]["uri"].as_str())
        {
            headers.push(("Mcp-Name", name));
        }
        http_post_request(&body, &headers)
    }

    fn response_json(response: &str) -> Value {
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[test]
    fn modern_http_discovery_and_catalog_require_no_handshake() {
        for method in ["ping", "server/discover", "tools/list", "resources/list"] {
            let request = modern_post(method, json!({}));
            let response = http_round_trip(Arc::new(HttpState), &request);
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert!(!response.contains("MCP-Session-Id:"));
            let value = response_json(&response);
            assert_eq!(value["result"]["resultType"], "complete");
            if method == "server/discover" {
                assert_eq!(value["result"]["supportedVersions"], json!(["2026-07-28"]));
                assert!(value["result"].get("tools").is_none());
                assert!(
                    crate::proxy::token_meter::TokenMeter::count_text(&value["result"].to_string())
                        < 300
                );
            }
        }
    }

    #[test]
    fn retired_http_state_is_ignored_and_endpoints_are_closed() {
        let request = String::from_utf8(modern_post("ping", json!({})))
            .unwrap()
            .replace(
            "Host: 127.0.0.1\r\n",
            "Host: 127.0.0.1\r\nMcp-Session-Id: stale-client-handle\r\nLast-Event-ID: stale\r\n",
        );
        let response = http_round_trip(Arc::new(HttpState), request.as_bytes());
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(!response.to_ascii_lowercase().contains("mcp-session-id:"));
        for method in ["GET", "DELETE"] {
            let response = http_round_trip(
                Arc::new(HttpState),
                format!("{method} /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            );
            assert!(response.starts_with("HTTP/1.1 405"), "{response}");
        }
    }

    #[test]
    fn modern_http_rejects_header_body_conflicts_before_dispatch() {
        let request = String::from_utf8(modern_post(
            "tools/call",
            json!({"name":"stats", "arguments":{}}),
        ))
        .unwrap();
        for changed in [
            request.replace("Mcp-Method: tools/call", "Mcp-Method: ping"),
            request.replace("Mcp-Name: stats", "Mcp-Name: other"),
            request.replace("Mcp-Name: stats\r\n", ""),
            request.replace("MCP-Protocol-Version: 2026-07-28\r\n", ""),
            request.replace(
                "MCP-Protocol-Version: 2026-07-28",
                "MCP-Protocol-Version: 2025-11-25",
            ),
        ] {
            let response = http_round_trip(Arc::new(HttpState), changed.as_bytes());
            assert!(response.starts_with("HTTP/1.1 400"), "{response}");
            assert_eq!(response_json(&response)["error"]["code"], -32020);
        }
    }

    #[test]
    fn modern_http_rejects_unsupported_versions_with_supported_list() {
        let request = String::from_utf8(modern_post("ping", json!({})))
            .unwrap()
            .replace("2026-07-28", "2025-11-25");
        let response = http_round_trip(Arc::new(HttpState), request.as_bytes());
        let value = response_json(&response);
        assert!(response.starts_with("HTTP/1.1 400"));
        assert_eq!(value["error"]["code"], -32022);
        assert_eq!(value["error"]["data"]["supported"], json!(["2026-07-28"]));
        assert_eq!(value["error"]["data"]["requested"], "2025-11-25");
    }

    #[test]
    fn modern_http_rejects_batches_and_client_responses() {
        for body in [
            br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1,"result":{}}"#.as_slice(),
        ] {
            let response = http_round_trip(Arc::new(HttpState), &http_post_request(body, &[]));
            assert_eq!(response_json(&response)["error"]["code"], -32600);
        }
        let batch = json!([{"jsonrpc":"2.0", "id":1, "method":"ping", "params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28", "io.modelcontextprotocol/clientCapabilities":{}
        }}}]);
        let response = http_round_trip(
            Arc::new(HttpState),
            &http_post_request(
                &serde_json::to_vec(&batch).unwrap(),
                &[
                    ("MCP-Protocol-Version", "2026-07-28"),
                    ("Mcp-Method", "ping"),
                ],
            ),
        );
        assert_eq!(response_json(&response)["error"]["code"], -32600);
    }

    #[test]
    fn modern_http_decodes_base64_names_and_rejects_malformed_encoding() {
        assert_eq!(
            decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?="),
            Some("Hello, 世界".into())
        );
        assert_eq!(
            decode_header_value("=?base64?IHBhZGRlZCA=?="),
            Some(" padded ".into())
        );
        assert_eq!(
            decode_header_value("=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="),
            Some("=?base64?literal?=".into())
        );
        for invalid in [
            "=?base64?YQ=?=",
            "=?base64?YR==?=",
            "=?base64?YQ==YQ==?=",
            "non-ascii-世界",
            " padded ",
        ] {
            assert!(decode_header_value(invalid).is_none(), "{invalid}");
        }
        let request = String::from_utf8(modern_post("resources/read", json!({"uri":"invalid"})))
            .unwrap()
            .replace("Mcp-Name: invalid", "Mcp-Name: =?base64?aW52YWxpZA==?=");
        let response = http_round_trip(Arc::new(HttpState), request.as_bytes());
        assert_ne!(
            response_json(&response)["error"]["code"],
            -32020,
            "{response}"
        );
    }

    #[test]
    fn modern_http_disconnect_cancels_only_its_own_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // why: the test asserts the follow-up response; connection errors
            // are intentionally isolated to this worker.
            let _ = handle_connection(stream, Arc::new(HttpState), Arc::new(InflightGuard::new(1)));
            done_tx.send(()).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(&modern_post("keel/test_delay_ms", json!({"ms":500})))
            .unwrap();
        client.flush().unwrap();
        drop(client);
        done_rx
            .recv_timeout(Duration::from_millis(400))
            .expect("disconnected request must stop before its delay completes");
        worker.join().unwrap();
        let response = http_round_trip(Arc::new(HttpState), &modern_post("ping", json!({})));
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
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
        let length_wrapped = format!("Bearer secret{}", "x".repeat(256));
        assert!(!authorization_header_matches(
            Some(&length_wrapped),
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
        assert!(!origin_allowed(Some("null")));
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
    fn duplicate_singleton_headers_are_rejected() {
        let headers = parse_headers(
            "POST /mcp HTTP/1.1\r\nOrigin: null\r\nOrigin: http://localhost\r\n\
             Content-Type: application/json\r\nContent-Type: application/json\r\n\
             Authorization: Bearer first\r\nAuthorization: Bearer second\r\n\
             MCP-Protocol-Version: 2025-11-25\r\nMCP-Protocol-Version: 2025-03-26\r\n\
             MCP-Session-Id: first\r\nMCP-Session-Id: second\r\n\r\n",
        );
        assert!(!headers.singleton_headers_valid);

        let repeated_accept = parse_headers(
            "POST /mcp HTTP/1.1\r\nAccept: application/json\r\n\
             Accept: text/event-stream\r\n\r\n",
        );
        assert!(repeated_accept.singleton_headers_valid);
        assert!(accepts_streamable_http(&repeated_accept.accept));
    }

    #[test]
    fn origin_null_and_non_json_media_type_fail_at_the_http_boundary() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let null_origin = http_post_request(body, &[("Origin", "null")]);
        let response = http_round_trip(Arc::new(HttpState), &null_origin);
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");

        let mut json_prefix = String::from(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json-seq\r\n\
             Accept: application/json, text/event-stream\r\n",
        );
        json_prefix.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        json_prefix.push_str(&String::from_utf8_lossy(body));
        let response = http_round_trip(Arc::new(HttpState), json_prefix.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 415 Unsupported Media Type"),
            "{response}"
        );
    }

    #[test]
    fn foreign_origin_is_forbidden() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = http_post_request(body, &[("Origin", "https://evil.example")]);
        let text = http_round_trip(Arc::new(HttpState), &request);
        assert!(text.contains("403"), "response={text}");
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
            handle_connection(stream, Arc::new(HttpState), Arc::new(InflightGuard::new(1)))
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
            handle_connection(stream, Arc::new(HttpState), Arc::new(InflightGuard::new(1)))
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
            handle_connection(stream, Arc::new(HttpState), Arc::new(InflightGuard::new(1)))
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
}
