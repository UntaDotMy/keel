//! Purpose: `tools/list` schema and `tools/call` dispatch for the keel
//!   MCP server — every `tool_*` handler lives here. Split out of `mcp/mod.rs`
//!   (which keeps JSON-RPC framing, the serve loop, dispatch, and the
//!   `resources/*` surface) so the protocol plumbing and the growing tool
//!   surface stay in separate, greppable files.
//! Caller: `super::handle_method` routes `tools/list` to [`handle_tools_list`]
//!   and `tools/call` to [`handle_tools_call`]. Tests drive the handlers
//!   directly and through `super::dispatch`.
//! Dependencies: serde_json for payloads; `super` for the shared `MethodError`,
//!   the `system_map_text`/`recall_status_payload` helpers (also used by the
//!   resources surface), and the `JSON_RPC_INVALID_PARAMS` code; the utility
//!   layer (`recall`, `skill_match`, `working_brief`, `memory_families`,
//!   `memory`, `workflow_ledger`) for the capabilities each tool wraps.
//! Side Effects: `recall`/`recall_status`/`memory_status` open the recall SQLite
//!   index; `run_command` executes a user-supplied shell command via the proxy;
//!   `cli` shells out to any non-refused keel subcommand (destructive
//!   ones gated behind `confirm`); `brief_create` writes a JSON brief under
//!   `<claude-home>/working-briefs/`; `system_map_refresh` writes SYSTEM_MAP.md
//!   under the workspace reference lane. `context_brief` and the skill/read-only
//!   brief tools only read installed files.
//!
//! Design: each tool is a thin wrapper over a function that already backs a CLI
//! surface, so the MCP channel and the CLI never drift. The skill, memory, and
//! brief tools exist because the equivalent guidance is otherwise delivered by
//! the harness lifecycle hooks, which are unreliable on some platforms — MCP is
//! a dependable pull channel, so mirroring the capabilities here routes around
//! the hook layer without rewriting it.

use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home, safe_path_segment};
use crate::utility::memory::refresh_system_map;
use crate::utility::memory_families::family_counts;
use crate::utility::recall::{collapse_dashes, search_recall_index, RecallSearchResult};
use crate::utility::skill_match::{
    installed_skill_path, match_skill_for_prompt, skill_catalog, skill_full_body,
    skill_inline_brief,
};
use crate::utility::workflow_ledger::{current_timestamp_millis, format_timestamp_iso8601};
use crate::utility::working_brief::{create_brief, list_briefs, read_brief, write_brief, Brief};

use super::{recall_status_payload, system_map_text, MethodError, JSON_RPC_INVALID_PARAMS};

/// Default wall-clock budget for MCP tools that spawn a child (`run_command`)
/// **and** for in-process tools that can block
/// (SQLite recall, skill catalog scan, system map render). The serve loop runs
/// concurrent workers, but a single hung tool still burns an in-flight slot and
/// can exhaust host patience — deadline so hosts get `isError` instead of a
/// permanent stall. Override with `KEEL_MCP_TOOL_TIMEOUT_SECS` (seconds, min 5,
/// max 3600).
///
/// **Must stay strictly under observed host MCP tool timeouts.** Grok has been
/// measured at `timeout_sec=30`; a 90s server budget made the host look hung
/// even when the server would eventually answer. Prefer CLI for multi-minute
/// builds; raise via env only when the host budget is known to be higher.
const DEFAULT_MCP_CHILD_TIMEOUT_SECS: u64 = 25;

/// Test-visible handle on the per-tool deadline so sibling modules can assert
/// relationships against it without re-hardcoding the number.
#[cfg(test)]
pub(crate) const DEFAULT_MCP_CHILD_TIMEOUT_SECS_FOR_TEST: u64 = DEFAULT_MCP_CHILD_TIMEOUT_SECS;

/// Soft cap for large text tool results (system_map, skill bodies, context_brief).
///
/// Hosts like Grok default to ~20KB MCP tool-result caps and parse stdio as
/// newline-delimited JSON-RPC. Oversized single-line frames are the main cause of
/// `mcp_transport_decode_error` followed by a full host tool timeout (the server
/// already answered; the host discarded the frame and waited). Keep the *text*
/// payload well under that budget so the JSON-RPC envelope still fits.
/// Override with `KEEL_MCP_MAX_TEXT_CHARS` (min 2_000, max 200_000).
const DEFAULT_MAX_MCP_TEXT_CHARS: usize = 12_000;

/// Cap for a skill body's embedded text inside `skill_get`. Leaves room for the
/// name/path envelope and the outer tools/call JSON-RPC frame.
const MAX_SKILL_BODY_CHARS: usize = 10_000;

/// Cap for each skill_list description / when_to_use field so a large catalog
/// cannot blow the wire budget (full catalog was ~33KB pretty-printed).
const MAX_SKILL_LIST_FIELD_CHARS: usize = 240;

/// Cap for each tool's top-level description on the wire `tools/list` frame.
/// Full prose stays in source; hosts only need a short trigger line.
const MAX_TOOLS_LIST_DESCRIPTION_CHARS: usize = 160;

/// Default cap for `recall` matches when the caller does not supply one. The
/// CLI uses the same default (see `utility::recall::DEFAULT_RECALL_LIMIT`).
const DEFAULT_RECALL_LIMIT: usize = 20;
const MAX_RECALL_LIMIT: usize = 100;

/// Command group whose memory families `memory_status` summarizes and under
/// which `system_map_refresh` writes. Must match the group string the CLI
/// dispatches for the plain memory lane — `commands.rs` routes it as the literal
/// `"memory"` (singular), so family records live at `<home>/memory/<family>/`.
/// Using `"memory"` here makes the MCP `memory_status` tool count the same tree
/// the CLI `memory status` reads; `system_map_reference_directory` separately
/// normalizes this to the `memories/workspaces/` map lane the `system_map` read
/// path expects, so `system_map_refresh` still writes where `system_map` reads.
const DEFAULT_MEMORY_GROUP: &str = "memory";

/// The `tools/list` response. Schemas are hand-written JSON so the descriptions
/// double as the model-facing usage hints the harness renders. The payload is
/// slimmed before return so the framed JSON-RPC line stays under the stdio
/// frame ceiling with headroom for future tools (see [`slim_tools_list_for_wire`]).
pub(super) fn handle_tools_list() -> Value {
    let list = slim_tools_list_for_wire(tools_list_catalog());
    // Keep catalog ↔ MCP_TOOL_NAMES length honest (handler table checked in unit tests).
    debug_assert_eq!(
        list.get("tools")
            .and_then(Value::as_array)
            .map(std::vec::Vec::len)
            .unwrap_or(0),
        MCP_TOOL_NAMES.len(),
        "tools/list count must match MCP_TOOL_NAMES"
    );
    list
}

