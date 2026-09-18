//! Purpose: Streamable HTTP transport for the keel MCP server — multi-client
//!   concurrent connections on one process (MCP 2026-07-28).
//! Caller: `run_mcp_command` `serve-http` arm.
//! Dependencies: std::net only (no async runtime); reuses
//!   `super::dispatch_for_test` / `dispatch_body` for JSON-RPC semantics.
//! Side Effects: Binds a TCP listener (default 127.0.0.1:3920), accepts bounded
//!   concurrent clients and writes request-scoped responses.

use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

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
                        b"server busy",
                    );
                    continue;
                };
                let inflight = Arc::clone(&inflight);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
                let spawn = thread::Builder::new()
                    .name("keel-mcp-http".into())
                    .spawn(move || {
                        let _connection_permit = connection_permit;
                        if let Err(error) = handle_connection(stream, inflight) {
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

fn handle_connection(mut stream: TcpStream, inflight: Arc<InflightGuard>) -> std::io::Result<()> {
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
                    b"server busy",
                )?;
                return Ok(());
            };
            respond(&mut stream, &headers, &body)?;
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
    session_id: Option<String>,
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
    let mut session_id = None;
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
                "mcp-session-id" => {
                    singleton_headers_valid &= session_id.replace(value.to_string()).is_none();
                }
                "mcp-method" => {
                    singleton_headers_valid &= mcp_method.replace(value.to_string()).is_none();
                }
                "mcp-name" => {
                    singleton_headers_valid &= mcp_name.replace(value.to_string()).is_none();
                }
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
        session_id,
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

fn respond(stream: &mut TcpStream, headers: &HttpHeaders, body: &[u8]) -> std::io::Result<()> {
    if !headers.singleton_headers_valid {
        return write_json_error(
            stream,
            400,
            super::error_response(Value::Null, -32020, "Duplicate singleton HTTP header"),
        );
    }
    if !origin_allowed(headers.origin.as_deref()) {
        return write_json_error(
            stream,
            403,
            super::error_response(Value::Null, JSON_RPC_INVALID_REQUEST, "Origin not allowed"),
        );
    }
    if allow_remote_bind() && !remote_http_authorized(headers.authorization.as_deref()) {
        return write_http(stream, 401, "text/plain; charset=utf-8", b"Unauthorized");
    }
    let path = headers.path.split('?').next().unwrap_or("/");
    if path != "/mcp" && path != "/mcp/" {
        return write_http(stream, 404, "text/plain; charset=utf-8", b"not found");
    }
    if headers.method != "POST" {
        return write_http(
            stream,
            405,
            "text/plain; charset=utf-8",
            b"Method Not Allowed; MCP 2026-07-28 requires POST",
        );
    }
    handle_post(stream, headers, body)
}

fn handle_post(stream: &mut TcpStream, headers: &HttpHeaders, body: &[u8]) -> std::io::Result<()> {
    if !headers
        .content_type
        .as_deref()
        .is_some_and(is_json_content_type)
    {
        return write_http(
            stream,
            415,
            "text/plain; charset=utf-8",
            b"Content-Type must be application/json",
        );
    }
    if !accepts_streamable_http(&headers.accept) {
        return write_http(
            stream,
            406,
            "text/plain; charset=utf-8",
            b"Accept must include application/json and text/event-stream",
        );
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error) => {
            return write_json_error(
                stream,
                400,
                super::error_response(
                    Value::Null,
                    JSON_RPC_PARSE_ERROR,
                    &format!("Parse error: {error}"),
                ),
            )
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    if !value.is_object()
        || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || value.get("method").and_then(Value::as_str).is_none()
    {
        return write_json_error(
            stream,
            400,
            super::error_response(
                id,
                JSON_RPC_INVALID_REQUEST,
                "Expected a single JSON-RPC request",
            ),
        );
    }
    let method = value["method"].as_str().unwrap_or("");
    let is_modern = method == "server/discover"
        || headers.protocol_version.as_deref() == Some(super::MCP_PROTOCOL_VERSION)
        || headers.mcp_method.is_some()
        || value["params"].get("_meta").is_some();

    // Modern HTTP remains sessionless; a client-supplied session id is rejected on modern path.
    if is_modern && headers.session_id.is_some() {
        return write_json_error(
            stream,
            400,
            super::unsupported_version_response(
                id,
                value["params"]["protocolVersion"]
                    .as_str()
                    .unwrap_or("legacy"),
            ),
        );
    }
    // Stack B keeps validate_http_metadata for modern discover/tools/resources/ping.
    // Stack A (classic initialize and legacy requests without modern headers) skips it.
    if is_modern {
        if let Err(response) = validate_http_metadata(headers, &value) {
            return write_json_error(stream, 400, response);
        }
    }
    let cancellation = Arc::new(AtomicBool::new(false));
    // Application identity uses the client-supplied session id when present.
    // The authoritative() call tolerates None for callers without session context.
    let context = if headers.session_id.is_some() {
        super::McpRequestContext::authoritative(headers.session_id.as_deref())
    } else {
        super::McpRequestContext::authoritative(None)
    };
    if is_modern {
        context.set_wire_era(super::WireEra::Modern);
    } else {
        context.set_wire_era(super::WireEra::Classic);
    }
    match super::dispatch_cancellable_with_context(&value, &cancellation, &context) {
        None => write_http(stream, 202, "application/json", b""),
        Some(response) => {
            let status = match response["error"]["code"].as_i64() {
                Some(-32601) => 404,
                Some(-32603) => 500,
                Some(_) => 400,
                None => 200,
            };
            write_json_error(stream, status, response)
        }
    }
}

fn write_json_error(stream: &mut TcpStream, status: u16, response: Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&response)?;
    write_http(stream, status, "application/json", &bytes)
}

fn validate_http_metadata(headers: &HttpHeaders, value: &Value) -> Result<(), Value> {
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let mismatch = |name: &str| {
        super::error_response(
            id.clone(),
            -32020,
            &format!("Missing, malformed or mismatched {name} header"),
        )
    };
    let version = headers
        .protocol_version
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| mismatch("MCP-Protocol-Version"))?;
    let method = headers
        .mcp_method
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| mismatch("Mcp-Method"))?;
    if Some(method) != value["method"].as_str() {
        return Err(mismatch("Mcp-Method"));
    }
    // Align Protocol-Version header with body `_meta`: accept when they match,
    // or when either side declares the modern revision (avoid -32020 hang-class
    // HeaderMismatch for Antigravity-class clients that disagree on where the
    // version lives). Body `_meta` remains authoritative via validate_request_metadata.
    let meta_version = value
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(super::MCP_PROTOCOL_META))
        .and_then(Value::as_str);
    let header_ok = version == super::MCP_PROTOCOL_VERSION
        || super::CLASSIC_PROTOCOL_VERSIONS.contains(&version);
    let aligned = match meta_version {
        Some(meta) if meta == version => true,
        Some(meta)
            if meta == super::MCP_PROTOCOL_VERSION || version == super::MCP_PROTOCOL_VERSION =>
        {
            true
        }
        None if header_ok => true,
        _ => false,
    };
    if !aligned {
        return Err(mismatch("MCP-Protocol-Version"));
    }
    super::validate_request_metadata(&value["params"], &id)?;
    let source = match method {
        "resources/read" => Some("uri"),
        "tools/call" | "prompts/get" => Some("name"),
        _ => None,
    };
    if let Some(source) = source {
        let raw = headers
            .mcp_name
            .as_deref()
            .ok_or_else(|| mismatch("Mcp-Name"))?;
        let decoded = decode_header_value(raw).ok_or_else(|| mismatch("Mcp-Name"))?;
        if Some(decoded.as_str()) != value["params"][source].as_str() {
            return Err(mismatch("Mcp-Name"));
        }
    }
    Ok(())
}

