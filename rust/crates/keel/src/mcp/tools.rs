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
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home, safe_path_segment};
use crate::utility::memory::refresh_system_map;
use crate::utility::memory_families::family_counts;
use crate::utility::recall::{collapse_dashes, search_recall_index, RecallSearchResult};
use crate::utility::skill_match::{
    match_skill_for_prompt, skill_catalog, skill_full_body, skill_inline_brief,
};
use crate::utility::workflow_ledger::{current_timestamp_millis, format_timestamp_iso8601};
use crate::utility::working_brief::{create_brief, list_briefs, read_brief, write_brief, Brief};

use super::{recall_status_payload, system_map_text, MethodError, JSON_RPC_INVALID_PARAMS};

/// Default wall-clock budget for MCP tools that spawn a child (`cli`,
/// `run_command`, `sprint`, …) **and** for in-process tools that can block
/// (SQLite recall, skill catalog scan, system map render). The serve loop runs
/// concurrent workers, but a single hung tool still burns an in-flight slot and
/// can exhaust host patience — deadline so hosts get `isError` instead of a
/// permanent stall. Override with `KEEL_MCP_TOOL_TIMEOUT_SECS` (seconds, min 5,
/// max 3600). Default stays under typical host MCP timeouts (~50–60s) while
/// allowing real builds.
const DEFAULT_MCP_CHILD_TIMEOUT_SECS: u64 = 90;

