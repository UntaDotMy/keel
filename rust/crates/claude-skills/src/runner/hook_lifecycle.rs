//! Purpose: Claude Code hook lifecycle management, installation, and removal.
//! Caller: runner/mod.rs for hook command group.
//! Dependencies: std::collections::BTreeMap, std::fs, std::path, serde_json, crate::runtime.
//! Main Functions: run_hook_command, build_hooks_payload, remove_managed_hooks.
//! Side Effects: Reads and writes Claude Code hooks.json configuration.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonDocument};

use crate::args::FlagSet;
use crate::hooks::claude::{event_by_name, event_by_slug, HookEvent, HOOK_EVENTS};
use crate::json::{write_indented, Value};
use crate::proxy::raw_store::RawStore;
use crate::runner::shell_rewrite::{
    bash_command_for_executable_args, platform_default_command_for_executable_args,
    rewrite_command_text_for_shell, RewriteShell,
};
use crate::runtime::{display_path, installed_executable_path, resolve_claude_home, write_text};
use crate::utility;

const RAW_OUTPUT_DEFAULT_RETENTION_DAYS: u64 = 14;

const MANAGED_PRE_TOOL_USE_EVENT: &str = "PreToolUse";
const MANAGED_PRE_TOOL_USE_COMMAND_SUFFIX: &str = "hook pre-tool-use";

/// SYSTEM_MAP.md is rebuilt every N edit-class tool calls so the workspace
/// pointer stays in sync with the repo without paying refresh cost on every
/// tool call. Tunable via `CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL`; setting
/// it to `0` disables the periodic refresh.
const SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD: u64 = 10;

/// Iterate canonical hook event names. Single-line wrapper around the table so
/// existing for-loops keep their `for event in claude_hook_event_names()` shape
/// without caring that the source is a typed row table.
fn claude_hook_event_names() -> impl Iterator<Item = &'static str> {
    HOOK_EVENTS.iter().map(|event| event.name)
}
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn run_hook_command(
    arguments: &[String],

    standard_output: &mut dyn Write,

    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        render_hook_help(standard_output);

        return if arguments.is_empty() { 1 } else { 0 };
    }

    let slug = arguments[0].as_str();

    match slug {
        "install" => run_hook_install(standard_output, standard_error),

        "uninstall" => run_hook_uninstall(standard_output, standard_error),

        "list" | "show" => run_hook_list(standard_output, standard_error),

        "instructions" => run_hook_instructions(&arguments[1..], standard_output, standard_error),

        "diagnose" => run_hook_diagnose(&arguments[1..], standard_output, standard_error),

        // PreToolUse runs the transparent rewriter and emits hookSpecificOutput.
        "pre-tool-use" => run_hook_pre_tool_use(standard_output, standard_error),

        // PostToolUse counts edit-class tool calls and refreshes SYSTEM_MAP.md
        // every N edits. The lifecycle context for this event stays empty
        // (silent per the prompt-cache budget rule), so we own the dispatch
        // here instead of going through run_hook_lifecycle.
        "post-tool-use" => run_hook_post_tool_use(standard_error),

        // Stop and SubagentStop must never return a non-zero exit code. Claude Code
        // treats a failing Stop hook as a signal to re-run the turn, which cascades
        // into a stop loop. lifecycle_additional_context already returns empty
        // string for these events, but routing them through run_hook_lifecycle
        // leaves a regression surface — any future change that introduces context,
        // mishandles serde, or panics could re-introduce the cascade. Short-circuit
        // here so no downstream change can accidentally bring back the bug.
        "stop" | "subagent-stop" => 0,

        // Every other slug is dispatched if and only if it appears in the canonical
        // event table. Using the same table that drives `settings.json` installation
        // means a stale binary cannot reject an event the install path advertises:
        // the dispatch list IS the canonical list. This is the structural fix for
        // the `Unknown hook command: post-tool-use-failure` regression seen on
        // Windows when the dispatch arm and the EVENTS array drifted apart.
        other if event_by_slug(other).is_some() => {
            run_hook_lifecycle(other, standard_output, standard_error)
        }

        other => {
            let _ = writeln!(standard_error, "Unknown hook command: {other}");

            render_hook_help(standard_output);

            1
        }
    }
}

fn run_hook_install(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);

    let hook_command = match managed_hook_command() {
        Ok(command) => command,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_payload = match build_hooks_payload(&hook_path, &hook_command) {
        Ok(payload) => payload,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    match write_text(&hook_path, &hook_payload) {
        Ok(()) => {
            let _ = writeln!(
                standard_output,
                "Installed Rust claude-skills lifecycle hooks at {}",
                display_path(&hook_path)
            );

            0
        }

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            1
        }
    }
}

fn run_hook_uninstall(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);

    match remove_managed_hook_payload(&hook_path) {
        Ok((payload, removed)) => {
            if removed {
                match write_text(&hook_path, &payload) {
                    Ok(()) => {
                        let _ = writeln!(
                            standard_output,
                            "Removed Rust claude-skills hook from {}",
                            display_path(&hook_path)
                        );

                        0
                    }

                    Err(error) => {
                        let _ = writeln!(standard_error, "{error}");

                        1
                    }
                }
            } else {
                let _ = writeln!(
                    standard_output,
                    "No claude-skills hook installed at {}",
                    display_path(&hook_path)
                );

                0
            }
        }

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            1
        }
    }
}

fn run_hook_list(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);

    match fs::read_to_string(&hook_path) {
        Ok(text) => {
            let _ = writeln!(standard_output, "{text}");

            0
        }

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(
                standard_output,
                "No claude-skills hook installed at {}",
                display_path(&hook_path)
            );

            0
        }

        Err(error) => {
            let _ = writeln!(standard_error, "read {}: {error}", display_path(&hook_path));

            1
        }
    }
}

fn run_hook_instructions(
    arguments: &[String],

    standard_output: &mut dyn Write,

    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook instructions");

    flag_set.string_flag("format", "markdown");

    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);

        return 1;
    }

    if flag_set.string_value("format") == "json" {
        let payload = Value::Object(vec![
            ("runtime".into(), Value::String("rust".into())),
            (
                "rerunPrefix".into(),
                Value::String("claude-skills run --".into()),
            ),
            (
                "activeHookEvent".into(),
                Value::String(MANAGED_PRE_TOOL_USE_EVENT.into()),
            ),
            (
                "supportedHookEvents".into(),
                Value::Array(
                    claude_hook_event_names()
                        .map(|event| Value::String(event.into()))
                        .collect(),
                ),
            ),
            ("semanticReducers".into(), Value::Bool(true)),
            (
                "streamingMode".into(),
                Value::String(
                    "bounded live output with --stream; full raw recovery always saved".into(),
                ),
            ),
            ("goFallback".into(), Value::Bool(false)),
        ]);

        let _ = write_indented(standard_output, &payload);

        return 0;
    }

    let _ = writeln!(

        standard_output,

        "claude-skills PreToolUse hook transparently rewrites noisy shell commands via `claude-skills run -- <command>`. No manual rerun needed."

    );

    let _ = writeln!(
        standard_output,
        "Claude Code exposes hook events including: {}.",
        claude_hook_event_names().collect::<Vec<_>>().join(", ")
    );

    let _ = writeln!(

        standard_output,

        "claude-skills installs managed entries for every supported lifecycle event; `PreToolUse` silently rewrites supported Bash commands with native compaction."

    );

    let _ = writeln!(

        standard_output,

        "The Rust runtime uses native semantic reducers, raw recovery, gain analytics, and no Go or third-party compaction fallback."

    );

    0
}