/// Raw tool catalog (full descriptions + property descriptions). Not sent on
/// the wire as-is — [`handle_tools_list`] always slims first.
fn tools_list_catalog() -> Value {
    json!({
        "tools": [
            {
                "name": "recall",
                "description": "Call this BEFORE claiming what you remember or previously learned — search your durable memory instead of relying on conversation alone. Full-text search over Markdown under <claude-home>/{memory,memories,working-briefs}. Auto-syncs the index before querying.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms; punctuation is stripped and tokens are AND-ed with prefix match." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RECALL_LIMIT, "description": "Maximum hits (default 20)." },
                        "workspace": { "type": "string", "description": "Workspace slug to boost in ranking (current-project hits outrank cross-project). Auto-derived from cwd if omitted; pass explicitly to force." },
                        "local_only": { "type": "boolean", "description": "Restrict results to the current workspace's lane only. A new project returns empty instead of flooding with cross-project hits. Default false." }
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
                "description": "Prefer this over a raw shell call for noisy commands (test, build, lint, logs, search): it runs the command through the compaction proxy so compacted high-signal output enters context instead of the raw stream. Three mutually-exclusive input forms: (1) argv — program plus argument array, executed DIRECTLY with NO shell: use this whenever possible, it can never misquote or hit the wrong shell; (2) script + shell — a script string run through the NAMED shell (powershell|cmd|bash): the shell is explicit, never guessed; (3) command — legacy single string, run through the platform default shell. Output is always neutralized for prompt-injection before reaching the model. Long commands: pass wait:false to run in the background and get a commandId to poll with command_output / kill with command_kill.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "argv": { "type": "array", "items": { "type": "string" }, "description": "Program plus its arguments, executed directly with no shell (no quoting issues). Example: [\"cargo\", \"test\", \"--workspace\"]. Alias: args as an array." },
                        "args": { "description": "Alias for argv when this is a string array, or for command when this is a string." },
                        "cmd": { "type": "string", "description": "Alias for command." },
                        "script": { "type": "string", "description": "Shell script line. Requires `shell` alongside it; use argv instead when no shell features are needed." },
                        "shell": { "type": "string", "enum": ["powershell", "cmd", "bash"], "description": "The exact shell for `script`. No fallback and no guessing: the script runs through this shell and only this shell." },
                        "command": { "type": "string", "description": "One shell command string run through the platform default shell. Prefer argv. Aliases: cmd, input." },
                        "input": { "type": "string", "description": "Alias for command." },
                        "cwd": { "type": "string", "description": "Working directory for the command. Defaults to the server's cwd." },
                        "wait": { "type": "boolean", "description": "Default true: wait for the command to finish and return its output. false: start in the background and return a commandId immediately — use for commands that may outlive the tool timeout (long builds, analyze)." },
                        "json": { "type": "boolean", "description": "Return the output as a JSON object (command, exit_code, stdout, stderr) instead of the text report. Default false." }
                    }
                }
            },
            {
                "name": "command_output",
                "description": "Poll a background command started with run_command wait:false. While running it returns the live stdout/stderr captured so far (running:true); once the command finishes it returns the final exit_code, full stdout/stderr (running:false) and releases the commandId — the finished result is delivered exactly once.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command_id": { "type": "string", "description": "The commandId returned by run_command with wait:false." },
                        "json": { "type": "boolean", "description": "Return a JSON object instead of the text report when the command has finished. Default false." }
                    },
                    "required": ["command_id"]
                }
            },
            {
                "name": "command_kill",
                "description": "Stop a background command started with run_command wait:false. Kills the process and reports the exit code. On Windows the whole process group dies together (Job Object), so a killed shell wrapper does not orphan the work it spawned. Safe to call on an already-finished command: it reports killed:false with the final exit code.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command_id": { "type": "string", "description": "The commandId returned by run_command with wait:false." }
                    },
                    "required": ["command_id"]
                }
            },
            {
                "name": "recall_status",
                "description": "Check recall index health before trusting or after rebuilding the memory index: document count, schema version, last-sync timestamp, and on-disk index path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "skill_route",
                "description": "Call this to find which keel skill a request should use — the on-demand equivalent of the per-prompt skill router. Returns the single distinctive skill match for a prompt plus a bounded inline brief of its operative guidance. Use it before answering when unsure which skill applies; call skill_get for the full body.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string", "description": "The user request (or a paraphrase) to route to a skill." }
                    },
                    "required": ["prompt"]
                }
            },
            {
                "name": "skill_get",
                "description": "Load an installed skill's SKILL.md by name (frontmatter included). Size-capped for MCP hosts: large skills may set truncated=true and include path so you can Read the file for the remainder. Prefer skill_route when a brief is enough.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Installed skill directory name, e.g. \"reviewer\" or \"systematic-debugging\"." }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "skill_list",
                "description": "List every installed keel skill with its name, description, and when_to_use. Prefer skill_route(prompt) when you already have a task — it is smaller and faster. Use skill_list only for discovery. Results are size-capped and compact so the MCP call cannot hang or blow host frame limits.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "memory_status",
                "description": "Report durable-memory health: the recall index snapshot (document count, schema version, last-sync) plus per-family record counts under the memory lane. Does not modify memory content (it does refresh the recall index cache).",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "brief_list",
                "description": "List stored working briefs (request, constraints, acceptance criteria, assumptions, workspace) under <claude-home>/working-briefs, oldest first. Read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "brief_get",
                "description": "Read one stored working brief by id. Returns the full brief, or a not-found marker when no brief has that id.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Brief id, e.g. \"wb-1a2b3c\"." }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "brief_create",
                "description": "Persist a working brief so the request, constraints, acceptance criteria, and assumptions survive compaction. Use when starting non-trivial work to record what was actually asked. Writes a JSON file under <claude-home>/working-briefs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "request": { "type": "string", "description": "The restated request this brief captures." },
                        "constraints": { "type": "array", "items": { "type": "string" }, "description": "Hard constraints the work must respect." },
                        "acceptance_criteria": { "type": "array", "items": { "type": "string" }, "description": "What done looks like." },
                        "assumptions": { "type": "array", "items": { "type": "string" }, "description": "Assumptions made while scoping." },
                        "id": { "type": "string", "description": "Optional explicit id; auto-generated (wb-<hex>) when omitted." }
                    },
                    "required": ["request"]
                }
            },
            {
                "name": "system_map_refresh",
                "description": "Regenerate the cached workspace SYSTEM_MAP.md (system_map only reads it). Use after creating, deleting, moving, or renaming files so the next system_map call reflects the current tree. Writes under ~/.claude/memories/workspaces/<slug>/reference/.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_root": { "type": "string", "description": "Absolute workspace root. Defaults to the server process working directory." }
                    }
                }
            },
            {
                "name": "context_brief",
                "description": "Call this FIRST when starting a session or task — one call that makes you aware of what this toolkit offers, even when no skill loaded automatically. Returns the iron law, the full installed skill catalog (name + when_to_use), durable-memory health, and the newest working brief. After reading it, use skill_route to pick a skill, skill_get to load one, recall for memory, and cli for any other keel surface. Read-only. Time-budgeted and size-capped so it cannot hang the MCP stdio loop.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "cli",
                "description": "Run any keel CLI subcommand and get its compacted output — the full toolkit surface including families without a dedicated tool (bridge, eval, bench, hook list/diagnose, status, platform, ...). Pass the subcommand and flags as `args`. Read/inspection subcommands run in-process; destructive or management subcommands (install, update, repair, uninstall, validate, all, self-replace, `checkpoint restore`, and `hook install`/`hook uninstall`) require `confirm: true`. The `mcp` and `cli` subcommands are refused so the MCP server cannot re-enter itself. Prefer dedicated tools (recall, observe, rewrite, skill_eval, anvil, design_intelligence, ...) when one fits; use cli for everything else.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "args": { "type": "array", "items": { "type": "string" }, "description": "keel arguments, e.g. [\"review\",\"pre-pr\",\"--base-ref\",\"origin/feat\"] or [\"anvil\",\"run\",\"--dry-run\"]." },
                        "confirm": { "type": "boolean", "description": "Required true to run a destructive/management subcommand. Default false." }
                    },
                    "required": ["args"]
                }
            },
            {
                "name": "anvil",
                "description": "Drive the Anvil delivery loop (compile → cast → sieve → stamp → loop). Subcommands: compile, cast, sieve, stamp, loop, run, prefix-check. This is the only keel delivery loop.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["compile", "cast", "sieve", "stamp", "loop", "run", "prefix-check"], "description": "Anvil operation to perform." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional CLI flags after the action." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "review",
                "description": "Run keel review gates (pre-commit, pre-pr, gates check). Use to get a deterministic local quality gate with fail-closed verdicts on the current diff.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["pre-commit", "pre-pr", "gates"], "description": "Review action to perform." },
                        "base_ref": { "type": "string", "description": "Base ref for diff comparison (e.g. \"origin/feat\")." },
                        "format": { "type": "string", "description": "Output format: json, markdown, or compact." },
                        "repo_root": { "type": "string", "description": "Repository root path. Defaults to cwd." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "git_workflow",
                "description": "Git workflow operations. await-ci waits for CI checks to go green and blocks (non-zero) on red/pending so you never merge blind past CI; configure/show save and recall the branch+commit workflow preference to per-workspace memory; preflight validates branch/clean state; commit-message/pr-body/lint-message generate and lint professional text.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["preflight", "await-ci", "configure", "show", "commit-message", "pr-body", "lint-message"], "description": "Git workflow operation." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional CLI arguments (e.g. [\"--watch\"], [\"--base-ref\",\"origin/feat\"], [\"--model\",\"four-tier\"])." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "memory",
                "description": "Durable memory operations. Prefer short calls. research-cache RECORD needs args [\"record\",\"--question\",\"...\",\"--answer\",\"...\"] (not --query/--result; those are aliases only). LOOKUP: [\"lookup\",\"--query\",\"...\"]. status/instincts/consolidate are fast. Use brief_* for working briefs; dedicated recall tool for FTS search.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["scope", "system-map", "recall", "instincts", "consolidate", "report", "research-cache", "retrieve", "maintenance", "status"], "description": "Memory operation." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "CLI args AFTER action. research-cache record: [\"record\",\"--question\",\"q\",\"--answer\",\"a\",\"--source\",\"url\"]. lookup: [\"lookup\",\"--query\",\"q\"]. scope: [\"--create-missing\",\"--refresh-system-map\"]. Do not pass huge blobs." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "gain",
                "description": "Report command-output compaction savings (exact o200k_base tokens saved, adapter breakdown, top commands) from the native event log. Use to quantify token ROI from the compaction proxy.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since": { "type": "string", "description": "Time window: today, 7d, 30d, or all. Default: today." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    }
                }
            },
            {
                "name": "raw",
                "description": "View raw output from compacted commands. Use to recover full stdout/stderr when the compacted view is insufficient.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "raw_id": { "type": "string", "description": "The raw output id to view." },
                        "action": { "type": "string", "enum": ["list", "prune"], "description": "List available raw outputs or prune old ones." },
                        "older_than": { "type": "string", "description": "Prune raw outputs older than this duration (e.g. \"30d\")." }
                    }
                }
            },
            {
                "name": "config_audit",
                "description": "Audit hook configuration for security. Checks installed hooks, settings.json, and managed files for drift or misconfiguration.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_root": { "type": "string", "description": "Repository root path. Defaults to cwd." }
                    }
                }
            },
            {
                "name": "skill_lint",
                "description": "Lint skill files for quality. Checks SKILL.md files against structural and content rules.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_root": { "type": "string", "description": "Repository root path. Defaults to cwd." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    }
                }
            },
            {
                "name": "telemetry",
                "description": "View compaction telemetry. Shows command-output compaction stats, adapter breakdowns, and top commands over a time window.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "days": { "type": "integer", "description": "Number of days to look back." },
                        "top": { "type": "integer", "description": "Number of top commands to show." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    }
                }
            },

            {
                "name": "checkpoint",
                "description": "Create/restore checkpoints for workflow state. Use to snapshot progress or recover from interruption.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["create", "list", "show", "restore"], "description": "Checkpoint operation." },
                        "id": { "type": "string", "description": "Checkpoint id." },
                        "confirm": { "type": "boolean", "description": "Required true for restore (destructive). Default false." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "session",
                "description": "View session history. Shows recent sessions with message counts, date ranges, and agents used.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since": { "type": "string", "description": "Filter sessions since this window: today, 7d, 30d, or all." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    }
                }
            },
            {
                "name": "doctor",
                "description": "Run diagnostic health check. Probes binary, raw store, event log, adapter registry, rewrite behavior, and hook/proxy setup with ok/warn/fix-style output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_root": { "type": "string", "description": "Repository root path. Defaults to cwd." }
                    }
                }
            },
            {
                "name": "code_search",
                "description": "Lexical substring search of the workspace via keel code-search (not embedding/semantic search). Returns matching file:line:snippet rows; optional path filter is cross-platform (/ and \\\\).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query text." },
                        "format": { "type": "string", "description": "Output format: json or compact." }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "flow",
                "description": "Preserve-Existing-Flow gate — the Iron Law's pre-edit ownership trace. Use `start` before editing an existing source file to record its owner path; `check` validates the trace still holds; `finish` clears it. Prevents blind edits to code whose ownership hasn't been traced.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "check", "finish"], "description": "Flow operation to perform." },
                        "file": { "type": "string", "description": "Target source file path (for start/check). Translated to --target-file." },
                        "target_function": { "type": "string", "description": "Target function name within the file (optional, for start/check)." },
                        "repo_root": { "type": "string", "description": "Repository root path. Defaults to cwd." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "code_graph",
                "description": "Deterministic codebase-understanding graph. `build` scans the workspace and writes a JSON artifact of nodes (source files with symbols/imports) and edges (cross-file import dependencies). `impact` reports the transitive reverse-dependency closure of changed files — the cheap \"what could this edit break\" query for review scoping. Languages: Rust, JS/TS, Python, Go.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["build", "impact"], "description": "Graph operation to perform." },
                        "changed": { "type": "string", "description": "Comma-separated list of changed files (required for impact, e.g. \"src/a.rs,src/b.rs\")." },
                        "workspace_root": { "type": "string", "description": "Workspace root path. Defaults to cwd." },
                        "output": { "type": "string", "description": "Output artifact path (for build). Default writes to the global per-workspace memory lane, never the workspace; pass a relative path to opt into a committable in-repo artifact." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "learn",
                "description": "Drive the autonomous learning loop (observe → instinct → generated skill). `status` reports windowed observations + instinct/skill counts; `dry-run` previews what a cycle would promote; `run` distills observations into instincts and promotes trusted clusters to learned skills. The same cycle fires automatically at session end.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["status", "dry-run", "run"], "description": "Learning operation to perform. Defaults to status." },
                        "window": { "type": "integer", "description": "Observation window in days (for status/dry-run)." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
                    }
                }
            },
            {
                "name": "observe",
                "description": "Read-only session/workspace health: recall index, working-brief count. Prefer this over guessing closeout readiness. Token-savings axis stays on gain/session.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_root": { "type": "string", "description": "Workspace root. Defaults to cwd." },
                        "json": { "type": "boolean", "description": "Output as JSON. Default true on this tool when omitted? Prefer true for agents." }
                    }
                }
            },
            {
                "name": "rewrite",
                "description": "Inspect how keel would compact a shell command (returns the resolved `keel run -- …` wrapper). Use before trusting a raw noisy command path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to rewrite, e.g. \"cargo test --workspace\"." },
                        "json": { "type": "boolean", "description": "Machine-readable rewrite payload when supported." }
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "skill_eval",
                "description": "Run the deterministic skill-routing eval fixtures against the installed skill catalog (pass/fail per fixture). Use after editing skill frontmatter or routing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_root": { "type": "string", "description": "Repository root that owns skills. Defaults to cwd." },
                        "json": { "type": "boolean", "description": "JSON report. Default true recommended." }
                    }
                }
            },
            {
                "name": "design_intelligence",
                "description": "UI design-system recommendation packet (styles, palettes, typography, anti-patterns) for a product request. Use before implementing UI so visual choices are catalog-backed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "request": { "type": "string", "description": "Product/UI request text." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Extra flags (e.g. --stack next, --format json)." },
                        "json": { "type": "boolean", "description": "Prefer JSON when true." }
                    },
                    "required": ["request"]
                }
            },
            {
                "name": "stats",
                "description": "Unified keel dashboard: token savings + savings %, commands observed/compacted, top space-saving commands, gate/enforcement activity, recall index health. Lead with headline numbers. Aggregates gain/telemetry/observe; read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "days": { "type": "integer", "description": "Window in days for savings/timings. Default 7." },
                        "workspace_root": { "type": "string", "description": "Workspace root. Defaults to cwd." },
                        "json": { "type": "boolean", "description": "Output as JSON. Default true on this tool when omitted." }
                    }
                }
            }
        ]
    })
}

/// Shrink `tools/list` for stdio framing: truncate tool descriptions and drop
/// per-property schema descriptions (types/enums/required stay). A full catalog
/// with property prose was ~20KB of a 24KB frame ceiling — one more tool or a
/// longer description would trip the hard frame guard and look hung to hosts.
fn slim_tools_list_for_wire(mut list: Value) -> Value {
    let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) else {
        return list;
    };
    for tool in tools.iter_mut() {
        if let Some(desc) = tool
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let (kept, _) = truncate_chars(&desc, MAX_TOOLS_LIST_DESCRIPTION_CHARS);
            tool["description"] = Value::String(kept);
        }
        if let Some(props) = tool
            .pointer_mut("/inputSchema/properties")
            .and_then(Value::as_object_mut)
        {
            for (_key, schema) in props.iter_mut() {
                if let Some(obj) = schema.as_object_mut() {
                    obj.remove("description");
                }
            }
        }
    }
    list
}

/// Compact single-line JSON for MCP tool text. Pretty multi-line payloads bloat
/// the outer tools/call frame and have triggered host transport decode failures
/// that surface as full host tool timeouts.
fn mcp_json_compact(payload: &Value) -> Result<String, String> {
    serde_json::to_string(payload).map_err(|error| format!("serialize: {error}"))
}

pub(super) fn handle_tools_call(params: &Value) -> Result<Value, MethodError> {
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

    // Unknown tools fail fast without starting the deadline worker.
    if !is_known_mcp_tool(tool_name) {
        return Err(MethodError {
            code: JSON_RPC_INVALID_PARAMS,
            message: format!("Unknown tool: {tool_name}"),
        });
    }

    // Every tools/call runs under a wall-clock deadline so a blocked SQLite
    // lock, hung child, or slow catalog scan cannot freeze the stdio loop.
    // Child-spawning tools also apply an inner kill timeout (same budget).
    let name = tool_name.to_string();
    let name_for_worker = name.clone();
    let outcome = run_tool_with_deadline(mcp_child_timeout(), &name, move || {
        dispatch_mcp_tool(&name_for_worker, &arguments)
    });

    match outcome {
        Ok(text) => Ok(json!({
            "content": [
                { "type": "text", "text": truncate_mcp_text(&text) }
            ],
            "isError": false,
        })),
        Err(message) => Ok(json!({
            "content": [
                { "type": "text", "text": truncate_mcp_text(&message) }
            ],
            "isError": true,
        })),
    }
}

/// Canonical MCP tool name set. `tools_list_catalog`, [`is_known_mcp_tool`], and
/// [`dispatch_mcp_tool`] must agree with this list — the unit test
/// `mcp_tool_list_known_and_dispatch_are_one_set` fails if they drift.
const MCP_TOOL_NAMES: &[&str] = &[
    "recall",
    "system_map",
    "run_command",
    "command_output",
    "command_kill",
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
    "checkpoint",
    "session",
    "doctor",
    "code_search",
    "flow",
    "code_graph",
    "learn",
    "observe",
    "rewrite",
    "skill_eval",
    "design_intelligence",
    "stats",
];

type McpToolHandler = fn(&Value) -> Result<String, String>;

/// Resolve the handler for a tool name without invoking it. Used by dispatch and
/// by the parity test so completeness is proven without re-exec side effects.
fn mcp_tool_handler(name: &str) -> Option<McpToolHandler> {
    Some(match name {
        "recall" => tool_recall,
        "system_map" => tool_system_map,
        "run_command" => tool_run_command,
        "command_output" => tool_command_output,
        "command_kill" => tool_command_kill,
        "recall_status" => tool_recall_status,
        "skill_route" => tool_skill_route,
        "skill_get" => tool_skill_get,
        "skill_list" => tool_skill_list,
        "memory_status" => tool_memory_status,
        "brief_list" => tool_brief_list,
        "brief_get" => tool_brief_get,
        "brief_create" => tool_brief_create,
        "system_map_refresh" => tool_system_map_refresh,
        "context_brief" => tool_context_brief,
        "cli" => tool_cli,
        "anvil" => tool_anvil,
        "review" => tool_review,
        "git_workflow" => tool_git_workflow,
        "memory" => tool_memory,
        "gain" => tool_gain,
        "raw" => tool_raw,
        "config_audit" => tool_config_audit,
        "skill_lint" => tool_skill_lint,
        "telemetry" => tool_telemetry,
        "checkpoint" => tool_checkpoint,
        "session" => tool_session,
        "doctor" => tool_doctor,
        "code_search" => tool_code_search,
        "flow" => tool_flow,
        "code_graph" => tool_code_graph,
        "learn" => tool_learn,
        "observe" => tool_observe,
        "rewrite" => tool_rewrite,
        "skill_eval" => tool_skill_eval,
        "design_intelligence" => tool_design_intelligence,
        "stats" => tool_stats,
        _ => return None,
    })
}