/// Soft cap for large text tool results (system_map, skill bodies, context_brief).
/// Hosts like Grok also cap MCP output (~20KB); returning a bounded body keeps the
/// stdio pipe from filling and the agent from waiting on megabyte maps.
const MAX_MCP_TEXT_CHARS: usize = 32_000;

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
/// double as the model-facing usage hints the harness renders.
pub(super) fn handle_tools_list() -> Value {
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
                "description": "Prefer this over a raw shell call for noisy commands (test, build, lint, logs, search): it runs the command through the compaction proxy so compacted high-signal output enters context instead of the raw stream. Safe to use for any shell command; output is always neutralized for prompt-injection before reaching the model.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command line to execute (joined with the platform shell when shell metacharacters are present)." },
                        "json": { "type": "boolean", "description": "Return the compacted output as a JSON object (command, exit_code, stdout, stderr) instead of the text report. Default false." }
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
                "description": "Load the full SKILL.md body for an installed keel skill by name. Use after skill_route (or skill_list) when you need the complete skill, not just the brief. Returns the entire file including frontmatter.",
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
                "description": "List every installed keel skill with its name, description, and when_to_use. Prefer skill_route(prompt) when you already have a task — it is smaller and faster. Use skill_list only for discovery. Results are size-capped and time-budgeted so the MCP call cannot hang.",
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
                "description": "Run any keel CLI subcommand and get its compacted output — the full toolkit surface (review, git-workflow, workflow, memory, orchestration, flow, code-search, config-audit, skill-lint, checkpoint, gain, session, telemetry, status, doctor, ...). Pass the subcommand and flags as `args`. Read/inspection subcommands run directly; destructive or management subcommands (install, update, repair, uninstall, validate, all, self-replace, `checkpoint restore`, and `hook install`/`hook uninstall`) require `confirm: true`. The `mcp` subcommand is refused. Prefer the dedicated tools (recall, skill_route, brief_create, sprint, user_story_lint, ...) when one fits; use cli for everything else.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "args": { "type": "array", "items": { "type": "string" }, "description": "keel arguments, e.g. [\"review\",\"pre-pr\",\"--base-ref\",\"origin/feat\"] or [\"workflow\",\"status\"]." },
                        "confirm": { "type": "boolean", "description": "Required true to run a destructive/management subcommand. Default false." }
                    },
                    "required": ["args"]
                }
            },
            {
                "name": "sprint",
                "description": "Drive the Scrum-style sprint loop (plan → implement → verify → review → LOOP until every story is Done). Subcommands: `plan` (create a sprint from confirmed stories), `status` (show current sprint state), `advance` (move a story to the next state), `review` (fail-closed gate that verifies every story meets Definition of Done), `list` (show all sprints). The sprint **must not** close until every story is Done — this is the anti-partial-completion backstop.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["plan", "status", "advance", "review", "list"], "description": "Sprint operation to perform." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional arguments for the action (e.g. story-id for advance, workspace-root for plan)." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "user_story_lint",
                "description": "Validate user stories against strict Agile/Jira format (Connextra \"As a/I want/so that\" + Gherkin Given/When/Then, validated against INVEST). Use before building to confirm the requirement spec is well-formed. Stories that fail INVEST or lack Gherkin criteria **must not** proceed to implementation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path to markdown file containing the stories." },
                        "stdin": { "type": "string", "description": "Story text to validate (alternative to file)." }
                    }
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
                "name": "workflow",
                "description": "Drive workflow state (route, start, cockpit, finish, status). Use to manage a proof-first workstream with tracked proof and closeout discipline.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["route", "start", "cockpit", "finish", "status"], "description": "Workflow operation to perform." },
                        "request": { "type": "string", "description": "The work request or description." },
                        "id": { "type": "string", "description": "Workflow entry id." },
                        "proof": { "type": "string", "description": "Proof evidence for finish." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "git_workflow",
                "description": "Git workflow operations (preflight, commit-message, pr-body, lint-message). Use for professional commit/PR text generation and linting.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["preflight", "commit-message", "pr-body", "lint-message"], "description": "Git workflow operation." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional CLI arguments (e.g. [\"--base-ref\",\"origin/feat\"])." }
                    },
                    "required": ["action"]
                }
            },
            {
                "name": "memory",
                "description": "Durable memory operations under ~/.claude/memories/. `scope` resolves the workspace memory lane; `system-map` shows/refreshes the workspace structural map; `recall` FTS5-searches memory; `instincts` lists distilled learning instincts; `consolidate` scans memory family directories and reports record counts/previews (status summary, not a merge); `report` summarizes memory state; `research-cache` saves/retrieves research answers; `retrieve` cross-family search; `maintenance` prunes stale records; `status` reports family counts. Use the dedicated brief_* tools for working briefs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["scope", "system-map", "recall", "instincts", "consolidate", "report", "research-cache", "retrieve", "maintenance", "status"], "description": "Memory operation to perform." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional CLI arguments (e.g. [\"--query\",\"terms\"] for recall, [\"--create-missing\",\"--refresh-system-map\"] for scope)." }
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
                "name": "orchestration",
                "description": "Orchestration operations (runtime-preflight, resume-status, task, checkpoint). Use for multi-agent or multi-step workflow coordination.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["runtime-preflight", "resume-status", "task", "checkpoint"], "description": "Orchestration operation." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Additional CLI arguments." }
                    },
                    "required": ["action"]
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
                "name": "user_story",
                "description": "Lint user stories against strict Agile/Jira format (Connextra \"As a/I want/so that\" + Gherkin Given/When/Then, validated against INVEST). Use before building to confirm the requirement spec is well-formed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path to markdown file containing the stories." },
                        "stdin": { "type": "string", "description": "Story text to validate (alternative to file)." }
                    }
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
                "name": "work",
                "description": "Dependency-aware work graph. Track items with depends_on/discovered-from edges, query `ready` (unblocked) or `blocked` items, capture work discovered mid-task so it is never dropped. Open + ready/blocked items survive compaction via the SessionStart digest.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["add", "list", "ready", "blocked", "dep", "discovered", "close", "show"], "description": "Work-graph operation to perform." },
                        "title": { "type": "string", "description": "Work item title (required for add)." },
                        "id": { "type": "string", "description": "Work item id (for dep/discovered/close/show)." },
                        "depends_on": { "type": "string", "description": "Id of the dependency B (for dep: A depends on B). Translated to --on." },
                        "from": { "type": "string", "description": "Id of the item this was discovered from (for discovered)." },
                        "status": { "type": "string", "description": "Initial status for add: open|in-progress|blocked|done." },
                        "priority": { "type": "string", "description": "Priority for add (default 2)." },
                        "workspace_root": { "type": "string", "description": "Workspace root path. Defaults to cwd." },
                        "json": { "type": "boolean", "description": "Output as JSON." }
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
                        "output": { "type": "string", "description": "Output artifact path (for build). Defaults to .understand/code-graph.json." },
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
            }
        ]
    })
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