fn run_hook_pre_tool_use(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to read Claude Code hook input: {error}"
            );

            return 1;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to decode Claude Code hook input: {error}"
            );

            return 1;
        }
    };

    if input
        .get("tool_name")
        .and_then(JsonDocument::as_str)
        .unwrap_or_default()
        != crate::hooks::claude::pre_tool_matcher()
    {
        return 0;
    }

    let command = input
        .get("tool_input")
        .and_then(|tool_input| tool_input.get("command"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    let rewrite = rewrite_command_text_for_shell(command, RewriteShell::Bash);

    if !rewrite.supported {
        return 0;
    }

    let payload = serde_json::json!({

        "hookSpecificOutput": {

            "hookEventName": MANAGED_PRE_TOOL_USE_EVENT,

            "permissionDecision": "allow",

            "updatedInput": {

                "command": rewrite.rewritten_command,

            },

        }

    });

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");

            0
        }

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render Claude Code hook output: {error}"
            );

            1
        }
    }
}

/// PostToolUse handler.
///
/// Two responsibilities:
///   1. Count edit-class tool calls (Edit, Write, MultiEdit, NotebookEdit) in a
///      per-workspace counter file under `<claude_home>/state/system-map-edit-counter/<key>`.
///   2. Refresh SYSTEM_MAP.md every N edits so the workspace pointer stays in
///      sync with the repo. N defaults to 10; override via
///      `CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL` (`0` disables).
///
/// PostToolUse stays silent on `additionalContext` (the model already sees the
/// tool result), so we never emit JSON — only do the side-effect and return 0.
fn run_hook_post_tool_use(standard_error: &mut dyn Write) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,

        // PostToolUse must never fail loudly: a non-zero exit teaches Claude
        // Code that the post-tool hook itself is broken. Log to stderr and
        // exit 0 — the lifecycle event is observability, not a gate.
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "claude-skills post-tool-use: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "claude-skills post-tool-use: unable to decode hook input: {error}"
            );

            return 0;
        }
    };

    let tool_name = input
        .get("tool_name")
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    if !is_edit_class_tool(tool_name) {
        return 0;
    }

    let threshold = system_map_refresh_threshold();

    if threshold == 0 {
        return 0;
    }

    let Some(counter_path) = system_map_edit_counter_path() else {
        return 0;
    };

    let next_count = match increment_counter_file(&counter_path) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "claude-skills post-tool-use: counter update failed: {error}"
            );

            return 0;
        }
    };

    if next_count >= threshold {
        let _ = refresh_memory_scope_for_current_directory(standard_error);
        let _ = reset_counter_file(&counter_path);
    }

    0
}

fn is_edit_class_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

fn system_map_refresh_threshold() -> u64 {
    std::env::var("CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD)
}

fn system_map_edit_counter_path() -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;
    let workspace_root = std::env::current_dir().ok()?;
    let workspace_key = sanitize_memory_key(&display_path(&workspace_root));

    Some(
        claude_home
            .join("state")
            .join("system-map-edit-counter")
            .join(workspace_key),
    )
}

fn increment_counter_file(path: &Path) -> std::io::Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let current = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);

    let next = current.saturating_add(1);

    fs::write(path, next.to_string())?;

    Ok(next)
}

fn reset_counter_file(path: &Path) -> std::io::Result<()> {
    fs::write(path, "0")
}

fn run_hook_lifecycle(
    subcommand: &str,

    standard_output: &mut dyn Write,

    standard_error: &mut dyn Write,
) -> u8 {
    // Look up the canonical row once; every behaviour below comes from that row.
    let event = match event_by_slug(subcommand) {
        Some(row) => row,
        // Unreachable in practice — `run_hook_command` only routes valid slugs to
        // us. Falling back to SessionStart preserves the legacy default that the
        // previous string-based mapping returned for unknown subcommands.
        None => event_by_name("SessionStart").expect("SessionStart row missing"),
    };

    // Refresh the workspace system map at the three natural transition
    // points: session start, before compaction (so the post-compact window
    // resumes against a fresh map), and session end (so the next session
    // starts from the latest layout). The agent does not have to remember
    // to invoke `claude-skills memory scope resolve` — these hooks fire it
    // automatically. The single source of truth is `should_refresh_system_map`
    // so the test for the trigger set stays pure and deterministic.
    if should_refresh_system_map(event.name) {
        let _ = refresh_memory_scope_for_current_directory(standard_error);
    }

    if event.name == "SessionEnd" {
        prune_raw_output_store(standard_error);
    }

    let context = lifecycle_additional_context(event.slug);

    if context.trim().is_empty() {
        return 0;
    }

    // Whether this event accepts `hookSpecificOutput` or must fall back to a
    // top-level `systemMessage` lives on the event row, so adding a new event
    // to the table automatically picks up the right schema.
    let payload = if event.supports_hook_specific_output {
        serde_json::json!({

            "hookSpecificOutput": {

                "hookEventName": event.name,

                "additionalContext": context,

            },

            "suppressOutput": true,

        })
    } else {
        serde_json::json!({

            "systemMessage": context,

            "suppressOutput": true,

        })
    };

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");

            0
        }

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render Claude Code lifecycle hook output: {error}"
            );

            1
        }
    }
}

/// Look up the PascalCase event name for a kebab slug. Used by callers that have a
/// slug in hand but need to reason in Claude Code's PascalCase vocabulary.
fn lifecycle_additional_context(subcommand: &str) -> String {
    match subcommand {
        "session-start" => session_start_context(),

        "pre-compact" => pre_compact_context(),

        "post-compact" => post_compact_context(),

        // UserPromptSubmit injects a short research-first iron-law restatement
        // so the bootstrap skill that SessionStart delivered stays top-of-mind
        // on every turn. The text is intentionally compact (~80 tokens)
        // because it lands per-prompt; the full bootstrap (Red Flags table,
        // skill catalog, decision flow) lives in using-claude-core/SKILL.md
        // and is delivered once via SessionStart.
        "user-prompt-submit" => user_prompt_submit_context(),

        // PostToolBatch fires after a batch of parallel tools resolves, just
        // before the next model turn. We inject a reviewer-on-close reminder
        // here because Stop/SubagentStop don't support additionalContext per
        // the official Claude Code hooks schema. Trivial work (docs-only,
        // formatting, single-line typo) stays exempt per the two-tier rule
        // documented in CLAUDE.md.
        "post-tool-batch" => post_tool_batch_context(),

        // Silenced events. Stop/SubagentStop/SessionEnd fire per turn end and
        // the schema rejects context injection on them. PostToolUse handles
        // its side-effect (the SYSTEM_MAP edit counter) inside
        // run_hook_post_tool_use; the context stays empty.
        "stop" | "subagent-stop" | "session-end" | "post-tool-use" => String::new(),

        _ => String::new(),
    }
}

/// Bootstrap skill text embedded at compile time.
///
/// The file lives at the repository root so `discover_repository_layout` picks
/// it up alongside the other skills and `sync_skills` installs it under
/// `~/.claude/skills/using-claude-core/SKILL.md`. We *also* embed it here so
/// SessionStart can inject the full text directly into `additionalContext`,
/// which Claude Code caches for the rest of the session. CLAUDE.md and the
/// individual SKILL.md files are read by the skill matcher on demand; this
/// single block is what the model sees up front, so it doubles as the
/// research-first iron law and the catalog of every other invokable skill.
const BOOTSTRAP_SKILL: &str = include_str!("../../../../../using-claude-core/SKILL.md");

fn session_start_context() -> String {
    // SessionStart fires once per session and the payload is cached for the
    // rest of the cache window, so this is the right place to deliver the
    // bootstrap skill. Per-prompt cost is zero after the first turn.
    //
    // Layout: full bootstrap skill (iron law + Red Flags + skill catalog +
    // workspace pointers) followed by the runtime-resolved memory pointer
    // that CLAUDE.md cannot know in advance.
    format!("{BOOTSTRAP_SKILL}\n\n{}", memory_scope_summary())
}