fn is_known_mcp_tool(name: &str) -> bool {
    mcp_tool_handler(name).is_some()
}

fn dispatch_mcp_tool(tool_name: &str, arguments: &Value) -> Result<String, String> {
    match mcp_tool_handler(tool_name) {
        Some(handler) => handler(arguments),
        None => Err(format!("Unknown tool: {tool_name}")),
    }
}

/// Run an in-process tool body on a worker thread and return by `timeout`.
/// Per-call safety net: the serve loop is concurrent, but one stuck tool would
/// still hold an in-flight slot until this deadline fires. The worker may
/// outlive the deadline if stuck in a native lock; the caller still recovers.
fn run_tool_with_deadline<F>(timeout: Duration, label: &str, work: F) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("keel-mcp-{label}"))
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|error| format!("{label}: spawn tool worker: {error}"))?;

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "{label}: timed out after {}s (set KEEL_MCP_TOOL_TIMEOUT_SECS to raise; \
             prefer skill_route over skill_list; kill orphan `keel mcp serve` if calls keep hanging)",
            timeout.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("{label}: tool worker disconnected without a result"))
        }
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
    let claude_home = tool_claude_home("recall")?;
    // Workspace affinity: boost current-project hits above cross-project, and
    // slugged from cwd like system_map. `workspace` forces a slug.
    let workspace_slug = optional_string_arg(arguments, "workspace")
        .map(crate::utility::system_map::sanitize_key)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|cwd| crate::utility::system_map::sanitize_key(&cwd.to_string_lossy()))
        });
    let result = search_recall_index(&claude_home, &query, limit, workspace_slug.as_deref())
        .map_err(|error| format!("recall: {error}"))?;
    let mut payload = render_recall_payload(&claude_home, &query, limit, result);
    // `local_only`: restrict to the current workspace's lane only (a new
    // project returns empty instead of flooding with cross-project hits).
    if Some(true) == optional_bool_arg(arguments, "local_only") {
        if let Some(slug) = &workspace_slug {
            let slug_norm = collapse_dashes(&slug.to_ascii_lowercase());
            if let Some(matches) = payload.get_mut("matches").and_then(|m| m.as_array_mut()) {
                matches.retain(|hit| {
                    hit.get("path")
                        .and_then(|p| p.as_str())
                        .map(|p| collapse_dashes(&p.to_ascii_lowercase()).contains(&slug_norm))
                        .unwrap_or(false)
                });
            }
        }
    }
    mcp_json_compact(&payload).map_err(|error| format!("recall: {error}"))
}

fn render_recall_payload(
    claude_home: &Path,
    query: &str,
    limit: usize,
    result: Option<RecallSearchResult>,
) -> Value {
    let (fts_query, stage, hits) = match result {
        Some(search_result) => (
            search_result.fts_query,
            search_result.stage,
            search_result.hits,
        ),
        None => (String::new(), "exact", Vec::new()),
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
        "stage": stage,
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
    let workspace_override = workspace_root_arg(arguments);
    // Prefix at the call site, not inside system_map_text — that helper is also
    // the backing for the keel://system-map resource, which should keep
    // its bare error message.
    let text = system_map_text(workspace_override.as_deref())
        .map_err(|error| format!("system_map: {error}"))?;
    Ok(truncate_mcp_text(&text))
}

fn string_list_field(arguments: &Value, name: &str) -> Option<Vec<String>> {
    arguments.get(name).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

fn first_string_field(arguments: &Value, names: &[&str]) -> String {
    for name in names {
        if let Some(text) = arguments.get(*name).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

fn tool_run_command(arguments: &Value) -> Result<String, String> {
    let argv: Option<Vec<String>> =
        string_list_field(arguments, "argv").or_else(|| string_list_field(arguments, "args"));
    let script = arguments
        .get("script")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let shell = arguments
        .get("shell")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let command = first_string_field(arguments, &["command", "cmd", "input"]);
    let command = if command.is_empty() {
        first_string_field(arguments, &["args"])
    } else {
        command
    };
    let cwd = arguments
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let wait = optional_bool_arg(arguments, "wait").unwrap_or(true);

    // Three mutually exclusive input forms. Exactly one must be present; each
    // maps to (label, program, args) for the child. No fallback between forms:
    // an invalid combination is an error the agent can correct, never a silent
    // reinterpretation.
    let (label, program, shell_args) = if let Some(argv) = argv {
        if !script.is_empty() || !command.is_empty() {
            return Err(
                "run_command: pass exactly one of argv, script (+shell), or command".to_string(),
            );
        }
        if argv.is_empty() {
            return Err("run_command: argv must contain the program and its arguments".to_string());
        }
        // Direct exec: no shell at all, so nothing can be misquoted and no
        // platform guessing is involved.
        let label = argv.join(" ");
        let program = argv[0].clone();
        let args = argv[1..].to_vec();
        (label, program, args)
    } else if !script.is_empty() {
        if !command.is_empty() {
            return Err(
                "run_command: pass exactly one of argv, script (+shell), or command".to_string(),
            );
        }
        if shell.is_empty() {
            return Err(
                "run_command: script requires `shell` (powershell|cmd|bash) — the shell is explicit, never guessed".to_string(),
            );
        }
        let (program, args) = crate::runtime::named_shell_command_parts(&shell, &script)
            .map_err(|error| format!("run_command: {error}"))?;
        (format!("[{shell}] {script}"), program, args)
    } else if !command.is_empty() {
        if !shell.is_empty() {
            return Err(
                "run_command: `shell` only applies to `script`; use script+shell or argv"
                    .to_string(),
            );
        }
        // Legacy single-string form: platform default shell, as before.
        let (program, args) = crate::runtime::platform_shell_command_parts(&command);
        (command.clone(), program, args)
    } else {
        return Err(
            "run_command: missing input — pass argv (or args[]), script+shell, or command (aliases: cmd, input, args as a string)".to_string(),
        );
    };

    if command_nests_mcp_serve(&program, &shell_args, &label) {
        return Err(
            "run_command: refusing to start `keel mcp` from inside MCP (that re-enters the server and hangs)"
                .into(),
        );
    }

    // Token saver stays in-process (`keel run`) so MCP never re-execs this
    // binary and never hits the host tool budget from a nested serve.
    if wait
        && cwd
            .as_ref()
            .map(|path| path.as_os_str().is_empty())
            .unwrap_or(true)
    {
        let mut run_args = vec!["--".to_string(), program.clone()];
        run_args.extend(shell_args.iter().cloned());
        let previous_hook = env::var("CLAUDE_SKILLS_HOOK").ok();
        env::set_var("CLAUDE_SKILLS_HOOK", "mcp");
        let result = run_inprocess_cli(&format!("keel run -- {label}"), |out, err| {
            crate::runner::run_run_command(&run_args, out, err)
        });
        match previous_hook {
            Some(value) => env::set_var("CLAUDE_SKILLS_HOOK", value),
            None => env::remove_var("CLAUDE_SKILLS_HOOK"),
        }
        return result;
    }

    // Background jobs, or a cwd that must not mutate the MCP process, still
    // spawn `keel run` (never `keel mcp`).
    let executable =
        env::current_exe().map_err(|error| format!("run_command: locate self: {error}"))?;
    let mut child = Command::new(&executable);
    child.arg("run");
    child.arg("--");
    child.arg(&program);
    for argument in &shell_args {
        child.arg(argument);
    }
    if let Some(directory) = &cwd {
        child.current_dir(directory);
    }
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());

    if !wait {
        let command_id = spawn_background_command(child, &label)?;
        let payload = json!({
            "commandId": command_id,
            "running": true,
            "label": label,
            "hint": "poll with command_output, stop with command_kill",
        });
        return mcp_json_compact(&payload).map_err(|error| format!("run_command: {error}"));
    }

    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout(child, mcp_child_timeout(), "run_command")?;
    // `json` mode returns a structured object; default text report keeps real
    // newlines so multi-line build/test logs stay legible in the tool-result view.
    if Some(true) == optional_bool_arg(arguments, "json") {
        let payload = json!({
            "command": label,
            "exit_code": exit_code,
            "stdout": stdout_text,
            "stderr": stderr_text,
        });
        return mcp_json_compact(&payload).map_err(|error| format!("run_command: {error}"));
    }
    Ok(render_run_command_report(
        &label,
        exit_code,
        &stdout_text,
        &stderr_text,
    ))
}

// ---------------------------------------------------------------------------
// Background command registry — the `wait:false` surface of run_command.
//
// Process-local: each `keel mcp serve` owns the commands it spawned. A reaper
// thread per command polls try_wait and records the exit code; two reader
// threads append chunked stdout/stderr into capped buffers so command_output
// can report live progress and a runaway producer cannot exhaust memory. The
// final result is returned exactly once: the finished:true response removes the
// entry from the registry.
// ---------------------------------------------------------------------------

/// Cap per captured stream of a background command. Beyond this the buffer
/// stops growing and a one-time marker records the truncation.
const BACKGROUND_STREAM_CAP_CHARS: usize = 2_000_000;

struct BackgroundCommand {
    label: String,
    started_at_millis: u128,
    pid: Option<u32>,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    /// `Some(child)` while running; the reaper or command_kill takes it out
    /// when the process ends and records the exit code in `exit`.
    child: Arc<Mutex<Option<std::process::Child>>>,
    exit: Arc<Mutex<Option<i32>>>,
}

fn background_registry() -> &'static Mutex<std::collections::HashMap<String, Arc<BackgroundCommand>>>
{
    use std::sync::LazyLock;
    static REGISTRY: LazyLock<Mutex<std::collections::HashMap<String, Arc<BackgroundCommand>>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    &REGISTRY
}

fn next_background_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::LazyLock;
    static COUNTER: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(1));
    format!("c{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Spawn `child` in the background and register it under a fresh command id.
/// On Unix the child becomes its own process-group leader (setsid) so
/// `kill_process_tree` can reach every descendant via `kill(-pid, SIGKILL)`.
/// On Windows `taskkill /T` walks the tree instead, so no setup is needed.
fn spawn_background_command(mut child: Command, label: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // setsid detaches the child into a new session + process group, making
        // its pid == pgid. Safe here: we never send SIGINT-style job-control
        // signals to background commands, only SIGKILL via kill_process_tree.
        unsafe {
            child.pre_exec(|| {
                if libc_setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut spawned = child
        .spawn()
        .map_err(|error| format!("run_command: background spawn: {error}"))?;
    let pid = spawned.id();
    let stdout_pipe = spawned
        .stdout
        .take()
        .ok_or_else(|| "run_command: background: missing stdout pipe".to_string())?;
    let stderr_pipe = spawned
        .stderr
        .take()
        .ok_or_else(|| "run_command: background: missing stderr pipe".to_string())?;

    let entry = Arc::new(BackgroundCommand {
        label: label.to_string(),
        started_at_millis: current_timestamp_millis(),
        pid: Some(pid),
        stdout: Arc::new(Mutex::new(String::new())),
        stderr: Arc::new(Mutex::new(String::new())),
        child: Arc::new(Mutex::new(Some(spawned))),
        exit: Arc::new(Mutex::new(None)),
    });
    let command_id = next_background_id();

    background_reader_thread(Arc::clone(&entry.stdout), stdout_pipe);
    background_reader_thread(Arc::clone(&entry.stderr), stderr_pipe);

    // Reaper: poll try_wait until the child exits (or kill takes it), then
    // record the exit code. Consistent with run_command_with_timeout_stdin's
    // poll loop — no platform-specific signals needed.
    let reaper_entry = Arc::clone(&entry);
    let _ = std::thread::Builder::new()
        .name(format!("keel-bg-reaper-{command_id}"))
        .spawn(move || loop {
            let outcome = {
                let mut guard = reaper_entry
                    .child
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match guard.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code().unwrap_or(-1);
                            *guard = None;
                            Some(code)
                        }
                        Ok(None) => None,
                        Err(_) => {
                            *guard = None;
                            Some(-1)
                        }
                    },
                    // command_kill already finished and nulled the child.
                    None => break,
                }
            };
            if let Some(code) = outcome {
                let mut exit_guard = reaper_entry
                    .exit
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *exit_guard = Some(code);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        });

    let mut registry = background_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.insert(command_id.clone(), entry);
    Ok(command_id)
}

/// Chunked reader: appends to the shared buffer as data arrives so
/// command_output can report live progress. Stops growing at the cap.
fn background_reader_thread(buffer: Arc<Mutex<String>>, mut pipe: impl Read + Send + 'static) {
    let _ = std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let mut guard = buffer
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if guard.chars().count() >= BACKGROUND_STREAM_CAP_CHARS {
                        if !guard.ends_with("[stream truncated at 2MB]") {
                            guard.push_str("\n[stream truncated at 2MB]");
                        }
                        continue;
                    }
                    guard.push_str(&String::from_utf8_lossy(&chunk[..count]));
                }
            }
        }
    });
}

fn tool_command_output(arguments: &Value) -> Result<String, String> {
    let command_id = arguments
        .get("command_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if command_id.is_empty() {
        return Err("command_output: missing command_id".to_string());
    }

    let entry = {
        let registry = background_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .get(&command_id)
            .cloned()
            .ok_or_else(|| format!("command_output: unknown command_id {command_id:?}"))?
    };

    let exit_code = *entry
        .exit
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stdout_text = entry
        .stdout
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let stderr_text = entry
        .stderr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let elapsed_millis = current_timestamp_millis().saturating_sub(entry.started_at_millis);

    if let Some(code) = exit_code {
        // Finished: return the full result exactly once, then release the id.
        let mut registry = background_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry.remove(&command_id);
        if Some(true) == optional_bool_arg(arguments, "json") {
            let payload = json!({
                "command_id": command_id,
                "label": entry.label,
                "running": false,
                "exit_code": code,
                "elapsed_ms": elapsed_millis,
                "stdout": stdout_text,
                "stderr": stderr_text,
            });
            return mcp_json_compact(&payload).map_err(|error| format!("command_output: {error}"));
        }
        let mut report = render_run_command_report(&entry.label, code, &stdout_text, &stderr_text);
        report.push_str(&format!("elapsed: {}ms\n", elapsed_millis));
        return Ok(report);
    }

    let payload = json!({
        "command_id": command_id,
        "label": entry.label,
        "running": true,
        "pid": entry.pid,
        "elapsed_ms": elapsed_millis,
        "stdout": truncate_chars(&stdout_text, max_mcp_text_chars()).0,
        "stderr": truncate_chars(&stderr_text, max_mcp_text_chars()).0,
        "hint": "still running — call command_output again later, or command_kill to stop it",
    });
    mcp_json_compact(&payload).map_err(|error| format!("command_output: {error}"))
}