fn is_known_mcp_tool(name: &str) -> bool {
    matches!(
        name,
        "recall"
            | "system_map"
            | "run_command"
            | "recall_status"
            | "skill_route"
            | "skill_get"
            | "skill_list"
            | "memory_status"
            | "brief_list"
            | "brief_get"
            | "brief_create"
            | "system_map_refresh"
            | "context_brief"
            | "cli"
            | "sprint"
            | "user_story_lint"
            | "review"
            | "workflow"
            | "git_workflow"
            | "memory"
            | "gain"
            | "raw"
            | "config_audit"
            | "skill_lint"
            | "telemetry"
            | "orchestration"
            | "checkpoint"
            | "session"
            | "doctor"
            | "code_search"
            | "user_story"
            | "flow"
            | "work"
            | "code_graph"
            | "learn"
    )
}

fn dispatch_mcp_tool(tool_name: &str, arguments: &Value) -> Result<String, String> {
    match tool_name {
        "recall" => tool_recall(arguments),
        "system_map" => tool_system_map(arguments),
        "run_command" => tool_run_command(arguments),
        "recall_status" => tool_recall_status(arguments),
        "skill_route" => tool_skill_route(arguments),
        "skill_get" => tool_skill_get(arguments),
        "skill_list" => tool_skill_list(arguments),
        "memory_status" => tool_memory_status(arguments),
        "brief_list" => tool_brief_list(arguments),
        "brief_get" => tool_brief_get(arguments),
        "brief_create" => tool_brief_create(arguments),
        "system_map_refresh" => tool_system_map_refresh(arguments),
        "context_brief" => tool_context_brief(arguments),
        "cli" => tool_cli(arguments),
        "sprint" => tool_sprint(arguments),
        "user_story_lint" => tool_user_story_lint(arguments),
        "review" => tool_review(arguments),
        "workflow" => tool_workflow(arguments),
        "git_workflow" => tool_git_workflow(arguments),
        "memory" => tool_memory(arguments),
        "gain" => tool_gain(arguments),
        "raw" => tool_raw(arguments),
        "config_audit" => tool_config_audit(arguments),
        "skill_lint" => tool_skill_lint(arguments),
        "telemetry" => tool_telemetry(arguments),
        "orchestration" => tool_orchestration(arguments),
        "checkpoint" => tool_checkpoint(arguments),
        "session" => tool_session(arguments),
        "doctor" => tool_doctor(arguments),
        "code_search" => tool_code_search(arguments),
        "user_story" => tool_user_story(arguments),
        "flow" => tool_flow(arguments),
        "work" => tool_work(arguments),
        "code_graph" => tool_code_graph(arguments),
        "learn" => tool_learn(arguments),
        other => Err(format!("Unknown tool: {other}")),
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
    serde_json::to_string_pretty(&payload).map_err(|error| format!("recall: serialize: {error}"))
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
    // Shell out to a fresh `keel run -- <command>` so the proxy
    // pipeline (capture, compaction, raw-store, gain analytics) runs in its
    // intended configuration. Setting `CLAUDE_SKILLS_HOOK` flips the proxy's
    // capture gate on for this child even when the parent MCP server was
    // launched from a plain shell.
    let executable =
        env::current_exe().map_err(|error| format!("run_command: locate self: {error}"))?;
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
    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout(child, mcp_child_timeout(), "run_command")?;
    // `json` mode returns a structured object; default text report keeps real
    // newlines so multi-line build/test logs stay legible in the tool-result view.
    if Some(true) == optional_bool_arg(arguments, "json") {
        let payload = json!({
            "command": command,
            "exit_code": exit_code,
            "stdout": stdout_text,
            "stderr": stderr_text,
        });
        return serde_json::to_string_pretty(&payload)
            .map_err(|error| format!("run_command: serialize: {error}"));
    }
    Ok(render_run_command_report(
        &command,
        exit_code,
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
            let brief = skill_inline_brief(&claude_home, &found.name);
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
                        .filter(|name| installed.contains(name) && name != &found.name)
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "matched": true,
                "name": found.name,
                "score": format!("{:.4}", found.score),
                "brief": brief,
                "relatedSkills": related_installed,
            })
        }
        None => json!({
            "matched": false,
            "name": Value::Null,
            "score": Value::Null,
            "brief": Value::Null,
        }),
    };
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("skill_route: serialize: {error}"))
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
            let payload = json!({
                "name": name,
                "path": display_path(&path),
                "body": body,
            });
            serde_json::to_string_pretty(&payload)
                .map_err(|error| format!("skill_get: serialize: {error}"))
        }
        None => Err(format!(
            "skill_get: no installed skill named {name:?} (or name is unsafe)"
        )),
    }
}

