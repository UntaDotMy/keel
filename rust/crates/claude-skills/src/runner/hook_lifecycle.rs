//! Purpose: Claude Code hook lifecycle management, installation, and removal.
//! Caller: runner/mod.rs for hook command group.
//! Dependencies: std::collections::BTreeMap, std::fs, std::path, serde_json, crate::runtime.
//! Main Functions: run_hook_command, build_hooks_payload, remove_managed_hooks.
//! Side Effects: Reads and writes Claude Code hooks.json configuration.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonDocument};

use crate::args::FlagSet;
use crate::hooks::claude::{event_by_name, event_by_slug, HookEvent, HOOK_EVENTS};
use crate::json::{write_indented, Value};
use crate::proxy::raw_store::RawStore;
use crate::runner::shell_rewrite::{rewrite_command_text_for_shell, RewriteShell};
use crate::runner::tool_timings;
use crate::runner::{learning, observation};
use crate::runtime::{display_path, installed_executable_path, resolve_claude_home, write_text};
use crate::utility;

const RAW_OUTPUT_DEFAULT_RETENTION_DAYS: u64 = 14;

/// Tool-timings JSONL rows are tiny (one short line per tool call) compared
/// to raw-output directories, so a longer default retention is fine. 30 days
/// gives an analyzer a useful month-long sample without letting the directory
/// grow unbounded across long sessions. Tunable via
/// `CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS`; setting it to `0` disables the
/// SessionEnd prune.
const TIMINGS_DEFAULT_RETENTION_DAYS: u64 = 30;

/// Behavioral observation JSONL rows feed the learning loop. They age out of
/// the loop's 7-day distillation window naturally, but the files are pruned on
/// a longer horizon so a late `learn --window 14` inspection still has data.
/// Tunable via `CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS`; `0` disables.
const OBSERVATION_DEFAULT_RETENTION_DAYS: u64 = 30;

const MANAGED_PRE_TOOL_USE_EVENT: &str = "PreToolUse";

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

        // PostToolUseFailure carries the same `duration_ms` field PostToolUse
        // does (CC 2.1.119) and we want failed tool timings on the same JSONL
        // as successes so an analyzer can compare them. The event has no
        // additionalContext to inject, so we own dispatch here too rather
        // than letting the lifecycle wildcard route it through an empty
        // context render. Keeping the slug in the canonical event table is
        // still important — that table is the dispatch invariant from the
        // earlier `Unknown hook command: post-tool-use-failure` regression.
        "post-tool-use-failure" => run_hook_post_tool_use_failure(standard_error),

        // Stop and SubagentStop must never return a non-zero exit code. Claude Code
        // treats a failing Stop hook as a signal to re-run the turn, which cascades
        // into a stop loop. lifecycle_additional_context already returns empty
        // string for these events, but routing them through run_hook_lifecycle
        // leaves a regression surface — any future change that introduces context,
        // mishandles serde, or panics could re-introduce the cascade. Short-circuit
        // here so no downstream change can accidentally bring back the bug.
        "stop" | "subagent-stop" => 0,

        // Notification fires when Claude Code wants the user's attention
        // (permission prompt, idle reminder). CC 2.1.141 added the
        // `terminalSequence` field to hook JSON output for exactly this case
        // — emitting bells/desktop notifications without a controlling
        // terminal. We ring the BEL so the user hears it even when the
        // terminal is in the background. Notification is documented as
        // top-level-only (no hookSpecificOutput), so we own dispatch here
        // rather than going through the lifecycle path.
        "notification" => run_hook_notification(standard_output),

        // UserPromptSubmit reads the same stdin payload Claude Code delivers to
        // PreToolUse so we can read `session_id` and apply the optional
        // compression-discipline nudge when that session has accumulated enough
        // tool-timings rows to suggest the context window is filling. The
        // sibling function owns stdin parsing; the slug-only `run_hook_lifecycle`
        // path stays the test surface for every event that does NOT need stdin.
        // Stdin is injected explicitly so tests can pass `&mut std::io::empty()`
        // to avoid blocking when cargo's parent process holds an open console
        // handle (real symptom on Windows under PowerShell).
        "user-prompt-submit" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_user_prompt_submit(&mut stdin, standard_output, standard_error)
        }

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

    let executable = match resolve_current_executable() {
        Ok(path) => path,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_payload = match build_hooks_payload(&hook_path, &executable) {
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

    // Record `duration_ms` for every tool, not just edit-class ones. CC 2.1.119
    // documents the field as "tool execution time, excluding permission
    // prompts and PreToolUse hooks", and we want a uniform sample so a slow
    // Bash or Read shows up alongside slow Edits. Errors (disk full, perm)
    // log to stderr and are swallowed — telemetry must never fail the hook.
    if let Err(error) = tool_timings::record_tool_timing("PostToolUse", &input) {
        let _ = writeln!(
            standard_error,
            "claude-skills post-tool-use: tool-timings record failed: {error}"
        );
    }

    // Capture a behavioral observation for the autonomous learning loop. This
    // is the signal `learning::run_learning_cycle` distills into instincts and,
    // once a pattern is trusted, into a generated skill. Like the timing record
    // above, any failure is logged and swallowed — learning capture must never
    // fail the hook.
    if let Err(error) = observation::record_observation(&input) {
        let _ = writeln!(
            standard_error,
            "claude-skills post-tool-use: observation record failed: {error}"
        );
    }

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

/// PostToolUseFailure handler.
///
/// PostToolUseFailure (CC 2.1.119+) carries the same `duration_ms` field as
/// PostToolUse so we can see how long failing tool calls took before they
/// errored. The handler reads stdin, records the timing alongside the
/// success entries, and returns 0. No edit-counter touch — a failing tool
/// call did not change files, so nudging the SYSTEM_MAP refresh would be
/// noise.
///
/// Like PostToolUse, this handler must never fail the hook: any I/O or
/// parse error is logged to stderr and swallowed.
fn run_hook_post_tool_use_failure(standard_error: &mut dyn Write) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "claude-skills post-tool-use-failure: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "claude-skills post-tool-use-failure: unable to decode hook input: {error}"
            );

            return 0;
        }
    };

    if let Err(error) = tool_timings::record_tool_timing("PostToolUseFailure", &input) {
        let _ = writeln!(
            standard_error,
            "claude-skills post-tool-use-failure: tool-timings record failed: {error}"
        );
    }

    0
}

/// Notification handler.
///
/// CC 2.1.141 added a `terminalSequence` top-level field to hook JSON output
/// so hooks can emit desktop notifications, window titles, and bells without
/// a controlling terminal. The Notification event fires when Claude Code
/// raises a permission prompt or an idle "needs your attention" cue, so it
/// is the natural place to ring the BEL. Allowed payload per the docs is
/// OSC 0/1/2/9/99/777 and BEL — `\u{0007}` is the BEL and is in the
/// allowlist. `suppressOutput` keeps the transcript clean.
///
/// The handler is input-agnostic: the JSON output is the same regardless of
/// what stdin contains, so we don't read it. Claude Code does not require
/// the hook to drain the pipe.
fn run_hook_notification(standard_output: &mut dyn Write) -> u8 {
    let _ = writeln!(standard_output, "{NOTIFICATION_BELL_OUTPUT}");

    0
}

/// Hook JSON emitted by Notification. BEL is in the CC 2.1.141
/// `terminalSequence` allowlist and is JSON-escaped as `\u0007` per
/// RFC 8259 (control characters U+0000–U+001F MUST be escaped inside a
/// JSON string). Claude Code unescapes the value before writing it to the
/// terminal, which is what produces the audible bell. `suppressOutput`
/// hides this row from the transcript so the bell is the only side effect.
const NOTIFICATION_BELL_OUTPUT: &str = "{\"suppressOutput\":true,\"terminalSequence\":\"\\u0007\"}";

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
        prune_tool_timings_store(standard_error);
        prune_observations_store(standard_error);
        run_session_end_learning(standard_error);
    }

    let context = lifecycle_additional_context(event.slug);

    if context.trim().is_empty() {
        return 0;
    }

    // Whether this event accepts `hookSpecificOutput.additionalContext`
    // or must fall back to a top-level `systemMessage` lives on the event
    // row, so adding a new event to the table automatically picks up the
    // right schema.
    let payload = render_lifecycle_payload(event, &context);

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

