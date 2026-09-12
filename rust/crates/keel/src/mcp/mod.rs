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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum McpCatalogProfile {
    #[default]
    Tiered,
    Full,
}

impl McpCatalogProfile {
    pub fn from_env() -> Self {
        match std::env::var("KEEL_MCP_CATALOG_PROFILE")
            .ok()
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("full") | Some("all") => Self::Full,
            _ => Self::Tiered,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tiered => "core",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolsListContextSnapshot {
    pub tool_count: usize,
    pub eager_tool_count: usize,
    pub deferred_tool_count: usize,
    /// What a client actually receives on the normal handshake: the packed
    /// default page from the dispatcher's own path.
    pub handshake_tokens: usize,
    /// The complete catalog at full schema. Reported separately because it is a
    /// real cost, but no client receives it on the default handshake.
    pub catalog_tokens: usize,
}

/// Exact cost of the response the dispatcher serves for a default `tools/list`.
/// The ledger and its regression test both read this, so the reported handshake
/// cost cannot drift from the emitted one.
pub(crate) fn measured_handshake_tokens(profile: McpCatalogProfile) -> usize {
    let context = McpRequestContext::authoritative(None);
    match tools::handle_tools_list_for_profile_params_with_context(
        profile,
        &serde_json::Value::Null,
        &context,
    ) {
        Ok(page) => tools::measure_tools_list_response(&page),
        Err(_) => {
            // A rejected handshake still needs a number; the unpaged catalog is
            // fallback: the closest available upper bound.
            let serialized = serde_json::to_string(&tools::handle_tools_list()).unwrap_or_default();
            crate::proxy::token_meter::TokenMeter::count_text(&serialized)
        }
    }
}

pub(crate) fn tools_list_context_snapshot() -> ToolsListContextSnapshot {
    let profile = McpCatalogProfile::from_env();
    let eager_tool_count = match profile {
        McpCatalogProfile::Tiered => tools::EAGER_MCP_TOOL_NAMES.len(),
        McpCatalogProfile::Full => tools::MCP_TOOL_NAMES.len(),
    };
    let deferred_tool_count = match profile {
        McpCatalogProfile::Tiered => tools::DEFERRED_MCP_TOOL_NAMES.len(),
        McpCatalogProfile::Full => 0,
    };
    // The catalog is hand-built JSON, so serialization cannot fail here.
    // why: an improbable failure measures zero rather than a wrong number.
    let complete = serde_json::to_string(&tools_complete_catalog(profile)).unwrap_or_default();
    ToolsListContextSnapshot {
        tool_count: tools::MCP_TOOL_NAMES.len(),
        eager_tool_count,
        deferred_tool_count,
        handshake_tokens: measured_handshake_tokens(profile),
        catalog_tokens: crate::proxy::token_meter::TokenMeter::count_text(&complete),
    }
}

pub(crate) fn tools_page_size() -> usize {
    tools::tools_page_size()
}

pub(crate) fn tools_discovery_snapshot() -> serde_json::Value {
    tools::discovery_snapshot()
}

/// Read one page through the canonical packing owner. The `stats tools
/// --benchmark` harness walks a profile exactly the way a client would, so it
/// must not build a second catalog path of its own.
pub(crate) fn tools_list_page(
    profile: McpCatalogProfile,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    tools::handle_tools_list_for_profile_params(profile, params)
}

/// The authoritative exact measurement for one emitted `tools/list` response.
pub(crate) fn measure_tools_list_response(payload: &serde_json::Value) -> usize {
    tools::measure_tools_list_response(payload)
}

/// The complete, unpaginated catalog for a profile: the comparison point the
/// benchmark uses for "every tool advertised eagerly".
pub(crate) fn tools_complete_catalog(profile: McpCatalogProfile) -> serde_json::Value {
    tools::handle_tools_list_for_profile(profile)
}

/// Ranked capability discovery, used to check that a deferred tool stays
/// nameable when it is not advertised on the first page.
pub(crate) fn tools_discover(
    query: &str,
    limit: usize,
    level: u64,
) -> Result<serde_json::Value, String> {
    tools::discover_capabilities(query, limit, level)
}

pub(crate) fn tools_stored_tool_count() -> usize {
    tools::MCP_TOOL_NAMES.len()
}

/// Advertised-tool count for the tiered profile. Only the benchmark regression
/// test needs the split form; production callers use the profile snapshot.
#[cfg(test)]
pub(crate) fn tools_eager_tool_count() -> usize {
    tools::EAGER_MCP_TOOL_NAMES.len()
}

/// Parse one JSON value per line, skipping anything that is not JSON.
/// why: a stdio session may interleave non-frame diagnostics with real replies.
#[cfg(test)]
pub(crate) fn parse_json_lines(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        // why: a line that is not JSON is a diagnostic, not a frame to report.
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

pub(crate) fn current_mcp_session_id() -> Option<String> {
    ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"]
        .iter()
        .find_map(|name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// Authoritative identity for one MCP request. HTTP sessions come from the
/// server-issued `MCP-Session-Id` header; stdio falls back to the host session
/// environment. The workspace is always the server process cwd. Client
/// arguments may describe a requested identity, but they never establish it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpRequestContext {
    pub(crate) session_id: String,
    pub(crate) workspace_id: String,
    pub(crate) request_id: Option<String>,
}

impl McpRequestContext {
    pub(crate) fn authoritative(http_session_id: Option<&str>) -> Self {
        let session_id = http_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(current_mcp_session_id)
            .unwrap_or_else(|| "default".to_string());
        let workspace_id = std::env::current_dir()
            .ok()
            .and_then(|path| path.canonicalize().ok().or(Some(path)))
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-workspace".to_string());
        Self {
            session_id,
            workspace_id,
            request_id: None,
        }
    }

    fn with_request_id(&self, request_id: Option<&Value>) -> Self {
        let mut scoped = self.clone();
        scoped.request_id = request_id.map(ToString::to_string);
        scoped
    }
}

/// Maximum bytes accepted for a single newline-delimited JSON-RPC frame. A peer
/// that streams data without a terminating newline would otherwise grow the
/// read buffer without bound and exhaust memory; capping the per-frame read
/// bounds that to a generous-but-finite size (8 MiB — far above any real tool
/// call, well below a DoS). An over-cap frame is refused, not truncated.
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;

/// The reader and request workers share one bounded event queue. A synchronous
/// channel applies backpressure to a streaming peer instead of allowing input
/// frames or completed responses to accumulate without limit.
const MAX_EVENT_QUEUE: usize = 128;

/// The test-only preload path also needs one slot for EOF/error after the
/// frames have been read. Production uses the channel directly and therefore
/// does not need this reserve.
#[cfg(test)]
const MAX_PRELOADED_EVENTS: usize = MAX_EVENT_QUEUE - 1;

/// Maximum parsed work waiting for a worker. This is separate from the worker
/// cap so a batch/input flood cannot turn into an unbounded `VecDeque` or batch
/// collector even while workers are busy.
const MAX_PENDING_JOBS: usize = 512;

/// Cancellation registrations are small, transient state, but a client can
/// keep many requests in flight. Keep this cap above the maximum worker count
/// while still making the collection finite.
const MAX_STDIO_CANCELLATIONS: usize = 1_024;

/// JSON-RPC ids are echoed in responses and cancellation notifications. Bound
/// their serialized key before storing or comparing them so a large string id
/// cannot turn one transient registration into megabytes of retained state.
const MAX_CANCELLATION_ID_BYTES: usize = 512;

/// Default max concurrent in-flight JSON-RPC requests per `mcp serve` process.
/// Hosts may stream multiple `tools/call`s before prior responses return; without
/// a cap a flood could spawn unbounded OS threads. Override with
/// `KEEL_MCP_MAX_INFLIGHT` (min 1, max 512). Multi-session scale (100+ harness
/// chats) is separate: each session is its own `keel mcp serve` process.
const DEFAULT_MAX_INFLIGHT: usize = 64;

/// Only the stateless, per-request metadata protocol is served.
pub(super) const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

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

// Listings describe a fixed workspace surface; reads return live state. Keep
// both cacheable, but never reusable through a shared intermediary cache.
const MCP_RESOURCE_LIST_CACHE_TTL_MS: u64 = 300_000;
const MCP_RESOURCE_READ_CACHE_TTL_MS: u64 = 60_000;
const MCP_RESOURCE_CACHE_SCOPE: &str = "private";

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
        "serve" | "serve-http" => {
            let (profile, remaining) = match profile_arguments(&arguments[1..]) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = writeln!(standard_error, "mcp: {error}");
                    return 1;
                }
            };
            let previous = env::var("KEEL_MCP_CATALOG_PROFILE").ok();
            if let Some(profile) = profile {
                env::set_var("KEEL_MCP_CATALOG_PROFILE", profile.as_str());
            }
            let result = if subcommand == "serve" {
                if !remaining.is_empty() {
                    let _ = writeln!(
                        standard_error,
                        "mcp serve: unexpected arguments: {}",
                        remaining.join(" ")
                    );
                    1
                } else {
                    serve_stdio_owned_stdin(standard_output, standard_error)
                }
            } else {
                http::serve_http(&remaining, standard_output, standard_error)
            };
            match previous {
                Some(value) => env::set_var("KEEL_MCP_CATALOG_PROFILE", value),
                None => env::remove_var("KEEL_MCP_CATALOG_PROFILE"),
            }
            result
        }
        "discover" => run_discover_command(&arguments[1..], standard_output, standard_error),
        "catalog" => run_catalog_command(&arguments[1..], standard_output, standard_error),
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
    let _ = writeln!(standard_output, "  keel mcp serve [--profile core|full]");
    let _ = writeln!(
        standard_output,
        "  keel mcp serve-http [--bind HOST:PORT] [--profile core|full]"
    );
    let _ = writeln!(
        standard_output,
        "  keel mcp discover <capability> [--limit N] [--level 0|1|2] [--json]"
    );
    let _ = writeln!(
        standard_output,
        "  keel mcp catalog [--profile core|full] [--budget N] [--json]"
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
        "  Tool wall-clock: KEEL_MCP_TOOL_TIMEOUT_SECS (default 25, under typical host ~30s)."
    );
    let _ = writeln!(
        standard_output,
        "  Text size: KEEL_MCP_MAX_TEXT_CHARS (default 12000); stdio frame cap 8MiB."
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

fn profile_arguments(
    arguments: &[String],
) -> Result<(Option<McpCatalogProfile>, Vec<String>), String> {
    let mut profile = None;
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--profile" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| "--profile requires core or full".to_string())?;
                profile = Some(match value.trim().to_ascii_lowercase().as_str() {
                    "core" | "tiered" => McpCatalogProfile::Tiered,
                    "full" => McpCatalogProfile::Full,
                    other => {
                        return Err(format!(
                            "unsupported profile {other:?}; expected core or full"
                        ))
                    }
                });
            }
            other => remaining.push(other.to_string()),
        }
        index += 1;
    }
    Ok((profile, remaining))
}

