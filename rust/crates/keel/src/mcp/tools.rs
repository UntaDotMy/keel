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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::runtime::{display_path, resolve_claude_home, safe_path_segment};
use crate::utility::memory::refresh_system_map;
use crate::utility::memory_families::family_counts;
use crate::utility::recall::{search_recall_index, RecallSearchResult};
use crate::utility::skill_match::{
    match_skill_for_prompt, skill_catalog, skill_full_body, skill_inline_brief,
};
use crate::utility::workflow_ledger::{current_timestamp_millis, format_timestamp_iso8601};
use crate::utility::working_brief::{create_brief, list_briefs, read_brief, write_brief, Brief};

use super::{recall_status_payload, system_map_text, MethodError, JSON_RPC_INVALID_PARAMS};

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
                "description": "Call this BEFORE claiming what you remember or previously learned — search your durable memory instead of relying on conversation alone. Full-text search over Markdown under <claude-home>/{memories,memoriesv2,working-briefs}. Auto-syncs the index before querying.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms; punctuation is stripped and tokens are AND-ed with prefix match." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RECALL_LIMIT, "description": "Maximum hits (default 20)." }
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
                "description": "Prefer this over a raw shell call for noisy commands (test, build, lint, logs, search): it runs the command through the compaction proxy so compacted high-signal output enters context instead of the raw stream.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command line to execute (joined with the platform shell when shell metacharacters are present)." }
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
                "description": "List every installed keel skill with its name, description, and when_to_use. Use to discover what skills exist before routing or loading one.",
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
                "description": "Call this FIRST when starting a session or task — one call that makes you aware of what this toolkit offers, even when no skill loaded automatically. Returns the iron law, the full installed skill catalog (name + when_to_use), durable-memory health, and the newest working brief. After reading it, use skill_route to pick a skill, skill_get to load one, recall for memory, and cli for any other keel surface. Read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "cli",
                "description": "Run any keel CLI subcommand and get its compacted output — the full toolkit surface (review, git-workflow, workflow, memory, memoriesv2, orchestration, flow, code-search, config-audit, skill-lint, checkpoint, gain, session, telemetry, status, doctor, ...). Pass the subcommand and flags as `args`. Read/inspection subcommands run directly; destructive or management subcommands (install, update, repair, uninstall, validate, all, self-replace, `checkpoint restore`, and `hook install`/`hook uninstall`) require `confirm: true`. The `mcp` subcommand is refused. Prefer the dedicated tools (recall, skill_route, brief_create, sprint, user_story_lint, ...) when one fits; use cli for everything else.",
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

    let outcome = match tool_name {
        "recall" => tool_recall(&arguments),
        "system_map" => tool_system_map(&arguments),
        "run_command" => tool_run_command(&arguments),
        "recall_status" => tool_recall_status(&arguments),
        "skill_route" => tool_skill_route(&arguments),
        "skill_get" => tool_skill_get(&arguments),
        "skill_list" => tool_skill_list(&arguments),
        "memory_status" => tool_memory_status(&arguments),
        "brief_list" => tool_brief_list(&arguments),
        "brief_get" => tool_brief_get(&arguments),
        "brief_create" => tool_brief_create(&arguments),
        "system_map_refresh" => tool_system_map_refresh(&arguments),
        "context_brief" => tool_context_brief(&arguments),
        "cli" => tool_cli(&arguments),
        "sprint" => tool_sprint(&arguments),
        "user_story_lint" => tool_user_story_lint(&arguments),
        other => {
            return Err(MethodError {
                code: JSON_RPC_INVALID_PARAMS,
                message: format!("Unknown tool: {other}"),
            });
        }
    };

    match outcome {
        Ok(text) => Ok(json!({
            "content": [
                { "type": "text", "text": text }
            ],
            "isError": false,
        })),
        Err(message) => Ok(json!({
            "content": [
                { "type": "text", "text": message }
            ],
            "isError": true,
        })),
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
    let result = search_recall_index(&claude_home, &query, limit)
        .map_err(|error| format!("recall: {error}"))?;
    let payload = render_recall_payload(&claude_home, &query, limit, result);
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
    system_map_text(workspace_override.as_deref()).map_err(|error| format!("system_map: {error}"))
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
    let output = child
        .output()
        .map_err(|error| format!("run_command: spawn: {error}"))?;
    let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    // Return a plain-text report rather than a JSON object. Embedding multi-line
    // stdout/stderr as JSON string values escapes every newline as a literal
    // `\n`, which turns a build/test log into an unreadable single line in the
    // MCP tool-result view. A text report keeps real newlines so the output is
    // legible to both the human reading the transcript and the model consuming
    // the result; the exit code stays on its own labeled line for easy parsing.
    Ok(render_run_command_report(
        &command,
        output.status.code().unwrap_or(-1),
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
            json!({
                "matched": true,
                "name": found.name,
                "score": format!("{:.4}", found.score),
                "brief": brief,
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
    let output = child
        .output()
        .map_err(|error| format!("cli: spawn: {error}"))?;
    let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(render_run_command_report(
        &format!("keel {}", args.join(" ")),
        output.status.code().unwrap_or(-1),
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
    let output = child
        .output()
        .map_err(|error| format!("sprint: spawn: {error}"))?;
    let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(render_run_command_report(
        &format!("keel sprint {} {}", action, args.join(" ")),
        output.status.code().unwrap_or(-1),
        &stdout_text,
        &stderr_text,
    ))
}

/// User story lint tool: validate user stories against strict Agile/Jira format.
/// Thin wrapper over the CLI user-story lint command.
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

    if let Some(file_path) = file {
        child.arg("--file");
        child.arg(file_path);
    } else if let Some(stdin_text) = stdin {
        child.arg("--stdin");
        child.stdin(Stdio::piped());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        let mut child_proc = child
            .spawn()
            .map_err(|error| format!("user_story_lint: spawn: {error}"))?;
        if let Some(stdin_pipe) = child_proc.stdin.as_mut() {
            use std::io::Write;
            stdin_pipe
                .write_all(stdin_text.as_bytes())
                .map_err(|error| format!("user_story_lint: write stdin: {error}"))?;
        }
        let output = child_proc
            .wait_with_output()
            .map_err(|error| format!("user_story_lint: wait: {error}"))?;
        let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
        return Ok(render_run_command_report(
            "keel user-story lint --stdin",
            output.status.code().unwrap_or(-1),
            &stdout_text,
            &stderr_text,
        ));
    }

    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let output = child
        .output()
        .map_err(|error| format!("user_story_lint: spawn: {error}"))?;
    let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(render_run_command_report(
        &format!("keel user-story lint --file {}", file.unwrap_or("")),
        output.status.code().unwrap_or(-1),
        &stdout_text,
        &stderr_text,
    ))
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
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        assert_eq!(names.len(), 16, "names: {names:?}");
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
    fn tools_list_includes_sprint_and_user_story_lint() {
        let response = handle_tools_list();
        let tools = response["tools"].as_array().expect("tools array");
        let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

        assert!(
            tool_names.contains(&"sprint"),
            "sprint tool not in tools list"
        );
        assert!(
            tool_names.contains(&"user_story_lint"),
            "user_story_lint tool not in tools list"
        );
        assert_eq!(
            tools.len(),
            16,
            "expected 16 tools (added sprint and user_story_lint)"
        );
    }
}