/// Wrap `context` in the JSON payload Claude Code expects for `event`.
///
/// Events whose schema accepts `hookSpecificOutput.additionalContext` get
/// the per-event shape; everything else falls back to top-level
/// `systemMessage`. The split lives on the event row so adding a new event
/// to `HOOK_EVENTS` automatically picks up the right schema.
///
/// Pulled out of `run_hook_lifecycle` so tests can exercise both branches
/// without setting up the surrounding side-effects (system map refresh,
/// raw-output prune). The previous in-line shape was effectively
/// untestable: the only events whose `lifecycle_additional_context`
/// returned non-empty all had `supports_hook_specific_output: true`, so
/// the `systemMessage` branch was dead code in tests and a regression
/// could have shipped silently.
pub(crate) fn render_lifecycle_payload(event: &HookEvent, context: &str) -> JsonDocument {
    if event.supports_hook_specific_output {
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
    }
}

/// Look up the PascalCase event name for a kebab slug. Used by callers that have a
/// slug in hand but need to reason in Claude Code's PascalCase vocabulary.
fn lifecycle_additional_context(subcommand: &str) -> String {
    match subcommand {
        "session-start" => session_start_context(),

        "pre-compact" => pre_compact_context(),

        "post-compact" => post_compact_context(),

        // UserPromptSubmit is intercepted before this match in `run_hook_command`
        // by the dedicated `run_hook_user_prompt_submit` dispatcher, which reads
        // stdin to extract `session_id` and applies the optional
        // compression-discipline nudge. Do NOT add an arm for "user-prompt-submit"
        // here — if the dedicated dispatcher is ever removed by mistake the
        // missing arm will surface as a hard test failure rather than silently
        // falling back to a stdin-blind path that drops the nudge.

        // PostToolBatch fires after a batch of parallel tools resolves, just
        // before the next model turn. We inject a reviewer-on-close reminder
        // here because Stop/SubagentStop are documented with top-level
        // decision fields only — they do not accept additionalContext per
        // the official Claude Code hooks schema. The reminder is portable:
        // it states the trivial/non-trivial split inline so it works in any
        // host repo, and treats project-level CLAUDE.md/AGENTS.md as an
        // optional override rather than a required citation.
        "post-tool-batch" => post_tool_batch_context(),

        // Silenced events. Stop/SubagentStop/SessionEnd fire per turn end and
        // the schema rejects context injection on them. PostToolUse and
        // PostToolUseFailure are owned by their dedicated dispatch arms in
        // run_hook_command (they record duration_ms via tool_timings), so the
        // lifecycle path returns empty for them too — the explicit listing
        // is a documentation receipt, not a behaviour change.
        "stop" | "subagent-stop" | "session-end" | "post-tool-use" | "post-tool-use-failure" => {
            String::new()
        }

        _ => String::new(),
    }
}

/// Bootstrap skill text embedded at compile time.
///
/// The file lives at the repository root so `discover_repository_layout` picks
/// it up alongside the other skills and `sync_skills` installs it under
/// `~/.claude/skills/using-claude-core/SKILL.md`. We *also* embed it here so
/// SessionStart can inject the full text directly into
/// `hookSpecificOutput.additionalContext` per the official Claude Code hooks
/// schema. CLAUDE.md and the individual SKILL.md files are read by the skill
/// matcher on demand; this single block is what the model sees up front, so
/// it doubles as the research-first iron law and the catalog of every other
/// invokable skill.
const BOOTSTRAP_SKILL: &str = include_str!("../../../../../using-claude-core/SKILL.md");