/// §31 diagnostics: the packing picture for the active profile at a supplied
/// budget. Every number comes from the packer that serves `tools/list`, so this
/// cannot describe a plan the server would not actually execute.
fn run_catalog_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut profile_override: Option<McpCatalogProfile> = None;
    let mut budget_override: Option<usize> = None;
    let mut json_output = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--json" => json_output = true,
            "--profile" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    let _ = writeln!(
                        standard_error,
                        "mcp catalog: --profile requires core or full"
                    );
                    return 1;
                };
                profile_override = Some(match value.trim().to_ascii_lowercase().as_str() {
                    "core" | "tiered" => McpCatalogProfile::Tiered,
                    "full" => McpCatalogProfile::Full,
                    other => {
                        let _ = writeln!(
                            standard_error,
                            "mcp catalog: unsupported profile {other:?}; expected core or full"
                        );
                        return 1;
                    }
                });
            }
            "--budget" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    let _ = writeln!(standard_error, "mcp catalog: --budget requires a number");
                    return 1;
                };
                match value.trim().parse::<usize>() {
                    Ok(parsed) if parsed > 0 => budget_override = Some(parsed),
                    _ => {
                        let _ = writeln!(
                            standard_error,
                            "mcp catalog: invalid budget {value:?}; expected a positive integer"
                        );
                        return 1;
                    }
                }
            }
            other => {
                let _ = writeln!(standard_error, "mcp catalog: unexpected argument {other:?}");
                return 1;
            }
        }
        index += 1;
    }

    let profile = profile_override.unwrap_or_else(McpCatalogProfile::from_env);
    let budget = budget_override.unwrap_or_else(|| tools::mcp_tools_list_budget(profile));
    // The budget owner reads its env var, so honour --budget through it.
    // why: an absent override is the normal case; only its presence is restored.
    let previous_budget = env::var("KEEL_MCP_PAGE_TOKENS").ok();
    if budget_override.is_some() {
        env::set_var("KEEL_MCP_PAGE_TOKENS", budget.to_string());
    }
    let report = tools::default_handshake_report(profile);
    match previous_budget {
        Some(value) => env::set_var("KEEL_MCP_PAGE_TOKENS", value),
        None => env::remove_var("KEEL_MCP_PAGE_TOKENS"),
    }
    let report = match report {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(standard_error, "mcp catalog: {error}");
            return 1;
        }
    };
    let visible = report.visible_tools;
    let first_page_tokens = report.first_page_tokens;
    let pagination = report.has_more;
    let snapshot = tools_list_context_snapshot();
    let catalog_snapshot =
        &report.snapshot_fingerprint[..12.min(report.snapshot_fingerprint.len())];

    if json_output {
        let payload = serde_json::json!({
            "schemaVersion": 1,
            "profile": profile.as_str(),
            "catalogSnapshot": catalog_snapshot,
            "visibleTools": visible,
            "visibleToolNames": report.visible_tool_names,
            "deferredTools": snapshot.deferred_tool_count,
            "totalTools": snapshot.tool_count,
            "hardPageBudget": budget,
            "firstPageTokens": first_page_tokens,
            "headroomTokens": budget.saturating_sub(first_page_tokens),
            "pagination": pagination,
            "pageSize": tools::tools_page_size(),
            "tokenizer": crate::utility::fixed_context::TOKENIZER,
            "reproductionCommand": format!(
                "keel mcp catalog --profile {} --budget {budget} --json",
                profile.as_str()
            ),
        });
        // The payload holds owned values, so serialization cannot fail here.
        // why: an improbable failure prints an empty object, never partial JSON.
        let rendered = serde_json::to_string_pretty(&payload).unwrap_or_default();
        let _ = writeln!(standard_output, "{rendered}");
        return 0;
    }

    let _ = writeln!(standard_output, "MCP profile: {}", profile.as_str());
    let _ = writeln!(standard_output, "Catalog snapshot: {catalog_snapshot}");
    let _ = writeln!(standard_output, "Visible tools: {visible}");
    let _ = writeln!(
        standard_output,
        "Deferred tools: {}",
        snapshot.deferred_tool_count
    );
    let _ = writeln!(standard_output, "Hard page budget: {budget}");
    let _ = writeln!(
        standard_output,
        "First page: {} / {budget} tokens",
        first_page_tokens
    );
    let _ = writeln!(
        standard_output,
        "Pagination: {}",
        if pagination {
            "enabled"
        } else {
            "idle (catalog fits)"
        }
    );
    0
}