fn pre_compact_context() -> String {
    "Before compaction, preserve claude-skills continuity: summarize active workflow stage, files changed, validation evidence, unresolved blockers, memory facts to save, and next review gate.".to_string()
}

fn post_compact_context() -> String {
    format!(

        "After compaction, resume using claude-skills automatically: reload workspace memory/system map, re-establish workflow proof state, and run review gates before final closeout.\n\n{}",

        memory_scope_summary()

    )
}

/// Per-prompt research-first iron law.
///
/// Compact by design: the schema lets us inject as much text as we want, but
/// every byte lands per prompt and is paid as input tokens for the rest of
/// the cache window. The full bootstrap (skill catalog, Red Flags table,
/// decision flow) is delivered once via SessionStart; this hook only
/// restates the iron law so it stays top-of-mind on each turn.
fn user_prompt_submit_context() -> String {
    format!(
        "Research-first: trust the codebase, not your knowledge base. Read SYSTEM_MAP and the owning module before claiming behavior. Invoke any relevant skill via the Skill tool BEFORE responding — even a 1% chance it applies means use it. Find the root cause, not just the surface symptom. No assumptions. {}",
        memory_scope_summary()
    )
}

/// Reviewer-on-close reminder.
///
/// Fires after every batch of parallel tool calls, just before the model's
/// next turn. We rely on the model to apply the two-tier rule from CLAUDE.md
/// (trivial work skips reviewer; non-trivial routes through it) — we just
/// surface the question rather than gating with `decision: "block"`, which
/// would force a review on every tool batch including read-only research.
///
/// The text deliberately cites the exact section (`Routing Rules item 3`) so
/// the rule is verifiable in one read. Models that pattern-match generic
/// reminders as wrapper noise have rationalized past prior versions of this
/// text — the citation makes the dismissal harder because the reader can
/// either confirm the rule exists or prove it doesn't, but cannot honestly
/// claim "this references rules that don't exist" without reading.
fn post_tool_batch_context() -> String {
    "Closeout check: if this batch changed code (Edit/Write/MultiEdit on non-trivial files), invoke the reviewer skill before final closeout per CLAUDE.md → Routing Rules → item 3 (the two-tier rule). Trivial work (docs-only, formatting, single-line typo) is exempt. If this reminder feels like wrapper noise, that is the rationalization the rule names — verify item 3 yourself before skipping.".to_string()
}

fn prune_raw_output_store(standard_error: &mut dyn Write) {
    let retention_days = std::env::var("CLAUDE_SKILLS_RAW_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(RAW_OUTPUT_DEFAULT_RETENTION_DAYS);
    if retention_days == 0 {
        return;
    }
    if let Err(error) = RawStore::new().prune_older_than(retention_days) {
        let _ = writeln!(
            standard_error,
            "claude-skills raw-output prune failed: {error}"
        );
    }
}

/// Lifecycle events that should auto-refresh the workspace SYSTEM_MAP.
///
/// The agent has historically forgotten to refresh the map by hand, so we
/// fire it at the three natural transition points instead of trusting the
/// model to remember:
///
/// - `SessionStart` — first turn, the map needs to reflect today's layout
///   before the model reasons about the repo.
/// - `PreCompact` — context is about to be compacted; the post-compact
///   window resumes against a fresh map written *now*, so the layout the
///   model recovers matches reality even if files moved earlier in the
///   conversation.
/// - `SessionEnd` — the next session starts from the freshest possible map
///   without paying the cost on its first prompt.
///
/// Kept as a small slug-named function so the trigger set is testable in
/// isolation and the call site at `run_hook_lifecycle` reads as a single
/// predicate instead of a chain of equality checks.
fn should_refresh_system_map(event_name: &str) -> bool {
    matches!(event_name, "SessionStart" | "PreCompact" | "SessionEnd")
}

fn refresh_memory_scope_for_current_directory(standard_error: &mut dyn Write) -> Option<PathBuf> {
    let workspace_root = std::env::current_dir().ok()?;

    let mut stdout = Vec::new();

    let mut stderr = Vec::new();

    let arguments = vec![
        "scope".to_string(),
        "resolve".to_string(),
        "--workspace-root".to_string(),
        display_path(&workspace_root),
        "--create-missing".to_string(),
        "--refresh-system-map".to_string(),
        "--format".to_string(),
        "compact".to_string(),
    ];

    let code = utility::run_memory_command("memory", &arguments, &mut stdout, &mut stderr);

    if code != 0 {
        let _ = writeln!(
            standard_error,
            "claude-skills lifecycle memory refresh failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        );

        return None;
    }

    memory_system_map_path_for_workspace(&workspace_root)
}

fn memory_scope_summary() -> String {
    match std::env::current_dir()

        .ok()

        .and_then(|workspace_root| memory_system_map_path_for_workspace(&workspace_root))

    {

        Some(path) => format!(

            "Workspace memory system map: {}. Read it before making repo-structure claims; refresh happens automatically at session start, pre-compact, and session end.",

            display_path(&path)

        ),

        None => "Workspace memory system map: unavailable; create it with claude-skills memory scope resolve --create-missing --refresh-system-map before repo-structure claims.".to_string(),

    }
}

fn memory_system_map_path_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;

    let workspace_key = sanitize_memory_key(&display_path(workspace_root));

    Some(
        claude_home
            .join("memories")
            .join("workspaces")
            .join(workspace_key)
            .join("reference")
            .join("SYSTEM_MAP.md"),
    )
}

fn sanitize_memory_key(value: &str) -> String {
    let mut key = String::new();

    let mut previous_dash = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            key.push(character.to_ascii_lowercase());

            previous_dash = false;
        } else if !previous_dash {
            key.push('-');

            previous_dash = true;
        }
    }

    let trimmed = key.trim_matches('-').to_string();

    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

fn render_hook_help(standard_output: &mut dyn Write) {
    let _ = writeln!(

        standard_output,

        "Usage: claude-skills hook [install|uninstall|list|show|instructions|diagnose|pre-tool-use|post-tool-use|post-tool-use-failure|permission-request|notification|user-prompt-submit|stop|subagent-stop|task-created|task-completed|pre-compact|post-compact|session-start|session-end]"

    );
}