fn session_start_context() -> String {
    // SessionStart fires once per session and is the documented entry point
    // for delivering durable model context via
    // `hookSpecificOutput.additionalContext`. Per-prompt token cost is paid
    // at most once per session, so this is the right place to deliver the
    // bootstrap skill instead of restating it on every UserPromptSubmit.
    //
    // Layout: full bootstrap skill (iron law + Red Flags + skill catalog +
    // workspace pointers), the runtime-resolved memory pointer that CLAUDE.md
    // cannot know in advance, the learned-instinct digest for the current
    // project (the always-on tier of the learning loop — what the user
    // reliably does here, surfaced without waiting for a skill match), and an
    // autonomous synthesis nudge so a freshly generated skill's deterministic
    // template gets upgraded to richer prose in the normal course of work
    // (no manual slash). The nudge self-clears once the agent refines the skill,
    // because the content-hash no-clobber guard then reports it as non-template.
    let mut context = format!("{BOOTSTRAP_SKILL}\n\n{}", memory_scope_summary());
    if let (Ok(claude_home), Ok(cwd)) = (resolve_claude_home(""), std::env::current_dir()) {
        let cwd = cwd.to_string_lossy();
        let digest = learning::project_instinct_digest(&claude_home, &cwd);
        if !digest.trim().is_empty() {
            context.push_str("\n\n");
            context.push_str(&digest);
        }
        let synthesis = learning::project_synthesis_nudge(&claude_home, &cwd);
        if !synthesis.trim().is_empty() {
            context.push_str("\n\n");
            context.push_str(&synthesis);
        }
    }
    context
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
/// every byte lands per prompt and is paid as input tokens. The full
/// bootstrap (skill catalog, Red Flags table, decision flow, four
/// implementation-discipline pillars) is delivered once via SessionStart;
/// this hook only restates the iron law, names the four pillars, adds the
/// understand-before-building rule (research the request before writing code —
/// the lever that stops the model building the wrong thing), and the one-line
/// parallel-fan-out independence test so they stay top-of-mind on each turn.
/// Body weight is roughly 260 tokens before `memory_scope_summary()` — within
/// budget for a per-prompt injection but expensive enough that adding more
/// text needs a deliberate reason.
fn user_prompt_submit_context() -> String {
    format!(
        "Research-first: trust the codebase, not your knowledge base. Read SYSTEM_MAP and the owning module before claiming behavior. Invoke any relevant skill via the Skill tool BEFORE responding — even a 1% chance it applies means use it. Understand before building: restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code — no guessing, no assuming, no building against an imagined spec. Researching first is what stops you building the wrong thing; the cost of an hour's research is always less than the cost of shipping the wrong feature. Find the root cause, not just the surface symptom: suspicion is a hypothesis, not a finding — trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing it. No assumptions. No jumping from \"this may be the case\" to a patch. Implementation discipline applies on every code-touching turn — Think Before Coding (state assumptions, deep-dive any suspected target before changing it), Simplicity First (minimum code, no speculative features or abstractions), Surgical Changes (every changed line traces to the request), Goal-Driven Execution (reproduce or trace the symptom before naming a root cause; turn the task into a verifiable goal before coding). Parallel fan-out: only batch agents in the same message when all four hold — no shared inputs, no shared file or git-index writes, no need to cancel/steer one based on another's interim result, and the work fits the current task scope. If any check fails, dispatch sequentially. {}",
        memory_scope_summary()
    )
}

/// Concrete per-prompt skill guidance. Emitted only when the prompt
/// distinctively matches one installed skill (see
/// `utility::skill_match::match_skill_for_prompt`).
///
/// Two parts, deliberately ordered:
///   1. A one-line header naming the matched skill and the `Skill("<name>")`
///      call that loads its full body.
///   2. The skill's *own* bounded brief (`brief`) — its description plus the
///      opening of its body. This is the model-independence fix: the operative
///      guidance is injected as input context for this turn, so it lands even
///      if the gateway model never makes the `Skill()` call. Earlier this hook
///      only asked the model to call `Skill()`; whether the skill loaded then
///      depended entirely on the model honoring that instruction.
fn skill_pointer_text(skill_name: &str, brief: &str) -> String {
    format!(
        "Skill match: this prompt strongly matches the `{skill_name}` skill. Its guidance is inlined below and applies now — follow it before writing code or giving a final answer. For the complete skill, call Skill(\"{skill_name}\"). If, after reading, the skill turns out not to apply, say so and proceed.\n\n--- begin {skill_name} skill brief ---\n{brief}\n--- end {skill_name} skill brief ---"
    )
}

/// Fallback per-prompt skill pointer used when the matched skill's body cannot
/// be read for inlining. Names the skill and the exact `Skill("<name>")` call so
/// the model still gets an actionable instruction, even though the brief itself
/// is unavailable this turn.
fn skill_pointer_fallback(skill_name: &str) -> String {
    format!(
        "Skill match: this prompt strongly matches the `{skill_name}` skill. Invoke it now with Skill(\"{skill_name}\") BEFORE writing code or giving a final answer. If, after reading it, the skill turns out not to apply, say so and proceed — but do not skip the check."
    )
}

/// UserPromptSubmit dispatcher that reads stdin and composes the per-prompt
/// `additionalContext`.
///
/// Claude Code delivers a JSON payload on stdin for this event with at least
/// `session_id`, `transcript_path`, `cwd`, and `prompt`. We use `session_id`
/// to read today's tool-timings JSONL and decide whether enough tool calls
/// have already happened in this session to merit the compression-discipline
/// nudge. Every failure path (no stdin, unparseable stdin, missing session id,
/// missing JSONL, no claude_home) falls back to the unchanged base text so
/// the existing back-compat test keeps passing and a hook misconfiguration
/// can never break the per-prompt injection.
fn run_hook_user_prompt_submit(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    // SessionStart already refreshed the system map at session boot; on every
    // prompt afterwards `should_refresh_system_map("UserPromptSubmit")` is
    // false, so this dispatcher does not duplicate that work.
    //
    // `standard_input` is injected by the caller — production passes a locked
    // stdin handle (which Claude Code closes after writing the JSON payload),
    // tests pass `std::io::empty()` so the read returns EOF immediately
    // instead of blocking on an inherited console handle.
    let stdin_payload: Option<JsonDocument> = {
        let mut text = String::new();
        match standard_input.read_to_string(&mut text) {
            Ok(_) if !text.trim().is_empty() => serde_json::from_str(&text).ok(),
            _ => None,
        }
    };

    let session_id = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("session_id"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    // The prompt text Claude Code delivers on stdin. This is what lets the
    // per-prompt nudge name the *specific* matching skill instead of repeating
    // the generic "invoke any relevant skill" reminder every turn. Absent or
    // empty prompt → no skill pointer, just the base context (fail-open).
    let prompt_text = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("prompt"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    let claude_home = resolve_claude_home("").ok();

    let mut base_context = user_prompt_submit_context();
    // Prepend the matched skill's own guidance when the prompt distinctively
    // matches one installed skill. Placed first so it is the first thing the
    // model reads. Conservative by construction: `match_skill_for_prompt` stays
    // silent for generic or ambiguous prompts (see utility::skill_match), so
    // this never mis-routes — it only sharpens an already-clear signal.
    //
    // Inline the skill's *actual brief* rather than only asking for a
    // `Skill()` call: injected context is consumed by the model as input
    // regardless of whether it honors the tool-call instruction, so the
    // guidance lands even behind a gateway model that ignores `Skill()`. Fall
    // back to the bare pointer only when the body cannot be read.
    if let (false, Some(home)) = (prompt_text.trim().is_empty(), claude_home.as_ref()) {
        if let Some(matched) =
            crate::utility::skill_match::match_skill_for_prompt(home, prompt_text)
        {
            let pointer = match crate::utility::skill_match::skill_inline_brief(home, &matched.name)
            {
                Some(brief) => skill_pointer_text(&matched.name, &brief),
                None => skill_pointer_fallback(&matched.name),
            };
            base_context = format!("{pointer}\n\n{base_context}");
        }
    }

    let final_context = match (session_id.is_empty(), claude_home.as_ref()) {
        (false, Some(home)) => match maybe_compression_hint(home, session_id) {
            Some(hint) => format!("{base_context}\n\n{hint}"),
            None => base_context,
        },
        _ => append_compression_hint_when_forced(base_context),
    };

    let event = match event_by_name("UserPromptSubmit") {
        Some(row) => row,
        None => {
            let _ = writeln!(
                standard_error,
                "UserPromptSubmit row missing from canonical event table"
            );
            return 1;
        }
    };

    let payload = render_lifecycle_payload(event, &final_context);

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

/// Per-session compression-discipline nudge.
///
/// Returns `Some(text)` when the heuristic decides this turn would benefit
/// from a reminder to compress tool output, or `None` to leave the per-prompt
/// payload unchanged.
///
/// Heuristic (deterministic):
///   * `CLAUDE_SKILLS_COMPRESSION_HINT=off`  -> always None
///   * `CLAUDE_SKILLS_COMPRESSION_HINT=force` -> always Some
///   * Otherwise: Some when this session has recorded at least
///     `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER` tool-timings rows in today's
///     JSONL (default 40), None below that threshold.
///
/// Telemetry rule: any read failure (no JSONL, unreadable file, malformed
/// rows) returns None silently. A telemetry hiccup must never fail the hook.
fn maybe_compression_hint(claude_home: &Path, session_id: &str) -> Option<&'static str> {
    match std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("off") => return None,
        Some("force") => return Some(compression_hint_text()),
        _ => {}
    }

    if session_id.is_empty() {
        return None;
    }

    let threshold = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(COMPRESSION_HINT_DEFAULT_THRESHOLD);
    if threshold == 0 {
        return None;
    }

    let row_count = count_session_tool_timing_rows(claude_home, session_id);
    if row_count >= threshold {
        Some(compression_hint_text())
    } else {
        None
    }
}

/// Honor `CLAUDE_SKILLS_COMPRESSION_HINT=force` even when stdin or
/// claude_home are unavailable so test scaffolding and operators can demand
/// the nudge for diagnostic runs without populating a real JSONL.
fn append_compression_hint_when_forced(base_context: String) -> String {
    match std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("force") => format!("{base_context}\n\n{}", compression_hint_text()),
        _ => base_context,
    }
}

/// Default threshold of 40 tool calls is calibrated against the per-day
/// tool-timings JSONL: a heavy investigation session typically logs 60-100
/// rows, so 40 fires the hint roughly halfway through and gives the model
/// real budget headroom for the back half of the work. Operators can tune
/// via `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER`; setting it to 0 disables.
const COMPRESSION_HINT_DEFAULT_THRESHOLD: usize = 40;

/// The compression-discipline reminder. Three concrete actions, ~50 tokens.
///
/// Compact by design: this lands per-prompt in addition to the existing
/// research-first iron law text. Token cost matters. Keep it surgical and
/// actionable.
fn compression_hint_text() -> &'static str {
    "Output compression is on for this turn — context is heavy. Read narrower line ranges (offset+limit) instead of whole files. Search before reading: use Grep/Glob to locate the exact symbol, then Read only the relevant window. Summarize logs and command output instead of pasting them in full. Skill: compression-discipline."
}

/// Count tool-timings JSONL rows for `session_id` recorded today. Returns 0
/// for any failure (missing file, unreadable, malformed lines). Each
/// matching row counts once; non-matching rows and parse errors are
/// silently skipped so a single corrupt line cannot poison the count.
fn count_session_tool_timing_rows(claude_home: &Path, session_id: &str) -> usize {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let body = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<JsonDocument>(line).ok())
        .filter(|row| {
            row.get("session_id")
                .and_then(JsonDocument::as_str)
                .map(|recorded| recorded == session_id)
                .unwrap_or(false)
        })
        .count()
}