fn tool_skill_list(_arguments: &Value) -> Result<String, String> {
    let claude_home = tool_claude_home("skill_list")?;
    let catalog = skill_catalog(&claude_home);
    let skills: Vec<Value> = catalog
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "description": entry.description,
                "whenToUse": entry.when_to_use,
                "useCount": entry.use_count,
                "relatedSkills": entry.related_skills,
            })
        })
        .collect();
    let payload = json!({
        "count": skills.len(),
        "skills": skills,
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("skill_list: serialize: {error}"))
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
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("memory_status: serialize: {error}"))
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
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("brief_list: serialize: {error}"))
}

fn tool_brief_get(arguments: &Value) -> Result<String, String> {
    let id = brief_id_arg(arguments, "brief_get")?;
    let claude_home = tool_claude_home("brief_get")?;
    let payload =
        match read_brief(&claude_home, &id).map_err(|error| format!("brief_get: {error}"))? {
            Some(brief) => json!({ "found": true, "brief": brief_to_json(&brief) }),
            None => json!({ "found": false, "id": id }),
        };
    serde_json::to_string_pretty(&payload).map_err(|error| format!("brief_get: serialize: {error}"))
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
    let payload = json!({
        "written": true,
        "path": display_path(&path),
        "brief": brief_to_json(&brief),
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("brief_create: serialize: {error}"))
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
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("system_map_refresh: serialize: {error}"))
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
            json!({
                "name": entry.name,
                "whenToUse": entry.when_to_use,
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
    serde_json::to_string_pretty(&payload)
        .map_err(|error| format!("context_brief: serialize: {error}"))
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

    if CLI_REFUSED_SUBCOMMANDS.contains(&subcommand.as_str()) {
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

    let executable = env::current_exe().map_err(|error| format!("cli: locate self: {error}"))?;
    let mut child = Command::new(&executable);
    for argument in &args {
        child.arg(argument);
    }
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout(child, mcp_child_timeout(), "cli")?;
    Ok(render_run_command_report(
        &format!("keel {}", args.join(" ")),
        exit_code,
        &stdout_text,
        &stderr_text,
    ))
}

/// Sprint tool: drive the Scrum-style sprint loop (plan, status, advance, review, list).
/// Thin wrapper over the CLI sprint commands, preserving the same interface.
fn tool_sprint(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "sprint: missing action (plan|status|advance|review|list)".to_string())?;

    let args: Vec<String> = match arguments.get("args") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::Null) | None => Vec::new(),
        _ => return Err("sprint: args must be a JSON array of strings".to_string()),
    };

    let executable = env::current_exe().map_err(|error| format!("sprint: locate self: {error}"))?;
    let mut child = Command::new(&executable);
    child.arg("sprint");
    child.arg(action);
    for argument in &args {
        child.arg(argument);
    }
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout(child, mcp_child_timeout(), "sprint")?;
    Ok(render_run_command_report(
        &format!("keel sprint {} {}", action, args.join(" ")),
        exit_code,
        &stdout_text,
        &stderr_text,
    ))
}