fn run_hook_diagnose(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook diagnose");
    flag_set.string_flag("format", "text");

    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let format = flag_set.string_value("format").to_string();
    if format != "text" && format != "json" {
        let _ = writeln!(
            standard_error,
            "hook diagnose: --format must be 'text' or 'json'"
        );
        return 1;
    }

    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let report = collect_hook_diagnostics(&claude_home);

    if format == "json" {
        match serde_json::to_string_pretty(&report.to_json()) {
            Ok(rendered) => {
                let _ = writeln!(standard_output, "{rendered}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "Unable to render diagnose output: {error}");
                return 1;
            }
        }
    } else {
        report.render_text(standard_output);
    }

    if report.healthy() {
        0
    } else {
        2
    }
}

#[derive(Debug)]
struct HookDiagnostics {
    claude_home: PathBuf,
    installed_executable: PathBuf,
    installed_executable_present: bool,
    settings_path: PathBuf,
    settings_present: bool,
    settings_parses: Option<bool>,
    managed_hook_command: Option<String>,
    settings_points_at_installed: Option<bool>,
    orphan_executable_siblings: Vec<PathBuf>,
}

impl HookDiagnostics {
    fn healthy(&self) -> bool {
        self.installed_executable_present
            && self.settings_present
            && self.settings_parses == Some(true)
            && self.settings_points_at_installed == Some(true)
            && self.orphan_executable_siblings.is_empty()
    }

    fn to_json(&self) -> JsonDocument {
        let orphans: Vec<JsonDocument> = self
            .orphan_executable_siblings
            .iter()
            .map(|path| JsonDocument::String(display_path(path)))
            .collect();
        serde_json::json!({
            "claudeHome": display_path(&self.claude_home),
            "installedExecutable": {
                "path": display_path(&self.installed_executable),
                "present": self.installed_executable_present,
            },
            "settings": {
                "path": display_path(&self.settings_path),
                "present": self.settings_present,
                "parses": self.settings_parses,
                "pointsAtInstalled": self.settings_points_at_installed,
            },
            "managedHookCommand": self.managed_hook_command,
            "orphanExecutableSiblings": orphans,
            "healthy": self.healthy(),
        })
    }

    fn render_text(&self, output: &mut dyn Write) {
        let check = |ok: bool| if ok { "ok" } else { "FAIL" };
        let unknown = "unknown";

        let _ = writeln!(output, "claude-skills hook diagnose");
        let _ = writeln!(output, "  claude home: {}", display_path(&self.claude_home));
        let _ = writeln!(
            output,
            "  installed executable [{}]: {}",
            check(self.installed_executable_present),
            display_path(&self.installed_executable)
        );

        if !self.settings_present {
            let _ = writeln!(
                output,
                "  settings.json [FAIL]: missing at {}",
                display_path(&self.settings_path)
            );
        } else {
            let parses = match self.settings_parses {
                Some(true) => "ok",
                Some(false) => "FAIL",
                None => unknown,
            };
            let points = match self.settings_points_at_installed {
                Some(true) => "ok",
                Some(false) => "FAIL",
                None => unknown,
            };
            let _ = writeln!(
                output,
                "  settings.json [parse {parses}, points-at-installed {points}]: {}",
                display_path(&self.settings_path)
            );
        }

        if self.orphan_executable_siblings.is_empty() {
            let _ = writeln!(output, "  orphan executable siblings [ok]: none");
        } else {
            let _ = writeln!(
                output,
                "  orphan executable siblings [FAIL]: {} found",
                self.orphan_executable_siblings.len()
            );
            for orphan in &self.orphan_executable_siblings {
                let _ = writeln!(output, "    {}", display_path(orphan));
            }
        }

        let _ = writeln!(
            output,
            "  status: {}",
            if self.healthy() {
                "healthy"
            } else {
                "issues found"
            }
        );
    }
}

fn collect_hook_diagnostics(claude_home: &Path) -> HookDiagnostics {
    let installed_executable = installed_executable_path(claude_home);
    let installed_executable_present = installed_executable.is_file();
    let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let settings_present = settings_path.is_file();

    let (settings_parses, settings_points_at_installed) = if !settings_present {
        (None, None)
    } else {
        match read_hooks_document(&settings_path) {
            Ok(document) => {
                let points =
                    settings_points_at_installed_executable(&document, &installed_executable);
                (Some(true), Some(points))
            }
            Err(_) => (Some(false), None),
        }
    };

    let managed_hook_command = managed_hook_command().ok();
    let orphan_executable_siblings = crate::manager::install::find_executable_orphans(claude_home);

    HookDiagnostics {
        claude_home: claude_home.to_path_buf(),
        installed_executable,
        installed_executable_present,
        settings_path,
        settings_present,
        settings_parses,
        managed_hook_command,
        settings_points_at_installed,
        orphan_executable_siblings,
    }
}

fn settings_points_at_installed_executable(
    document: &JsonDocument,
    installed_executable: &Path,
) -> bool {
    // Casefold paths only on Windows. NTFS and `cmd /C` arguments are
    // case-insensitive, so the rendered hook command may carry the same path
    // with a different casing than `display_path()` returns. On Linux and
    // macOS, filesystems are case-sensitive and `~/.claude/claude-skills` is a
    // genuinely different file from `~/.Claude/claude-skills` — lowercasing
    // would mask a real misconfiguration. `casefold` is therefore the identity
    // on Unix and `to_ascii_lowercase` only on Windows.
    let casefold = |value: &str| -> String {
        if cfg!(windows) {
            value.to_ascii_lowercase()
        } else {
            value.to_string()
        }
    };

    let installed_normalized = casefold(&display_path(installed_executable));
    // Path matches must be full path. A file-name-only fallback would
    // accept stale settings that point at a sibling executable (e.g.
    // claude_home/elsewhere/claude-skills.exe) just because the file name
    // matches, which is exactly the misconfiguration this check is meant
    // to catch.

    let Some(hooks) = document.get("hooks").and_then(JsonDocument::as_object) else {
        return false;
    };

    let mut managed_seen = false;
    let mut all_managed_point_at_installed = true;

    for (_event_name, event_entries) in hooks.iter() {
        let Some(entries) = event_entries.as_array() else {
            continue;
        };
        for matcher_entry in entries {
            let Some(commands) = matcher_entry.get("hooks").and_then(JsonDocument::as_array) else {
                continue;
            };
            for command_entry in commands {
                let Some(command) = command_entry.get("command").and_then(JsonDocument::as_str)
                else {
                    continue;
                };
                if !is_managed_hook_command(command) {
                    continue;
                }
                managed_seen = true;
                let command_normalized = casefold(command);
                // The PowerShell encoded payload is always produced from a
                // Windows host and uses Windows path conventions, so its
                // decoded form is matched case-insensitively regardless of
                // the host running the doctor.
                let decoded_normalized = decode_powershell_encoded_command(command)
                    .map(|decoded| decoded.to_ascii_lowercase());
                let installed_for_decoded = display_path(installed_executable).to_ascii_lowercase();
                let plain_match = command_normalized.contains(&installed_normalized);
                let decoded_match = decoded_normalized
                    .as_ref()
                    .map(|decoded| decoded.contains(&installed_for_decoded))
                    .unwrap_or(false);
                if !(plain_match || decoded_match) {
                    all_managed_point_at_installed = false;
                }
            }
        }
    }

    managed_seen && all_managed_point_at_installed
}

fn is_help_argument(argument: &str) -> bool {
    matches!(argument, "help" | "--help" | "-h")
}

pub fn managed_hook_command() -> Result<String, String> {
    std::env::current_exe()
        .map(|path| hook_command_for_executable_args(&path, MANAGED_PRE_TOOL_USE_COMMAND_SUFFIX))
        .map_err(|error| format!("resolve current executable: {error}"))
}

pub fn build_hooks_payload(hook_path: &Path, hook_command: &str) -> Result<String, String> {
    let mut document = read_hooks_document(hook_path)?;

    ensure_hooks_object(&mut document)?;

    remove_managed_hooks(&mut document);

    append_managed_hooks(&mut document, hook_command)?;

    ensure_skill_listing_budget_fraction(&mut document)?;

    serde_json::to_string_pretty(&document)
        .map(|rendered| format!("{rendered}\n"))
        .map_err(|error| format!("render hooks config: {error}"))
}

fn ensure_skill_listing_budget_fraction(document: &mut JsonDocument) -> Result<(), String> {
    let object = document
        .as_object_mut()
        .ok_or_else(|| "settings.json root is not a JSON object".to_string())?;
    if !object.contains_key("skillListingBudgetFraction") {
        object.insert(
            "skillListingBudgetFraction".to_string(),
            serde_json::json!(0.02),
        );
    }
    Ok(())
}

pub fn remove_managed_hook_payload(hook_path: &Path) -> Result<(String, bool), String> {
    let mut document = read_hooks_document(hook_path)?;

    let before = serde_json::to_string(&document).unwrap_or_default();

    ensure_hooks_object(&mut document)?;

    remove_managed_hooks(&mut document);

    let after = serde_json::to_string(&document).unwrap_or_default();

    let rendered = serde_json::to_string_pretty(&document)
        .map(|value| format!("{value}\n"))
        .map_err(|error| format!("render hooks config: {error}"))?;

    Ok((rendered, before != after))
}

pub fn read_hooks_document(hook_path: &Path) -> Result<JsonDocument, String> {
    match fs::read_to_string(hook_path) {
        Ok(text) if text.trim().is_empty() => Ok(serde_json::json!({"hooks": {}})),

        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("parse {}: {error}", display_path(hook_path))),

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::json!({"hooks": {}}))
        }

        Err(error) => Err(format!("read {}: {error}", display_path(hook_path))),
    }
}