fn run_discover_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut query = Vec::new();
    let mut limit = 5usize;
    let mut level = 1u64;
    let mut json_output = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--json" => json_output = true,
            "--limit" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    let _ = writeln!(standard_error, "mcp discover: --limit requires an integer");
                    return 1;
                };
                match value.parse::<usize>() {
                    Ok(value) if value > 0 => limit = value.min(20),
                    _ => {
                        let _ = writeln!(standard_error, "mcp discover: --limit must be positive");
                        return 1;
                    }
                }
            }
            "--level" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    let _ = writeln!(standard_error, "mcp discover: --level requires 0, 1, or 2");
                    return 1;
                };
                match value.parse::<u64>() {
                    Ok(value) if value <= 2 => level = value,
                    _ => {
                        let _ =
                            writeln!(standard_error, "mcp discover: --level must be 0, 1, or 2");
                        return 1;
                    }
                }
            }
            value => query.push(value.to_string()),
        }
        index += 1;
    }
    let query = query.join(" ");
    let payload = match tools::discover_capabilities(&query, limit, level) {
        Ok(payload) => payload,
        Err(error) => {
            let _ = writeln!(standard_error, "mcp discover: {error}");
            return 1;
        }
    };
    if json_output {
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "mcp discover: serialize: {error}");
                1
            }
        }
    } else {
        let _ = writeln!(
            standard_output,
            "keel mcp discover {:?} (level={}): {} match(es)",
            query,
            level,
            payload["count"].as_u64().unwrap_or(0)
        );
        if let Some(capabilities) = payload["capabilities"].as_array() {
            for capability in capabilities {
                let _ = writeln!(
                    standard_output,
                    "  {} [{}] score={} — {}",
                    capability["name"].as_str().unwrap_or("unknown"),
                    capability["category"].as_str().unwrap_or("keel"),
                    capability["score"].as_str().unwrap_or("0"),
                    capability["description"].as_str().unwrap_or("")
                );
            }
        }
        0
    }
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
    Response {
        value: Value,
        cancellation_key: Option<String>,
        cancellation: Arc<AtomicBool>,
    },
    /// A worker finished a notification (no response). Frees an in-flight slot.
    WorkerDone {
        cancellation_key: Option<String>,
        cancellation: Arc<AtomicBool>,
    },
}

