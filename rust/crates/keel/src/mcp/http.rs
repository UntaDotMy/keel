//! Purpose: Streamable HTTP transport for the keel MCP server — multi-client
//!   concurrent connections on one process (MCP 2025-11-25 transports).
//! Caller: `run_mcp_command` `serve-http` arm.
//! Dependencies: std::net only (no async runtime); reuses `super::dispatch` /
//!   `dispatch_body` for JSON-RPC semantics.
//! Side Effects: Binds a TCP listener (default 127.0.0.1:3920), accepts bounded
//!   concurrent clients, writes responses, and tracks sessions/cancellation.

use std::collections::HashMap;
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

#[derive(Debug, Clone)]
struct HttpSession {
    last_seen: Instant,
    protocol_version: String,
}

#[derive(Debug, Clone)]
struct CancellationRegistration {
    token: Arc<AtomicBool>,
    registered_at: Instant,
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionValidationError {
    MissingSession,
    MissingProtocol,
    Unknown,
    ProtocolMismatch,
    UnsupportedProtocol,
}

#[derive(Default)]
struct HttpState {
    sessions: Mutex<HashMap<String, HttpSession>>,
    cancellations: Mutex<HashMap<String, CancellationRegistration>>,
}

const MAX_HTTP_SESSIONS: usize = 1_000;
const MAX_HTTP_CANCELLATIONS: usize = 1_024;
const MAX_HTTP_SESSION_ID_BYTES: usize = 256;
const MAX_HTTP_CANCELLATION_KEY_BYTES: usize = 1_024;

fn http_cancellation_ttl() -> Duration {
    env::var("KEEL_MCP_CANCELLATION_TTL_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| Duration::from_secs(value.clamp(1, 86_400)))
        .unwrap_or_else(|| Duration::from_secs(900))
}

fn cancellation_registration_ttl() -> Duration {
    // A configured cancellation TTL is the stale-entry policy, not a license to
    // forget an in-flight request; keep the handle through the owner deadline.
    http_cancellation_ttl()
        .max(super::tools::mcp_child_timeout())
        .max(HTTP_BATCH_WALL_BUDGET)
}

fn valid_http_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_HTTP_SESSION_ID_BYTES
        && value
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
}

fn http_session_ttl() -> Duration {
    env::var("KEEL_MCP_SESSION_TTL_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| Duration::from_secs(value.clamp(1, 86_400)))
        .unwrap_or_else(|| Duration::from_secs(900))
}

fn effective_http_session_ttl() -> Duration {
    // A session must stay valid while a request it owns is cancellable, so
    // extend short TTLs through the single-tool and batch owner deadlines.
    http_session_ttl()
        .max(super::tools::mcp_child_timeout())
        .max(HTTP_BATCH_WALL_BUDGET)
}

impl HttpState {
    fn with_live_sessions<T>(
        &self,
        operation: impl FnOnce(&mut HashMap<String, HttpSession>, Instant) -> T,
    ) -> T {
        let now = Instant::now();
        let ttl = effective_http_session_ttl();
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.retain(|_, session| now.saturating_duration_since(session.last_seen) < ttl);
        operation(&mut sessions, now)
    }

    fn purge_expired_sessions(&self) {
        self.with_live_sessions(|_, _| ());
    }

    #[cfg(test)]
    fn touch_session(&self, id: &str) -> bool {
        self.touch_session_with_protocol(id, None).is_ok()
    }

    fn touch_session_with_protocol(
        &self,
        id: &str,
        protocol_version: Option<&str>,
    ) -> Result<(), SessionValidationError> {
        self.with_live_sessions(|sessions, now| {
            let Some(session) = sessions.get_mut(id) else {
                return Err(SessionValidationError::Unknown);
            };
            if protocol_version.is_some_and(|version| version != session.protocol_version) {
                return Err(SessionValidationError::ProtocolMismatch);
            }
            session.last_seen = now;
            Ok(())
        })
    }

    fn register_session(&self, id: String, protocol_version: String) -> bool {
        self.with_live_sessions(|sessions, now| {
            if sessions.len() >= MAX_HTTP_SESSIONS || sessions.contains_key(&id) {
                return false;
            }
            sessions.insert(
                id,
                HttpSession {
                    last_seen: now,
                    protocol_version,
                },
            );
            true
        })
    }

    fn remove_session(&self, id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
    }

    fn with_live_cancellations<T>(
        &self,
        operation: impl FnOnce(&mut HashMap<String, CancellationRegistration>, Instant) -> T,
    ) -> T {
        let now = Instant::now();
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Registrations are process-local cancellation handles, so reap only
        // after the effective owner deadline; otherwise active work loses its handle.
        cancellations.retain(|_, registration| now < registration.expires_at);
        operation(&mut cancellations, now)
    }

    /// Register one request for cancellation. A bounded registry is fail
    /// closed: when it is full, the caller rejects a new request instead of
    /// silently running work that can never be cancelled. Re-registering the
    /// same token is allowed for a preregistered batch member.
    fn register_cancellation(&self, key: String, token: Arc<AtomicBool>) -> bool {
        if key.len() > MAX_HTTP_CANCELLATION_KEY_BYTES {
            return false;
        }
        self.with_live_cancellations(|cancellations, now| {
            let expires_at = now + cancellation_registration_ttl();
            if let Some(existing) = cancellations.get_mut(&key) {
                if Arc::ptr_eq(&existing.token, &token) {
                    existing.registered_at = now;
                    existing.expires_at = expires_at;
                    return true;
                }
                return false;
            }
            if cancellations.len() >= MAX_HTTP_CANCELLATIONS {
                return false;
            }
            cancellations.insert(
                key,
                CancellationRegistration {
                    token,
                    registered_at: now,
                    expires_at,
                },
            );
            true
        })
    }

    fn unregister_cancellation(&self, key: &str, token: &Arc<AtomicBool>) {
        self.with_live_cancellations(|cancellations, _| {
            if cancellations
                .get(key)
                .map(|registered| Arc::ptr_eq(&registered.token, token))
                .unwrap_or(false)
            {
                cancellations.remove(key);
            }
        });
    }
}

fn validate_http_session(
    state: &HttpState,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    require_session: bool,
) -> Result<(), SessionValidationError> {
    if protocol_version.is_some_and(|version| !supported_http_protocol_version(version)) {
        return Err(SessionValidationError::UnsupportedProtocol);
    }
    let Some(session_id) = session_id else {
        if require_session || protocol_version.is_some() {
            return Err(SessionValidationError::MissingSession);
        }
        return Ok(());
    };
    let Some(protocol_version) = protocol_version else {
        return Err(SessionValidationError::MissingProtocol);
    };
    state.touch_session_with_protocol(session_id, Some(protocol_version))
}

fn write_http_session_validation_error(
    stream: &mut TcpStream,
    error: SessionValidationError,
) -> std::io::Result<()> {
    let (status, body) = match error {
        SessionValidationError::MissingSession => (
            400,
            b"MCP-Session-Id required for subsequent MCP requests".as_slice(),
        ),
        SessionValidationError::MissingProtocol => (
            400,
            b"MCP-Protocol-Version required for subsequent MCP requests".as_slice(),
        ),
        SessionValidationError::Unknown => (404, b"Unknown MCP-Session-Id".as_slice()),
        SessionValidationError::ProtocolMismatch => (
            400,
            b"MCP-Protocol-Version does not match the MCP session".as_slice(),
        ),
        SessionValidationError::UnsupportedProtocol => {
            (400, b"Unsupported MCP-Protocol-Version".as_slice())
        }
    };
    write_http(stream, status, "text/plain; charset=utf-8", None, body)
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
    content_type: Option<String>,
    content_length: Option<usize>,
    content_length_valid: bool,
    accept: String,
    authorization: Option<String>,
    protocol_version: Option<String>,
    session_id: Option<String>,
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
        // Non-browser clients omit Origin; cross-origin browser requests attach it.
        // Bearer token authentication gates non-browser clients.
        None => true,
        Some("null") => false,
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
    state.purge_expired_sessions();
    if !headers.singleton_headers_valid {
        return write_http(
            stream,
            400,
            "text/plain; charset=utf-8",
            None,
            b"duplicate singleton HTTP header",
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
    if allow_remote_bind() && !remote_http_authorized(headers.authorization.as_deref()) {
        return write_http(
            stream,
            401,
            "text/plain; charset=utf-8",
            None,
            b"Unauthorized",
        );
    }
    if headers
        .session_id
        .as_deref()
        .is_some_and(|id| !valid_http_session_id(id))
    {
        return write_http(
            stream,
            400,
            "text/plain; charset=utf-8",
            None,
            b"invalid MCP-Session-Id",
        );
    }

    let path = headers.path.split('?').next().unwrap_or("/");
    if path != "/mcp" && path != "/mcp/" {
        return write_http(stream, 404, "text/plain; charset=utf-8", None, b"not found");
    }

    match headers.method.to_ascii_uppercase().as_str() {
        "GET" => {
            if let Err(error) = validate_http_session(
                state,
                headers.session_id.as_deref(),
                headers.protocol_version.as_deref(),
                false,
            ) {
                return write_http_session_validation_error(stream, error);
            }
            // Optional SSE listen; we offer a minimal open stream then close.
            if media_type_list_contains(&headers.accept, "text/event-stream") {
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
            let Some(id) = headers.session_id.as_deref() else {
                return write_http_session_validation_error(
                    stream,
                    SessionValidationError::MissingSession,
                );
            };
            if let Err(error) =
                validate_http_session(state, Some(id), headers.protocol_version.as_deref(), true)
            {
                return write_http_session_validation_error(stream, error);
            }
            state.remove_session(id);
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
    if headers
        .session_id
        .as_deref()
        .is_some_and(|id| !valid_http_session_id(id))
    {
        return write_http(
            stream,
            400,
            "text/plain; charset=utf-8",
            None,
            b"invalid MCP-Session-Id",
        );
    }
    let valid_content_type = headers
        .content_type
        .as_deref()
        .is_some_and(is_json_content_type);
    if !valid_content_type {
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

    let method = value.get("method").and_then(Value::as_str);
    let is_initialize = method == Some("initialize");
    if value
        .as_array()
        .is_some_and(|items| items.iter().any(is_initialize_message))
    {
        return write_http(
            stream,
            400,
            "application/json",
            None,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"initialize must not be part of a JSON-RPC batch"}}"#,
        );
    }
    if is_initialize && is_legacy_http_initialize(&value) {
        return write_http(
            stream,
            400,
            "application/json",
            None,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32602,"message":"protocol 2024-11-05 requires the deprecated HTTP+SSE transport; use Streamable HTTP with 2025-03-26 or 2025-11-25"}}"#,
        );
    }
    if is_initialize && headers.session_id.is_some() {
        return write_http(
            stream,
            400,
            "application/json",
            None,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"initialize must not include MCP-Session-Id"}}"#,
        );
    }
    if is_initialize
        && !value
            .get("id")
            .is_some_and(|id| id.is_string() || id.as_i64().is_some() || id.as_u64().is_some())
    {
        return write_http(
            stream,
            400,
            "application/json",
            None,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"initialize must be a request with a string or integer id"}}"#,
        );
    }
    if !is_initialize {
        if let Err(error) = validate_http_session(
            state,
            headers.session_id.as_deref(),
            headers.protocol_version.as_deref(),
            request_requires_http_session(&value),
        ) {
            return write_http_session_validation_error(stream, error);
        }
    }
    if value.is_array() && headers.protocol_version.as_deref() != Some("2025-03-26") {
        return write_http(
            stream,
            400,
            "application/json",
            headers.session_id.as_deref(),
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"JSON-RPC batching is not supported by the negotiated MCP protocol version"}}"#,
        );
    }
    if matches!(
        method,
        Some("notifications/initialized" | "notifications/cancelled")
    ) && value.get("id").is_some()
    {
        return write_http(
            stream,
            400,
            "application/json",
            headers.session_id.as_deref(),
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"MCP notifications must not include an id"}}"#,
        );
    }

    // Client responses are accepted only after the same session/protocol checks
    // as requests: they still belong to an established MCP session.
    if (value.get("result").is_some() || value.get("error").is_some())
        && value.get("method").is_none()
    {
        return write_http(stream, 202, "text/plain; charset=utf-8", None, b"");
    }

    // Cancellation notifications may arrive on a separate connection from the
    // request they target, so scan every body before dispatching it.
    apply_http_cancellations(&value, state, headers.session_id.as_deref());

    // Batch members stay in the bounded connection worker; per-item threads
    // would multiply the connection limit by the batch-size limit.
    let outcome = if value.is_array() {
        dispatch_body_bounded(&value, state, headers.session_id.as_deref())
    } else {
        dispatch_http_value(&value, state, headers.session_id.as_deref())
    };

    match outcome {
        DispatchBodyResult::Accepted => write_http(
            stream,
            202,
            "text/plain; charset=utf-8",
            headers.session_id.as_deref(),
            b"",
        ),
        DispatchBodyResult::Json(response) => {
            let new_session = if is_initialize && response.get("error").is_none() {
                let protocol_version = response
                    .get("result")
                    .and_then(|result| result.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or(super::MCP_PROTOCOL_VERSION)
                    .to_string();
                let Some(token) = generate_session_token() else {
                    return write_http(
                        stream,
                        500,
                        "text/plain; charset=utf-8",
                        None,
                        b"unable to generate a secure MCP session id",
                    );
                };
                let session = format!("keel-{token}");
                if !state.register_session(session.clone(), protocol_version) {
                    return write_http(
                        stream,
                        503,
                        "text/plain; charset=utf-8",
                        None,
                        b"MCP session capacity reached",
                    );
                }
                Some(session)
            } else {
                None
            };
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

fn is_initialize_message(value: &Value) -> bool {
    value.get("method").and_then(Value::as_str) == Some("initialize")
}

fn is_legacy_http_initialize(value: &Value) -> bool {
    value
        .get("params")
        .and_then(Value::as_object)
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        == Some(super::MCP_LEGACY_PROTOCOL_VERSION)
}

fn request_requires_http_session(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(request_requires_http_session),
        Value::Object(object) => {
            if object.get("method").is_none()
                && (object.get("result").is_some() || object.get("error").is_some())
            {
                return true;
            }
            matches!(
                object.get("method").and_then(Value::as_str),
                Some(method) if !matches!(method, "initialize" | "ping")
            )
        }
        _ => false,
    }
}

fn http_cancellation_key(session_id: Option<&str>, request_id: &Value) -> Option<String> {
    let request_key = super::cancellation_key(request_id)?;
    let key = format!("{}\0{request_key}", session_id.unwrap_or(""));
    (key.len() <= MAX_HTTP_CANCELLATION_KEY_BYTES).then_some(key)
}

fn apply_http_cancellations(value: &Value, state: &HttpState, session_id: Option<&str>) {
    let messages: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    state.with_live_cancellations(|cancellations, _| {
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
                cancellation.token.store(true, Ordering::Release);
            }
        }
    });
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
        if !state.register_cancellation(key.clone(), Arc::clone(&cancellation)) {
            return DispatchBodyResult::Json(super::error_response(
                value.get("id").cloned().unwrap_or(Value::Null),
                JSON_RPC_INTERNAL_ERROR,
                "request cancellation registry is full or request id is already in use",
            ));
        }
    }
    let request_context = super::McpRequestContext::authoritative(session_id);
    let response = super::dispatch_cancellable_with_context(value, &cancellation, &request_context);
    if let Some(key) = key.as_ref() {
        state.unregister_cancellation(key, &cancellation);
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
        let mut registration_failure = false;
        for (item, cancellation) in items.iter().zip(&batch_cancellations) {
            if let Some(key) = item
                .get("id")
                .and_then(|request_id| http_cancellation_key(session_id, request_id))
            {
                if !state.register_cancellation(key, Arc::clone(cancellation)) {
                    registration_failure = true;
                    break;
                }
            }
        }
        if registration_failure {
            for (item, cancellation) in items.iter().zip(&batch_cancellations) {
                if let Some(key) = item
                    .get("id")
                    .and_then(|request_id| http_cancellation_key(session_id, request_id))
                {
                    state.unregister_cancellation(&key, cancellation);
                }
            }
            return DispatchBodyResult::Json(super::error_response(
                Value::Null,
                JSON_RPC_INTERNAL_ERROR,
                "request cancellation registry is full or request id is already in use",
            ));
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
        for (item, cancellation) in items.iter().zip(&batch_cancellations) {
            if let Some(key) = item
                .get("id")
                .and_then(|request_id| http_cancellation_key(session_id, request_id))
            {
                state.unregister_cancellation(&key, cancellation);
            }
        }
    }
    if responses.is_empty() {
        DispatchBodyResult::Accepted
    } else {
        DispatchBodyResult::Json(Value::Array(responses))
    }
}