fn ensure_hooks_object(document: &mut JsonDocument) -> Result<(), String> {
    if !document.is_object() {
        *document = serde_json::json!({"hooks": {}});

        return Ok(());
    }

    let object = document.as_object_mut().expect("object checked");

    if !object.contains_key("hooks") {
        object.insert("hooks".into(), JsonDocument::Object(JsonMap::new()));
    }

    if !object
        .get("hooks")
        .map(JsonDocument::is_object)
        .unwrap_or(false)
    {
        return Err("settings.json contains a non-object hooks field".into());
    }

    Ok(())
}

pub fn remove_managed_hooks(document: &mut JsonDocument) {
    let Some(hooks) = document
        .get_mut("hooks")
        .and_then(JsonDocument::as_object_mut)
    else {
        return;
    };

    for (_event_name, event_entries) in hooks.iter_mut() {
        let Some(entries) = event_entries.as_array_mut() else {
            continue;
        };

        for matcher_entry in entries.iter_mut() {
            let Some(commands) = matcher_entry
                .get_mut("hooks")
                .and_then(JsonDocument::as_array_mut)
            else {
                continue;
            };

            commands.retain(|command_entry| {
                !command_entry
                    .get("command")
                    .and_then(JsonDocument::as_str)
                    .map(is_managed_hook_command)
                    .unwrap_or(false)
            });
        }

        entries.retain(|matcher_entry| {
            matcher_entry
                .get("hooks")
                .and_then(JsonDocument::as_array)
                .map(|commands| !commands.is_empty())
                .unwrap_or(true)
        });
    }
}

fn append_managed_hooks(document: &mut JsonDocument, hook_command: &str) -> Result<(), String> {
    let hooks = document
        .get_mut("hooks")
        .and_then(JsonDocument::as_object_mut)
        .ok_or_else(|| "settings.json missing hooks object".to_string())?;

    for event in HOOK_EVENTS {
        let event_entries = hooks
            .entry(event.name.to_string())
            .or_insert_with(|| JsonDocument::Array(Vec::new()));

        let event_array = event_entries
            .as_array_mut()
            .ok_or_else(|| format!("{} hooks entry is not an array", event.name))?;

        let (matcher, command, status) = managed_hook_entry_for_event(event, hook_command);

        event_array.push(serde_json::json!({

            "matcher": matcher,

            "hooks": [{

                "type": "command",

                "command": command,

                "statusMessage": status

            }]

        }));
    }

    sort_hook_events(hooks);

    Ok(())
}

/// Pull matcher / command / status straight from the event row. The PreToolUse
/// command is the only one that flows in from the outside (it carries the rewriter
/// payload built in `managed_hook_command`); every other event derives its command
/// from `managed_lifecycle_command(slug)`.
fn managed_hook_entry_for_event(
    event: &HookEvent,

    pre_tool_use_command: &str,
) -> (&'static str, String, &'static str) {
    let command = if event.name == MANAGED_PRE_TOOL_USE_EVENT {
        pre_tool_use_command.to_string()
    } else {
        managed_lifecycle_command(event.slug)
    };

    (event.matcher, command, event.status)
}

fn sort_hook_events(hooks: &mut JsonMap<String, JsonDocument>) {
    let sorted: BTreeMap<String, JsonDocument> = hooks
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    hooks.clear();

    for (key, value) in sorted {
        hooks.insert(key, value);
    }
}

pub fn is_managed_hook_command(command: &str) -> bool {
    is_managed_hook_command_with_depth(command, 0)
}

fn is_managed_hook_command_with_depth(command: &str, depth: usize) -> bool {
    const MAX_DECODE_DEPTH: usize = 2;

    let normalized = command.to_ascii_lowercase();

    let has_any_lifecycle = HOOK_EVENTS
        .iter()
        .any(|event| normalized.contains(&format!("hook {}", event.slug)));

    let plain_managed = normalized.contains("claude-skills")
        && (has_any_lifecycle || normalized.contains("hook instructions --format json"));

    if plain_managed {
        return true;
    }

    if depth >= MAX_DECODE_DEPTH {
        return false;
    }

    decode_powershell_encoded_command(command)
        .map(|decoded| is_managed_hook_command_with_depth(&decoded, depth + 1))
        .unwrap_or(false)
}

pub fn managed_lifecycle_command(subcommand: &str) -> String {
    match std::env::current_exe() {
        Ok(path) => hook_command_for_executable_args(&path, &format!("hook {subcommand}")),

        Err(_) => format!("claude-skills hook {subcommand}"),
    }
}

/// Build the platform-correct shell command Claude Code should invoke for a
/// given hook subcommand.
///
/// **Linux / macOS** use a plain `bash`-style invocation: the executable path
/// is shell-quoted and concatenated with the subcommand arguments. POSIX
/// shells parse this exactly as a user would type it.
///
/// **Windows** is more delicate. Claude Code's hook runner spawns commands
/// through PowerShell, and the hook command is a single string the user will
/// see in `settings.json`. Three problems show up if we naively emit
/// `& 'C:\Users\Some User\.claude\claude-skills.exe' hook pre-tool-use`:
///
///   1. PowerShell single-quote rules differ from bash; embedded apostrophes
///      in user-profile paths (`C:\Users\O'Brien\...`) are easy to mis-quote.
///   2. The string ends up serialized into JSON, then re-parsed by Claude
///      Code, then handed to PowerShell. Each round trip is a chance for
///      backslashes or quotes to be re-interpreted.
///   3. UTF-16 paths (Cyrillic profile names, accented characters) get
///      corrupted by the UTF-8 → ANSI conversion PowerShell does on positional
///      argument strings.
///
/// PowerShell's `-EncodedCommand` flag sidesteps all three: the script is
/// encoded as UTF-16 LE, base64-wrapped, and handed to PowerShell verbatim.
/// PowerShell decodes it as UTF-16 directly, so no transcoding happens. The
/// JSON serializer only sees a base64 string with no special characters, so
/// the round trip is lossless.
///
/// `is_managed_hook_command` decodes the same encoding when checking whether
/// a settings.json entry is one of ours, so doctor and uninstall paths can
/// reason about Windows hook commands without re-implementing the parsing.
pub fn hook_command_for_executable_args(path: &Path, arguments: &str) -> String {
    if cfg!(windows) {
        let script = platform_default_command_for_executable_args(path, arguments);

        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
            powershell_encoded_command(&script)
        )
    } else {
        bash_command_for_executable_args(path, arguments)
    }
}