/// One scheduled newline-delimited request.
struct PendingJob {
    request: Value,
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
    let (event_tx, event_rx) = mpsc::sync_channel::<ServeEvent>(MAX_EVENT_QUEUE);

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
                if preloaded.len() >= MAX_EVENT_QUEUE {
                    preloaded.clear();
                    preloaded.push_back(ServeEvent::ReaderError(
                        "event queue capacity exceeded".to_string(),
                    ));
                    break;
                }
                preloaded.push_back(ServeEvent::ReaderEof);
                break;
            }
            Ok(_) => {}
            Err(error) => {
                if preloaded.len() >= MAX_EVENT_QUEUE {
                    preloaded.clear();
                    preloaded.push_back(ServeEvent::ReaderError(
                        "event queue capacity exceeded".to_string(),
                    ));
                    break;
                }
                preloaded.push_back(ServeEvent::ReaderError(error.to_string()));
                break;
            }
        }
        if raw_line.len() as u64 >= MAX_FRAME_BYTES && raw_line.last() != Some(&b'\n') {
            if preloaded.len() >= MAX_EVENT_QUEUE {
                preloaded.clear();
                preloaded.push_back(ServeEvent::ReaderError(
                    "event queue capacity exceeded".to_string(),
                ));
                break;
            }
            preloaded.push_back(ServeEvent::OversizedFrame);
            break;
        }
        let line = String::from_utf8_lossy(&raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if preloaded.len() >= MAX_PRELOADED_EVENTS {
            preloaded.clear();
            preloaded.push_back(ServeEvent::ReaderError(
                "event queue capacity exceeded".to_string(),
            ));
            break;
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
    let (event_tx, event_rx) = mpsc::sync_channel::<ServeEvent>(MAX_EVENT_QUEUE);
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

fn read_frames_into(input: &mut dyn Read, event_tx: mpsc::SyncSender<ServeEvent>) {
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
        Value::Null | Value::String(_) | Value::Number(_) => {
            let Ok(key) = serde_json::to_string(request_id) else {
                // A scalar id that cannot be serialized is simply not cancellable;
                // the request still dispatches normally.
                return None;
            };
            (key.len() <= MAX_CANCELLATION_ID_BYTES).then_some(key)
        }
        _ => None,
    }
}

fn new_pending_job(
    request: Value,
    cancellations: &mut HashMap<String, Arc<AtomicBool>>,
) -> Result<PendingJob, PendingJobError> {
    let key = request.get("id").and_then(cancellation_key);
    let cancellation = Arc::new(AtomicBool::new(false));
    if let Some(key) = key.as_ref() {
        if cancellations.contains_key(key) {
            return Err(PendingJobError::DuplicateRequestId);
        }
        if cancellations.len() >= MAX_STDIO_CANCELLATIONS {
            return Err(PendingJobError::CancellationRegistryFull);
        }
        cancellations.insert(key.clone(), Arc::clone(&cancellation));
    }
    Ok(PendingJob {
        request,
        cancellation_key: key,
        cancellation,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingJobError {
    DuplicateRequestId,
    CancellationRegistryFull,
}

impl PendingJobError {
    fn code(self) -> i64 {
        match self {
            Self::DuplicateRequestId => JSON_RPC_INVALID_REQUEST,
            Self::CancellationRegistryFull => JSON_RPC_INTERNAL_ERROR,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::DuplicateRequestId => "request id is already in use",
            Self::CancellationRegistryFull => "server busy: cancellation registry is full",
        }
    }
}

fn rejected_request_id(request: &Value) -> Option<Value> {
    match request {
        Value::Object(object) => object.get("id").cloned(),
        // A non-object is an invalid JSON-RPC request, so it receives the required
        // null id response; an object without an id is a notification.
        _ => Some(Value::Null),
    }
}

fn rejected_request_response(request: &Value, code: i64, message: &str) -> Option<Value> {
    rejected_request_id(request).map(|id| error_response(id, code, message))
}

#[cfg(test)]
fn remove_pending_job_registration(
    cancellations: &mut HashMap<String, Arc<AtomicBool>>,
    job: &PendingJob,
) {
    remove_cancellation_registration(
        cancellations,
        job.cancellation_key.as_deref(),
        &job.cancellation,
    );
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
    event_tx: mpsc::SyncSender<ServeEvent>,
    event_rx: mpsc::Receiver<ServeEvent>,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    max_inflight: usize,
) -> u8 {
    let mut in_flight: usize = 0;
    let mut pending: VecDeque<PendingJob> = VecDeque::new();
    let mut cancellations: HashMap<String, Arc<AtomicBool>> = HashMap::new();
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
                        if pending.len() >= MAX_PENDING_JOBS {
                            if let Some(busy) = rejected_request_response(
                                &request,
                                JSON_RPC_INTERNAL_ERROR,
                                "server busy: pending request queue is full",
                            ) {
                                if write_framed_response(standard_output, standard_error, &busy)
                                    .is_err()
                                {
                                    exit_code = 1;
                                    reader_done = true;
                                    pending.clear();
                                }
                            }
                        } else {
                            let request_for_error = request.clone();
                            match new_pending_job(request, &mut cancellations) {
                                Ok(job) => pending.push_back(job),
                                Err(error) => {
                                    if let Some(response) = rejected_request_response(
                                        &request_for_error,
                                        error.code(),
                                        error.message(),
                                    ) {
                                        if write_framed_response(
                                            standard_output,
                                            standard_error,
                                            &response,
                                        )
                                        .is_err()
                                        {
                                            exit_code = 1;
                                            reader_done = true;
                                            pending.clear();
                                        }
                                    }
                                }
                            }
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
                    continue;
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
                cancellation_key,
                cancellation,
            } => {
                in_flight = in_flight.saturating_sub(1);
                remove_cancellation_registration(
                    &mut cancellations,
                    cancellation_key.as_deref(),
                    &cancellation,
                );
            }
        }

        while in_flight < max_inflight {
            let Some(job) = pending.pop_front() else {
                break;
            };
            in_flight += 1;
            let worker_tx = event_tx.clone();
            let request_id = job.request.get("id").cloned().unwrap_or(Value::Null);
            let cancellation_key = job.cancellation_key;
            let cancellation = job.cancellation;
            let worker_cancellation_key = cancellation_key.clone();
            let worker_cancellation = Arc::clone(&cancellation);
            let request = job.request;
            let request_context = McpRequestContext::authoritative(None);
            let spawn_result =
                thread::Builder::new()
                    .name("keel-mcp-req".into())
                    .spawn(move || {
                        match dispatch_cancellable_with_context(
                            &request,
                            &worker_cancellation,
                            &request_context,
                        ) {
                            Some(response) => {
                                let _ = worker_tx.send(ServeEvent::Response {
                                    value: response,
                                    cancellation_key: worker_cancellation_key,
                                    cancellation: worker_cancellation,
                                });
                            }
                            None => {
                                let _ = worker_tx.send(ServeEvent::WorkerDone {
                                    cancellation_key: worker_cancellation_key,
                                    cancellation: worker_cancellation,
                                });
                            }
                        }
                    });
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
                if write_framed_response(standard_output, standard_error, &response).is_err() {
                    exit_code = 1;
                    reader_done = true;
                    pending.clear();
                    break;
                }
            }
        }

        if reader_done && pending.is_empty() && in_flight == 0 {
            break;
        }
    }

    exit_code
}

enum FrameParse {
    /// Single JSON-RPC object (normal stdio line).
    Single(Value),
    /// Parse/shape error that must be answered immediately without a worker.
    Immediate(Value),
}

/// Parse one newline-delimited request; modern MCP forbids batch arrays.
fn parse_frame(frame: &str) -> FrameParse {
    let parsed: Result<Value, serde_json::Error> = serde_json::from_str(frame);
    match parsed {
        Err(parse_error) => FrameParse::Immediate(error_response(
            Value::Null,
            JSON_RPC_PARSE_ERROR,
            &format!("Parse error: {parse_error}"),
        )),
        Ok(Value::Array(_)) => FrameParse::Immediate(error_response(
            Value::Null,
            JSON_RPC_INVALID_REQUEST,
            "JSON-RPC batches are not supported by MCP 2026-07-28",
        )),
        Ok(other) => FrameParse::Single(other),
    }
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
    let context = McpRequestContext::authoritative(None);
    dispatch_cancellable_with_context(request, &Arc::new(AtomicBool::new(false)), &context)
}

pub(super) fn dispatch_cancellable_with_context(
    request: &Value,
    cancellation: &Arc<AtomicBool>,
    context: &McpRequestContext,
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
    let request_context = context.with_request_id(id.as_ref());

    // Notifications never dispatch request handlers or produce responses.
    let is_notification = id.is_none();

    if is_notification {
        return None;
    }

    let request_id = id.unwrap_or(Value::Null);
    if !request_id.is_string() && request_id.as_i64().is_none() && request_id.as_u64().is_none() {
        return Some(error_response(
            Value::Null,
            JSON_RPC_INVALID_REQUEST,
            "Request id must be a string or integer",
        ));
    }
    if method == "initialize" {
        return Some(unsupported_protocol_response(
            request_id,
            params.get("protocolVersion").and_then(Value::as_str),
        ));
    }
    if let Err(response) = validate_request_metadata(&params, &request_id) {
        return Some(response);
    }
    Some(
        match handle_method_cancellable(&method, &params, cancellation, &request_context) {
            Ok(result) => success_response(request_id, result),
            Err(MethodError { code, message }) => error_response(request_id, code, &message),
        },
    )
}

pub(super) fn unsupported_protocol_response(id: Value, requested: Option<&str>) -> Value {
    let mut response = error_response(
        id,
        -32022,
        "Unsupported protocol version; supported: 2026-07-28",
    );
    response["error"]["data"] =
        json!({"supported": [MCP_PROTOCOL_VERSION], "requested": requested});
    response
}

pub(super) fn validate_request_metadata(params: &Value, id: &Value) -> Result<(), Value> {
    let meta = params.get("_meta");
    let Some(version) = meta
        .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
    else {
        return Err(error_response(
            id.clone(),
            JSON_RPC_INVALID_PARAMS,
            "Missing required _meta protocolVersion",
        ));
    };
    if version != MCP_PROTOCOL_VERSION {
        return Err(unsupported_protocol_response(id.clone(), Some(version)));
    }
    if !meta
        .and_then(|m| m.get("io.modelcontextprotocol/clientCapabilities"))
        .is_some_and(Value::is_object)
    {
        return Err(error_response(
            id.clone(),
            JSON_RPC_INVALID_PARAMS,
            "Missing or invalid _meta clientCapabilities",
        ));
    }
    if let Some(info) = meta.and_then(|m| m.get("io.modelcontextprotocol/clientInfo")) {
        if !["name", "version"].iter().all(|field| {
            info.get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        }) {
            return Err(error_response(
                id.clone(),
                JSON_RPC_INVALID_PARAMS,
                "Invalid _meta clientInfo",
            ));
        }
    }
    Ok(())
}

/// Dispatcher for a single MCP method. Kept method-keyed (rather than
/// argument-keyed) so the protocol surface is greppable and each handler
/// stays small.
fn handle_method_cancellable(
    method: &str,
    params: &Value,
    cancellation: &Arc<AtomicBool>,
    context: &McpRequestContext,
) -> Result<Value, MethodError> {
    match method {
        "server/discover" => Ok(json!({
            "supportedVersions": [MCP_PROTOCOL_VERSION],
            "capabilities": {"tools": {}, "resources": {}},
            "_meta": {"io.modelcontextprotocol/serverInfo": {
                "name": MCP_SERVER_NAME, "version": MCP_SERVER_VERSION
            }},
            "ttlMs": 3_600_000,
            "cacheScope": "public"
        })),
        "tools/list" => tools::handle_tools_list_for_profile_params_with_context(
            McpCatalogProfile::from_env(),
            params,
            context,
        )
        .map_err(|message| MethodError {
            code: JSON_RPC_INVALID_PARAMS,
            message,
        }),
        "tools/call" => tools::handle_tools_call_cancellable_with_context(
            params,
            Some(Arc::clone(cancellation)),
            context.clone(),
        ),
        "keel/discover" => {
            let query = params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 20) as usize;
            let level = params.get("level").and_then(Value::as_u64).unwrap_or(1);
            let payload = tools::discover_capabilities(query, limit, level).map_err(|message| {
                MethodError {
                    code: JSON_RPC_INVALID_PARAMS,
                    message,
                }
            })?;
            project_protocol_json(
                payload,
                "keel/discover",
                crate::proxy::context::ContextSource::McpTool,
                context,
            )
        }
        "keel/activate" => {
            let payload =
                tools::activate_capability_with_context(params, context).map_err(|message| {
                    MethodError {
                        code: JSON_RPC_INVALID_PARAMS,
                        message,
                    }
                })?;
            project_protocol_json(
                payload,
                "keel/activate",
                crate::proxy::context::ContextSource::McpTool,
                context,
            )
        }
        "resources/list" => Ok(handle_resources_list()),
        "resources/read" => handle_resources_read(params, context),
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

fn handle_resources_list() -> Value {
    json!({
        "ttlMs": MCP_RESOURCE_LIST_CACHE_TTL_MS,
        "cacheScope": MCP_RESOURCE_CACHE_SCOPE,
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

fn project_protocol_json(
    payload: Value,
    surface: &str,
    source: crate::proxy::context::ContextSource,
    context: &McpRequestContext,
) -> Result<Value, MethodError> {
    let serialized = serde_json::to_string(&payload).map_err(|error| MethodError {
        code: JSON_RPC_INTERNAL_ERROR,
        message: format!("{surface}: serialize result: {error}"),
    })?;
    let projection =
        tools::project_mcp_context(surface, &serialized, source, context).map_err(|message| {
            MethodError {
                code: JSON_RPC_INTERNAL_ERROR,
                message,
            }
        })?;
    let metadata = serde_json::to_value(projection.metadata()).map_err(|error| MethodError {
        code: JSON_RPC_INTERNAL_ERROR,
        message: format!("{surface}: serialize context metadata: {error}"),
    })?;

    // Preserve the established object shape only when the measured content is
    // unchanged; otherwise return the projection envelope, never raw content.
    if projection.truncated || projection.summary != serialized {
        return Ok(json!({
            "content": [{ "type": "text", "text": projection.summary }],
            "isError": false,
            "context": metadata,
        }));
    }
    let mut object = payload.as_object().cloned().ok_or_else(|| MethodError {
        code: JSON_RPC_INTERNAL_ERROR,
        message: format!("{surface}: result must be a JSON object"),
    })?;
    object.insert("context".to_string(), metadata);
    Ok(Value::Object(object))
}

fn handle_resources_read(
    params: &Value,
    context: &McpRequestContext,
) -> Result<Value, MethodError> {
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
    let (mime_type, text, source, is_error) = match uri {
        SYSTEM_MAP_RESOURCE_URI => {
            // why: Grok auto-reads this resource on session start. A live
            // walk of a large tree, or an untruncated 16KB+ frame, makes the
            // host drop the line and wait out 30–60s. Deadline + truncate.
            let (text, is_error) = match tools::run_tool_with_deadline(
                tools::mcp_child_timeout(),
                "resources/read system_map",
                || system_map_text(None),
            ) {
                Ok(text) => (text, false),
                Err(message) => (message, true),
            };
            (
                "text/markdown",
                text,
                crate::proxy::context::ContextSource::McpTool,
                is_error,
            )
        }
        RECALL_STATUS_RESOURCE_URI => {
            let (text, is_error) = match tools::run_tool_with_deadline(
                tools::mcp_child_timeout(),
                "resources/read recall_status",
                || {
                    let payload = recall_status_payload()?;
                    // Compact JSON: pretty multi-line resource text bloats the stdio frame.
                    serde_json::to_string(&payload)
                        .map_err(|error| format!("serialize recall status: {error}"))
                },
            ) {
                Ok(text) => (text, false),
                Err(message) => (message, true),
            };
            (
                "application/json",
                text,
                crate::proxy::context::ContextSource::McpTool,
                is_error,
            )
        }
        other => {
            return Err(MethodError {
                code: JSON_RPC_INVALID_PARAMS,
                message: format!("Unknown resource URI: {other}"),
            })
        }
    };
    let projection = tools::project_mcp_context(
        &format!("resources/read {uri}"),
        &text,
        if is_error {
            crate::proxy::context::ContextSource::Error
        } else {
            source
        },
        context,
    )
    .map_err(|message| MethodError {
        code: JSON_RPC_INTERNAL_ERROR,
        message,
    })?;
    let mut response = json!({
        "ttlMs": MCP_RESOURCE_READ_CACHE_TTL_MS,
        "cacheScope": MCP_RESOURCE_CACHE_SCOPE,
        "contents": [
            {
                "uri": uri,
                "mimeType": mime_type,
                "text": projection.summary,
            }
        ],
        "context": projection.metadata(),
    });
    if is_error {
        response["isError"] = Value::Bool(true);
    }
    Ok(response)
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

/// `resultType` value for an ordinary result. Revision `2026-07-28` requires the
/// field on every result; clients **MUST** treat a missing field from an
/// earlier-revision server as `complete`, so omitting it would be a defect on
/// the server side rather than a compatibility choice.
pub(super) const MCP_RESULT_TYPE_COMPLETE: &str = "complete";

/// Reserved result metadata key carrying the server's self-reported identity
/// on every modern response. Keep this in the envelope owner so individual
/// handlers cannot accidentally omit it.
pub(super) const MCP_SERVER_INFO_META_KEY: &str = "io.modelcontextprotocol/serverInfo";

fn mcp_server_info() -> Value {
    json!({
        "name": MCP_SERVER_NAME,
        "version": MCP_SERVER_VERSION,
    })
}

/// Stamp the required `resultType` onto an object result. Applied at the single
/// envelope owner so no method can forget it. Re-stamping a result that already
/// carries the field (a measured `tools/list` page, a `tools/call` envelope) is
/// idempotent, so this cannot widen a payload the budget already measured.
pub(super) fn mark_result_complete(result: Value) -> Value {
    match result {
        Value::Object(mut object) => {
            object.insert(
                "resultType".to_string(),
                Value::String(MCP_RESULT_TYPE_COMPLETE.to_string()),
            );
            let metadata = object
                .entry("_meta".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            match metadata {
                Value::Object(metadata) => {
                    metadata
                        .entry(MCP_SERVER_INFO_META_KEY.to_string())
                        .or_insert_with(mcp_server_info);
                }
                // A scalar `_meta` cannot carry reserved identity; replace only
                // that malformed container while preserving valid handler metadata.
                malformed => {
                    let mut fallback = serde_json::Map::new();
                    fallback.insert(MCP_SERVER_INFO_META_KEY.to_string(), mcp_server_info());
                    *malformed = Value::Object(fallback);
                }
            }
            Value::Object(object)
        }
        // MCP results are objects; anything else keeps its shape rather than
        // gaining a field the schema does not define for it.
        other => other,
    }
}

pub(super) fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": mark_result_complete(result),
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

    fn modern_request(mut value: Value) -> Value {
        if value.get("id").is_some() && value.get("method").is_some() {
            if !value.get("params").is_some_and(Value::is_object) {
                value["params"] = json!({});
            }
            value["params"]["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": MCP_PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {}
            });
        }
        value
    }

    fn dispatch_modern(value: &Value) -> Option<Value> {
        super::dispatch(&modern_request(value.clone()))
    }

    #[test]
    fn modern_requests_validate_metadata_and_supported_versions() {
        let request = modern_request(json!({"jsonrpc":"2.0", "id":1, "method":"server/discover"}));
        assert!(super::dispatch(&request).unwrap().get("result").is_some());
        for field in [
            "io.modelcontextprotocol/protocolVersion",
            "io.modelcontextprotocol/clientCapabilities",
        ] {
            let mut missing = request.clone();
            missing["params"]["_meta"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert_eq!(super::dispatch(&missing).unwrap()["error"]["code"], -32602);
        }
        let mut unsupported = request.clone();
        unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
            json!("2025-11-25");
        let response = super::dispatch(&unsupported).unwrap();
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(
            response["error"]["data"]["supported"],
            json!(["2026-07-28"])
        );
        assert_eq!(response["error"]["data"]["requested"], "2025-11-25");
        let mut malformed = request;
        malformed["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] = json!([]);
        assert_eq!(
            super::dispatch(&malformed).unwrap()["error"]["code"],
            -32602
        );
    }

    #[test]
    fn modern_discovery_is_compact_and_does_not_expose_catalog() {
        let response = super::dispatch(&modern_request(
            json!({"jsonrpc":"2.0", "id":"discover", "method":"server/discover"}),
        ))
        .unwrap();
        let result = &response["result"];
        assert_eq!(result["supportedVersions"], json!([MCP_PROTOCOL_VERSION]));
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            MCP_SERVER_NAME
        );
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result.get("tools").is_none());
        assert!(tools::measure_tools_list_response(result) <= 300);
    }

    #[test]
    fn legacy_initialize_is_explicitly_unsupported() {
        let response = super::dispatch(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":"2025-11-25"}})).unwrap();
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(
            response["error"]["data"]["supported"],
            json!([MCP_PROTOCOL_VERSION])
        );
        assert!(response.get("result").is_none());
    }

    #[test]
    fn modern_request_ids_reject_null_float_and_composite_values() {
        for id in [Value::Null, json!(1.5), json!({}), json!([])] {
            let response = super::dispatch(&modern_request(
                json!({"jsonrpc":"2.0", "id":id, "method":"server/discover"}),
            ))
            .unwrap();
            assert_eq!(response["error"]["code"], -32600);
        }
    }

    #[test]
    fn stdio_rejects_nonempty_batches_and_missing_metadata_on_real_boundary() {
        let frames = format!(
            "{}\n{}\n",
            json!([modern_request(
                json!({"jsonrpc":"2.0", "id":1, "method":"server/discover"})
            )]),
            json!({"jsonrpc":"2.0", "id":2, "method":"server/discover"})
        );
        let mut input = frames.as_bytes();
        let mut output = Vec::new();
        assert_eq!(
            super::serve_stdio(&mut input, &mut output, &mut Vec::new()),
            0
        );
        let responses = parse_json_lines(&String::from_utf8(output).unwrap());
        assert_eq!(responses.len(), 2);
        assert!(responses
            .iter()
            .any(|response| response["error"]["code"] == -32600));
        assert!(responses
            .iter()
            .any(|response| response["error"]["code"] == -32602));
    }

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
    fn notifications_initialized_produces_no_response() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        assert!(dispatch_modern(&request).is_none());
    }

    /// Revision `2026-07-28` retires the old `ping` method. It must now follow
    /// the ordinary JSON-RPC method-not-found path.
    #[test]
    fn retired_ping_is_not_dispatched() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "ping-1",
            "method": "ping"
        });
        let response = dispatch_modern(&request).expect("response present");
        assert_eq!(response["id"], json!("ping-1"));
        assert_eq!(response["error"]["code"], json!(JSON_RPC_METHOD_NOT_FOUND));
    }

    #[test]
    fn every_success_result_carries_server_identity() {
        for (method, params) in [
            ("server/discover", json!({})),
            ("tools/list", json!({})),
            ("resources/list", json!({})),
        ] {
            let response = dispatch_modern(&json!({
                "jsonrpc": "2.0",
                "id": method,
                "method": method,
                "params": params,
            }))
            .expect("response present");
            assert_eq!(
                response["result"]["_meta"][MCP_SERVER_INFO_META_KEY],
                json!({"name": MCP_SERVER_NAME, "version": MCP_SERVER_VERSION}),
                "{method} response: {response}"
            );
        }
    }

    /// The stamp is applied at the single envelope owner, so a result already
    /// carrying it is not widened or reordered by re-stamping.
    #[test]
    fn marking_a_result_complete_is_idempotent() {
        let once = mark_result_complete(json!({"tools": [], "ttlMs": 900000}));
        let twice = mark_result_complete(once.clone());
        assert_eq!(once, twice);
        assert_eq!(
            serde_json::to_string(&once).expect("serialize"),
            serde_json::to_string(&twice).expect("serialize"),
            "re-stamping must not change the serialized payload"
        );
        assert_eq!(
            mark_result_complete(json!({"resultType": "complete"}))["resultType"],
            "complete"
        );
        let authored = mark_result_complete(json!({
            "_meta": {MCP_SERVER_INFO_META_KEY: {"name": "custom", "version": "v"}}
        }));
        assert_eq!(
            authored["_meta"][MCP_SERVER_INFO_META_KEY],
            json!({"name": "custom", "version": "v"})
        );
    }

    #[test]
    fn tools_list_advertises_eager_tools_in_default_profile() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list"
        });
        let response = dispatch_modern(&request).expect("response present");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .collect();
        assert!(!names.is_empty());
        assert!(names.len() <= tools::EAGER_MCP_TOOL_NAMES.len());
        assert_eq!(
            response["result"].get("nextCursor").is_some(),
            names.len() < tools::EAGER_MCP_TOOL_NAMES.len(),
            "bounded first page must advertise continuation when tools remain"
        );
        for expected in names.iter() {
            assert!(tools::EAGER_MCP_TOOL_NAMES.contains(expected));
        }
        for deferred in tools::DEFERRED_MCP_TOOL_NAMES {
            assert!(
                !names.contains(deferred),
                "deferred tool {deferred} should not be in default tools/list"
            );
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
    fn tools_list_empty_params_object_matches_omitted_params() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let omitted = dispatch_modern(&json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list"
        }))
        .expect("omitted params response");
        let empty = dispatch_modern(&json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/list",
            "params": {}
        }))
        .expect("empty params response");
        assert!(empty.get("error").is_none(), "{empty}");
        assert_eq!(empty["result"]["tools"], omitted["result"]["tools"]);
        let serialized = serde_json::to_string(&empty["result"]).expect("serialize");
        let tokens = crate::proxy::token_meter::TokenMeter::count_text(&serialized);
        assert!(
            tokens <= crate::proxy::context::DEFAULT_MAX_TOOL_CATALOG_TOKENS,
            "empty-params catalog {tokens} exceeds {}",
            crate::proxy::context::DEFAULT_MAX_TOOL_CATALOG_TOKENS
        );
    }

    #[test]
    fn dispatched_tools_list_response_stays_within_page_budget() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_profile = std::env::var("KEEL_MCP_CATALOG_PROFILE").ok();
        let previous_budget = std::env::var("KEEL_MCP_PAGE_TOKENS").ok();
        std::env::set_var("KEEL_MCP_CATALOG_PROFILE", "full");
        std::env::set_var("KEEL_MCP_PAGE_TOKENS", "1200");

        let response = dispatch_modern(&json!({
            "jsonrpc": "2.0",
            "id": "tools-budget",
            "method": "tools/list",
            "params": { "level": 2 }
        }))
        .expect("response present");
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        let measured = tools::measure_tools_list_response(result);
        assert!(
            measured <= 1200,
            "dispatched tools/list page measured {measured} tokens"
        );

        match previous_profile {
            Some(value) => std::env::set_var("KEEL_MCP_CATALOG_PROFILE", value),
            None => std::env::remove_var("KEEL_MCP_CATALOG_PROFILE"),
        }
        match previous_budget {
            Some(value) => std::env::set_var("KEEL_MCP_PAGE_TOKENS", value),
            None => std::env::remove_var("KEEL_MCP_PAGE_TOKENS"),
        }
    }

    #[test]
    fn resources_list_advertises_system_map_and_recall_status() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "resources/list"
        });
        let response = dispatch_modern(&request).expect("response present");
        let resources = response["result"]["resources"]
            .as_array()
            .expect("resources array");
        assert!(response["result"]["ttlMs"].as_u64().is_some());
        assert_eq!(response["result"]["cacheScope"], "private");
        let uris: Vec<&str> = resources
            .iter()
            .filter_map(|entry| entry.get("uri").and_then(Value::as_str))
            .collect();
        assert!(uris.contains(&SYSTEM_MAP_RESOURCE_URI));
        assert!(uris.contains(&RECALL_STATUS_RESOURCE_URI));
        assert_eq!(uris.len(), 2);
    }

    #[test]
    fn dynamic_resource_reads_are_firewalled_and_provenanced() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut context = McpRequestContext::authoritative(None);
        context.session_id = format!("resource-firewall-test-{}", std::process::id());
        context.request_id = Some("resource-request".to_string());
        let request = json!({
            "jsonrpc": "2.0",
            "id": "resource-1",
            "method": "resources/read",
            "params": { "uri": RECALL_STATUS_RESOURCE_URI }
        });
        let response = dispatch_cancellable_with_context(
            &modern_request(request),
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            &context,
        )
        .expect("response present");
        let result = &response["result"];
        assert!(result["ttlMs"].as_u64().is_some());
        assert_eq!(result["cacheScope"], "private");
        assert_eq!(result["contents"][0]["uri"], RECALL_STATUS_RESOURCE_URI);
        assert_eq!(result["context"]["source"], "mcp_tool");
        assert!(result["context"]["provenance_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("prov-fnv1a:")));
        let text = result["contents"][0]["text"].as_str().unwrap_or("");
        assert!(
            crate::proxy::token_meter::TokenMeter::count_text(text)
                <= crate::proxy::context::DEFAULT_MAX_SINGLE_RESULT_TOKENS
        );
    }

    #[test]
    fn discovery_response_keeps_legacy_fields_with_a_bounded_context_record() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut context = McpRequestContext::authoritative(None);
        context.session_id = format!("discover-firewall-test-{}", std::process::id());
        context.request_id = Some("discover-request".to_string());
        let request = json!({
            "jsonrpc": "2.0",
            "id": "discover-1",
            "method": "keel/discover",
            "params": { "query": "flutter testing", "limit": 3, "level": 1 }
        });
        let response = dispatch_cancellable_with_context(
            &modern_request(request),
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            &context,
        )
        .expect("response present");
        let result = &response["result"];
        assert_eq!(result["query"], "flutter testing");
        assert!(result["capabilities"].is_array());
        assert_eq!(result["context"]["source"], "mcp_tool");
        assert!(
            crate::proxy::token_meter::TokenMeter::count_text(
                &serde_json::to_string(result).expect("serialize result")
            ) <= crate::proxy::context::DEFAULT_MAX_DISCOVERY_RESULT_TOKENS + 64
        );
    }

    #[test]
    fn activation_response_is_firewalled_and_uses_authoritative_identity() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = crate::test_support::unique_temp_dir("mcp-activation-firewall");
        let previous = std::env::var("KEEL_HOME").ok();
        std::env::set_var("KEEL_HOME", &*home);

        let mut context = McpRequestContext::authoritative(None);
        context.session_id = format!("activation-firewall-test-{}", std::process::id());
        context.request_id = Some("activation-request".to_string());
        let request = json!({
            "jsonrpc": "2.0",
            "id": "activation-1",
            "method": "keel/activate",
            "params": { "capability": "stats", "reason": "inspect context metrics" }
        });
        let response = dispatch_cancellable_with_context(
            &modern_request(request),
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            &context,
        )
        .expect("response present");
        let result = &response["result"];
        assert_eq!(result["capabilityId"], "stats");
        assert_eq!(result["sessionId"], context.session_id);
        assert_eq!(result["context"]["source"], "mcp_tool");
        assert!(result["context"]["provenance_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("prov-fnv1a:")));

        match previous {
            Some(value) => std::env::set_var("KEEL_HOME", value),
            None => std::env::remove_var("KEEL_HOME"),
        }
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "tools/teleport"
        });
        let response = dispatch_modern(&request).expect("response present");
        assert_eq!(response["error"]["code"], json!(JSON_RPC_METHOD_NOT_FOUND));
    }

    #[test]
    fn missing_jsonrpc_version_returns_invalid_request() {
        let request = json!({
            "id": 1,
            "method": "initialize"
        });
        let response = dispatch_modern(&request).expect("response present");
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
        let response = dispatch_modern(&request).expect("response present");
        assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_PARAMS));
    }

    #[test]
    fn serve_stdio_handles_request_then_eof() {
        let request = serde_json::to_string(&modern_request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover"
        })))
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
        assert!(
            rendered.contains("\"supportedVersions\":[\"2026-07-28\"]"),
            "rendered: {rendered}"
        );
        assert!(rendered.ends_with('\n'), "rendered: {rendered}");
    }

    #[test]
    fn serve_stdio_handles_many_inflight_discoveries_without_shell() {
        // why: prove multi-request scheduling without spawning OS children.
        // A previous wall-clock test used hanging `run_command` (sleep) and
        // could freeze a developer machine under full suite load — do not restore it.
        let mut input_bytes = Vec::new();
        for id in 1u64..=32 {
            let request = json!({"jsonrpc": "2.0", "id": id, "method": "server/discover"});
            input_bytes
                .extend(serde_json::to_vec(&modern_request(request.clone())).expect("serialize"));
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
            "expected 32 discovery responses; got: {rendered}"
        );
        for id in 1u64..=32 {
            assert!(
                rendered.contains(&format!("\"id\":{id}")),
                "missing id {id} in {rendered}"
            );
        }
    }

    #[test]
    fn duplicate_stdio_request_ids_are_rejected_without_overwriting_owner() {
        let mut cancellations = HashMap::new();
        let first = new_pending_job(
            json!({"jsonrpc":"2.0", "id":"duplicate", "method":"server/discover"}),
            &mut cancellations,
        )
        .expect("first request registers");
        let first_owner = cancellations
            .get("\"duplicate\"")
            .cloned()
            .expect("first cancellation owner");

        let error = match new_pending_job(
            json!({"jsonrpc":"2.0", "id":"duplicate", "method":"server/discover"}),
            &mut cancellations,
        ) {
            Ok(_) => panic!("duplicate request id must fail closed"),
            Err(error) => error,
        };
        assert_eq!(error, PendingJobError::DuplicateRequestId);
        assert!(
            Arc::ptr_eq(
                cancellations.get("\"duplicate\"").expect("owner remains"),
                &first_owner
            ),
            "a duplicate must not replace the in-flight cancellation owner"
        );
        remove_pending_job_registration(&mut cancellations, &first);
        assert!(cancellations.is_empty());
    }

    #[test]
    fn rejected_stdio_notifications_never_emit_an_id_null_response() {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        assert!(
            rejected_request_response(&notification, JSON_RPC_INTERNAL_ERROR, "server busy")
                .is_none()
        );
    }

    #[test]
    fn parse_frame_empty_batch_is_invalid_request() {
        match parse_frame("[]") {
            FrameParse::Immediate(response) => {
                assert_eq!(response["error"]["code"], json!(JSON_RPC_INVALID_REQUEST));
            }
            FrameParse::Single(_) => {
                panic!("empty batch must not schedule workers")
            }
        }
    }

    #[test]
    fn serve_stdio_concurrent_in_process_delays_share_wall_clock() {
        // why: completion order proves overlap without process-hang children; wall-clock ceilings flake on saturated CI.
        // FIFO yields d1,d2,discovery; concurrent workers yield discovery,d2,d1.
        let delay = |id: u64, ms: u64| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "keel/test_delay_ms",
                "params": { "ms": ms }
            })
        };
        let discovery = json!({"jsonrpc": "2.0", "id": 99, "method": "server/discover"});
        let mut input_bytes = Vec::new();
        for value in [delay(1, 300), delay(2, 80), discovery] {
            input_bytes
                .extend(serde_json::to_vec(&modern_request(value.clone())).expect("serialize"));
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
        let discovery_pos = rendered.find("\"id\":99").expect("discovery");
        let d1 = rendered.find("\"id\":1").expect("d1");
        let d2 = rendered.find("\"id\":2").expect("d2");
        assert!(
            discovery_pos < d2 && d2 < d1,
            "fast jobs must overtake the slow one (discovery, then 80ms, then 300ms); \
             serial FIFO would render d1 first: {rendered}"
        );
    }

    /// §29.4: the normal handshake path must stay correct when callers
    /// interleave. Four `tools/list` requests and a `tools/call` are pipelined
    /// into one stdio session; every page must come back individually valid and
    /// within the profile budget, none may duplicate or lose a tool, and the
    /// concurrent call must not disturb any of them.
    #[test]
    fn concurrent_tools_list_and_tools_call_stay_independent() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut requests: Vec<serde_json::Value> = (0..4u64)
            .map(|id| json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
            .collect();
        requests.push(json!({
            "jsonrpc": "2.0",
            "id": "call-1",
            "method": "tools/call",
            "params": {
                "name": "rewrite",
                // Distinct command string: the scoped gateway dedupes by content
                // per workspace+session, so a shared literal would cross tests.
                "arguments": { "command": "concurrency-probe-only" }
            }
        }));
        requests.push(json!({"jsonrpc": "2.0", "id": "list-last", "method": "tools/list"}));

        let mut input_bytes = Vec::new();
        for value in &requests {
            input_bytes
                .extend(serde_json::to_vec(&modern_request(value.clone())).expect("serialize"));
            input_bytes.push(b'\n');
        }
        let mut input: &[u8] = &input_bytes;
        let mut output: Vec<u8> = Vec::new();
        let mut error_output: Vec<u8> = Vec::new();
        assert_eq!(serve_stdio(&mut input, &mut output, &mut error_output), 0);

        let rendered = String::from_utf8_lossy(&output);
        let frames = parse_json_lines(&rendered);

        let budget = tools::mcp_tools_list_budget(McpCatalogProfile::from_env());
        let mut page_shapes = Vec::new();
        for frame in &frames {
            if frame["id"] == "call-1" {
                assert_eq!(
                    frame["result"]["isError"],
                    json!(false),
                    "the concurrent tools/call must succeed: {frame}"
                );
                continue;
            }
            let Some(result) = frame.get("result") else {
                continue;
            };
            if !result.get("tools").is_some_and(serde_json::Value::is_array) {
                continue;
            }
            assert!(
                frame.get("error").is_none(),
                "a concurrent tools/list must not fail: {frame}"
            );
            let measured = tools::measure_tools_list_response(result);
            assert!(
                measured <= budget,
                "a concurrent page measured {measured} against a {budget}-token budget"
            );
            let names = result["tools"]
                .as_array()
                .expect("tools array")
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect::<Vec<_>>();
            let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                unique.len(),
                names.len(),
                "a concurrent page duplicated a tool"
            );
            page_shapes.push(names);
        }

        assert_eq!(
            page_shapes.len(),
            5,
            "all five tools/list requests must answer: {rendered}"
        );
        // Identical requests over an unchanged catalog must agree, so one
        // caller's page cannot leak into another's.
        for page in &page_shapes[1..] {
            assert_eq!(page, &page_shapes[0], "concurrent pages disagreed");
        }
    }

    /// §31: the catalog diagnostic must report the packing picture the packer
    /// would actually serve, at the requested budget, and fail closed on a bad
    /// budget instead of silently using the default.
    #[test]
    fn mcp_catalog_command_reports_the_live_packing_picture() {
        let _env_guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // why: an absent override is the normal case; only its presence matters.
        let previous = env::var("KEEL_MCP_PAGE_TOKENS").ok();
        env::remove_var("KEEL_MCP_PAGE_TOKENS");

        let run = |arguments: &[&str]| {
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let owned = arguments
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>();
            let code = run_mcp_command(&owned, &mut stdout, &mut stderr);
            (
                code,
                String::from_utf8_lossy(&stdout).to_string(),
                String::from_utf8_lossy(&stderr).to_string(),
            )
        };

        let (code, stdout, stderr) =
            run(&["catalog", "--profile", "full", "--budget", "1200", "--json"]);
        assert_eq!(code, 0, "{stderr}");
        let payload: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        assert_eq!(payload["profile"], "full");
        assert_eq!(payload["hardPageBudget"], 1200);
        let first_page = payload["firstPageTokens"]
            .as_u64()
            .expect("first page tokens") as usize;
        assert!(
            first_page <= 1200,
            "the reported first page must fit its reported budget: {payload}"
        );
        assert!(
            payload["catalogSnapshot"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "the snapshot id must be reported: {payload}"
        );
        assert!(payload["deferredTools"].as_u64().is_some());

        // §34: an identical request must select the same page. The cursor's own
        // token count may drift; the selected tools may not.
        let first_names = payload["visibleToolNames"]
            .as_array()
            .expect("visible tool names")
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(!first_names.is_empty(), "the page must advertise tools");
        let (code, stdout, _) =
            run(&["catalog", "--profile", "full", "--budget", "1200", "--json"]);
        assert_eq!(code, 0);
        let second: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        let second_names = second["visibleToolNames"]
            .as_array()
            .expect("visible tool names")
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(
            second_names, first_names,
            "an identical request selected a different page"
        );
        assert_eq!(second["catalogSnapshot"], payload["catalogSnapshot"]);

        // A default invocation reads the env budget like every other surface.
        let (code, stdout, _) = run(&["catalog"]);
        assert_eq!(code, 0);
        assert!(stdout.contains("MCP profile: core"), "{stdout}");

        let (code, _, stderr) = run(&["catalog", "--budget", "abc"]);
        assert_eq!(code, 1, "a non-numeric budget must fail closed");
        assert!(stderr.contains("invalid budget"), "{stderr}");

        let (code, _, stderr) = run(&["catalog", "--budget", "0"]);
        assert_eq!(code, 1, "a zero budget must fail closed");
        assert!(stderr.contains("invalid budget"), "{stderr}");

        let (code, _, stderr) = run(&["catalog", "--profile", "nope"]);
        assert_eq!(code, 1);
        assert!(stderr.contains("unsupported profile"), "{stderr}");

        match previous {
            Some(value) => env::set_var("KEEL_MCP_PAGE_TOKENS", value),
            None => env::remove_var("KEEL_MCP_PAGE_TOKENS"),
        }
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
            json!({"jsonrpc": "2.0", "id": "still-live", "method": "server/discover"}),
        ] {
            input_bytes
                .extend(serde_json::to_vec(&modern_request(value.clone())).expect("serialize"));
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
    fn tools_list_framed_response_under_stdio_ceiling_with_headroom() {
        // Drive real dispatch → handle_tools_list (slimmed). Pins the wire
        // budget so discovery cannot fill the 24KB frame and trip hosts.
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list"
        });
        let response = dispatch_modern(&request).expect("response present");
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