/// User story lint tool: validate user stories against strict Agile/Jira format.
/// Thin wrapper over the CLI user-story lint command. Always uses a kill timeout
/// so a hung lint child cannot freeze MCP (hosts report that as "stuck").
fn tool_user_story_lint(arguments: &Value) -> Result<String, String> {
    let file = arguments
        .get("file")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let stdin = arguments
        .get("stdin")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if file.is_none() && stdin.is_none() {
        return Err("user_story_lint: must provide either 'file' or 'stdin'".to_string());
    }

    let executable =
        env::current_exe().map_err(|error| format!("user_story_lint: locate self: {error}"))?;
    let mut child = Command::new(&executable);
    child.arg("user-story");
    child.arg("lint");
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());

    let label;
    let stdin_bytes = if let Some(file_path) = file {
        child.arg("--file");
        child.arg(file_path);
        child.stdin(Stdio::null());
        label = format!("keel user-story lint --file {file_path}");
        None
    } else {
        let text = stdin.unwrap_or_default();
        child.arg("--stdin");
        child.stdin(Stdio::piped());
        label = "keel user-story lint --stdin".to_string();
        Some(text.as_bytes().to_vec())
    };

    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout_stdin(child, stdin_bytes, mcp_child_timeout(), "user_story_lint")?;
    Ok(render_run_command_report(
        &label,
        exit_code,
        &stdout_text,
        &stderr_text,
    ))
}