fn tool_command_kill(arguments: &Value) -> Result<String, String> {
    let command_id = arguments
        .get("command_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if command_id.is_empty() {
        return Err("command_kill: missing command_id".to_string());
    }

    let entry = {
        let registry = background_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .get(&command_id)
            .cloned()
            .ok_or_else(|| format!("command_kill: unknown command_id {command_id:?}"))?
    };

    // Take the child out of the registry entry so the reaper stops polling it,
    // then kill + wait here. If the reaper already nulled it, the process is
    // already gone.
    let maybe_child = entry
        .child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    match maybe_child {
        Some(mut child) => {
            // Kill the whole tree, not just the direct child: on Windows the
            // direct child is usually a shell wrapper (`pwsh`/`cmd`) and
            // killing only it would orphan the real work it spawned. The
            // wrapper was started in a kill-on-close Job Object (Windows) or
            // its own process group (Unix), so the tree is reachable.
            kill_process_tree(&mut child);
            let status = child.wait();
            let code = status.ok().and_then(|status| status.code()).unwrap_or(-1);
            let mut exit_guard = entry
                .exit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if exit_guard.is_none() {
                *exit_guard = Some(code);
            }
            let payload = json!({
                "command_id": command_id,
                "label": entry.label,
                "killed": true,
                "exit_code": code,
            });
            mcp_json_compact(&payload).map_err(|error| format!("command_kill: {error}"))
        }
        None => {
            let code = *entry
                .exit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let payload = json!({
                "command_id": command_id,
                "label": entry.label,
                "killed": false,
                "already_finished": true,
                "exit_code": code,
            });
            mcp_json_compact(&payload).map_err(|error| format!("command_kill: {error}"))
        }
    }
}

/// Kill a child process AND everything it spawned, so a killed shell wrapper
/// never orphans the real work (e.g. a `flutter analyze` it launched).
///
/// Windows: `taskkill /T /F /PID <root>` force-terminates the root and all
/// its descendants by walking the process tree — no FFI needed, available on
/// every Windows edition. Direct `Child::kill` is the fallback if taskkill is
/// somehow unavailable.
/// Unix: the child runs in its own process group (spawned below via
/// `pre_exec` setsid), so `kill(-pgid, SIGKILL)` reaches the whole tree.
#[cfg(windows)]
fn kill_process_tree(child: &mut std::process::Child) {
    let pid = child.id();
    let taskkill = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if taskkill.is_err() {
        // taskkill unavailable: degrade to killing the root process only.
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn kill_process_tree(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    // Negative pid targets the process group; the child's pgid == its pid
    // because spawn_background_command made it a group leader via setsid.
    // (No `unsafe` here: the FFI is wrapped inside libc_kill_process_group.)
    let group_kill = libc_kill_process_group(pid);
    if group_kill != 0 {
        // Process group gone or not a leader: fall back to the root.
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn libc_kill_process_group(pid: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SIGKILL = 9.
    unsafe { kill(-pid, 9) }
}

#[cfg(unix)]
fn libc_setsid() -> i32 {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe { setsid() }
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
    mcp_json_compact(&payload).map_err(|error| format!("recall_status: {error}"))
}

fn tool_skill_route(arguments: &Value) -> Result<String, String> {
    let prompt = arguments
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if prompt.is_empty() {
        return Err("skill_route: missing prompt".to_string());
    }
    let claude_home = tool_claude_home("skill_route")?;
    let payload = match match_skill_for_prompt(&claude_home, &prompt) {
        Some(found) => {
            // Present on disk is mandatory: never return a name the agent cannot
            // open (host Skill() catalog lag → use path + skill_get / Read).
            let Some(path) = installed_skill_path(&claude_home, &found.name) else {
                return Err(format!(
                    "skill_route: matched `{}` but SKILL.md is missing under installed skills — run `keel install`",
                    found.name
                ));
            };
            let brief = skill_inline_brief(&claude_home, &found.name);
            if brief.is_none() {
                return Err(format!(
                    "skill_route: matched `{}` but skill body is unreadable at {} — reinstall with `keel install`",
                    found.name,
                    display_path(&path)
                ));
            }
            // Surface related skills that are actually installed, so the agent
            // knows adjacent skills exist without a separate skill_list call.
            let installed: std::collections::BTreeSet<String> = skill_catalog(&claude_home)
                .into_iter()
                .map(|entry| entry.name)
                .collect();
            let related_installed: Vec<String> = skill_catalog(&claude_home)
                .into_iter()
                .find(|entry| entry.name == found.name)
                .map(|entry| {
                    entry
                        .related_skills
                        .into_iter()
                        .filter(|name| {
                            installed.contains(name)
                                && name != &found.name
                                && installed_skill_path(&claude_home, name).is_some()
                        })
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "matched": true,
                "name": found.name,
                "score": format!("{:.4}", found.score),
                "path": display_path(&path),
                "present": true,
                "brief": brief,
                "relatedSkills": related_installed,
                "note": "If host Skill() says Unknown skill, Read `path` or call skill_get — file is on disk.",
            })
        }
        None => json!({
            "matched": false,
            "name": Value::Null,
            "score": Value::Null,
            "path": Value::Null,
            "present": false,
            "brief": Value::Null,
        }),
    };
    mcp_json_compact(&payload).map_err(|error| format!("skill_route: {error}"))
}

fn tool_skill_get(arguments: &Value) -> Result<String, String> {
    let name = arguments
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("skill_get: missing name".to_string());
    }
    let claude_home = tool_claude_home("skill_get")?;
    match skill_full_body(&claude_home, &name) {
        Some((path, body)) => {
            // Compact JSON (not pretty): pretty multi-line inner payloads inflate
            // the outer tools/call frame and have triggered host transport decode
            // failures that surface as 120s MCP tool timeouts.
            let body_chars = body.chars().count();
            let (body_out, truncated) = truncate_chars(&body, MAX_SKILL_BODY_CHARS);
            let payload = json!({
                "name": name,
                "path": display_path(&path),
                "body": body_out,
                "bodyChars": body_chars,
                "truncated": truncated,
            });
            mcp_json_compact(&payload).map_err(|error| format!("skill_get: {error}"))
        }
        None => Err(format!(
            "skill_get: no installed skill named {name:?} (or name is unsafe)"
        )),
    }
}

fn tool_skill_list(_arguments: &Value) -> Result<String, String> {
    let claude_home = tool_claude_home("skill_list")?;
    let catalog = skill_catalog(&claude_home);
    let installed_names: std::collections::BTreeSet<String> =
        catalog.iter().map(|entry| entry.name.clone()).collect();
    let skills: Vec<Value> = catalog
        .iter()
        .filter_map(|entry| {
            // Only list skills with a readable SKILL.md (same gate as skill_get).
            let path = installed_skill_path(&claude_home, &entry.name)?;
            let (description, _) = truncate_chars(&entry.description, MAX_SKILL_LIST_FIELD_CHARS);
            let (when_to_use, _) = truncate_chars(&entry.when_to_use, MAX_SKILL_LIST_FIELD_CHARS);
            let related: Vec<String> = entry
                .related_skills
                .iter()
                .filter(|name| {
                    installed_names.contains(*name)
                        && installed_skill_path(&claude_home, name).is_some()
                })
                .cloned()
                .collect();
            Some(json!({
                "name": entry.name,
                "path": display_path(&path),
                "present": true,
                "description": description,
                "whenToUse": when_to_use,
                "useCount": entry.use_count,
                "relatedSkills": related,
            }))
        })
        .collect();
    let payload = json!({
        "count": skills.len(),
        "skills": skills,
    });
    // Compact: pretty catalog was ~33KB and blew host MCP frame budgets.
    mcp_json_compact(&payload).map_err(|error| format!("skill_list: {error}"))
}

fn tool_memory_status(_arguments: &Value) -> Result<String, String> {
    let index = recall_status_payload().map_err(|message| format!("memory_status: {message}"))?;
    let claude_home = tool_claude_home("memory_status")?;
    let families: Vec<Value> = family_counts(&claude_home, DEFAULT_MEMORY_GROUP)
        .iter()
        .map(|(family, count)| {
            json!({
                "family": family,
                "records": count,
            })
        })
        .collect();
    let payload = json!({
        "group": DEFAULT_MEMORY_GROUP,
        "index": index,
        "families": families,
    });
    mcp_json_compact(&payload).map_err(|error| format!("memory_status: {error}"))
}

/// Build a serde_json view of a [`Brief`] for MCP output. Mirrors
/// `working_brief::brief_to_value` (which produces the crate's own `json::Value`)
/// but emits `serde_json::Value` so the MCP layer stays on one JSON type.
fn brief_to_json(brief: &Brief) -> Value {
    json!({
        "id": brief.id,
        "request": brief.request,
        "constraints": brief.constraints,
        "acceptanceCriteria": brief.acceptance_criteria,
        "assumptions": brief.assumptions,
        "workspace": brief.workspace,
        "createdAt": brief.created_at,
    })
}

fn tool_brief_list(_arguments: &Value) -> Result<String, String> {
    let claude_home = tool_claude_home("brief_list")?;
    let briefs = list_briefs(&claude_home).map_err(|error| format!("brief_list: {error}"))?;
    let entries: Vec<Value> = briefs.iter().map(brief_to_json).collect();
    let payload = json!({
        "count": entries.len(),
        "briefs": entries,
    });
    mcp_json_compact(&payload).map_err(|error| format!("brief_list: {error}"))
}

fn tool_brief_get(arguments: &Value) -> Result<String, String> {
    let id = brief_id_arg(arguments, "brief_get")?;
    let claude_home = tool_claude_home("brief_get")?;
    let payload =
        match read_brief(&claude_home, &id).map_err(|error| format!("brief_get: {error}"))? {
            Some(brief) => json!({ "found": true, "brief": brief_to_json(&brief) }),
            None => json!({ "found": false, "id": id }),
        };
    mcp_json_compact(&payload).map_err(|error| format!("brief_get: {error}"))
}

fn tool_brief_create(arguments: &Value) -> Result<String, String> {
    let request = arguments
        .get("request")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if request.is_empty() {
        return Err("brief_create: missing request".to_string());
    }
    let claude_home = tool_claude_home("brief_create")?;
    let now_millis = current_timestamp_millis();
    // An explicit id, when supplied, becomes the brief filename stem, so it must
    // be a single safe path segment — never a separator/`..`/absolute path that
    // could steer the write outside the working-briefs directory. Omitted id
    // gets a generated `wb-<hex>` stem, which is always safe.
    let id = match arguments.get("id").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => safe_path_segment(raw).ok_or_else(|| {
            format!("brief_create: id must be a single safe path segment, got {raw:?}")
        })?,
        _ => format!("wb-{now_millis:x}"),
    };
    // Capture the workspace this brief belongs to so the working-brief gate can
    // scope it to a project; empty when the cwd cannot be resolved (treated as
    // "applies anywhere"), exactly like the CLI write path.
    let workspace = env::current_dir()
        .map(|cwd| display_path(&cwd))
        .unwrap_or_default();
    let brief = create_brief(
        id,
        request,
        string_list_arg(arguments, "constraints"),
        string_list_arg(arguments, "acceptance_criteria"),
        string_list_arg(arguments, "assumptions"),
        workspace,
        format_timestamp_iso8601(now_millis),
    );
    let path =
        write_brief(&claude_home, &brief).map_err(|error| format!("brief_create: {error}"))?;
    // Multi-piece on-ramp: 2+ criteria means drive them through Anvil.
    let mut payload = json!({
        "written": true,
        "path": display_path(&path),
        "brief": brief_to_json(&brief),
    });
    if brief.acceptance_criteria.len() >= 2 {
        payload["next_step"] = Value::String(format!(
            "{} acceptance criteria -> run `keel anvil compile --goal ... --bar ...` then `keel anvil run`",
            brief.acceptance_criteria.len()
        ));
    }
    mcp_json_compact(&payload).map_err(|error| format!("brief_create: {error}"))
}

fn tool_system_map_refresh(arguments: &Value) -> Result<String, String> {
    let workspace_root = match workspace_root_arg(arguments) {
        Some(path) => path,
        None => env::current_dir().map_err(|error| format!("system_map_refresh: cwd: {error}"))?,
    };
    let claude_home = tool_claude_home("system_map_refresh")?;
    let path = refresh_system_map(&claude_home, DEFAULT_MEMORY_GROUP, &workspace_root)
        .map_err(|error| format!("system_map_refresh: {error}"))?;
    let payload = json!({
        "refreshed": true,
        "path": display_path(&path),
        "workspaceRoot": display_path(&workspace_root),
    });
    mcp_json_compact(&payload).map_err(|error| format!("system_map_refresh: {error}"))
}

/// One-call awareness payload: the iron law, the installed skill catalog,
/// durable-memory health, and the newest working brief. The point is that an
/// agent reaching the MCP surface with no skill auto-loaded can call this once
/// and know what exists. Read-only; every section fails open to an empty/marker
/// value rather than erroring the whole call, so partial state still informs.
fn tool_context_brief(_arguments: &Value) -> Result<String, String> {
    let claude_home = tool_claude_home("context_brief")?;

    let catalog = skill_catalog(&claude_home);
    let skills: Vec<Value> = catalog
        .iter()
        .map(|entry| {
            let (when_to_use, _) = truncate_chars(&entry.when_to_use, MAX_SKILL_LIST_FIELD_CHARS);
            json!({
                "name": entry.name,
                "whenToUse": when_to_use,
            })
        })
        .collect();

    // Memory health: reuse the recall-status snapshot, tolerate failure.
    let memory = match recall_status_payload() {
        Ok(index) => json!({
            "index": index,
            "families": family_counts(&claude_home, DEFAULT_MEMORY_GROUP)
                .iter()
                .map(|(family, count)| json!({ "family": family, "records": count }))
                .collect::<Vec<Value>>(),
        }),
        Err(message) => json!({ "unavailable": message }),
    };

    // Newest working brief, if any. list_briefs is oldest-first → take the last.
    let newest_brief = match list_briefs(&claude_home) {
        Ok(briefs) => briefs.last().map(brief_to_json).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };

    let payload = json!({
        "ironLaw": IRON_LAW_SUMMARY,
        "skillCount": skills.len(),
        "skills": skills,
        "memory": memory,
        "newestBrief": newest_brief,
        "next": "Use skill_route to pick a skill, skill_get to load its full body, recall for memory, and cli for any other keel surface.",
    });
    // Compact JSON: pretty context_brief was multi-line and near frame limits.
    mcp_json_compact(&payload).map_err(|error| format!("context_brief: {error}"))
}

/// Compact restatement of the four-rule Iron Law for the awareness payload. The
/// authoritative long form is injected at SessionStart by the hook layer; this
/// is the pull-channel equivalent for sessions where that injection did not
/// reach the model.
const IRON_LAW_SUMMARY: &str = "Before anything that could touch code, config, or architecture: (1) Read first — read the SYSTEM_MAP and the owning file before claiming behavior. (2) Understand before building — restate the request and research what is needed; never build against an imagined spec. (3) Invoke relevant skills — if a keel skill might apply, route to it before answering. (4) Find the root cause — trace the symptom with file:line evidence before changing anything.";

/// keel subcommands the `cli` passthrough refuses outright. `mcp` would
/// recurse into another server; the destructive/management set is gated behind
/// `confirm: true` rather than refused (see [`CLI_CONFIRM_SUBCOMMANDS`]).
const CLI_REFUSED_SUBCOMMANDS: &[&str] = &["mcp"];

/// keel subcommands that mutate the install, the binary, or the working
/// tree destructively. The `cli` tool runs these only when the caller passes
/// `confirm: true`, so a model cannot silently reinstall, repair, uninstall, or
/// restore over a working tree. Read/inspection subcommands are not listed and
/// run directly. Two further actions are gated by second-arg checks in
/// [`tool_cli`] rather than whole-group entries here, because their group also
/// has read-only members: `checkpoint restore` and `hook install`/`hook uninstall`.
/// `verify` is intentionally absent — it is a read-only diff/audit pass.
const CLI_CONFIRM_SUBCOMMANDS: &[&str] = &[
    "install",
    "update",
    "repair",
    "uninstall",
    "remove",
    "validate",
    "all",
    "__self-replace",
];

/// Generic passthrough to the full keel CLI. Covers every subcommand
/// the dedicated tools do not, so the MCP surface matches the CLI surface
/// without one wrapper per subcommand. Output is the same compacted report shape
/// as `run_command`. Destructive/management subcommands require `confirm: true`;
/// `checkpoint restore` (overwrites the working tree) and `hook install`/`hook
/// uninstall` (mutate global settings.json) are gated the same way.
fn tool_cli(arguments: &Value) -> Result<String, String> {
    let args: Vec<String> = match arguments.get("args") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => return Err("cli: missing args (a JSON array of strings)".to_string()),
    };
    let subcommand = match args.first() {
        Some(first) if !first.trim().is_empty() => first.trim().to_string(),
        _ => return Err("cli: args must start with a subcommand".to_string()),
    };

    if CLI_REFUSED_SUBCOMMANDS.contains(&subcommand.as_str()) || subcommand == "cli" {
        return Err(format!(
            "cli: subcommand {subcommand:?} is not available through MCP"
        ));
    }

    let confirm = arguments
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Two subcommand GROUPS gate on a second-arg action rather than the whole
    // group, because the group also has read-only members:
    //   * `checkpoint restore` overwrites the working tree (list/show/create are safe).
    //   * `hook install`/`hook uninstall` mutate the global ~/.claude/settings.json
    //     (list/show/diagnose/instructions are read-only).
    let second_arg = args.get(1).map(String::as_str).unwrap_or("");
    let is_checkpoint_restore = subcommand == "checkpoint" && second_arg == "restore";
    let is_hook_mutating = subcommand == "hook" && matches!(second_arg, "install" | "uninstall");
    let needs_confirm = CLI_CONFIRM_SUBCOMMANDS.contains(&subcommand.as_str())
        || is_checkpoint_restore
        || is_hook_mutating;
    if needs_confirm && !confirm {
        return Err(format!(
            "cli: subcommand {subcommand:?} is destructive/management — re-call with confirm:true to run it"
        ));
    }

    // In-process: spawning current_exe() from `keel mcp serve` re-enters the
    // same binary and regularly exceeds the host's ~30s tool budget.
    run_inprocess_cli(&format!("keel {}", args.join(" ")), |out, err| {
        crate::commands::Application::new(env!("CARGO_PKG_VERSION")).run(&args, out, err)
    })
}

fn tool_anvil(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if action.is_empty() {
        return Err(
            "anvil: missing action (compile|cast|sieve|stamp|loop|run|prefix-check)".into(),
        );
    }
    let mut owned = vec![action];
    if let Some(Value::Array(items)) = arguments.get("args") {
        for item in items {
            if let Some(text) = item.as_str() {
                owned.push(text.to_string());
            }
        }
    }
    run_inprocess_cli("keel anvil", |out, err| {
        crate::utility::run_anvil_command(&owned, out, err)
    })
}

/// Run a dedicated MCP wrapper through the same in-process CLI dispatcher.
/// Spawning `current_exe()` from `keel mcp serve` re-enters the binary and
/// regularly exceeds the host tool budget.
fn run_keel_subcommand<S: AsRef<str>>(
    subcommand: &str,
    extra_args: &[S],
) -> Result<String, String> {
    if matches!(subcommand, "mcp" | "cli") {
        return Err(format!(
            "{subcommand}: not available through MCP (would re-enter the server)"
        ));
    }
    let mut args = vec![subcommand.to_string()];
    for arg in extra_args {
        args.push(arg.as_ref().to_string());
    }
    run_inprocess_cli(&format!("keel {}", args.join(" ")), |out, err| {
        crate::commands::Application::new(env!("CARGO_PKG_VERSION")).run(&args, out, err)
    })
}

fn command_nests_mcp_serve(program: &str, args: &[String], label: &str) -> bool {
    fn is_keel(name: &str) -> bool {
        std::path::Path::new(name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(name)
            .eq_ignore_ascii_case("keel")
    }
    if is_keel(program) && args.iter().any(|arg| arg == "mcp") {
        return true;
    }
    let tokens: Vec<&str> = label.split_whitespace().collect();
    tokens
        .windows(2)
        .any(|pair| is_keel(pair[0]) && pair[1] == "mcp")
}

fn mcp_child_timeout() -> Duration {
    let secs = env::var("KEEL_MCP_TOOL_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_MCP_CHILD_TIMEOUT_SECS)
        .clamp(5, 3_600);
    Duration::from_secs(secs)
}

/// Run a prepared `Command` with piped stdio, draining stdout/stderr on
/// helper threads so a full pipe cannot deadlock, and kill the child if it
/// exceeds `timeout`. Returns `(exit_code, stdout, stderr)`.
fn run_command_with_timeout(
    command: Command,
    timeout: Duration,
    label: &str,
) -> Result<(i32, String, String), String> {
    run_command_with_timeout_stdin(command, None, timeout, label)
}

/// Same as [`run_command_with_timeout`], optionally feeding `stdin_bytes` on a
/// writer thread so a slow consumer cannot block the kill path.
fn run_command_with_timeout_stdin(
    mut command: Command,
    stdin_bytes: Option<Vec<u8>>,
    timeout: Duration,
    label: &str,
) -> Result<(i32, String, String), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label}: spawn: {error}"))?;

    if let Some(bytes) = stdin_bytes {
        if let Some(mut stdin_pipe) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = stdin_pipe.write_all(&bytes);
                let _ = stdin_pipe.flush();
            });
        }
    }

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label}: missing stdout pipe"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| format!("{label}: missing stderr pipe"))?;

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = stdout_pipe;
        let _ = reader.read_to_string(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = stderr_pipe;
        let _ = reader.read_to_string(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    let async_hint = if label == "run_command" {
                        "; for long commands pass wait:false and poll command_output instead of waiting"
                    } else {
                        ""
                    };
                    return Err(format!(
                        "{label}: timed out after {}s{async_hint} (set KEEL_MCP_TOOL_TIMEOUT_SECS to raise; kill orphan `keel mcp serve` processes if tools keep hanging)",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(format!("{label}: wait: {error}"));
            }
        }
    };

    let stdout_text = stdout_handle
        .join()
        .unwrap_or_else(|_| String::from("(stdout reader panicked)"));
    let stderr_text = stderr_handle
        .join()
        .unwrap_or_else(|_| String::from("(stderr reader panicked)"));
    Ok((status.code().unwrap_or(-1), stdout_text, stderr_text))
}