fn powershell_encoded_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);

    for unit in script.encode_utf16() {
        bytes.push((unit & 0x00ff) as u8);

        bytes.push((unit >> 8) as u8);
    }

    base64_encode(&bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];

        let second = *chunk.get(1).unwrap_or(&0);

        let third = *chunk.get(2).unwrap_or(&0);

        rendered.push(BASE64_ALPHABET[(first >> 2) as usize] as char);

        rendered
            .push(BASE64_ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);

        if chunk.len() > 1 {
            rendered.push(
                BASE64_ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char,
            );
        } else {
            rendered.push('=');
        }

        if chunk.len() > 2 {
            rendered.push(BASE64_ALPHABET[(third & 0b0011_1111) as usize] as char);
        } else {
            rendered.push('=');
        }
    }

    rendered
}

fn base64_decode(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len() / 4 * 3);

    let mut chunk = [0u8; 4];

    let mut chunk_len = 0usize;

    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        let decoded = match byte {
            b'A'..=b'Z' => byte - b'A',

            b'a'..=b'z' => byte - b'a' + 26,

            b'0'..=b'9' => byte - b'0' + 52,

            b'+' => 62,

            b'/' => 63,

            b'=' => 64,

            _ => return None,
        };

        chunk[chunk_len] = decoded;

        chunk_len += 1;

        if chunk_len != 4 {
            continue;
        }

        output.push((chunk[0] << 2) | (chunk[1] >> 4));

        if chunk[2] != 64 {
            output.push((chunk[1] << 4) | (chunk[2] >> 2));
        }

        if chunk[3] != 64 {
            output.push((chunk[2] << 6) | chunk[3]);
        }

        chunk_len = 0;
    }

    if chunk_len == 0 {
        Some(output)
    } else {
        None
    }
}