fn decode_header_value(value: &str) -> Option<String> {
    if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|v| v.strip_suffix("?="))
    {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        if encoded.len() % 4 != 0 {
            return None;
        }
        let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
        for (index, chunk) in encoded.as_bytes().chunks_exact(4).enumerate() {
            let mut bits = 0u32;
            let mut padding = 0;
            for (position, byte) in chunk.iter().enumerate() {
                let digit = if *byte == b'=' {
                    if position < 2 || index + 1 != encoded.len() / 4 {
                        return None;
                    }
                    padding += 1;
                    0
                } else {
                    if padding != 0 {
                        return None;
                    }
                    alphabet.iter().position(|entry| entry == byte)? as u32
                };
                bits = (bits << 6) | digit;
            }
            if padding > 2
                || (padding == 2 && bits & 0xffff != 0)
                || (padding == 1 && bits & 0xff != 0)
            {
                return None;
            }
            decoded.push((bits >> 16) as u8);
            if padding < 2 {
                decoded.push((bits >> 8) as u8);
            }
            if padding == 0 {
                decoded.push(bits as u8);
            }
        }
        return String::from_utf8(decoded).ok();
    }
    (value
        .bytes()
        .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
        && value.trim() == value)
        .then(|| value.to_string())
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
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

    fn http_round_trip(request: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, Arc::new(InflightGuard::new(8))).expect("handle");
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
        assert!(accepts_streamable_http(
            "Application/JSON, TEXT/EVENT-STREAM"
        ));
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("Application/JSON; charset=utf-8"));
        assert!(!is_json_content_type("application/json-seq"));
        assert!(!is_json_content_type("application/jsonp"));
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
        let response = http_round_trip(&null_origin);
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");

        let mut json_prefix = String::from(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json-seq\r\n\
             Accept: application/json, text/event-stream\r\n",
        );
        json_prefix.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        json_prefix.push_str(&String::from_utf8_lossy(body));
        let response = http_round_trip(json_prefix.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 415 Unsupported Media Type"),
            "{response}"
        );
    }
    use serde_json::json;

    #[test]
    fn http_post_discovery_without_handshake() {
        let body = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}})).unwrap();
        let request = http_post_request(
            &body,
            &[
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "server/discover"),
            ],
        );
        let text = http_round_trip(&request);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        let response: Value = serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            response["result"]["supportedVersions"],
            json!(["2026-07-28"])
        );
        assert!(!text.contains("MCP-Session-Id"));
    }

    #[test]
    fn http_routing_headers_must_match_body_and_decode_names() {
        let request = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
            "name":"café", "_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28", "io.modelcontextprotocol/clientCapabilities":{}}
        }});
        let base = "POST /mcp HTTP/1.1\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: =?base64?Y2Fmw6k=?=\r\n\r\n";
        assert!(validate_http_metadata(&parse_headers(base), &request).is_ok());
        // Method mismatch and Mcp-Name mismatch still fail with -32020.
        // Protocol-Version header vs `_meta` disagreement is aligned when either
        // side speaks the modern revision (no longer a hard -32020).
        for invalid in [
            base.replace("tools/call", "tools/list"),
            base.replace("=?base64?Y2Fmw6k=?=", "other"),
        ] {
            let error = validate_http_metadata(&parse_headers(&invalid), &request).unwrap_err();
            assert_eq!(error["error"]["code"], -32020);
            assert_eq!(error["id"], 7);
        }
        let version_aligned = base.replace(
            "MCP-Protocol-Version: 2026-07-28",
            "MCP-Protocol-Version: 2025-11-25",
        );
        assert!(
            validate_http_metadata(&parse_headers(&version_aligned), &request).is_ok(),
            "header/body version disagree must align when body is modern"
        );
        assert!(decode_header_value("=?base64?Y2Fmw6k?=").is_none());
    }

    #[test]
    fn foreign_origin_is_forbidden() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = http_post_request(body, &[("Origin", "https://evil.example")]);
        let text = http_round_trip(&request);
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
            handle_connection(stream, Arc::new(InflightGuard::new(1))).expect("handle");
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
            handle_connection(stream, Arc::new(InflightGuard::new(1))).expect("handle");
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
            handle_connection(stream, Arc::new(InflightGuard::new(1))).expect("handle");
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