fn max_mcp_text_chars() -> usize {
    env::var("KEEL_MCP_MAX_TEXT_CHARS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_MCP_TEXT_CHARS)
        .clamp(2_000, 200_000)
}

/// Truncate on a char boundary. Returns `(text, truncated)`.
fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let kept: String = text.chars().take(max_chars).collect();
    (kept, true)
}

fn truncate_mcp_text(text: &str) -> String {
    let max_chars = max_mcp_text_chars();
    let (kept, truncated) = truncate_chars(text, max_chars);
    if !truncated {
        return kept;
    }
    // why: never insert raw newlines into tool text — if a host ever treats the
    // content as a bare frame (or mis-buffers), interior newlines desync
    // newline-delimited JSON-RPC and surface as transport decode timeouts.
    format!(
        "{kept} … truncated for MCP (>{max_chars} chars). Prefer skill_route over skill_list; Read the skill path from skill_get when truncated=true; CLI for full output."
    )
}

#[cfg(test)]
mod mcp_timeout_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn truncate_mcp_text_leaves_small_input() {
        assert_eq!(truncate_mcp_text("hello"), "hello");
    }

    #[test]
    fn truncate_mcp_text_caps_large_input() {
        let max_chars = max_mcp_text_chars();
        let big = "x".repeat(max_chars + 5_000);
        let out = truncate_mcp_text(&big);
        assert!(out.contains("truncated for MCP"));
        assert!(out.chars().count() < big.chars().count());
        assert!(out.chars().count() <= max_chars + 200);
        assert!(
            !out.contains('\n'),
            "truncated tool text must not insert raw newlines (breaks NDJSON framing if mishandled)"
        );
    }

    #[test]
    fn truncate_chars_reports_flag() {
        let (small, flag) = truncate_chars("abc", 10);
        assert_eq!(small, "abc");
        assert!(!flag);
        let (big, flag) = truncate_chars("abcdefghij", 4);
        assert_eq!(big, "abcd");
        assert!(flag);
    }

    #[test]
    fn skill_get_payload_is_compact_single_line() {
        // The tools/call wrapper embeds this string; pretty multi-line payloads
        // bloat the outer frame and have caused host transport timeouts.
        let body = "line1\nline2\n".repeat(100);
        let (body_out, truncated) = truncate_chars(&body, MAX_SKILL_BODY_CHARS);
        let payload = json!({
            "name": "demo",
            "path": "/tmp/demo/SKILL.md",
            "body": body_out,
            "bodyChars": body.chars().count(),
            "truncated": truncated,
        });
        let text = serde_json::to_string(&payload).expect("serialize");
        assert!(
            !text.contains('\n'),
            "skill_get payload must be one JSON line"
        );
        assert!(text.contains("\"truncated\":"));
    }

    #[test]
    fn run_tool_with_deadline_returns_timeout_error() {
        let err = run_tool_with_deadline(Duration::from_millis(200), "slow-tool", || {
            std::thread::sleep(Duration::from_secs(5));
            Ok("should not return".into())
        })
        .expect_err("must time out");
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}"
        );
    }

    #[test]
    fn run_tool_with_deadline_returns_fast_ok() {
        let out = run_tool_with_deadline(Duration::from_secs(2), "fast-tool", || {
            Ok("ok-result".into())
        })
        .expect("fast tool");
        assert_eq!(out, "ok-result");
    }

    #[test]
    fn is_known_mcp_tool_covers_core_surface() {
        assert!(is_known_mcp_tool("skill_list"));
        assert!(is_known_mcp_tool("context_brief"));
        assert!(is_known_mcp_tool("recall"));
        assert!(!is_known_mcp_tool("not_a_real_tool"));
    }

    #[test]
    fn run_command_with_timeout_kills_long_child() {
        let mut command = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 >nul"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let err = run_command_with_timeout(command, Duration::from_secs(1), "timeout-test")
            .expect_err("must time out");
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}"
        );
    }

    #[test]
    fn default_mcp_tool_timeout_under_host_budgets() {
        // Host budgets often sit at 30s; default must stay below that floor.
        const {
            assert!(DEFAULT_MCP_CHILD_TIMEOUT_SECS < 30);
            assert!(DEFAULT_MCP_CHILD_TIMEOUT_SECS >= 5);
        }
        // With env unset, the live budget must match the constant (clamp applied).
        // Only assert when the override env is absent so parallel tests that set
        // KEEL_MCP_TOOL_TIMEOUT_SECS do not flake this pin.
        if env::var_os("KEEL_MCP_TOOL_TIMEOUT_SECS").is_none() {
            assert_eq!(
                mcp_child_timeout(),
                Duration::from_secs(DEFAULT_MCP_CHILD_TIMEOUT_SECS)
            );
        }
    }

    #[test]
    fn tools_list_wire_payload_under_stdio_ceiling_with_headroom() {
        // Drive the real list path (slimmed catalog). Framed envelope must leave
        // headroom under MAX_STDIO_FRAME_BYTES so adding tools does not instantly
        // trip the hard frame guard (which hosts experience as a hang).
        let listed = handle_tools_list();
        let framed = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": listed,
        });
        let serialized = serde_json::to_string(&framed).expect("serialize tools/list frame");
        assert!(
            !serialized.contains('\n'),
            "tools/list frame must be one NDJSON line"
        );
        const FRAME_CEILING: usize = 24_000; // keep in lockstep with mod.rs MAX_STDIO_FRAME_BYTES
        const HEADROOM: usize = 4_000;
        assert!(
            serialized.len() <= FRAME_CEILING,
            "tools/list frame {} exceeds stdio ceiling {}",
            serialized.len(),
            FRAME_CEILING
        );
        assert!(
            serialized.len() <= FRAME_CEILING - HEADROOM,
            "tools/list frame {} needs ≥{HEADROOM} bytes headroom under {FRAME_CEILING}",
            serialized.len()
        );
        // Slimming must drop property descriptions (types/enums remain).
        let tools = listed["tools"].as_array().expect("tools");
        assert!(!tools.is_empty());
        if let Some(props) = tools[0]
            .pointer("/inputSchema/properties")
            .and_then(Value::as_object)
        {
            for (key, schema) in props {
                assert!(
                    schema.get("description").is_none(),
                    "property {key} must not ship description on the wire"
                );
            }
        }
    }

    #[test]
    fn mcp_json_compact_is_single_line() {
        let payload = json!({"a": 1, "b": {"nested": true}});
        let text = mcp_json_compact(&payload).expect("serialize");
        assert!(!text.contains('\n'));
        assert!(!text.contains('\r'));
        assert!(text.starts_with('{'));
    }

    #[test]
    fn tools_call_envelope_with_multiline_text_is_single_json_line() {
        // Multi-line tool bodies (system_map, run_command) must ride inside JSON
        // string escapes so the NDJSON frame stays one physical line. Hosts
        // desync when a frame embeds raw 0x0A bytes mid-line.
        let multiline = "line1\nline2\r\nline3\n";
        let framed = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "result": {
                "content": [{ "type": "text", "text": truncate_mcp_text(multiline) }],
                "isError": false,
            },
        });
        let serialized = serde_json::to_string(&framed).expect("serialize");
        assert!(
            !serialized.as_bytes().contains(&b'\n'),
            "framed tools/call must not contain raw LF"
        );
        assert!(
            !serialized.as_bytes().contains(&b'\r'),
            "framed tools/call must not contain raw CR"
        );
        let parsed: Value = serde_json::from_str(&serialized).expect("parse");
        let text = parsed["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(text.contains("line1") && text.contains("line2"));
        // User-facing "/n" bug class: literal slash-n must not stand in for newline.
        assert!(
            !text.contains("/n"),
            "tool text must not contain literal /n stand-in for newline"
        );
    }

    #[test]
    fn truncate_mcp_text_suffix_never_uses_literal_slash_n() {
        let max_chars = max_mcp_text_chars();
        let big = format!("{}\nmore\n", "y".repeat(max_chars + 100));
        let out = truncate_mcp_text(&big);
        assert!(out.contains("truncated for MCP"));
        assert!(!out.contains("/n"), "suffix must not use /n");
        // Truncation path joins a single-line suffix; multi-line input is
        // shortened by char count but must not reintroduce raw newlines via
        // the suffix itself. Prefer compact single-line kept+suffix when
        // truncated.
        assert!(
            !out.contains('\n') && !out.contains('\r'),
            "truncated MCP text must stay free of raw CR/LF (was: {out:?})"
        );
    }

    #[test]
    fn using_keel_skill_fits_mcp_skill_get_body_cap() {
        // Bootstrap skill must fit skill_get's body budget so hosts receive the
        // full operative contract, not a truncated=true stub that drops rules.
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("using-keel")
            .join("SKILL.md");
        let text = std::fs::read_to_string(&repo)
            .unwrap_or_else(|e| panic!("read {}: {e}", repo.display()));
        // Body after frontmatter (frontmatter is small); whole file is the
        // practical wire cost when skill_get embeds `body`.
        assert!(
            text.chars().count() <= MAX_SKILL_BODY_CHARS,
            "using-keel/SKILL.md is {} chars; MCP skill_get caps body at {} — densify the skill or raise the cap with frame-size proof",
            text.chars().count(),
            MAX_SKILL_BODY_CHARS
        );
        assert!(
            text.contains("Iron Law") || text.contains("EXTREMELY_IMPORTANT"),
            "using-keel must still carry the iron-law contract after densification"
        );
        assert!(
            text.contains("preserve-existing-flow") || text.contains("Skill("),
            "using-keel must still point at skill invocation"
        );
    }

    #[test]
    fn slim_tools_list_truncates_long_descriptions() {
        let raw = json!({
            "tools": [{
                "name": "demo",
                "description": "x".repeat(MAX_TOOLS_LIST_DESCRIPTION_CHARS + 40),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "q": { "type": "string", "description": "should be stripped" }
                    }
                }
            }]
        });
        let slim = slim_tools_list_for_wire(raw);
        let desc = slim["tools"][0]["description"].as_str().unwrap_or("");
        assert_eq!(desc.chars().count(), MAX_TOOLS_LIST_DESCRIPTION_CHARS);
        assert!(slim["tools"][0]["inputSchema"]["properties"]["q"]
            .get("description")
            .is_none());
        assert_eq!(
            slim["tools"][0]["inputSchema"]["properties"]["q"]["type"],
            "string"
        );
    }
}

