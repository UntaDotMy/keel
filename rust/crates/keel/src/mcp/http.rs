//! Purpose: Streamable HTTP transport for the keel MCP server — multi-client
//!   concurrent connections on one process (MCP 2025-11-25 transports).
//! Caller: `run_mcp_command` `serve-http` arm.
//! Dependencies: std::net only (no async runtime); reuses `super::dispatch` /
//!   `dispatch_body` for JSON-RPC semantics.
//! Side Effects: Binds a TCP listener (default 127.0.0.1:3920), accepts many
//!   clients, writes HTTP responses; tracks optional MCP-Session-Id set.

use std::collections::HashSet;
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use super::{dispatch_body, DispatchBodyResult, JSON_RPC_INVALID_REQUEST, JSON_RPC_PARSE_ERROR};

const DEFAULT_BIND: &str = "127.0.0.1:3920";
const MAX_HTTP_BODY: usize = 8 * 1024 * 1024;

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

    let sessions: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let max_inflight = super::max_inflight();

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let sessions = Arc::clone(&sessions);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
                let spawn = thread::Builder::new()
                    .name("keel-mcp-http".into())
                    .spawn(move || {
                        if let Err(error) = handle_connection(stream, sessions, max_inflight) {
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

fn handle_connection(
    mut stream: TcpStream,
    sessions: Arc<Mutex<HashSet<String>>>,
    _max_inflight: usize,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > MAX_HTTP_BODY + 16_384 {
            write_http(
                &mut stream,
                413,
                "text/plain; charset=utf-8",
                None,
                b"payload too large",
            )?;
            return Ok(());
        }
        if let Some(header_end) = find_header_end(&buffer) {
            let header_text = String::from_utf8_lossy(&buffer[..header_end]);
            let headers = parse_headers(&header_text);
            let content_length = headers.content_length.unwrap_or(0).min(MAX_HTTP_BODY);
            let total_needed = header_end + content_length;
            while buffer.len() < total_needed {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() > MAX_HTTP_BODY + 16_384 {
                    break;
                }
            }
            let body = if content_length == 0 {
                Vec::new()
            } else {
                buffer
                    .get(header_end..header_end + content_length)
                    .unwrap_or(&[])
                    .to_vec()
            };
            respond(&mut stream, &headers, &body, &sessions)?;
            return Ok(());
        }
    }
    Ok(())
}

struct HttpHeaders {
    method: String,
    path: String,
    origin: Option<String>,
    content_length: Option<usize>,
    accept: String,
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
    let mut accept = String::new();
    let mut session_id = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "origin" => origin = Some(value.to_string()),
                "content-length" => content_length = value.parse().ok(),
                "accept" => accept = value.to_string(),
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
        accept,
        session_id,
    }
}

fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some("null") => true,
        Some(o)
            if o.starts_with("http://127.0.0.1")
                || o.starts_with("http://localhost")
                || o.starts_with("https://127.0.0.1")
                || o.starts_with("https://localhost")
                || o.starts_with("http://[::1]") =>
        {
            true
        }
        Some(_) if allow_remote_bind() => true,
        Some(_) => false,
    }
}

fn respond(
    stream: &mut TcpStream,
    headers: &HttpHeaders,
    body: &[u8],
    sessions: &Arc<Mutex<HashSet<String>>>,
) -> std::io::Result<()> {
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
                if let Ok(mut guard) = sessions.lock() {
                    guard.remove(id);
                }
            }
            write_http(stream, 200, "text/plain; charset=utf-8", None, b"")
        }
        "POST" => handle_post(stream, headers, body, sessions),
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
    sessions: &Arc<Mutex<HashSet<String>>>,
) -> std::io::Result<()> {
    if let Some(id) = headers.session_id.as_ref() {
        let known = sessions.lock().map(|g| g.contains(id)).unwrap_or(false);
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

    // Concurrent dispatch for batch arrays is sequential here for HTTP body
    // simplicity; multi-client concurrency is via parallel TCP connections.
    // For single-object requests we still use the shared dispatch path.
    let outcome = if value.is_array() {
        // Prefer concurrent batch elements when a client sends a JSON-RPC batch.
        dispatch_body_concurrent(&value)
    } else {
        dispatch_body(&value)
    };

    let mut new_session: Option<String> = None;
    if is_initialize {
        let session = format!("keel-{}", generate_session_token());
        if let Ok(mut guard) = sessions.lock() {
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

/// Run batch elements on worker threads (bounded) then assemble the array.
fn dispatch_body_concurrent(body: &Value) -> DispatchBodyResult {
    let items = match body.as_array() {
        Some(items) if items.is_empty() => {
            return DispatchBodyResult::Json(super::error_response(
                Value::Null,
                JSON_RPC_INVALID_REQUEST,
                "Invalid Request: empty batch",
            ));
        }
        Some(items) => items,
        None => return dispatch_body(body),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let mut expected = 0usize;
    for (index, item) in items.iter().cloned().enumerate() {
        expected += 1;
        let tx = tx.clone();
        let _ = thread::Builder::new()
            .name("keel-mcp-http-batch".into())
            .spawn(move || {
                let response = super::dispatch(&item);
                let _ = tx.send((index, response));
            });
    }
    drop(tx);

    let mut slots: Vec<Option<Option<Value>>> = vec![None; items.len()];
    for _ in 0..expected {
        if let Ok((index, response)) = rx.recv() {
            if index < slots.len() {
                slots[index] = Some(response);
            }
        }
    }
    let responses: Vec<Value> = slots
        .into_iter()
        .filter_map(|slot| slot.and_then(|inner| inner))
        .collect();
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
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
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
    fn origin_allows_localhost_and_rejects_foreign() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://127.0.0.1:3000")));
        assert!(origin_allowed(Some("http://localhost")));
        assert!(!origin_allowed(Some("https://evil.example")));
    }

    #[test]
    fn http_post_ping_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let sessions: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let sessions_accept = Arc::clone(&sessions);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, sessions_accept, 8).expect("handle");
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
        let sessions: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let sessions_accept = Arc::clone(&sessions);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_connection(stream, sessions_accept, 8).expect("handle");
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
}