/// Generic passthrough helper: shell out to the keel binary with the given
/// subcommand and args. Returns the compacted report text.
fn run_keel_subcommand<S: AsRef<str>>(
    subcommand: &str,
    extra_args: &[S],
) -> Result<String, String> {
    let executable =
        env::current_exe().map_err(|error| format!("{subcommand}: locate self: {error}"))?;
    let mut child = Command::new(&executable);
    child.arg(subcommand);
    for arg in extra_args {
        child.arg(arg.as_ref());
    }
    child.env("CLAUDE_SKILLS_HOOK", "mcp");
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let (exit_code, stdout_text, stderr_text) =
        run_command_with_timeout(child, mcp_child_timeout(), subcommand)?;
    let label = format!(
        "keel {subcommand} {}",
        extra_args
            .iter()
            .map(|a| a.as_ref())
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(render_run_command_report(
        &label,
        exit_code,
        &stdout_text,
        &stderr_text,
    ))
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
                    return Err(format!(
                        "{label}: timed out after {}s (set KEEL_MCP_TOOL_TIMEOUT_SECS to raise; kill orphan `keel mcp serve` processes if tools keep hanging)",
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

fn truncate_mcp_text(text: &str) -> String {
    if text.chars().count() <= MAX_MCP_TEXT_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_MCP_TEXT_CHARS).collect();
    format!(
        "{kept}\n\n… truncated for MCP (>{MAX_MCP_TEXT_CHARS} chars). Prefer skill_route over skill_list, or CLI for full output."
    )
}

#[cfg(test)]
mod mcp_timeout_tests {
    use super::*;

    #[test]
    fn truncate_mcp_text_leaves_small_input() {
        assert_eq!(truncate_mcp_text("hello"), "hello");
    }

    #[test]
    fn truncate_mcp_text_caps_large_input() {
        let big = "x".repeat(MAX_MCP_TEXT_CHARS + 5_000);
        let out = truncate_mcp_text(&big);
        assert!(out.contains("truncated for MCP"));
        assert!(out.chars().count() < big.chars().count());
        assert!(out.chars().count() <= MAX_MCP_TEXT_CHARS + 200);
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
    let mut owned: Vec<String> = Vec::new();
    if let Some(r) = optional_string_arg(arguments, "base_ref") {
        owned.push(format!("--base-ref={r}"));
    }
    if let Some(f) = optional_string_arg(arguments, "format") {
        owned.push(format!("--format={f}"));
    }
    if let Some(root) = optional_string_arg(arguments, "repo_root") {
        owned.push(format!("--repo-root={root}"));
    }
    let mut all_args: Vec<&str> = vec![action];
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("review", &all_args)
}

fn tool_workflow(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "workflow: missing action (route|start|cockpit|finish|status)".to_string()
        })?;
    let mut owned: Vec<String> = Vec::new();
    if let Some(r) = optional_string_arg(arguments, "request") {
        owned.push(format!("--request={r}"));
    }
    if let Some(i) = optional_string_arg(arguments, "id") {
        owned.push(format!("--id={i}"));
    }
    if let Some(p) = optional_string_arg(arguments, "proof") {
        owned.push(format!("--proof={p}"));
    }
    let mut all_args: Vec<&str> = vec![action];
    for s in &owned {
        all_args.push(s);
    }
    run_keel_subcommand("workflow", &all_args)
}

fn tool_git_workflow(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "git_workflow: missing action (preflight|commit-message|pr-body|lint-message)"
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

fn tool_orchestration(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "orchestration: missing action (runtime-preflight|resume-status|task|checkpoint)"
                .to_string()
        })?;
    let all_args = orchestration_args(action, arguments);
    run_keel_subcommand("orchestration", &all_args)
}

/// Pure arg-builder for `tool_orchestration` (see `memory_args` for the testability rationale).
fn orchestration_args(action: &str, arguments: &Value) -> Vec<String> {
    let mut all_args: Vec<String> = vec![action.to_string()];
    for extra in collect_extra_args(arguments) {
        all_args.push(extra);
    }
    all_args
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

fn tool_user_story(arguments: &Value) -> Result<String, String> {
    tool_user_story_lint(arguments)
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

/// Dependency-aware work graph. `add` needs --title; `dep` needs --id + --on
/// (A depends on B); `close`/`show` need --id; `discovered` needs --id + --from.
/// Reads named fields and translates to the real CLI flags so nothing is dropped.
fn tool_work(arguments: &Value) -> Result<String, String> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "work: missing action (add|list|ready|blocked|dep|discovered|close|show)".to_string()
        })?;
    let all_args = work_args(action, arguments);
    run_keel_subcommand("work", &all_args)
}

/// Pure arg-builder for `tool_work` (see `memory_args` for the testability rationale).
fn work_args(action: &str, arguments: &Value) -> Vec<String> {
    let mut all_args: Vec<String> = vec![action.to_string()];
    for flag in work_cli_args(arguments) {
        all_args.push(flag);
    }
    all_args
}