fn optional_string_arg<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn optional_bool_arg(arguments: &Value, key: &str) -> Option<bool> {
    arguments.get(key).and_then(Value::as_bool)
}

fn optional_int_arg(arguments: &Value, key: &str) -> Option<i64> {
    arguments.get(key).and_then(Value::as_i64)
}

fn collect_extra_args(arguments: &Value) -> Vec<String> {
    match arguments.get("args") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn tool_review(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "review: missing action (pre-commit|pre-pr|gates)".to_string())?;
    let all_args = review_args(action, arguments);
    run_keel_subcommand("review", &all_args)
}

/// Pure arg-builder for `tool_review`, extracted so the `gates → gates check`
/// rewrite is unit-testable without shelling out to the real binary. The CLI
/// surface is `keel review gates check [flags]` (see `run_review_gates_command`);
/// without the injected `check` the child exits 1 with "Unknown review gates
/// command", which surfaced to hosts as `isError` on a valid read-only call.
fn review_args(action: &str, arguments: &Value) -> Vec<String> {
    let mut all_args: Vec<String> = vec![action.to_string()];
    if action == "gates" {
        all_args.push("check".to_string());
    }
    if let Some(r) = optional_string_arg(arguments, "base_ref") {
        all_args.push(format!("--base-ref={r}"));
    }
    if let Some(f) = optional_string_arg(arguments, "format") {
        all_args.push(format!("--format={f}"));
    }
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        all_args.push(format!("--repo-root={root}"));
    }
    all_args
}

fn tool_git_workflow(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "git_workflow: missing action (preflight|await-ci|configure|show|commit-message|pr-body|lint-message)"
                .to_string()
        })?;
    let extras = collect_extra_args(arguments);
    let mut all_args: Vec<&str> = vec![action];
    let mut owned: Vec<String> = Vec::new();
    for e in &extras {
        owned.push(e.clone());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("git-workflow", &all_args)
}

fn tool_memory(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "memory: missing action (scope|system-map|recall|instincts|consolidate|report|research-cache|retrieve|maintenance|status)".to_string()
        })?;
    let all_args = memory_args(action, arguments);
    run_keel_subcommand("memory", &all_args)
}

/// Pure arg-builder for `tool_memory`, extracted so the double-prefix guard
/// (subcommand must not appear in the extra-args vector) is unit-testable
/// without shelling out to the real binary — which hangs in CI's clean env.
fn memory_args(action: &str, arguments: &Value) -> Vec<String> {
    let mut all_args: Vec<String> = vec![action.to_string()];
    for extra in collect_extra_args(arguments) {
        all_args.push(extra);
    }
    all_args
}

fn tool_gain(arguments: &Value) -> Result<String, String> {
    let since = optional_string_arg(arguments, "since").unwrap_or("today");
    let mut all_args: Vec<&str> = vec!["--since", since];
    let mut owned: Vec<String> = Vec::new();
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("gain", &all_args)
}