fn decode_powershell_encoded_command(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();

    while let Some(word) = words.next() {
        if !word.eq_ignore_ascii_case("-EncodedCommand") {
            continue;
        }

        let encoded = words.next()?.trim_matches('"').trim_matches('\'');

        let bytes = base64_decode(encoded)?;

        if bytes.len() % 2 != 0 {
            return None;
        }

        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        return String::from_utf16(&units).ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hook_payload_preserves_unrelated_events_and_replaces_managed_hook() {
        let hook_path = temp_hook_path("claude-skills-hook-payload");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(
            &hook_path,
            r#"{

  "hooks": {

    "PostToolUse": [

      {

        "matcher": "Write|Edit",

        "hooks": [

          {

            "type": "command",

            "command": "./scripts/post_write_figma_parity_check.sh"

          }

        ]

      }

    ],

    "PreToolUse": [

      {

        "matcher": "Bash",

        "hooks": [

          {

            "type": "command",

            "command": "claude-skills hook instructions --format json"

          }

        ]

      }

    ]

  }

}

"#,
        )
        .unwrap();

        let rendered = build_hooks_payload(
            &hook_path,
            r#""C:\tools\claude-skills.exe" hook pre-tool-use"#,
        )
        .unwrap();

        assert!(rendered.contains("PostToolUse"));

        assert!(rendered.contains("PermissionRequest"));

        assert!(rendered.contains("Notification"));

        assert!(rendered.contains("PreCompact"));

        assert!(rendered.contains("PostCompact"));

        assert!(rendered.contains("SessionStart"));

        assert!(rendered.contains("SessionEnd"));

        assert!(rendered.contains("UserPromptSubmit"));

        assert!(rendered.contains("SubagentStop"));

        assert!(rendered.contains("Stop"));

        assert!(rendered.contains("post_write_figma_parity_check"));

        assert!(rendered.contains("hook pre-tool-use"));

        assert!(!rendered.contains("hook instructions --format json"));

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn hook_payload_uses_exact_managed_commands_for_each_event() {
        let hook_path = temp_hook_path("claude-skills-hook-command-prefix");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let pre_tool_command = managed_hook_command().unwrap();

        let rendered = build_hooks_payload(&hook_path, &pre_tool_command).unwrap();

        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        let hooks = document
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();

        for event in claude_hook_event_names() {
            let commands = hooks
                .get(event)
                .and_then(JsonDocument::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("hooks"))
                .and_then(JsonDocument::as_array)
                .unwrap();

            let command = commands
                .first()
                .and_then(|hook| hook.get("command"))
                .and_then(JsonDocument::as_str)
                .unwrap();

            let expected = expected_managed_command_for_event(event, &pre_tool_command);

            assert_eq!(command, expected, "unexpected command for {event}");

            if cfg!(windows) {
                assert!(command.starts_with(
                    "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand "
                ));
            } else {
                assert!(!command.starts_with("& "));
            }
        }

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn powershell_encoded_command_roundtrips_to_runnable_script() {
        // The hook entry on Windows is shipped as a base64-encoded UTF-16 LE
        // PowerShell script. Claude Code launches it with `powershell.exe
        // -EncodedCommand <payload>`, so the payload must decode back to a
        // script that references the installed executable and the requested
        // hook subcommand. A regression in base64_encode (chunk loop), UTF-16
        // endianness, or the script template would silently produce a
        // command Claude Code can run but that does nothing useful — and
        // because is_managed_hook_command does its own decode, the entry
        // would still look "managed" while pointing at gibberish.
        let executable = if cfg!(windows) {
            Path::new(r"C:\Users\Example User\.claude\claude-skills.exe")
        } else {
            Path::new("/home/example/.claude/claude-skills")
        };
        for subcommand in ["pre-tool-use", "session-start", "post-compact", "stop"] {
            let arguments = format!("hook {subcommand}");
            let command = hook_command_for_executable_args(executable, &arguments);

            if cfg!(windows) {
                let decoded = decode_powershell_encoded_command(&command)
                    .unwrap_or_else(|| panic!("decode failed for {subcommand}: {command}"));
                let executable_lower = executable.to_string_lossy().to_ascii_lowercase();
                assert!(
                    decoded.to_ascii_lowercase().contains(&executable_lower),
                    "decoded script for {subcommand} must reference the installed executable; got: {decoded}"
                );
                assert!(
                    decoded.contains(&arguments),
                    "decoded script for {subcommand} must include `{arguments}`; got: {decoded}"
                );
            } else {
                // Unix wrappers are already plain bash; no encoding to verify.
                assert!(command.contains(&arguments));
            }
        }
    }

    #[test]
    fn managed_hook_detection_handles_encoded_powershell_commands() {
        let path = Path::new(r"C:\Users\Example User\.claude\claude-skills.exe");

        let command = hook_command_for_executable_args(path, "hook session-start");

        assert!(is_managed_hook_command(&command));

        assert!(!is_managed_hook_command(
            "powershell.exe -NoProfile -EncodedCommand SQBuAHYAYQBsAGkAZAA="
        ));
    }

    #[test]
    fn pre_and_post_tool_use_matchers_scope_to_bash() {
        let hook_path = temp_hook_path("claude-skills-hook-matcher-scope");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let rendered = build_hooks_payload(&hook_path, "claude-skills hook pre-tool-use").unwrap();

        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        let hooks = document
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();

        for (event, expected_matcher) in [
            ("PreToolUse", "Bash"),
            ("PostToolUse", "Bash"),
            ("UserPromptSubmit", ""),
            ("SessionStart", ""),
        ] {
            let matcher = hooks
                .get(event)
                .and_then(JsonDocument::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("matcher"))
                .and_then(JsonDocument::as_str)
                .unwrap_or_else(|| panic!("missing matcher for {event}"));

            assert_eq!(matcher, expected_matcher, "unexpected matcher for {event}");
        }

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn silenced_high_frequency_hooks_emit_no_additional_context() {
        // PostToolUse / Stop / SubagentStop / SessionEnd fire per tool call or
        // turn end. They either (a) don't support hookSpecificOutput per the
        // official Claude Code schema or (b) carry a prompt-cache cost that
        // outweighs the value of any per-call reminder. The operating contract
        // belongs in CLAUDE.md and SessionStart, both paid once per cache
        // window. These events must stay silent.
        //
        // UserPromptSubmit and PostToolBatch are deliberately *not* in this
        // list: they emit short research-first / reviewer-on-close pointers,
        // gated by their own dedicated tests below.
        for subcommand in ["post-tool-use", "stop", "subagent-stop", "session-end"] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let code = run_hook_lifecycle(subcommand, &mut stdout, &mut stderr);

            assert_eq!(
                code,
                0,
                "stderr for {subcommand}: {}",
                String::from_utf8_lossy(&stderr)
            );

            assert!(
                stdout.is_empty(),
                "{subcommand} must emit no additional context to avoid per-prompt token cost; got: {}",
                String::from_utf8_lossy(&stdout)
            );
        }
    }

    #[test]
    fn system_map_refresh_fires_on_session_start_pre_compact_and_session_end() {
        // The agent has historically forgotten to invoke memory scope
        // resolve / system-map refresh by hand. The lifecycle handler now
        // does it automatically at the three natural transition points so
        // SYSTEM_MAP.md is fresh when a new session starts, when context is
        // about to be compacted, and after the session ends. Any change to
        // this trigger set is a behavior change the user should see — this
        // test pins it.
        for event_name in ["SessionStart", "PreCompact", "SessionEnd"] {
            assert!(
                should_refresh_system_map(event_name),
                "{event_name} must auto-refresh the workspace SYSTEM_MAP"
            );
        }
    }

    #[test]
    fn system_map_refresh_does_not_fire_on_per_prompt_or_per_tool_events() {
        // Per-prompt and per-tool-call events fire too often to pay the
        // SYSTEM_MAP refresh cost on each. The PostToolUse path has its own
        // edit-counter gate (see run_hook_post_tool_use); these slugs must
        // stay out of the lifecycle auto-refresh trigger set.
        for event_name in [
            "UserPromptSubmit",
            "PostToolUse",
            "PostToolBatch",
            "Stop",
            "SubagentStop",
            "SubagentStart",
            "PostCompact",
            "Notification",
            "PermissionRequest",
        ] {
            assert!(
                !should_refresh_system_map(event_name),
                "{event_name} must not auto-refresh the workspace SYSTEM_MAP"
            );
        }
    }

    #[test]
    fn user_prompt_submit_emits_research_first_pointer() {
        // UserPromptSubmit lands per-prompt, so the injected text must be
        // short and pointer-shaped. The iron law (trust the codebase, invoke
        // skills before responding, find root cause) restates the bootstrap
        // skill that SessionStart already delivered, so it stays top-of-mind
        // on each turn even after the cache window rolls.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_lifecycle("user-prompt-submit", &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "stderr for user-prompt-submit: {}",
            String::from_utf8_lossy(&stderr)
        );

        let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

        let context = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("additionalContext"))
            .and_then(JsonDocument::as_str)
            .expect("UserPromptSubmit must emit additionalContext");

        assert!(context.contains("Research-first"));
        assert!(context.contains("SYSTEM_MAP"));
        assert!(context.contains("trust the codebase"));
        assert!(context.contains("Skill tool"));
        assert!(context.contains("root cause"));
        assert!(context.contains("No assumptions"));

        let event_name = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("hookEventName"))
            .and_then(JsonDocument::as_str);

        assert_eq!(event_name, Some("UserPromptSubmit"));
    }

    #[test]
    fn post_tool_batch_emits_reviewer_on_close_reminder() {
        // PostToolBatch fires after a batch of parallel tools resolves, just
        // before the model's next turn. It's the officially-supported event
        // for "before close" reminders — Stop/SubagentStop don't accept
        // hookSpecificOutput per the schema.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_lifecycle("post-tool-batch", &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "stderr for post-tool-batch: {}",
            String::from_utf8_lossy(&stderr)
        );

        let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

        let context = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("additionalContext"))
            .and_then(JsonDocument::as_str)
            .expect("PostToolBatch must emit additionalContext");

        assert!(context.contains("reviewer"));
        assert!(
            context.contains("Routing Rules"),
            "PostToolBatch reminder must cite the exact CLAUDE.md section so the rule is verifiable in one read"
        );
        assert!(context.contains("two-tier"));
        assert!(context.contains("Trivial"));
        assert!(
            context.contains("rationalization"),
            "PostToolBatch reminder must pre-empt the 'wrapper noise' dismissal"
        );

        let event_name = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("hookEventName"))
            .and_then(JsonDocument::as_str);

        assert_eq!(event_name, Some("PostToolBatch"));
    }

    #[test]
    fn edit_counter_increments_and_resets_at_threshold() {
        // The counter file is the bridge between PostToolUse fires (one per
        // tool call) and the periodic SYSTEM_MAP refresh. Verify the file
        // round-trips correctly so the threshold check in run_hook_post_tool_use
        // sees the right value.
        let dir =
            std::env::temp_dir().join(format!("claude-skills-edit-counter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let counter = dir.join("counter");

        for expected in 1..=3 {
            let next = increment_counter_file(&counter).unwrap();
            assert_eq!(next, expected);
        }

        reset_counter_file(&counter).unwrap();
        assert_eq!(std::fs::read_to_string(&counter).unwrap(), "0");

        let after_reset = increment_counter_file(&counter).unwrap();
        assert_eq!(after_reset, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_class_tools_match_documented_set() {
        // Only edit-class tools should bump the counter; read-only tools must
        // not, otherwise the SYSTEM_MAP refresh fires on every Read/Grep too.
        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            assert!(
                is_edit_class_tool(tool),
                "{tool} should count as edit-class"
            );
        }
        for tool in ["Read", "Grep", "Glob", "Bash", "Task"] {
            assert!(
                !is_edit_class_tool(tool),
                "{tool} must not count as edit-class"
            );
        }
    }

    #[test]
    fn session_start_context_embeds_bootstrap_skill_and_memory_pointer() {
        // SessionStart fires once per session and the payload is cached for
        // the rest of the cache window, so it carries the bootstrap skill
        // (iron law + Red Flags + skill catalog) plus the runtime-resolved
        // workspace memory pointer. Both pieces have to be there: the skill
        // delivers the operating contract, the pointer delivers the
        // workspace-specific memory path that CLAUDE.md cannot know in
        // advance.
        let context = session_start_context();

        // Bootstrap skill markers — these come from
        // <repo>/using-claude-core/SKILL.md via include_str! and are what
        // make the model treat skill invocation as non-optional.
        assert!(
            context.contains("EXTREMELY_IMPORTANT"),
            "SessionStart must embed the bootstrap skill iron-law block"
        );
        assert!(
            context.contains("Trust the codebase, not your knowledge base"),
            "SessionStart must restate the trust-the-codebase rule"
        );
        assert!(
            context.contains("Red Flags"),
            "SessionStart must embed the Red Flags rationalization table"
        );
        // Catalog spot-check: a couple of representative skill names so the
        // model knows what is invokable. Full enumeration lives in the skill
        // file; this assertion just guards that the catalog survived the
        // include.
        assert!(
            context.contains("preserve-existing-flow"),
            "SessionStart skill catalog must list preserve-existing-flow"
        );
        assert!(
            context.contains("reviewer"),
            "SessionStart skill catalog must list the reviewer skill"
        );

        // Runtime memory pointer.
        assert!(
            context.contains("Workspace memory system map"),
            "SessionStart must include the runtime memory pointer"
        );

        // Memory-writes section. Auto-refresh on PreCompact/SessionEnd
        // covers SYSTEM_MAP only; working-brief writes are still on the
        // agent. The bootstrap skill teaches when to call the four real
        // memory subcommands; this assertion guards that block from being
        // silently deleted in a future edit.
        assert!(
            context.contains("Memory writes (when you learn something durable)"),
            "SessionStart must embed the memory-writes instruction block"
        );
        assert!(
            context.contains("claude-skills memory working-brief write"),
            "SessionStart memory-writes block must name the working-brief write surface"
        );
        assert!(
            context.contains("claude-skills memory completion-gate check"),
            "SessionStart memory-writes block must name the completion-gate probe"
        );
    }

    #[test]
    fn stop_hook_uses_top_level_system_message_not_hook_specific_output() {
        for subcommand in [
            "stop",
            "subagent-stop",
            "session-start",
            "session-end",
            "notification",
            "permission-request",
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let code = run_hook_lifecycle(subcommand, &mut stdout, &mut stderr);

            if stdout.is_empty() {
                continue;
            }

            assert_eq!(
                code,
                0,
                "stderr for {subcommand}: {}",
                String::from_utf8_lossy(&stderr)
            );

            let output: JsonDocument = serde_json::from_slice(&stdout)
                .unwrap_or_else(|error| panic!("invalid JSON for {subcommand}: {error}"));

            assert!(
                output.get("hookSpecificOutput").is_none(),
                "{subcommand} must not emit hookSpecificOutput — Claude Code only allows it for PreToolUse/UserPromptSubmit/PostToolUse/PostToolBatch"
            );

            assert!(
                output
                    .get("systemMessage")
                    .and_then(JsonDocument::as_str)
                    .is_some(),
                "{subcommand} must emit systemMessage as a top-level string"
            );
        }
    }

    #[test]
    fn diagnose_reports_healthy_when_settings_point_at_installed_executable() {
        let claude_home = std::env::temp_dir().join(format!(
            "claude-skills-diagnose-healthy-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).unwrap();

        let executable = crate::runtime::installed_executable_path(&claude_home);
        std::fs::write(&executable, b"installed").unwrap();

        let pre_tool_command = hook_command_for_executable_args(&executable, "hook pre-tool-use");
        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &pre_tool_command).unwrap();
        std::fs::write(&settings_path, &payload).unwrap();

        let report = collect_hook_diagnostics(&claude_home);

        assert!(
            report.healthy(),
            "expected healthy diagnose, got {report:?}"
        );
        assert_eq!(report.settings_parses, Some(true));
        assert_eq!(report.settings_points_at_installed, Some(true));
        assert!(report.orphan_executable_siblings.is_empty());

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn diagnose_flags_settings_pointing_at_wrong_executable() {
        let claude_home = std::env::temp_dir().join(format!(
            "claude-skills-diagnose-mismatch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).unwrap();

        let executable = crate::runtime::installed_executable_path(&claude_home);
        std::fs::write(&executable, b"installed").unwrap();

        // settings.json points at a different binary (the historical
        // ~/.claude/claude-skills.exe.stale-* leakage shape, where the hook
        // was registered against an old path that no longer exists).
        let other_path = claude_home
            .join("elsewhere")
            .join(crate::runtime::executable_file_name());
        std::fs::create_dir_all(other_path.parent().unwrap()).unwrap();
        let other_command = hook_command_for_executable_args(&other_path, "hook pre-tool-use");
        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &other_command).unwrap();
        std::fs::write(&settings_path, &payload).unwrap();

        let report = collect_hook_diagnostics(&claude_home);

        assert!(!report.healthy(), "expected unhealthy diagnose");
        assert_eq!(report.settings_points_at_installed, Some(false));

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn diagnose_flags_orphan_siblings_as_unhealthy() {
        let claude_home = std::env::temp_dir().join(format!(
            "claude-skills-diagnose-orphan-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).unwrap();

        let executable = crate::runtime::installed_executable_path(&claude_home);
        std::fs::write(&executable, b"installed").unwrap();

        let pre_tool_command = hook_command_for_executable_args(&executable, "hook pre-tool-use");
        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &pre_tool_command).unwrap();
        std::fs::write(&settings_path, &payload).unwrap();

        // Drop a legacy stale sibling.
        let orphan = executable.with_file_name(format!(
            "{}.stale-1778857819",
            crate::runtime::executable_file_name()
        ));
        std::fs::write(&orphan, b"legacy").unwrap();

        let report = collect_hook_diagnostics(&claude_home);

        assert!(!report.healthy(), "orphan sibling must mark unhealthy");
        assert_eq!(report.orphan_executable_siblings.len(), 1);

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn diagnose_text_output_lists_failures() {
        let claude_home = std::env::temp_dir().join(format!(
            "claude-skills-diagnose-text-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).unwrap();

        // No installed executable, no settings.json — every check fails.
        let report = collect_hook_diagnostics(&claude_home);
        let mut output = Vec::new();
        report.render_text(&mut output);
        let rendered = String::from_utf8(output).unwrap();

        assert!(rendered.contains("[FAIL]"));
        assert!(rendered.contains("issues found"));

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn stop_and_subagent_stop_short_circuit_at_dispatch() {
        // Stop and SubagentStop must always exit 0 with empty output, even if
        // a future change to lifecycle_additional_context, the JSON renderer,
        // or the rendering path itself would otherwise emit text or fail.
        // run_hook_command short-circuits these events before they reach
        // run_hook_lifecycle so no downstream regression can re-introduce the
        // stop-cascade bug (Claude Code re-runs the turn on a non-zero Stop
        // exit, which loops).
        for subcommand in ["stop", "subagent-stop"] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let code = run_hook_command(&[subcommand.to_string()], &mut stdout, &mut stderr);

            assert_eq!(
                code,
                0,
                "{subcommand} must always exit 0; stderr: {}",
                String::from_utf8_lossy(&stderr)
            );
            assert!(
                stdout.is_empty(),
                "{subcommand} must emit no stdout; got: {}",
                String::from_utf8_lossy(&stdout)
            );
            assert!(
                stderr.is_empty(),
                "{subcommand} must emit no stderr; got: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
    }

    #[test]
    fn memory_key_sanitization_matches_scope_command_shape() {
        let key = sanitize_memory_key(r#"C:\Users\riezh\OneDrive\Documents\test\claude_core"#);

        assert_eq!(key, "c-users-riezh-onedrive-documents-test-claude-core");
    }

    fn expected_managed_command_for_event(event: &str, pre_tool_command: &str) -> String {
        if event == "PreToolUse" {
            return pre_tool_command.to_string();
        }

        managed_lifecycle_command(crate::hooks::claude::lifecycle_subcommand(event))
    }

    fn temp_hook_path(name: &str) -> PathBuf {
        let unique = format!("{}-{}", name, std::process::id());

        std::env::temp_dir().join(unique).join("settings.json")
    }

    #[test]
    fn install_writes_default_skill_listing_budget_fraction() {
        let hook_path = temp_hook_path("claude-skills-skill-budget-default");
        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let pre_tool_command = managed_hook_command().unwrap();
        let rendered = build_hooks_payload(&hook_path, &pre_tool_command).unwrap();
        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        assert_eq!(
            document
                .get("skillListingBudgetFraction")
                .and_then(JsonDocument::as_f64),
            Some(0.02),
        );

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn install_preserves_user_skill_listing_budget_fraction() {
        let hook_path = temp_hook_path("claude-skills-skill-budget-preserve");
        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
        std::fs::write(
            &hook_path,
            r#"{"hooks": {}, "skillListingBudgetFraction": 0.05}"#,
        )
        .unwrap();

        let pre_tool_command = managed_hook_command().unwrap();
        let rendered = build_hooks_payload(&hook_path, &pre_tool_command).unwrap();
        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        assert_eq!(
            document
                .get("skillListingBudgetFraction")
                .and_then(JsonDocument::as_f64),
            Some(0.05),
        );

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }
}