/// Pure translation of the `work` tool's named MCP fields to real CLI flags.
/// Extracted so the mapping is unit-testable without shelling out. Guards
/// against silent field drops (the bug where `collect_extra_args` ignored
/// named fields and `--title`/`--on` never reached the CLI).
fn work_cli_args(arguments: &Value) -> Vec<String> {
    let mut owned: Vec<String> = Vec::new();
    if let Some(title) = optional_string_arg(arguments, "title") {
        owned.push(format!("--title={title}"));
    }
    if let Some(id) = optional_string_arg(arguments, "id") {
        owned.push(format!("--id={id}"));
    }
    if let Some(depends_on) = optional_string_arg(arguments, "depends_on") {
        owned.push(format!("--on={depends_on}"));
    }
    if let Some(from) = optional_string_arg(arguments, "from") {
        owned.push(format!("--from={from}"));
    }
    if let Some(status) = optional_string_arg(arguments, "status") {
        owned.push(format!("--status={status}"));
    }
    if let Some(priority) = optional_string_arg(arguments, "priority") {
        owned.push(format!("--priority={priority}"));
    }
    if let Some(root) = optional_string_arg(arguments, "workspace_root") {
        owned.push(format!("--workspace-root={root}"));
    }
    if Some(true) == optional_bool_arg(arguments, "json") {
        owned.push("--json".to_string());
    }
    owned
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
    fn tools_list_includes_new_cli_passthrough_tools() {
        let response = handle_tools_list();
        let tools = response["tools"].as_array().expect("tools array");
        let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

        for name in [
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
            assert!(
                tool_names.contains(&name),
                "{name} tool not in tools list: {tool_names:?}"
            );
        }
        assert!(
            tools.iter().all(|t| t.get("inputSchema").is_some()),
            "every advertised tool must have an inputSchema: {tool_names:?}"
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
    fn workflow_requires_action() {
        let params = json!({ "name": "workflow", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
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
    fn orchestration_requires_action() {
        let params = json!({ "name": "orchestration", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("missing action"), "text: {text}");
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
    fn user_story_delegates_to_user_story_lint() {
        let params = json!({ "name": "user_story", "arguments": {} });
        let result = handle_tools_call(&params).expect("envelope present");
        assert_eq!(result["isError"], json!(true));
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("must provide"),
            "user_story should delegate to user_story_lint: {text}"
        );
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

    /// Regression: the `work` tool's named fields must translate to real CLI
    /// flags. The old `collect_extra_args` handler dropped every named field,
    /// so `work add {title:"x"}` created an item with no title. This pins the
    /// translation: each named field produces its real CLI flag.
    #[test]
    fn work_tool_translates_named_fields_to_cli_flags() {
        let args = work_cli_args(&json!({
            "title": "fix the bug",
            "id": "w1",
            "depends_on": "w2",
            "from": "w0",
            "status": "in-progress",
            "priority": "1",
            "workspace_root": "/tmp/repo",
            "json": true
        }));
        let joined = args.join(" ");
        assert!(
            joined.contains("--title=fix the bug"),
            "title dropped: {joined}"
        );
        assert!(joined.contains("--id=w1"), "id dropped: {joined}");
        // depends_on MUST become --on (the real CLI flag for `work dep A --on B`).
        assert!(
            joined.contains("--on=w2"),
            "depends_on not translated to --on: {joined}"
        );
        assert!(joined.contains("--from=w0"), "from dropped: {joined}");
        assert!(
            joined.contains("--status=in-progress"),
            "status dropped: {joined}"
        );
        assert!(
            joined.contains("--priority=1"),
            "priority dropped: {joined}"
        );
        assert!(
            joined.contains("--workspace-root=/tmp/repo"),
            "workspace_root dropped: {joined}"
        );
        assert!(joined.contains("--json"), "json dropped: {joined}");
    }

    /// Regression: an empty/absent field must NOT produce an empty flag (which
    /// the CLI would reject). `optional_string_arg` already trims and filters
    /// empty, but pin it so a future change can't silently reintroduce `--id=`.
    #[test]
    fn work_tool_omits_empty_fields() {
        let args = work_cli_args(&json!({ "title": "  ", "id": "" }));
        assert!(
            args.is_empty(),
            "empty fields should produce no flags: {args:?}"
        );
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
    fn work_args_does_not_repeat_subcommand() {
        let args = work_args("list", &json!({}));
        assert_eq!(args[0], "list");
        assert!(
            !args.iter().any(|a| a == "work"),
            "subcommand leaked into extra args: {args:?}"
        );
    }

    #[test]
    fn orchestration_args_does_not_repeat_subcommand() {
        let args = orchestration_args("resume-status", &json!({}));
        assert_eq!(args[0], "resume-status");
        assert!(
            !args.iter().any(|a| a == "orchestration"),
            "subcommand leaked into extra args: {args:?}"
        );
    }
}