fn tool_raw(arguments: &Value) -> Result<String, String> {
    let action = optional_string_arg(arguments, "action");
    let raw_id = optional_string_arg(arguments, "raw_id");
    let older_than = optional_string_arg(arguments, "older_than");
    let mut all_args: Vec<&str> = Vec::new();
    let mut owned: Vec<String> = Vec::new();
    match action {
        Some("list") => {
            all_args.push("list");
        }
        Some("prune") => {
            all_args.push("prune");
            if let Some(ot) = older_than {
                owned.push(format!("--older-than={ot}"));
            }
        }
        _ => {}
    }
    if let Some(id) = raw_id {
        owned.push(id.to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    if action.is_none() && raw_id.is_none() {
        all_args = vec!["list"];
    }
    run_keel_subcommand("raw", &all_args)
}

fn tool_config_audit(arguments: &Value) -> Result<String, String> {
    let mut all_args: Vec<&str> = Vec::new();
    let mut owned: Vec<String> = Vec::new();
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        owned.push(format!("--repo-root={root}"));
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("config-audit", &all_args)
}

fn tool_skill_lint(arguments: &Value) -> Result<String, String> {
    let mut all_args: Vec<&str> = Vec::new();
    let mut owned: Vec<String> = Vec::new();
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        owned.push(format!("--repo-root={root}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("skill-lint", &all_args)
}

fn tool_telemetry(arguments: &Value) -> Result<String, String> {
    let mut all_args: Vec<&str> = vec!["summary"];
    let mut owned: Vec<String> = Vec::new();
    if let Some(d) = optional_int_arg(arguments, "days") {
        owned.push(format!("--days={d}"));
    }
    if let Some(t) = optional_int_arg(arguments, "top") {
        owned.push(format!("--top={t}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("telemetry", &all_args)
}

fn tool_checkpoint(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "checkpoint: missing action (create|list|show|restore)".to_string())?;
    if action == "restore" && !optional_bool_arg(arguments, "confirm").unwrap_or(false) {
        return Err(
            "checkpoint: restore is destructive — re-call with confirm:true to run it".to_string(),
        );
    }
    let mut all_args: Vec<&str> = vec![action];
    let mut owned: Vec<String> = Vec::new();
    if let Some(i) = optional_string_arg(arguments, "id") {
        owned.push(format!("--id={i}"));
    }
    if Some(true) == optional_bool_arg(arguments, "confirm") {
        owned.push("--confirm".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("checkpoint", &all_args)
}

fn tool_session(arguments: &Value) -> Result<String, String> {
    let mut all_args: Vec<&str> = Vec::new();
    let mut owned: Vec<String> = Vec::new();
    if let Some(s) = optional_string_arg(arguments, "since") {
        owned.push(format!("--since={s}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("session", &all_args)
}

fn tool_doctor(arguments: &Value) -> Result<String, String> {
    let mut all_args: Vec<&str> = Vec::new();
    let mut owned: Vec<String> = Vec::new();
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        owned.push(format!("--repo-root={root}"));
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("doctor", &all_args)
}

fn tool_code_search(arguments: &Value) -> Result<String, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("code_search: missing query".to_string());
    }
    let mut all_args: Vec<&str> = vec!["search"];
    let mut owned: Vec<String> = Vec::new();
    owned.push(format!("--query={query}"));
    if let Ok(cwd) = env::current_dir() {
        owned.push(format!("--workspace-root={}", display_path(&cwd)));
    }
    if let Some(f) = optional_string_arg(arguments, "format") {
        owned.push(format!("--format={f}"));
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("code-search", &all_args)
}

/// Preserve-Existing-Flow gate: the Iron Law's pre-edit ownership trace.
/// `start` records the owning file before an edit; `check` validates evidence
/// still holds; `finish` clears it. Routes through the compaction proxy.
fn tool_flow(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "flow: missing action (start|check|finish)".to_string())?;
    let all_args = flow_args(action, arguments);
    run_keel_subcommand("flow", &all_args)
}

/// Pure arg-builder for `tool_flow` (see `memory_args` for the testability rationale).
fn flow_args(action: &str, arguments: &Value) -> Vec<String> {
    let mut all_args: Vec<String> = vec![action.to_string()];
    if let Some(file) = optional_string_arg(arguments, "file") {
        all_args.push(format!("--target-file={file}"));
    }
    if let Some(target_function) = optional_string_arg(arguments, "target_function") {
        all_args.push(format!("--target-function={target_function}"));
    }
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        all_args.push(format!("--repo-root={root}"));
    }
    all_args
}

/// Deterministic codebase-understanding graph. `build` scans the workspace and
/// writes a JSON artifact of nodes (source files + symbols/imports) and edges
/// (cross-file import dependencies); `impact --changed a,b,c` reports the
/// transitive reverse-dependency closure, the cheap "what could this edit
/// break" query for review scoping.
fn tool_code_graph(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "code_graph: missing action (build|impact)".to_string())?;
    let mut all_args: Vec<&str> = vec![action];
    let mut owned: Vec<String> = Vec::new();
    if let Some(changed) = optional_string_arg(arguments, "changed") {
        owned.push(format!("--changed={changed}"));
    }
    if let Some(root) = optional_string_arg(arguments, "workspace_root") {
        owned.push(format!("--workspace-root={root}"));
    }
    if let Some(output) = optional_string_arg(arguments, "output") {
        owned.push(format!("--output={output}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("code-graph", &all_args)
}

/// Drive the autonomous learning loop (observe → instinct → skill). `status`
/// reports windowed observations + instinct/skill counts; `dry-run` previews
/// what a cycle would promote; `run` distills + promotes now. Pairs with the
/// session-end learning that fires automatically.
fn tool_learn(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status")
        .trim()
        .to_string();
    let allowed = ["status", "dry-run", "run"];
    if !allowed.contains(&action.as_str()) {
        return Err(format!(
            "learn: action {action:?} not recognized (status|dry-run|run)"
        ));
    }
    let mut all_args: Vec<&str> = vec![&action];
    let mut owned: Vec<String> = Vec::new();
    if let Some(w) = optional_int_arg(arguments, "window") {
        owned.push(format!("--window={w}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("learn", &all_args)
}

/// Capture stdout/stderr from an in-process CLI handler (same binary functions
/// as `keel <subcommand>`). Avoids re-exec of `current_exe()`, which under
/// `cargo test` is the test harness and can hang or recurse.
fn run_inprocess_cli<F>(label: &str, work: F) -> Result<String, String>
where
    F: FnOnce(&mut Vec<u8>, &mut Vec<u8>) -> u8,
{
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = i32::from(work(&mut stdout, &mut stderr));
    let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
    Ok(render_run_command_report(
        label,
        code,
        &stdout_text,
        &stderr_text,
    ))
}

/// Read-only aggregator: recall health + working briefs.
fn tool_observe(arguments: &Value) -> Result<String, String> {
    let mut owned: Vec<String> = Vec::new();
    if let Some(root) = optional_string_arg(arguments, "workspace_root") {
        owned.push(format!("--workspace-root={root}"));
    }
    // Agents almost always want structured health; default to JSON unless false.
    if optional_bool_arg(arguments, "json") != Some(false) {
        owned.push("--json".to_string());
    }
    run_inprocess_cli("keel observe", |out, err| {
        crate::utility::run_observe_command(&owned, out, err)
    })
}

/// Unified dashboard over gain/telemetry/gates/recall. Read-only.
fn tool_stats(arguments: &Value) -> Result<String, String> {
    let mut owned: Vec<String> = Vec::new();
    if let Some(days) = optional_int_arg(arguments, "days") {
        owned.push(format!("--days={days}"));
    }
    if let Some(root) = optional_string_arg(arguments, "workspace_root") {
        owned.push(format!("--workspace-root={root}"));
    }
    // Agents almost always want structured output; default to JSON unless false.
    if optional_bool_arg(arguments, "json") != Some(false) {
        owned.push("--json".to_string());
    }
    run_inprocess_cli("keel stats", |out, err| {
        crate::utility::run_stats_command(&owned, out, err)
    })
}

/// Inspect the compaction rewrite for a shell command (no execution).
fn tool_rewrite(arguments: &Value) -> Result<String, String> {
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if command.is_empty() {
        return Err("rewrite: missing command".to_string());
    }
    let mut owned: Vec<String> = Vec::new();
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    owned.push(command);
    run_inprocess_cli("keel rewrite", |out, err| {
        crate::runner::run_rewrite_command(&owned, out, err)
    })
}

/// Deterministic skill-routing fixture eval against the skill catalog.
fn tool_skill_eval(arguments: &Value) -> Result<String, String> {
    let mut owned: Vec<String> = Vec::new();
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        owned.push(format!("--repo-root={root}"));
    }
    if optional_bool_arg(arguments, "json") != Some(false) {
        owned.push("--json".to_string());
    }
    run_inprocess_cli("keel skill-eval", |out, err| {
        crate::utility::run_skill_eval_command(&owned, out, err)
    })
}

/// Catalog-backed UI design recommendation packet.
fn tool_design_intelligence(arguments: &Value) -> Result<String, String> {
    let request = arguments
        .get("request")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if request.is_empty() {
        return Err("design_intelligence: missing request".to_string());
    }
    let mut owned: Vec<String> = vec!["recommend".to_string(), request];
    if let Some(Value::Array(items)) = arguments.get("args") {
        for item in items {
            if let Some(s) = item.as_str() {
                if !s.trim().is_empty() {
                    owned.push(s.to_string());
                }
            }
        }
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--format=json".to_string());
    }
    run_inprocess_cli("keel design-intelligence", |out, err| {
        crate::utility::run_design_intelligence_command(&owned, out, err)
    })
}

/// Resolve the default harness home, prefixing any failure with the calling
/// tool's name so a resolution error reads `"<tool>: <reason>"` in the
/// tool-result envelope. Every handler resolves the same way; this keeps the
/// per-tool error prefix consistent without repeating the `map_err` closure.
fn tool_claude_home(tool: &str) -> Result<PathBuf, String> {
    resolve_claude_home("").map_err(|error| format!("{tool}: {error}"))
}

/// Read a `workspace_root` string argument, trimming and rejecting empties.
fn workspace_root_arg(arguments: &Value) -> Option<PathBuf> {
    arguments
        .get("workspace_root")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read and validate a required brief `id` argument for `tool`. The id is the
/// brief filename stem, so it must be a single safe path segment — this rejects
/// separators, `.`/`..`, absolute paths, and Windows drive-relative prefixes
/// before the id ever reaches `brief_path`, sandboxing the read/write to the
/// working-briefs directory.
fn brief_id_arg(arguments: &Value, tool: &str) -> Result<String, String> {
    let raw = arguments.get("id").and_then(Value::as_str).unwrap_or("");
    if raw.trim().is_empty() {
        return Err(format!("{tool}: missing id"));
    }
    safe_path_segment(raw)
        .ok_or_else(|| format!("{tool}: id must be a single safe path segment, got {raw:?}"))
}

/// Coerce a brief list field into `Vec<String>`. Accepts a JSON array of
/// strings (the schema-advertised shape) or, defensively, a single string. In
/// both cases every element is split on newlines: working-brief storage joins
/// list fields with `\n` and `read_brief` splits on `\n`, so an element that
/// itself contained a newline would round-trip as several elements. Splitting
/// here makes the in-memory list match what survives the disk round-trip.
/// Blank fragments are trimmed out; anything that is not an array or string
/// yields an empty list.
fn string_list_arg(arguments: &Value, key: &str) -> Vec<String> {
    let raw_pieces: Vec<&str> = match arguments.get(key) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(text)) => vec![text.as_str()],
        _ => return Vec::new(),
    };
    raw_pieces
        .iter()
        .flat_map(|piece| piece.split('\n'))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tools_list_advertises_all_tools() {
        let listed = handle_tools_list();
        let tools = listed["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .collect();
        for expected in MCP_TOOL_NAMES {
            assert!(
                names.contains(expected),
                "missing {expected} from tools/list: {names:?}"
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
        assert_eq!(
            names.len(),
            MCP_TOOL_NAMES.len(),
            "tools/list count must match MCP_TOOL_NAMES (extra names? listed={names:?})"
        );
    }

    #[test]
    fn mcp_tool_list_known_and_dispatch_are_one_set() {
        // Mechanical parity without invoking handlers (handlers may re-exec CLI).
        // list names == MCP_TOOL_NAMES == handler table.
        let listed = handle_tools_list();
        let tools = listed["tools"].as_array().expect("tools array");
        let mut list_names: Vec<String> = tools
            .iter()
            .filter_map(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        list_names.sort();
        let mut known: Vec<String> = MCP_TOOL_NAMES.iter().map(|s| (*s).to_string()).collect();
        known.sort();
        assert_eq!(
            list_names, known,
            "tools/list names must equal MCP_TOOL_NAMES"
        );

        for name in MCP_TOOL_NAMES {
            assert!(
                is_known_mcp_tool(name),
                "is_known_mcp_tool({name}) must be true"
            );
            assert!(
                mcp_tool_handler(name).is_some(),
                "dispatch handler table missing arm for {name}"
            );
            // MCP 2025-11-25: inputSchema MUST be a JSON Schema object.
            let schema = tools
                .iter()
                .find(|t| t.get("name").and_then(Value::as_str) == Some(*name))
                .and_then(|t| t.get("inputSchema"))
                .expect("inputSchema present");
            assert_eq!(
                schema.get("type").and_then(Value::as_str),
                Some("object"),
                "{name} inputSchema.type must be object"
            );
        }
        assert!(
            mcp_tool_handler("definitely_not_a_tool").is_none(),
            "unknown names must not resolve"
        );
    }

    #[test]
    fn cli_allowlist_covers_agent_critical_subcommand_families() {
        // Policy proof (no re-exec): agent-critical families are not refused and
        // are not whole-group confirm-gated. Live binary smoke is in mcp-smoke.log.
        // Dedicated MCP tools cover observe/rewrite/skill_eval/dispatch/design_intelligence;
        // bridge/eval/bench/team/hook remain via-cli.
        for sub in [
            "observe",
            "rewrite",
            "skill-eval",
            "anvil",
            "design-intelligence",
            "bridge",
            "eval",
            "bench",
            "hook",
        ] {
            assert!(
                !CLI_REFUSED_SUBCOMMANDS.contains(&sub),
                "{sub} must not be MCP-refused"
            );
            assert!(
                !CLI_CONFIRM_SUBCOMMANDS.contains(&sub),
                "{sub} must not require confirm as a whole group (mutate via second-arg or CLI --confirm)"
            );
        }
        // Destructive members still gated
        assert!(CLI_CONFIRM_SUBCOMMANDS.contains(&"install"));
        assert!(CLI_REFUSED_SUBCOMMANDS.contains(&"mcp"));
    }

    #[test]
    fn anvil_requires_action() {
        let params = json!({
            "name": "anvil",
            "arguments": {}
        });
        let result = handle_tools_call(&params).expect("envelope");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("missing action"),
            "anvil must require action: {text}"
        );
    }

    #[test]
    fn observe_and_rewrite_tools_smoke() {
        let obs = handle_tools_call(&json!({
            "name": "observe",
            "arguments": { "json": true }
        }))
        .expect("observe envelope");
        assert_eq!(obs["isError"], json!(false), "observe body: {}", obs);
        let rew = handle_tools_call(&json!({
            "name": "rewrite",
            "arguments": { "command": "cargo test" }
        }))
        .expect("rewrite envelope");
        assert_eq!(rew["isError"], json!(false), "rewrite body: {}", rew);
        let text = rew["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("run") || text.contains("cargo"),
            "rewrite should mention run/cargo: {text}"
        );
    }

    #[test]
    fn iron_law_orientation_tools_call_succeed() {
        // Isolate claude-home so parallel suite tests cannot lock our recall SQLite.
        let _env = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let home = std::env::temp_dir().join(format!("keel-mcp-iron-law-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("temp claude home");
        let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &home);

        // Shipped handle_tools_call path — protocol envelope isError:false.
        for (name, args) in [
            ("context_brief", json!({})),
            ("system_map", json!({})),
            ("recall_status", json!({})),
            (
                "skill_route",
                json!({ "prompt": "review this pull request for production readiness" }),
            ),
        ] {
            let result = handle_tools_call(&json!({
                "name": name,
                "arguments": args
            }))
            .unwrap_or_else(|e| panic!("{name} protocol error: {e:?}"));
            assert_eq!(
                result["isError"],
                json!(false),
                "{name} isError true: {}",
                result
            );
            let text = result["content"][0]["text"].as_str().unwrap_or("");
            assert!(!text.trim().is_empty(), "{name} empty content");
        }
        // recall needs a query; empty corpus may return zero hits but not isError.
        let recall = handle_tools_call(&json!({
            "name": "recall",
            "arguments": { "query": "iron law system map", "limit": 5 }
        }))
        .expect("recall envelope");
        assert_eq!(
            recall["isError"],
            json!(false),
            "recall isError: {}",
            recall
        );

        match previous {
            Some(v) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", v),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_eval_design_intelligence_dispatch_list_succeed() {
        let se = handle_tools_call(&json!({
            "name": "skill_eval",
            "arguments": { "repo_root": ".", "json": true }
        }))
        .expect("skill_eval envelope");
        assert_eq!(se["isError"], json!(false), "skill_eval failed: {}", se);

        let di = handle_tools_call(&json!({
            "name": "design_intelligence",
            "arguments": {
                "request": "saas analytics dashboard",
                "json": true
            }
        }))
        .expect("design_intelligence envelope");
        assert_eq!(
            di["isError"],
            json!(false),
            "design_intelligence failed: {}",
            di
        );

        let anvil = handle_tools_call(&json!({
            "name": "anvil",
            "arguments": { "action": "prefix-check" }
        }))
        .expect("anvil envelope");
        assert_eq!(anvil["isError"], json!(false), "anvil failed: {}", anvil);
    }

    #[test]
    fn unknown_tool_reports_invalid_params() {
        let params = json!({ "name": "definitely-not-real", "arguments": {} });
        let error = handle_tools_call(&params).expect_err("unknown tool errors");
        assert_eq!(error.code, JSON_RPC_INVALID_PARAMS);
    }

    #[test]
    fn skill_route_missing_prompt_is_tool_error() {
        // Empty prompt -> Ok envelope with isError true, not a protocol error.
        let params = json!({ "name": "skill_route", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing prompt"), "text: {text}");
    }

    #[test]
    fn brief_create_missing_request_is_tool_error() {
        let params = json!({ "name": "brief_create", "arguments": { "request": "   " } });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing request"), "text: {text}");
    }

    #[test]
    fn cli_missing_args_is_tool_error() {
        let params = json!({ "name": "cli", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing args"), "text: {text}");
    }

    #[test]
    fn cli_refuses_mcp_subcommand() {
        // The mcp subcommand would recurse into another server; refuse outright.
        let params = json!({ "name": "cli", "arguments": { "args": ["mcp", "serve"] } });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("not available through MCP"), "text: {text}");
    }

    #[test]
    fn cli_refuses_nested_cli_subcommand() {
        let params = json!({ "name": "cli", "arguments": { "args": ["cli", "status"] } });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("not available through MCP"), "text: {text}");
    }

    #[test]
    fn cli_gates_destructive_subcommand_without_confirm() {
        // A management subcommand must refuse unless confirm:true is passed. This
        // asserts the gate WITHOUT running the subcommand (no confirm → early
        // return before any spawn).
        for sub in [
            "install",
            "uninstall",
            "repair",
            "update",
            "validate",
            "all",
        ] {
            let params = json!({ "name": "cli", "arguments": { "args": [sub] } });
            let result = handle_tools_call(&params).expect("envelope present");
            assert_eq!(result["isError"], json!(true), "sub {sub} must gate");
            let text = result["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("confirm:true"),
                "sub {sub} must name the confirm gate: {text}"
            );
        }
    }

    #[test]
    fn cli_gates_checkpoint_restore_but_not_checkpoint_list() {
        // `checkpoint restore` overwrites the working tree → gated. Other
        // checkpoint actions are not gated by the confirm rule (they still run;
        // here we only assert the gate decision, so use a no-confirm restore).
        let restore = json!({ "name": "cli", "arguments": { "args": ["checkpoint", "restore", "--id", "x"] } });
        let result = handle_tools_call(&restore).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("confirm:true"), "restore must gate: {text}");
    }

    #[test]
    fn cli_gates_hook_install_and_uninstall() {
        // `hook install`/`hook uninstall` mutate the global settings.json, so they
        // are gated by a second-arg check even though the `hook` group has
        // read-only members. No confirm → refuse before any spawn.
        for action in ["install", "uninstall"] {
            let params = json!({ "name": "cli", "arguments": { "args": ["hook", action] } });
            let result = handle_tools_call(&params).expect("envelope present");
            assert_eq!(result["isError"], json!(true), "hook {action} must gate");
            let text = result["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("confirm:true"),
                "hook {action} must name the confirm gate: {text}"
            );
        }
    }

    #[test]
    fn string_list_arg_accepts_array_and_string() {
        let from_array =
            string_list_arg(&json!({ "constraints": ["a", "  ", "b"] }), "constraints");
        assert_eq!(from_array, vec!["a".to_string(), "b".to_string()]);
        let from_string = string_list_arg(&json!({ "constraints": "one\n  \ntwo" }), "constraints");
        assert_eq!(from_string, vec!["one".to_string(), "two".to_string()]);
        let missing = string_list_arg(&json!({}), "constraints");
        assert!(missing.is_empty());
    }

    #[test]
    fn string_list_arg_splits_embedded_newlines_in_array_elements() {
        // Regression: a list element containing a newline would round-trip
        // through write_brief (joins on \n) / read_brief (splits on \n) as
        // multiple elements. Splitting here makes the in-memory list match the
        // disk round-trip so brief_create's response cannot diverge from a later
        // brief_get.
        let split = string_list_arg(
            &json!({ "constraints": ["foo\nbar", "baz"] }),
            "constraints",
        );
        assert_eq!(
            split,
            vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]
        );
    }

    #[test]
    fn brief_create_rejects_traversal_id() {
        // Security regression: a caller-supplied id is the brief filename stem,
        // so a separator/.. /absolute id must be refused before it can steer the
        // write outside the working-briefs directory.
        for evil in ["../evil", "../../etc/passwd", "/abs/path", "a/b", "C:foo"] {
            let params = json!({
                "name": "brief_create",
                "arguments": { "request": "do the thing", "id": evil }
            });
            let result = handle_tools_call(&params).expect("envelope present");
            assert_eq!(
                result["isError"],
                json!(true),
                "id {evil:?} must be rejected"
            );
            let text = result["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("safe path segment"),
                "id {evil:?} text: {text}"
            );
        }
    }

    #[test]
    fn brief_get_rejects_traversal_id() {
        for evil in ["../secret", "/etc/hosts", "a\\b", "C:foo", "   "] {
            let params = json!({ "name": "brief_get", "arguments": { "id": evil } });
            let result = handle_tools_call(&params).expect("envelope present");
            assert_eq!(
                result["isError"],
                json!(true),
                "id {evil:?} must be rejected"
            );
        }
    }

    #[test]
    fn brief_to_json_carries_all_fields() {
        let brief = Brief {
            id: "wb-1".to_string(),
            request: "do the thing".to_string(),
            constraints: vec!["fast".to_string()],
            acceptance_criteria: vec!["tests pass".to_string()],
            assumptions: vec!["x is y".to_string()],
            workspace: "/repo".to_string(),
            created_at: "2026-06-08T00:00:00Z".to_string(),
        };
        let value = brief_to_json(&brief);
        assert_eq!(value["id"], json!("wb-1"));
        assert_eq!(value["request"], json!("do the thing"));
        assert_eq!(value["constraints"], json!(["fast"]));
        assert_eq!(value["acceptanceCriteria"], json!(["tests pass"]));
        assert_eq!(value["assumptions"], json!(["x is y"]));
        assert_eq!(value["workspace"], json!("/repo"));
        assert_eq!(value["createdAt"], json!("2026-06-08T00:00:00Z"));
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

    #[test]
    fn tools_list_includes_every_mcp_tool_name() {
        // Driven by MCP_TOOL_NAMES (includes observe/rewrite/skill_eval/dispatch/
        // design_intelligence). Replaces the stale hand-maintained subset list.
        let response = handle_tools_list();
        let tools = response["tools"].as_array().expect("tools array");
        let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for name in MCP_TOOL_NAMES {
            assert!(
                tool_names.contains(name),
                "{name} tool not in tools list: {tool_names:?}"
            );
        }
        assert!(
            tools.iter().all(|t| {
                t.get("inputSchema")
                    .and_then(|s| s.get("type"))
                    .and_then(Value::as_str)
                    == Some("object")
            }),
            "every tool inputSchema.type must be object: {tool_names:?}"
        );
    }

    #[test]
    fn review_requires_action() {
        let params = json!({ "name": "review", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
    }

    #[test]
    fn review_gates_inserts_check_subcommand() {
        // CLI surface is `keel review gates check [flags]`; the MCP `gates`
        // action must inject `check`, else the child exits 1 with
        // "Unknown review gates command" and hosts see isError on a valid call.
        let args = review_args("gates", &json!({ "format": "json", "repo_root": "/tmp/x" }));
        assert_eq!(
            args,
            vec!["gates", "check", "--format=json", "--repo-root=/tmp/x"]
        );
    }

    #[test]
    fn review_surface_actions_skip_check_subcommand() {
        for action in ["pre-commit", "pre-pr"] {
            let args = review_args(action, &json!({ "base_ref": "origin/main" }));
            assert_eq!(args, vec![action, "--base-ref=origin/main"]);
        }
    }

    #[test]
    fn git_workflow_requires_action() {
        let params = json!({ "name": "git_workflow", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
    }

    #[test]
    fn memory_requires_action() {
        let params = json!({ "name": "memory", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
    }

    #[test]
    fn checkpoint_requires_action() {
        let params = json!({ "name": "checkpoint", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
    }

    #[test]
    fn checkpoint_restore_requires_confirm() {
        let params =
            json!({ "name": "checkpoint", "arguments": { "action": "restore", "id": "cp-1" } });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("confirm:true"), "text: {text}");
    }

    #[test]
    fn code_search_requires_query() {
        let params = json!({ "name": "code_search", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing query"), "text: {text}");
    }

    #[test]
    fn all_new_tools_have_schemas() {
        let listed = handle_tools_list();
        let tools = listed["tools"].as_array().expect("tools array");
        // Every advertised tool must declare an inputSchema. The count itself
        // is pinned by doc_parity_test.rs, so this asserts structure, not a number.
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<missing name>");
            assert!(
                tool.get("inputSchema").is_some(),
                "tool {name:?} must have an inputSchema"
            );
            assert!(
                tool.get("description").and_then(Value::as_str).is_some(),
                "tool {name:?} must have a description"
            );
        }
        assert!(!tools.is_empty(), "tools list must not be empty");
    }

    // Regression guards for the double-prefix bug: every `run_keel_subcommand`
    // caller once passed the subcommand name as `all_args[0]` AND as the
    // `subcommand` param, so the helper prepended it again — producing
    // `keel memory memory status` etc., which the CLI rejected with
    // "Unknown <x> command: <x>" and exit 1. These tests call the pure
    // arg-builders (no subprocess) and assert the subcommand name does NOT
    // appear in the extra-args vector. Shelling out via `current_exe()` hung
    // in CI's clean environment, so the guard is structural, not behavioral.

    #[test]
    fn memory_args_does_not_repeat_subcommand() {
        let args = memory_args("status", &json!({}));
        assert_eq!(args[0], "status");
        assert!(
            !args.iter().any(|a| a == "memory"),
            "subcommand leaked into extra args: {args:?}"
        );
    }

    #[test]
    fn flow_args_does_not_repeat_subcommand() {
        let args = flow_args("finish", &json!({ "file": "src/lib.rs" }));
        assert_eq!(args[0], "finish");
        assert!(args.iter().any(|a| a == "--target-file=src/lib.rs"));
        assert!(
            !args.iter().any(|a| a == "flow"),
            "subcommand leaked into extra args: {args:?}"
        );
    }

    #[test]
    fn run_command_input_validation_rejects_ambiguous_forms() {
        // No input at all.
        let error = tool_run_command(&json!({})).expect_err("no input must fail");
        assert!(error.contains("missing input"), "got: {error}");

        // Two forms at once are never silently interpreted.
        let error = tool_run_command(&json!({
            "argv": ["echo", "hi"],
            "script": "echo hi",
        }))
        .expect_err("argv+script must fail");
        assert!(error.contains("exactly one of"), "got: {error}");

        // script without a shell is an error: the shell is explicit, never guessed.
        let error =
            tool_run_command(&json!({ "script": "echo hi" })).expect_err("script alone must fail");
        assert!(error.contains("requires `shell`"), "got: {error}");

        // Unknown shell names fail fast with the allowed set.
        let error = tool_run_command(&json!({ "script": "echo hi", "shell": "zsh9000" }))
            .expect_err("unknown shell must fail");
        assert!(error.contains("unknown shell"), "got: {error}");

        // cmd exists only on Windows.
        if cfg!(not(windows)) {
            let error = tool_run_command(&json!({ "script": "echo hi", "shell": "cmd" }))
                .expect_err("cmd on non-Windows must fail");
            assert!(error.contains("only exists on Windows"), "got: {error}");
        }

        // Empty argv.
        let error = tool_run_command(&json!({ "argv": [] })).expect_err("empty argv must fail");
        assert!(error.contains("argv must contain"), "got: {error}");

        // Host aliases: cmd / args[] / args string must resolve to a form.
        let error = tool_run_command(&json!({ "cmd": "" })).expect_err("empty cmd still missing");
        assert!(error.contains("missing input"), "got: {error}");

        // `shell` only applies to script — never to the legacy command string.
        let error = tool_run_command(&json!({ "command": "echo hi", "shell": "bash" }))
            .expect_err("command+shell must fail");
        assert!(error.contains("only applies to `script`"), "got: {error}");
    }

    #[test]
    fn run_command_refuses_nested_mcp_serve() {
        let error = tool_run_command(&json!({
            "argv": ["keel", "mcp", "serve"]
        }))
        .expect_err("nested mcp must fail");
        assert!(
            error.contains("refusing to start `keel mcp`"),
            "got: {error}"
        );

        let error = tool_run_command(&json!({
            "command": "keel mcp serve"
        }))
        .expect_err("command form nested mcp must fail");
        assert!(
            error.contains("refusing to start `keel mcp`"),
            "got: {error}"
        );
    }

    #[test]
    fn command_nests_mcp_serve_detects_keel_mcp() {
        assert!(command_nests_mcp_serve(
            "keel",
            &["mcp".into(), "serve".into()],
            "keel mcp serve"
        ));
        assert!(command_nests_mcp_serve(
            r"C:\Users\HP\.keel\keel.exe",
            &["mcp".into()],
            "C:\\Users\\HP\\.keel\\keel.exe mcp serve"
        ));
        assert!(!command_nests_mcp_serve(
            "cargo",
            &["test".into()],
            "cargo test"
        ));
    }

    #[test]
    fn background_command_lifecycle_runs_polls_and_finishes() {
        let mut child = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo keel-bg-test"]);
            command
        } else {
            let mut command = Command::new("bash");
            command.args(["-c", "echo keel-bg-test"]);
            command
        };
        child.stdin(Stdio::null());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        let id = spawn_background_command(child, "echo keel-bg-test").expect("spawn");

        // Poll until finished; the command is sub-second, budget generously.
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut finished: Option<Value> = None;
        while Instant::now() < deadline {
            let result = tool_command_output(&json!({ "command_id": id, "json": true }))
                .expect("poll output");
            let payload: Value = serde_json::from_str(&result).expect("json payload");
            if payload["running"] == json!(false) {
                finished = Some(payload);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let payload = finished.expect("background command must finish");
        assert_eq!(payload["exit_code"], json!(0), "payload: {payload}");
        assert!(
            payload["stdout"]
                .as_str()
                .unwrap_or("")
                .contains("keel-bg-test"),
            "payload: {payload}"
        );

        // Exactly-once delivery: the finished id is released immediately.
        let second =
            tool_command_output(&json!({ "command_id": id })).expect_err("id must be released");
        assert!(second.contains("unknown command_id"), "got: {second}");
    }

    #[test]
    fn background_command_kill_stops_a_long_command() {
        let mut child = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 60 127.0.0.1 >nul"]);
            command
        } else {
            let mut command = Command::new("bash");
            command.args(["-c", "sleep 60"]);
            command
        };
        child.stdin(Stdio::null());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        let id = spawn_background_command(child, "long-runner").expect("spawn");

        // Still running right after spawn.
        let running = tool_command_output(&json!({ "command_id": id })).expect("poll");
        assert!(running.contains("\"running\":true"), "got: {running}");

        // Kill reports killed:true.
        let killed = tool_command_kill(&json!({ "command_id": id })).expect("kill");
        assert!(killed.contains("\"killed\":true"), "got: {killed}");

        // Polling after kill returns the final result and releases the id.
        let final_result = tool_command_output(&json!({ "command_id": id })).expect("final");
        assert!(final_result.contains("exit code"), "got: {final_result}");
        let gone =
            tool_command_output(&json!({ "command_id": id })).expect_err("id must be released");
        assert!(gone.contains("unknown command_id"), "got: {gone}");

        // Unknown ids are rejected by both tools.
        let error = tool_command_kill(&json!({ "command_id": "nope" })).expect_err("unknown");
        assert!(error.contains("unknown command_id"), "got: {error}");
        let error = tool_command_output(&json!({ "command_id": "nope" })).expect_err("unknown");
        assert!(error.contains("unknown command_id"), "got: {error}");
    }
}