/// Reviewer-on-close reminder.
///
/// Fires after every batch of parallel tool calls, just before the model's
/// next turn. We surface the question rather than gating with
/// `decision: "block"`, which would force a review on every tool batch
/// including read-only research.
///
/// `claude-skills` installs globally, so this hook fires in every host
/// repo — most of which have no `CLAUDE.md`, no `AGENTS.md`, and no
/// `reviewer` skill. The text therefore states the trivial/non-trivial
/// split inline so the rule is self-contained in any project, and treats
/// project-level convention files as an optional override rather than a
/// required citation. We still pre-empt the "wrapper noise" rationalization
/// because models that pattern-match generic reminders as noise have
/// rationalized past prior versions of this text.
fn post_tool_batch_context() -> String {
    "Closeout check: if this batch changed code with logic edits, multi-file changes, public-API touches, or security-sensitive surfaces, route the diff through a reviewer pass before final closeout. Trivial work (docs-only, formatting-only, single-line typo or comment fixes, generated-only) is exempt. The standard is: non-trivial code does not self-review. If a project-level CLAUDE.md or AGENTS.md defines stricter routing rules, those take precedence. If this reminder feels like wrapper noise, that is the rationalization the rule names — re-read the diff and decide deliberately before skipping.".to_string()
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

/// Drop tool-timings JSONL rows older than the configured retention.
///
/// Mirrors `prune_raw_output_store` in shape: SessionEnd-only, env var
/// override, swallow errors so a telemetry housekeeping failure cannot
/// fail the hook. The store is per-day JSONL files under
/// `<claude_home>/state/tool-timings/`; the prune helper in
/// `tool_timings::prune_older_than` parses the date out of each filename
/// and removes files older than the cutoff.
///
/// `pub(crate)` so the env-var override and `retention=0` disable paths
/// are exercisable from the `tool_timings` module's existing isolated
/// `with_isolated_claude_home` test harness without duplicating the
/// `CLAUDE_TARGET_OVERRIDE` plumbing here.
pub(crate) fn prune_tool_timings_store(standard_error: &mut dyn Write) {
    let retention_days = std::env::var("CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(TIMINGS_DEFAULT_RETENTION_DAYS);
    if retention_days == 0 {
        return;
    }
    if let Err(error) = tool_timings::prune_older_than(retention_days) {
        let _ = writeln!(
            standard_error,
            "claude-skills tool-timings prune failed: {error}"
        );
    }
}

/// Drop behavioral observation JSONL rows older than the configured retention.
/// SessionEnd-only, env-var override, errors swallowed — same housekeeping
/// contract as the timings and raw-output prunes.
fn prune_observations_store(standard_error: &mut dyn Write) {
    let retention_days = std::env::var("CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(OBSERVATION_DEFAULT_RETENTION_DAYS);
    if retention_days == 0 {
        return;
    }
    if let Err(error) = observation::prune_older_than(retention_days) {
        let _ = writeln!(
            standard_error,
            "claude-skills observation prune failed: {error}"
        );
    }
}

/// Run the autonomous learning cycle at session end: distill the session's
/// observations into instincts and evolve trusted clusters into generated
/// skills. Fully automatic — no slash command. Set
/// `CLAUDE_SKILLS_LEARNING=off` to disable. Errors are swallowed so a learning
/// failure can never fail the SessionEnd hook.
fn run_session_end_learning(standard_error: &mut dyn Write) {
    if std::env::var("CLAUDE_SKILLS_LEARNING")
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(_) => return,
    };
    let report = learning::run_learning_cycle(
        &claude_home,
        &learning::CycleOptions::default(),
        standard_error,
    );
    if report.skills_generated > 0 || report.agents_generated > 0 {
        let _ = writeln!(
            standard_error,
            "claude-skills learn: recorded {} instinct(s), generated {} skill(s) and {} agent(s)",
            report.instincts_recorded, report.skills_generated, report.agents_generated
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
    // Build the slug list straight from HOOK_EVENTS so the help line
    // cannot drift from the dispatch table. The earlier hard-coded list
    // shipped with 14 missing slugs (post-tool-batch, user-prompt-expansion,
    // setup, file-changed, ...) that dispatched correctly but were
    // invisible to anyone who ran `claude-skills hook` to learn what was
    // available. Pulling from the table makes "advertised == dispatched"
    // a structural property.
    let admin_verbs = [
        "install",
        "uninstall",
        "list",
        "show",
        "instructions",
        "diagnose",
    ];
    let event_slugs: Vec<&'static str> = HOOK_EVENTS.iter().map(|event| event.slug).collect();
    let joined = admin_verbs
        .iter()
        .copied()
        .chain(event_slugs)
        .collect::<Vec<_>>()
        .join("|");

    let _ = writeln!(standard_output, "Usage: claude-skills hook [{joined}]");
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
                if !is_managed_hook_entry(command_entry) {
                    continue;
                }
                // Unreachable for any entry that passed the gate above: both
                // the args-form and legacy-string-form detectors require
                // `command` to be a present string. Kept as a defensive skip
                // so a future entry shape (e.g. one that ships only `args`)
                // never panics here, and so `managed_seen` doesn't get flipped
                // on an entry the doctor can't actually reason about.
                let Some(command) = command_entry.get("command").and_then(JsonDocument::as_str)
                else {
                    continue;
                };
                managed_seen = true;
                let command_normalized = casefold(command);
                // Legacy single-string PowerShell-encoded entries embed the
                // path inside a base64 UTF-16 LE script. Decode and match
                // case-insensitively so doctor recognizes upgrades from older
                // claude-skills versions that haven't been re-installed yet.
                // Args-form entries store the path directly in `command`, so
                // the plain match below covers them without needing decode.
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

/// One managed hook entry as Claude Code's `args` exec form (added in CC 2.1.139).
///
/// `command` is the bare executable path; `args` is the argv that follows. Claude
/// Code spawns the binary directly without going through a shell, so neither
/// field needs shell quoting. Per code.claude.com/docs/en/hooks the `args` form
/// supersedes the historical single-string `command` shape that required
/// platform-specific quoting (PowerShell `-EncodedCommand` on Windows, shell
/// quoting on POSIX).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedHookEntry {
    pub command: String,
    pub args: Vec<String>,
}

/// Build the args-form managed hook entry for `slug` against `executable`.
///
/// The result drops straight into settings.json under
/// `hooks[<event>][N].hooks[0]` once `type` and `statusMessage` are added by
/// the caller.
pub fn managed_hook_entry(executable: &Path, slug: &str) -> ManagedHookEntry {
    ManagedHookEntry {
        command: display_path(executable),
        args: vec!["hook".to_string(), slug.to_string()],
    }
}

fn resolve_current_executable() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("resolve current executable: {error}"))
}

/// Human-readable summary of what `claude-skills hook install` writes for the
/// PreToolUse event. Used by `claude-skills hook diagnose` to surface the
/// expected hook command in JSON output. Not a shell-runnable string — just a
/// diagnostic.
pub fn managed_hook_command() -> Result<String, String> {
    resolve_current_executable().map(|path| {
        let entry = managed_hook_entry(&path, "pre-tool-use");
        format!("{} {}", entry.command, entry.args.join(" "))
    })
}

pub fn build_hooks_payload(hook_path: &Path, executable: &Path) -> Result<String, String> {
    let mut document = read_hooks_document(hook_path)?;

    ensure_hooks_object(&mut document)?;

    remove_managed_hooks(&mut document);

    append_managed_hooks(&mut document, executable)?;

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

/// Strip the managed hook stanzas from `<claude_home>/settings.json`, writing
/// the file back only when something changed. Used by `manager` uninstall so a
/// full uninstall does not leave Claude Code firing hooks at a deleted binary.
/// A missing settings file is a no-op (nothing to clean), not an error.
pub fn remove_managed_hook_payload_for_home(claude_home: &Path) -> Result<bool, String> {
    let hook_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let (payload, removed) = remove_managed_hook_payload(&hook_path)?;
    if removed {
        write_text(&hook_path, &payload)?;
    }
    Ok(removed)
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

            commands.retain(|command_entry| !is_managed_hook_entry(command_entry));
        }

        entries.retain(|matcher_entry| {
            matcher_entry
                .get("hooks")
                .and_then(JsonDocument::as_array)
                .map(|commands| !commands.is_empty())
                .unwrap_or(true)
        });
    }

    // Drop event keys whose array is now empty so a clean uninstall leaves no
    // trace. An event we managed but the user also added a hook to keeps its
    // key (the retain above preserves their non-managed matcher entries); only
    // events that became fully empty after removing our entries are pruned.
    // An empty `"Stop": []` array carries no behavior, so removing it is safe
    // and restores settings.json to its pre-install shape.
    hooks.retain(|_event_name, event_entries| {
        event_entries
            .as_array()
            .map(|entries| !entries.is_empty())
            .unwrap_or(true)
    });
}

fn append_managed_hooks(document: &mut JsonDocument, executable: &Path) -> Result<(), String> {
    let hooks = document
        .get_mut("hooks")
        .and_then(JsonDocument::as_object_mut)
        .ok_or_else(|| "settings.json missing hooks object".to_string())?;

    for event in HOOK_EVENTS {
        // Some events declare themselves not installable into settings.json
        // because their canonical config requires per-repo decisions we
        // can't make at install time (FileChanged in particular: per
        // code.claude.com/docs/en/hooks the matcher value is the watch
        // list, so installing with `matcher: ""` would ship dead config).
        // Dispatch still works for ad-hoc invocations; we just skip the
        // settings stanza.
        if !event.installs_in_settings {
            continue;
        }

        let event_entries = hooks
            .entry(event.name.to_string())
            .or_insert_with(|| JsonDocument::Array(Vec::new()));

        let event_array = event_entries
            .as_array_mut()
            .ok_or_else(|| format!("{} hooks entry is not an array", event.name))?;

        let entry = managed_hook_entry(executable, event.slug);

        event_array.push(serde_json::json!({

            "matcher": event.matcher,

            "hooks": [{

                "type": "command",

                "command": entry.command,

                "args": entry.args,

                "statusMessage": event.status

            }]

        }));
    }

    sort_hook_events(hooks);

    Ok(())
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

/// True if `command_entry` is a managed claude-skills hook (either the modern
/// args-form CC 2.1.139+ or any legacy single-string shape we shipped earlier).
///
/// Detection is permissive on purpose. `claude-skills hook uninstall` runs
/// against arbitrary user settings that may have been written by an older
/// version of this binary, so we accept both shapes:
///
///   1. Args form: `{"command": "<exe>", "args": ["hook", "<slug>"]}` where
///      `<exe>` ends in `claude-skills` (with or without `.exe`) and `<slug>`
///      matches a row in `HOOK_EVENTS`.
///
///   2. Legacy string form: `{"command": "<single-string>"}` where the string
///      mentions `claude-skills` together with `hook <slug>` or
///      `hook instructions --format json`. Windows historically wrapped that
///      string in `powershell.exe -EncodedCommand <base64>`; we decode and
///      retry once so PowerShell-encoded entries from older installs still get
///      cleaned up.
pub fn is_managed_hook_entry(command_entry: &JsonDocument) -> bool {
    if is_managed_args_form(command_entry) {
        return true;
    }

    command_entry
        .get("command")
        .and_then(JsonDocument::as_str)
        .map(is_managed_hook_command)
        .unwrap_or(false)
}

fn is_managed_args_form(command_entry: &JsonDocument) -> bool {
    let Some(command) = command_entry.get("command").and_then(JsonDocument::as_str) else {
        return false;
    };

    if !command_path_is_managed_executable(command) {
        return false;
    }

    let Some(args) = command_entry.get("args").and_then(JsonDocument::as_array) else {
        return false;
    };

    let mut iter = args.iter().filter_map(JsonDocument::as_str);
    let first = iter.next();
    let second = iter.next();

    matches!(first, Some("hook"))
        && second
            .map(|slug| HOOK_EVENTS.iter().any(|event| event.slug == slug))
            .unwrap_or(false)
}

/// True if `command` resolves to the claude-skills binary (case-insensitive
/// basename match — Windows file systems are case-insensitive and the args
/// form embeds the exact path string CC will invoke).
fn command_path_is_managed_executable(command: &str) -> bool {
    let basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase());

    matches!(
        basename.as_deref(),
        Some("claude-skills") | Some("claude-skills.exe")
    )
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

        let rendered =
            build_hooks_payload(&hook_path, Path::new(r"C:\tools\claude-skills.exe")).unwrap();

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

        // Args-form: each managed entry now carries the slug in `args`, not in
        // the command string. The legacy single-string entry that lived in the
        // fixture's PreToolUse stanza must be gone (replaced by our managed
        // args-form entry).
        assert!(rendered.contains("\"pre-tool-use\""));

        assert!(!rendered.contains("hook instructions --format json"));

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn hook_payload_uses_exact_managed_commands_for_each_event() {
        let hook_path = temp_hook_path("claude-skills-hook-command-prefix");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let executable = std::env::current_exe().unwrap();

        let rendered = build_hooks_payload(&hook_path, &executable).unwrap();

        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        let hooks = document
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();

        let expected_command = display_path(&executable);

        for event in HOOK_EVENTS {
            if !event.installs_in_settings {
                // FileChanged (and any future opt-out) is not written to
                // settings.json by `claude-skills hook install`, so the
                // payload won't contain a stanza for it.
                continue;
            }
            let entry = hooks
                .get(event.name)
                .and_then(JsonDocument::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("hooks"))
                .and_then(JsonDocument::as_array)
                .and_then(|commands| commands.first())
                .unwrap_or_else(|| panic!("missing hooks entry for {}", event.name));

            // CC 2.1.139 args exec form: command is the bare executable path,
            // args carries `["hook", <slug>]`, no shell wrapping.
            let command = entry
                .get("command")
                .and_then(JsonDocument::as_str)
                .unwrap_or_else(|| panic!("missing command for {}", event.name));
            assert_eq!(
                command, expected_command,
                "command must be the bare executable for {}",
                event.name
            );

            let args: Vec<&str> = entry
                .get("args")
                .and_then(JsonDocument::as_array)
                .unwrap_or_else(|| panic!("missing args for {}", event.name))
                .iter()
                .map(|value| value.as_str().expect("args entries are strings"))
                .collect();
            assert_eq!(
                args,
                vec!["hook", event.slug],
                "args must be [\"hook\", \"{}\"] for {}",
                event.slug,
                event.name
            );

            assert!(
                !command.contains("powershell"),
                "args form must not wrap the command in PowerShell for {}",
                event.name
            );
            assert!(
                !command.starts_with("& "),
                "args form must not use the PowerShell call operator for {}",
                event.name
            );
        }

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn managed_hook_detection_recognizes_args_form_and_legacy_string_form() {
        let executable = if cfg!(windows) {
            Path::new(r"C:\Users\Example User\.claude\claude-skills.exe")
        } else {
            Path::new("/home/example/.claude/claude-skills")
        };

        // Args form: managed entry built by managed_hook_entry.
        let entry = managed_hook_entry(executable, "session-start");
        let args_form = serde_json::json!({
            "type": "command",
            "command": entry.command,
            "args": entry.args,
            "statusMessage": "test",
        });
        assert!(is_managed_hook_entry(&args_form));

        // Legacy single-string form (older claude-skills versions): plain
        // string mentioning `claude-skills` and a known slug. Detector must
        // still flag it so uninstall cleans up upgrades from older builds.
        let legacy_plain = serde_json::json!({
            "type": "command",
            "command": "claude-skills hook session-start",
        });
        assert!(is_managed_hook_entry(&legacy_plain));

        // Legacy PowerShell-encoded form (Windows installs from older
        // claude-skills versions). Hand-rolled snapshot of what the previous
        // encoder produced for `& 'claude-skills' hook session-start` so we
        // don't depend on the deleted encoder. The base64 below decodes via
        // the still-present decode_powershell_encoded_command helper.
        let encoded_script = "& 'claude-skills' hook session-start";
        let encoded_bytes: Vec<u8> = encoded_script
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let encoded = base64_encode_for_test(&encoded_bytes);
        let legacy_encoded_command = format!("powershell.exe -NoProfile -EncodedCommand {encoded}");
        let legacy_encoded = serde_json::json!({
            "type": "command",
            "command": legacy_encoded_command,
        });
        assert!(is_managed_hook_entry(&legacy_encoded));

        // Unrelated entries must not be flagged.
        let unrelated = serde_json::json!({
            "type": "command",
            "command": "./scripts/format.sh",
        });
        assert!(!is_managed_hook_entry(&unrelated));

        let unrelated_encoded = serde_json::json!({
            "type": "command",
            "command": "powershell.exe -NoProfile -EncodedCommand SQBuAHYAYQBsAGkAZAA=",
        });
        assert!(!is_managed_hook_entry(&unrelated_encoded));

        // Args form with the right binary basename but a slug that isn't in
        // HOOK_EVENTS must be rejected, so a hand-rolled user entry for a
        // future or experimental subcommand isn't auto-removed by uninstall.
        let unknown_slug = serde_json::json!({
            "type": "command",
            "command": entry.command,
            "args": ["hook", "not-a-real-slug"],
        });
        assert!(!is_managed_hook_entry(&unknown_slug));
    }

    #[test]
    fn install_then_uninstall_leaves_no_managed_hook_keys() {
        // Round-trip: building the full payload installs a stanza per
        // installable event; removing it must strip every key it added so the
        // hooks object returns to empty. Regression for the bug where empty
        // `"Stop": []` arrays were left behind after uninstall (28 dead keys).
        let executable = Path::new(if cfg!(windows) {
            r"C:\Users\Example\.claude\claude-skills.exe"
        } else {
            "/home/example/.claude/claude-skills"
        });
        let hook_path = temp_hook_path("claude-skills-uninstall-roundtrip");
        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let installed = build_hooks_payload(&hook_path, executable).unwrap();
        std::fs::write(&hook_path, &installed).unwrap();
        // Sanity: the install added stanzas.
        let installed_doc: JsonDocument = serde_json::from_str(&installed).unwrap();
        assert!(
            !installed_doc
                .get("hooks")
                .and_then(JsonDocument::as_object)
                .unwrap()
                .is_empty(),
            "install must add hook stanzas"
        );

        let (removed_payload, removed) = remove_managed_hook_payload(&hook_path).unwrap();
        assert!(removed, "uninstall must report a change");
        let removed_doc: JsonDocument = serde_json::from_str(&removed_payload).unwrap();
        let hooks = removed_doc
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();
        assert!(
            hooks.is_empty(),
            "uninstall must leave zero hook event keys, found: {:?}",
            hooks.keys().collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn uninstall_preserves_user_authored_hook_on_shared_event() {
        // A user's own hook on an event we also manage must survive uninstall —
        // only our managed entry is removed, and the event key is preserved
        // because it still holds the user's matcher.
        let mut document = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "claude-skills", "args": ["hook", "stop"] },
                            { "type": "command", "command": "/usr/local/bin/my-own-stop.sh" }
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "claude-skills", "args": ["hook", "post-tool-use"] }
                        ]
                    }
                ]
            }
        });

        remove_managed_hooks(&mut document);
        let hooks = document
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();

        // PostToolUse held only our entry -> key pruned entirely.
        assert!(
            !hooks.contains_key("PostToolUse"),
            "fully-managed event key must be pruned"
        );
        // Stop still holds the user's script -> key preserved with that entry.
        let stop_commands = hooks
            .get("Stop")
            .and_then(JsonDocument::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("hooks"))
            .and_then(JsonDocument::as_array)
            .expect("Stop event must survive with the user's hook");
        assert_eq!(stop_commands.len(), 1, "only the user's hook remains");
        assert_eq!(
            stop_commands[0]
                .get("command")
                .and_then(JsonDocument::as_str),
            Some("/usr/local/bin/my-own-stop.sh"),
            "the user's own hook must be preserved verbatim"
        );
    }

    fn base64_encode_for_test(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut rendered = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let first = chunk[0];
            let second = *chunk.get(1).unwrap_or(&0);
            let third = *chunk.get(2).unwrap_or(&0);
            rendered.push(ALPHABET[(first >> 2) as usize] as char);
            rendered
                .push(ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
            if chunk.len() > 1 {
                rendered.push(
                    ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char,
                );
            } else {
                rendered.push('=');
            }
            if chunk.len() > 2 {
                rendered.push(ALPHABET[(third & 0b0011_1111) as usize] as char);
            } else {
                rendered.push('=');
            }
        }
        rendered
    }

    #[test]
    fn pre_tool_use_scopes_to_bash_and_post_tool_use_fires_for_all_tools() {
        // PreToolUse stays Bash-scoped: the rewriter only operates on shell
        // commands. PostToolUse must fire for every tool — the handler gates
        // the edit-counter path on `is_edit_class_tool` (Edit/Write/MultiEdit/
        // NotebookEdit) at runtime, which would be unreachable if Claude Code
        // only delivered Bash events. The empty matcher also lets
        // `tool_timings::record_tool_timing` sample non-Bash tools so the
        // compression-discipline nudge fires when context fills with file
        // reads and edits, not only with shell output.
        let hook_path = temp_hook_path("claude-skills-hook-matcher-scope");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let rendered = build_hooks_payload(&hook_path, Path::new("claude-skills")).unwrap();

        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        let hooks = document
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap();

        for (event, expected_matcher) in [
            ("PreToolUse", "Bash"),
            ("PostToolUse", ""),
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
        // turn end. They either (a) are documented with top-level decision
        // fields only and don't accept additionalContext per the official
        // Claude Code schema, or (b) carry a per-prompt token cost that
        // outweighs the value of any per-call reminder. The operating
        // contract belongs in CLAUDE.md and SessionStart, both paid at most
        // once per session. These events must stay silent.
        //
        // UserPromptSubmit and PostToolBatch are deliberately *not* in this
        // list: they emit short research-first / reviewer-on-close pointers,
        // gated by their own dedicated tests below.
        for subcommand in [
            "post-tool-use",
            "post-tool-use-failure",
            "stop",
            "subagent-stop",
            "session-end",
        ] {
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
        // on each turn.
        //
        // Production path: `run_hook_command` routes the slug to
        // `run_hook_user_prompt_submit`, which reads stdin for `session_id`
        // and applies the optional compression-discipline nudge. This test
        // exercises that dispatcher directly with an empty reader so the
        // fail-open branch yields the base `user_prompt_submit_context()`
        // with no compression nudge appended — exactly the back-compat
        // contract. The empty reader is also what makes this test safe to
        // run from a parent process that holds an open stdin handle (e.g.
        // PowerShell on Windows); reading the real stdin handle there
        // blocks indefinitely.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        // Make the assertion deterministic: even if some operator has
        // CLAUDE_SKILLS_COMPRESSION_HINT=force exported in the test
        // environment, this test asserts the unforced base contract.
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

        let mut stdin = std::io::empty();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }

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

        // Understand-before-building — the per-prompt hook must require the
        // model to understand the request and research what is needed before
        // writing code, so it does not build the wrong thing. This is distinct
        // from the root-cause/debugging cue below: it governs the front of the
        // task (what to build), not the middle (where the bug is). It lands
        // per prompt because the SessionStart bootstrap drops out of the
        // working window after a few turns.
        assert!(
            context.contains("Understand before building"),
            "UserPromptSubmit must name the understand-before-building rule"
        );
        assert!(
            context.contains("building the wrong thing"),
            "UserPromptSubmit must state that research prevents building the wrong thing"
        );

        // Deep-dive cues — the per-prompt pointer must keep the model from
        // jumping from suspicion to fix. These two phrases name the failure
        // mode ("this may be the case" → patch) and the required discipline
        // (trace the symptom and confirm the suspect is on that path before
        // changing it). They live here, not just in the bootstrap, because
        // SessionStart context drops out of the working window after a few
        // turns while UserPromptSubmit lands per prompt.
        assert!(
            context.contains("suspicion is a hypothesis"),
            "UserPromptSubmit must restate that suspicion is a hypothesis, not a finding"
        );
        assert!(
            context.contains("trace the symptom"),
            "UserPromptSubmit must require tracing the symptom before naming a root cause"
        );
        assert!(
            context.contains("this may be the case"),
            "UserPromptSubmit must name the \"this may be the case\" jump as the failure mode"
        );

        // Implementation-discipline pillars — UserPromptSubmit lands per
        // prompt, so naming the four pillars by name keeps them top-of-mind
        // even after the SessionStart bootstrap drops out of the model's
        // working window. The full text lives in the bootstrap and in
        // _shared/common-discipline.md; this hook only restates the names.
        assert!(
            context.contains("Think Before Coding"),
            "UserPromptSubmit must name the Think Before Coding pillar"
        );
        assert!(
            context.contains("Simplicity First"),
            "UserPromptSubmit must name the Simplicity First pillar"
        );
        assert!(
            context.contains("Surgical Changes"),
            "UserPromptSubmit must name the Surgical Changes pillar"
        );
        assert!(
            context.contains("Goal-Driven Execution"),
            "UserPromptSubmit must name the Goal-Driven Execution pillar"
        );

        let event_name = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("hookEventName"))
            .and_then(JsonDocument::as_str);

        assert_eq!(event_name, Some("UserPromptSubmit"));
    }

    #[test]
    fn user_prompt_submit_consumes_injected_stdin_payload_without_blocking() {
        // Regression test for the stdin-blocking hang fixed in this commit.
        // Before the fix, `run_hook_user_prompt_submit` read directly from
        // `std::io::stdin()`, which on Windows under PowerShell hangs
        // indefinitely because the parent's open console handle is inherited
        // by the test runner. The fix injects the reader, so this test can
        // pass real JSON bytes through `&mut &[u8]` and prove the parser
        // actually consumed them. If this test ever hangs, the fix has
        // regressed: the function is reading the global stdin handle again.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

        // Real Claude Code payload shape — UserPromptSubmit always carries a
        // `session_id`. The hook fail-opens to the base context when the
        // session-keyed compression-hint heuristic returns None (no JSONL
        // rows recorded yet for this session in the current claude_home),
        // so the assertion here is just "exit 0 + base context present"
        // rather than "compression hint included," which keeps the test
        // stable across hosts that may or may not have CLAUDE_TARGET_OVERRIDE
        // populated.
        let payload = br#"{"session_id":"test-session-stdin-injection","hook_event_name":"UserPromptSubmit"}"#;
        let mut stdin: &[u8] = payload;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }

        assert_eq!(
            code,
            0,
            "stderr for injected-payload user-prompt-submit: {}",
            String::from_utf8_lossy(&stderr)
        );

        let output: JsonDocument =
            serde_json::from_slice(&stdout).expect("valid JSON for injected payload");
        let context = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("additionalContext"))
            .and_then(JsonDocument::as_str)
            .expect("UserPromptSubmit must emit additionalContext for injected payload");
        assert!(
            context.contains("Research-first"),
            "base context must still appear when stdin carries a real payload"
        );

        // Reader was fully consumed. `&[u8]` advances on read, so a
        // post-call slice length of 0 proves the function read the whole
        // payload (the fix is exercising the injection point) and did not
        // silently drop straight to the empty-stdin fallback. Combined
        // with the function having exactly one read path (line 870), this
        // is sufficient: a regression that re-introduced a global stdin
        // read would have to also remove this read of the injected reader,
        // which would leave the byte slice non-empty.
        assert!(
            stdin.is_empty(),
            "function must drain the injected reader; remaining bytes signal a regression"
        );
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

        assert!(
            context.contains("reviewer pass"),
            "PostToolBatch reminder must surface the reviewer-pass closeout requirement"
        );
        assert!(
            context.contains("non-trivial"),
            "PostToolBatch reminder must state the trivial/non-trivial split inline so the rule works in any host repo"
        );
        assert!(
            context.contains("Trivial"),
            "PostToolBatch reminder must spell out the exempt trivial cases"
        );
        assert!(
            context.contains("CLAUDE.md") && context.contains("AGENTS.md"),
            "PostToolBatch reminder must mention CLAUDE.md/AGENTS.md as an optional override, not a required citation"
        );
        assert!(
            context.contains("take precedence")
                || context.contains("optional")
                || context.contains("override"),
            "PostToolBatch reminder must frame project-level convention files as optional, not mandatory"
        );
        assert!(
            !context.contains("Routing Rules"),
            "PostToolBatch reminder must not cite a repo-specific section name; the rule is stated inline so it works across host repos"
        );
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
        // SessionStart is the documented entry point for delivering durable
        // model context, so it carries the bootstrap skill (iron law + Red
        // Flags + skill catalog) plus the runtime-resolved workspace memory
        // pointer. Both pieces have to be there: the skill delivers the
        // operating contract, the pointer delivers the workspace-specific
        // memory path that CLAUDE.md cannot know in advance.
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

        // Implementation-discipline pillars — the bootstrap skill carries
        // the full Code Implementation Discipline section so the model
        // gets the four pillars on every session start, not only when an
        // on-demand skill match fires. Each pillar name is asserted so a
        // future trim of the SKILL.md cannot silently drop them.
        assert!(
            context.contains("Think Before Coding"),
            "SessionStart must embed the Think Before Coding pillar"
        );
        assert!(
            context.contains("Simplicity First"),
            "SessionStart must embed the Simplicity First pillar"
        );
        assert!(
            context.contains("Surgical Changes"),
            "SessionStart must embed the Surgical Changes pillar"
        );
        assert!(
            context.contains("Goal-Driven Execution"),
            "SessionStart must embed the Goal-Driven Execution pillar"
        );

        // Root-cause deep-dive guard — the bootstrap must teach that
        // suspicion is a hypothesis, not a finding, so the model does not
        // jump from "this looks like the cause" to a patch. The exact
        // phrasing lives in using-claude-core/SKILL.md; this assertion
        // protects the rule from being silently trimmed during a future
        // edit of the bootstrap.
        assert!(
            context.contains("Suspicion is a hypothesis, not a finding"),
            "SessionStart must restate that suspicion is a hypothesis, not a finding"
        );
        assert!(
            context.contains("Oh this may be the case"),
            "SessionStart Red Flags must name the \"Oh this may be the case\" jump"
        );
    }

    #[test]
    fn top_level_only_hooks_use_system_message_not_hook_specific_output() {
        // Per code.claude.com/docs/en/hooks, every event row carries a
        // `supports_hook_specific_output` flag. Events with `true` accept
        // `hookSpecificOutput.additionalContext`; everything else must use
        // top-level fields like `systemMessage`. This test exercises the
        // wrapper directly with a non-empty context for *every* event so
        // both branches are reached. The earlier version called
        // `run_hook_lifecycle` with five hand-picked events that all
        // produce empty stdout in tests, so the assertions never ran and
        // a regression in either branch would have shipped silently.
        const SAMPLE_CONTEXT: &str = "non-empty test payload";

        for event in HOOK_EVENTS {
            let payload = render_lifecycle_payload(event, SAMPLE_CONTEXT);

            if event.supports_hook_specific_output {
                let hook_specific = payload
                    .get("hookSpecificOutput")
                    .unwrap_or_else(|| panic!("{} must emit hookSpecificOutput", event.name));
                assert_eq!(
                    hook_specific
                        .get("hookEventName")
                        .and_then(JsonDocument::as_str),
                    Some(event.name),
                    "{}: hookSpecificOutput.hookEventName must match the event row",
                    event.name
                );
                assert_eq!(
                    hook_specific
                        .get("additionalContext")
                        .and_then(JsonDocument::as_str),
                    Some(SAMPLE_CONTEXT),
                    "{}: hookSpecificOutput.additionalContext must carry the context",
                    event.name
                );
                assert!(
                    payload.get("systemMessage").is_none(),
                    "{}: schema-supported events must not duplicate context into top-level systemMessage",
                    event.name
                );
            } else {
                assert!(
                    payload.get("hookSpecificOutput").is_none(),
                    "{}: top-level-only events must not emit hookSpecificOutput — the official Claude Code schema documents top-level decision fields only for this event",
                    event.name
                );
                assert_eq!(
                    payload.get("systemMessage").and_then(JsonDocument::as_str),
                    Some(SAMPLE_CONTEXT),
                    "{}: top-level-only events must wrap context in systemMessage",
                    event.name
                );
            }

            assert_eq!(
                payload.get("suppressOutput").and_then(JsonDocument::as_bool),
                Some(true),
                "{}: every payload must set suppressOutput=true so plain stdout doesn't leak into the transcript",
                event.name
            );
        }
    }

    #[test]
    fn session_start_emits_hook_specific_output_additional_context() {
        // SessionStart is the documented entry point for delivering durable
        // model context via `hookSpecificOutput.additionalContext` per
        // code.claude.com/docs/en/hooks. The bootstrap skill must land in
        // that field, not in the user-facing top-level `systemMessage`
        // warning slot. The inner-string assertions live in
        // `session_start_context_embeds_bootstrap_skill_and_memory_pointer`;
        // this test pins the wrapper shape.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_lifecycle("session-start", &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "stderr for session-start: {}",
            String::from_utf8_lossy(&stderr)
        );

        let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

        let event_name = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("hookEventName"))
            .and_then(JsonDocument::as_str)
            .expect("SessionStart must emit hookSpecificOutput.hookEventName");
        assert_eq!(event_name, "SessionStart");

        let context = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("additionalContext"))
            .and_then(JsonDocument::as_str)
            .expect("SessionStart must emit hookSpecificOutput.additionalContext");
        assert!(
            !context.trim().is_empty(),
            "SessionStart additionalContext must not be empty"
        );

        assert!(
            output.get("systemMessage").is_none(),
            "SessionStart must not emit top-level systemMessage — additionalContext is the documented vehicle for model-context injection"
        );
    }

    #[test]
    fn hook_help_lists_every_official_event_slug() {
        // Regression guard: an earlier hand-maintained help string was
        // missing 14 of the 29 official slugs. Anyone running
        // `claude-skills hook` to discover what's available saw a partial
        // list even though every slug dispatched. Generate the help line
        // from HOOK_EVENTS so the "advertised == dispatched" invariant is
        // structural rather than habit.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_hook_command(&[], &mut stdout, &mut stderr);
        // No-args invocation prints help and exits 1 — that's intentional
        // so a misconfigured pipeline can't silently no-op.
        assert_eq!(exit, 1);
        let rendered = String::from_utf8(stdout).expect("help is UTF-8");
        for event in HOOK_EVENTS {
            assert!(
                rendered.contains(event.slug),
                "hook help is missing slug `{}`; rendered: {rendered}",
                event.slug
            );
        }
        // Admin verbs must also be present.
        for verb in [
            "install",
            "uninstall",
            "list",
            "show",
            "instructions",
            "diagnose",
        ] {
            assert!(
                rendered.contains(verb),
                "hook help is missing admin verb `{verb}`; rendered: {rendered}"
            );
        }
        let _ = stderr;
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

        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &executable).unwrap();
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
        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &other_path).unwrap();
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

        let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let payload = build_hooks_payload(&settings_path, &executable).unwrap();
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
    fn notification_emits_bell_terminal_sequence() {
        // CC 2.1.141: Notification fires when Claude Code wants the user's
        // attention (permission prompt, idle reminder). Our handler emits a
        // top-level `terminalSequence` carrying the BEL so the user hears
        // it even when the terminal is in the background. The output also
        // sets `suppressOutput` so the bell is the only visible side
        // effect. The hook must always exit 0 — a non-zero notification
        // exit is treated as a permission denial in some CC builds.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_command(&["notification".to_string()], &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "notification must exit 0; stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            stderr.is_empty(),
            "notification must emit no stderr; got: {}",
            String::from_utf8_lossy(&stderr)
        );

        // Stdout is one JSON object terminated by a newline.
        let rendered = String::from_utf8(stdout).expect("notification output is UTF-8");
        let trimmed = rendered.strip_suffix('\n').unwrap_or(&rendered);

        let parsed: JsonDocument =
            serde_json::from_str(trimmed).expect("notification output is valid JSON");
        assert_eq!(
            parsed.get("suppressOutput").and_then(JsonDocument::as_bool),
            Some(true),
            "notification must set suppressOutput so the row stays out of the transcript",
        );
        assert_eq!(
            parsed
                .get("terminalSequence")
                .and_then(JsonDocument::as_str),
            Some("\u{0007}"),
            "terminalSequence must be the BEL byte (CC 2.1.141 allowlist)",
        );
    }

    #[test]
    fn memory_key_sanitization_matches_scope_command_shape() {
        let key = sanitize_memory_key(r#"C:\Users\riezh\OneDrive\Documents\test\claude_core"#);

        assert_eq!(key, "c-users-riezh-onedrive-documents-test-claude-core");
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

        let executable = std::env::current_exe().unwrap();
        let rendered = build_hooks_payload(&hook_path, &executable).unwrap();
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

        let executable = std::env::current_exe().unwrap();
        let rendered = build_hooks_payload(&hook_path, &executable).unwrap();
        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        assert_eq!(
            document
                .get("skillListingBudgetFraction")
                .and_then(JsonDocument::as_f64),
            Some(0.05),
        );

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    // ----- Compression-discipline hint tests -----
    //
    // The following tests exercise the auto compression-output heuristic that
    // gates the optional per-prompt nudge appended to UserPromptSubmit. They
    // mutate process-global env vars `CLAUDE_SKILLS_COMPRESSION_HINT` and
    // `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER`, so each one takes the shared
    // `crate::test_support::ENV_LOCK` before touching the environment. See
    // the doc comment on that lock for the full design note.

    fn compression_hint_tempdir(label: &str) -> PathBuf {
        let unique_suffix: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        std::fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    fn write_session_timing_rows(claude_home: &Path, session_id: &str, count: usize) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let dir = claude_home.join("state").join("tool-timings");
        std::fs::create_dir_all(&dir).expect("create timings dir");
        let path = dir.join(format!("{date}.jsonl"));
        let mut body = String::new();
        for index in 0..count {
            body.push_str(&format!(
                r#"{{"recorded_at_ms":{index},"event":"PostToolUse","tool_name":"Read","duration_ms":12,"session_id":"{session_id}","cwd":"","effort_level":""}}"#
            ));
            body.push('\n');
        }
        std::fs::write(&path, body).expect("write timings fixture");
    }

    #[test]
    fn compression_hint_text_names_three_actions() {
        // Pure text assertion — no env mutation, so no lock needed. The hint
        // must name the three discipline points so a model that sees only
        // this fragment still gets actionable guidance.
        let hint = compression_hint_text();
        assert!(
            hint.contains("narrower line ranges"),
            "compression hint must point at narrower line ranges"
        );
        assert!(
            hint.contains("Search before reading"),
            "compression hint must point at search-before-read"
        );
        assert!(
            hint.contains("Summarize logs"),
            "compression hint must point at summarizing logs"
        );
        assert!(
            hint.contains("compression-discipline"),
            "compression hint must reference the compression-discipline skill"
        );
    }

    #[test]
    fn maybe_compression_hint_returns_none_when_threshold_not_reached() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-below-threshold");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        write_session_timing_rows(&claude_home, "session-A", 5);

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

        let hint = maybe_compression_hint(&claude_home, "session-A");
        assert!(
            hint.is_none(),
            "5 rows is below threshold of 40, must not inject hint"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn maybe_compression_hint_returns_some_when_threshold_reached() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-at-threshold");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        write_session_timing_rows(&claude_home, "session-B", 50);

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

        let hint = maybe_compression_hint(&claude_home, "session-B");
        assert!(
            hint.is_some(),
            "50 rows exceeds threshold of 40, must inject hint"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn maybe_compression_hint_respects_off_override() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-off");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        write_session_timing_rows(&claude_home, "session-C", 1000);

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "off");

        let hint = maybe_compression_hint(&claude_home, "session-C");
        assert!(
            hint.is_none(),
            "CLAUDE_SKILLS_COMPRESSION_HINT=off must override even at 1000 rows"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn maybe_compression_hint_respects_force_override_below_threshold() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-force");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        // Deliberately no JSONL on disk: force override must win even when the
        // heuristic would normally fail open.

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "force");

        let hint = maybe_compression_hint(&claude_home, "session-D");
        assert!(
            hint.is_some(),
            "CLAUDE_SKILLS_COMPRESSION_HINT=force must inject the hint even with no JSONL on disk"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn maybe_compression_hint_returns_none_for_missing_jsonl() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-missing-jsonl");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        // No state/tool-timings/<date>.jsonl on purpose. Heuristic must fail
        // open silently — telemetry hiccups never break the hook.

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

        let hint = maybe_compression_hint(&claude_home, "session-E");
        assert!(
            hint.is_none(),
            "missing JSONL must yield no hint (fail-open)"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn count_session_tool_timing_rows_filters_to_named_session() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-count-session-rows");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        // Mix two sessions in the same JSONL: the count must only attribute
        // rows whose session_id matches the query.
        write_session_timing_rows(&claude_home, "session-mine", 7);
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{date}.jsonl"));
        let mut existing = std::fs::read_to_string(&path).expect("read fixture");
        for index in 0..3 {
            existing.push_str(&format!(
                r#"{{"recorded_at_ms":{index},"event":"PostToolUse","tool_name":"Read","duration_ms":12,"session_id":"session-other","cwd":"","effort_level":""}}"#
            ));
            existing.push('\n');
        }
        // Add a deliberately malformed row to confirm parse errors are
        // skipped silently.
        existing.push_str("not-json\n");
        std::fs::write(&path, existing).expect("rewrite fixture");

        let count = count_session_tool_timing_rows(&claude_home, "session-mine");
        assert_eq!(
            count, 7,
            "must count only the 7 rows tagged with session-mine, ignore session-other and malformed rows"
        );

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn maybe_compression_hint_returns_none_when_threshold_is_zero() {
        // Operator escape hatch: setting CLAUDE_SKILLS_COMPRESSION_HINT_AFTER=0
        // disables the heuristic by short-circuiting before the JSONL is read.
        // This is a different code path from CLAUDE_SKILLS_COMPRESSION_HINT=off
        // and deserves its own coverage so a future change cannot remove the
        // `if threshold == 0` guard without surfacing as a test failure.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = compression_hint_tempdir("claude-skills-hint-threshold-zero");
        let claude_home = temp.join("claude-home");
        std::fs::create_dir_all(&claude_home).expect("create claude home");
        write_session_timing_rows(&claude_home, "session-Z", 1000);

        let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "0");

        let hint = maybe_compression_hint(&claude_home, "session-Z");
        assert!(
            hint.is_none(),
            "CLAUDE_SKILLS_COMPRESSION_HINT_AFTER=0 must disable the heuristic even at 1000 rows"
        );

        match previous_after {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
        }
        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn append_compression_hint_when_forced_injects_hint_under_force() {
        // The fallback path used when stdin or claude_home are unavailable
        // re-reads CLAUDE_SKILLS_COMPRESSION_HINT independently of
        // maybe_compression_hint so diagnostic runs (force override, no real
        // session) still emit the nudge. Cover the force arm directly.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "force");

        let result = append_compression_hint_when_forced("base context".to_string());
        assert!(
            result.contains("base context"),
            "force path must preserve base context"
        );
        assert!(
            result.contains("Output compression is on"),
            "force path must append the compression hint"
        );

        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
    }

    #[test]
    fn append_compression_hint_when_forced_is_noop_without_force() {
        // The fallback must NOT inject the hint when no force override is set,
        // even if stdin was unavailable. Keeps the default behaviour exactly
        // equal to the unchanged base context.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

        let result = append_compression_hint_when_forced("base context".to_string());
        assert_eq!(
            result, "base context",
            "fallback must be a no-op without the force override"
        );

        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }
    }

    #[test]
    fn lifecycle_additional_context_does_not_handle_user_prompt_submit() {
        // Invariant guard for the dispatch split between run_hook_command and
        // lifecycle_additional_context. The "user-prompt-submit" slug is
        // handled exclusively by run_hook_user_prompt_submit because that
        // dispatcher reads stdin to extract session_id. If anyone re-adds an
        // arm for it in lifecycle_additional_context the per-prompt nudge
        // would silently regress to a stdin-blind path. This test asserts
        // the wildcard fall-through (-> empty string) is in force, which is
        // exactly the contract the dispatcher relies on.
        let result = lifecycle_additional_context("user-prompt-submit");
        assert!(
            result.is_empty(),
            "lifecycle_additional_context must not handle user-prompt-submit; got: {result:?}"
        );
    }
}