fn generate_session_token() -> Option<String> {
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).ok()?;
    let token = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(token)
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
        let response = http_round_trip(Arc::new(HttpState::default()), &null_origin);
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");

        let mut json_prefix = String::from(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json-seq\r\n\
             Accept: application/json, text/event-stream\r\n",
        );
        json_prefix.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        json_prefix.push_str(&String::from_utf8_lossy(body));
        let response = http_round_trip(Arc::new(HttpState::default()), json_prefix.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 415 Unsupported Media Type"),
            "{response}"
        );
    }

    #[test]
    fn http_post_ping_roundtrip() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = http_post_request(body, &[("Origin", "http://127.0.0.1")]);
        let text = http_round_trip(Arc::new(HttpState::default()), &request);
        assert!(
            text.contains("200") && text.contains("\"id\":1"),
            "response={text}"
        );
    }

    #[test]
    fn foreign_origin_is_forbidden() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = http_post_request(body, &[("Origin", "https://evil.example")]);
        let text = http_round_trip(Arc::new(HttpState::default()), &request);
        assert!(text.contains("403"), "response={text}");
    }

    #[test]
    fn initialize_returns_server_info() {
        let result = super::super::dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": super::super::MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
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
    fn http_sessions_expire_and_refresh_on_use() {
        let state = HttpState::default();
        {
            let mut sessions = state.sessions.lock().expect("session lock");
            sessions.insert(
                "expired".to_string(),
                HttpSession {
                    last_seen: Instant::now() - Duration::from_secs(901),
                    protocol_version: super::super::MCP_PROTOCOL_VERSION.to_string(),
                },
            );
        }
        state.purge_expired_sessions();
        assert!(!state.touch_session("expired"));

        assert!(state.register_session(
            "live".to_string(),
            super::super::MCP_PROTOCOL_VERSION.to_string(),
        ));
        assert!(state.touch_session("live"));
        assert_eq!(state.sessions.lock().expect("session lock").len(), 1);
    }

    /// §29.4 session-expiry-during-call: a TTL shorter than the tool deadline
    /// must not expire a session that still owns a cancellable request. The
    /// effective TTL is stretched through the owner deadlines instead.
    #[test]
    fn session_ttl_outlives_an_inflight_call() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        env::set_var("KEEL_MCP_SESSION_TTL_SECONDS", "1");
        // why: an absent override is the normal case; only its presence matters.
        let previous = env::var("KEEL_MCP_SESSION_TTL_SECONDS").ok();

        let effective = effective_http_session_ttl();
        assert!(
            effective >= crate::mcp::tools::mcp_child_timeout(),
            "a session must outlive its own tool call: {effective:?}"
        );
        assert!(effective >= HTTP_BATCH_WALL_BUDGET, "{effective:?}");

        // A session idle just past the configured TTL but still inside the
        // stretched window must survive, so its in-flight call stays cancellable.
        let state = HttpState::default();
        {
            let mut sessions = state.sessions.lock().expect("session lock");
            sessions.insert(
                "inflight".to_string(),
                HttpSession {
                    last_seen: Instant::now() - Duration::from_secs(2),
                    protocol_version: super::super::MCP_PROTOCOL_VERSION.to_string(),
                },
            );
        }
        state.purge_expired_sessions();
        assert!(
            state.touch_session("inflight"),
            "a session inside its stretched TTL must not be reaped"
        );

        // A session idle past the stretched window is still reaped: the bound
        // is extended, never removed.
        {
            let mut sessions = state.sessions.lock().expect("session lock");
            sessions.insert(
                "abandoned".to_string(),
                HttpSession {
                    last_seen: Instant::now() - effective - Duration::from_secs(5),
                    protocol_version: super::super::MCP_PROTOCOL_VERSION.to_string(),
                },
            );
        }
        state.purge_expired_sessions();
        assert!(
            !state.touch_session("abandoned"),
            "stale state must not linger"
        );

        match previous {
            Some(value) => env::set_var("KEEL_MCP_SESSION_TTL_SECONDS", value),
            None => env::remove_var("KEEL_MCP_SESSION_TTL_SECONDS"),
        }
    }

    #[test]
    fn http_session_binds_the_negotiated_protocol_version() {
        let state = HttpState::default();
        assert!(state.register_session("bound".to_string(), "2025-03-26".to_string()));
        assert!(!state.register_session("bound".to_string(), "2025-11-25".to_string()));
        assert_eq!(
            state.touch_session_with_protocol("bound", Some("2025-03-26")),
            Ok(())
        );
        assert_eq!(
            state.touch_session_with_protocol("bound", Some("2025-11-25")),
            Err(SessionValidationError::ProtocolMismatch)
        );
        assert_eq!(
            state.touch_session_with_protocol("missing", Some("2025-03-26")),
            Err(SessionValidationError::Unknown)
        );
    }

    #[test]
    fn http_session_capacity_refuses_new_sessions_without_evicting_live_ones() {
        let state = HttpState::default();
        for index in 0..MAX_HTTP_SESSIONS {
            assert!(state.register_session(format!("session-{index}"), "2025-11-25".to_string()));
        }
        assert!(!state.register_session("overflow".to_string(), "2025-11-25".to_string()));
        assert!(!state.register_session("session-0".to_string(), "2025-11-25".to_string()));
        assert!(state.touch_session("session-0"));
        assert_eq!(
            state.sessions.lock().expect("session lock").len(),
            MAX_HTTP_SESSIONS
        );
    }

    #[test]
    fn cancellation_registry_is_bounded_and_expirable() {
        let state = HttpState::default();
        let token = Arc::new(AtomicBool::new(false));
        for index in 0..MAX_HTTP_CANCELLATIONS {
            assert!(state.register_cancellation(
                format!("session\0{index}"),
                Arc::new(AtomicBool::new(false)),
            ));
        }
        assert!(
            !state.register_cancellation("session\0overflow".to_string(), token.clone()),
            "a saturated cancellation registry must reject new work"
        );

        {
            let mut registrations = state.cancellations.lock().expect("cancellation lock");
            registrations.insert(
                "session\0expired".to_string(),
                CancellationRegistration {
                    token,
                    registered_at: Instant::now() - Duration::from_secs(901),
                    expires_at: Instant::now() - Duration::from_secs(1),
                },
            );
        }
        state.with_live_cancellations(|registrations, _| {
            assert!(
                !registrations.contains_key("session\0expired"),
                "expired cancellation must be purged before use"
            );
        });
        assert_eq!(
            state.cancellations.lock().expect("cancellation lock").len(),
            MAX_HTTP_CANCELLATIONS
        );
    }

    #[test]
    fn active_cancellation_survives_ttl_until_worker_unregisters() {
        let state = HttpState::default();
        let token = Arc::new(AtomicBool::new(false));
        {
            let mut registrations = state.cancellations.lock().expect("cancellation lock");
            registrations.insert(
                "session\0long-running".to_string(),
                CancellationRegistration {
                    token: Arc::clone(&token),
                    registered_at: Instant::now() - Duration::from_secs(901),
                    expires_at: Instant::now() + Duration::from_secs(1),
                },
            );
        }

        state.with_live_cancellations(|registrations, _| {
            assert!(
                registrations.contains_key("session\0long-running"),
                "an active request must remain cancellable until its owner deadline"
            );
        });
        state.unregister_cancellation("session\0long-running", &token);
        assert!(state
            .cancellations
            .lock()
            .expect("cancellation lock")
            .is_empty());
    }

    #[test]
    fn cancellation_registration_ttl_covers_request_owner_deadlines() {
        let state = HttpState::default();
        let started = Instant::now();
        let token = Arc::new(AtomicBool::new(false));
        assert!(state.register_cancellation("session\0deadline".to_string(), token));
        let registrations = state.cancellations.lock().expect("cancellation lock");
        let registration = registrations
            .get("session\0deadline")
            .expect("registration");
        let minimum_expiry =
            started + cancellation_registration_ttl().saturating_sub(Duration::from_millis(1));
        assert!(
            registration.expires_at >= minimum_expiry,
            "registration expiry must cover the configured and owner deadlines"
        );
        assert!(cancellation_registration_ttl() >= HTTP_BATCH_WALL_BUDGET);
        assert!(cancellation_registration_ttl() >= super::super::tools::mcp_child_timeout());
    }

    #[test]
    fn session_and_cancellation_identity_values_are_bounded() {
        assert!(valid_http_session_id("keel-session"));
        assert!(!valid_http_session_id(
            &"x".repeat(MAX_HTTP_SESSION_ID_BYTES + 1)
        ));
        assert!(!valid_http_session_id("session\r\nX-Injected: yes"));
        assert!(http_cancellation_key(Some("session"), &json!("short")).is_some());
        assert!(http_cancellation_key(
            Some("session"),
            &json!("x".repeat(super::super::MAX_CANCELLATION_ID_BYTES + 1)),
        )
        .is_none());
    }

    #[test]
    fn stateful_http_request_detection_covers_tools_list_and_batches() {
        assert!(request_requires_http_session(&json!({
            "jsonrpc": "2.0",
            "method": "tools/list"
        })));
        assert!(request_requires_http_session(&json!([
            {"jsonrpc": "2.0", "method": "ping"},
            {"jsonrpc": "2.0", "method": "tools/list"}
        ])));
        assert!(request_requires_http_session(&json!({
            "jsonrpc": "2.0",
            "method": "resources/list"
        })));
        assert!(!request_requires_http_session(&json!({
            "jsonrpc": "2.0",
            "method": "ping"
        })));
    }

    #[test]
    fn initialize_requires_a_request_id() {
        let body = br#"{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
        let state = Arc::new(HttpState::default());
        let response = http_round_trip(Arc::clone(&state), &http_post_request(body, &[]));
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(
            response.contains("initialize must be a request"),
            "{response}"
        );
        assert!(state.sessions.lock().expect("session lock").is_empty());
    }

    #[test]
    fn initialized_notification_rejects_a_request_id() {
        let state = Arc::new(HttpState::default());
        assert!(state.register_session("initialized-session".to_string(), "2025-11-25".to_string()));
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"notifications/initialized"}"#;
        let request = http_post_request(
            body,
            &[
                ("MCP-Session-Id", "initialized-session"),
                ("MCP-Protocol-Version", "2025-11-25"),
            ],
        );
        let response = http_round_trip(state, &request);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(
            response.contains("notifications must not include an id"),
            "{response}"
        );
    }

    #[test]
    fn negotiated_protocol_controls_http_batch_support() {
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#;
        for (version, expected_status) in [("2025-11-25", 400), ("2025-03-26", 200)] {
            let state = Arc::new(HttpState::default());
            assert!(state.register_session("batch-session".to_string(), version.to_string()));
            let request = http_post_request(
                body,
                &[
                    ("MCP-Session-Id", "batch-session"),
                    ("MCP-Protocol-Version", version),
                ],
            );
            let response = http_round_trip(state, &request);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {expected_status}")),
                "version={version} response={response}"
            );
        }
    }

    #[test]
    fn http_tools_list_without_session_is_rejected_before_dispatch() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let text = http_round_trip(
            Arc::new(HttpState::default()),
            &http_post_request(body, &[]),
        );
        assert!(text.contains("400"), "response={text}");
        assert!(text.contains("MCP-Session-Id required"), "response={text}");
    }

    #[test]
    fn http_client_response_without_session_is_rejected_before_accept() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let text = http_round_trip(
            Arc::new(HttpState::default()),
            &http_post_request(body, &[]),
        );
        assert!(text.contains("400"), "response={text}");
        assert!(text.contains("MCP-Session-Id required"), "response={text}");
    }

    #[test]
    fn http_initialize_batch_is_rejected_before_dispatch() {
        let state = Arc::new(HttpState::default());
        let body = serde_json::to_vec(&json!([{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": super::super::MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }]))
        .expect("serialize body");
        let text = http_round_trip(Arc::clone(&state), &http_post_request(&body, &[]));
        assert!(text.contains("400"), "response={text}");
        assert!(
            text.contains("initialize must not be part of a JSON-RPC batch"),
            "response={text}"
        );
        assert!(
            state.sessions.lock().expect("session lock").is_empty(),
            "a rejected initialize batch must not create a session"
        );
    }

    #[test]
    fn http_legacy_initialize_is_rejected_for_streamable_http() {
        let state = Arc::new(HttpState::default());
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": super::super::MCP_LEGACY_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "1.0.0" }
            }
        }))
        .expect("serialize body");
        let text = http_round_trip(Arc::clone(&state), &http_post_request(&body, &[]));
        assert!(text.contains("400"), "response={text}");
        assert!(text.contains("2024-11-05"), "response={text}");
        assert!(text.contains("HTTP+SSE transport"), "response={text}");
        assert!(
            state.sessions.lock().expect("session lock").is_empty(),
            "a rejected legacy initialize must not create a session"
        );
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
            .token
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
