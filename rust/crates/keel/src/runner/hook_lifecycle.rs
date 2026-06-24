//! Purpose: the harness hook lifecycle management, installation, and removal.
//! Caller: runner/mod.rs for hook command group.
//! Dependencies: std::collections::BTreeMap, std::fs, std::path, serde_json, crate::runtime.
//! Main Functions: run_hook_command, build_hooks_payload, remove_managed_hooks.
//! Side Effects: Reads and writes the harness hooks.json configuration.

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
use crate::runtime::{
    display_path, installed_executable_path, resolve_claude_home, resolve_repository_root,
    write_text,
};
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

/// Env var that disables the SessionStart MCP-registration self-heal. Unset (the
/// default) keeps the self-heal on; set to `off` to skip it (used by tests that
/// must not touch any `~/.claude.json`, and as an operator escape hatch). Any
/// other value leaves the self-heal enabled.
const MCP_SELF_HEAL_ENV_VAR: &str = "CLAUDE_SKILLS_MCP_SELF_HEAL";

/// Env var that disables the SessionEnd auto-capture of a session work summary
/// to memory. Unset (the default) keeps it on; set to `off` to skip it. Any
/// other value leaves it enabled. The capture is silent on sessions that did no
/// edit-class work, so research/question-only turns never write a summary.
const SESSION_CAPTURE_ENV_VAR: &str = "CLAUDE_SKILLS_SESSION_CAPTURE";

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
        "install" => run_hook_install(&arguments[1..], standard_output, standard_error),

        "uninstall" => run_hook_uninstall(&arguments[1..], standard_output, standard_error),

        "list" | "show" => run_hook_list(&arguments[1..], standard_output, standard_error),

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

        // Stop and SubagentStop must never return a non-zero exit code, and must
        // never emit hookSpecificOutput.additionalContext. Two distinct hazards:
        //   1. A non-zero exit makes the harness re-run the turn, which cascades
        //      into a stop loop.
        //   2. additionalContext on a Stop hook means "keep going" — emitting it
        //      unconditionally makes the agent loop forever (finish -> inject ->
        //      forced to continue -> finish -> inject -> ...). This was the
        //      regression shipped in PR #121 and reverted here.
        // The closeout reminder lives on PostToolBatch instead, which fires
        // mid-turn before the next model call and cannot loop. Short-circuit to
        // exit 0 with no output so no downstream change can re-introduce either
        // hazard.
        "stop" | "subagent-stop" => 0,

        // Notification fires when the harness wants the user's attention
        // (permission prompt, idle reminder). CC 2.1.141 added the
        // `terminalSequence` field to hook JSON output for exactly this case
        // — emitting bells/desktop notifications without a controlling
        // terminal. We ring the BEL so the user hears it even when the
        // terminal is in the background. Notification is documented as
        // top-level-only (no hookSpecificOutput), so we own dispatch here
        // rather than going through the lifecycle path.
        "notification" => run_hook_notification(standard_output),

        // PermissionRequest: auto-approve keel commands to reduce
        // permission prompt friction. Reads stdin to check tool_name/tool_input.
        "permission-request" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_permission_request(&mut stdin, standard_output, standard_error)
        }

        // PermissionDenied: signal retry:true so the model knows it can
        // retry the denied call. Reads stdin to check tool context.
        "permission-denied" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_permission_denied(&mut stdin, standard_output, standard_error)
        }

        // SubagentStart: inject iron law context into spawned subagents so
        // they start informed instead of blind. Reads stdin for agent_type.
        "subagent-start" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_subagent_start(&mut stdin, standard_output, standard_error)
        }

        // CwdChanged: refresh system map when the working directory changes
        // so the workspace pointer stays current.
        "cwd-changed" => run_hook_cwd_changed(standard_output, standard_error),

        // UserPromptSubmit reads the same stdin payload the harness delivers to
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

        // PostToolBatch reads stdin for `session_id` so the optional review gate
        // (CLAUDE_SKILLS_REVIEW_GATE) can scope its per-session block counter and
        // edit-vs-review telemetry. With the gate disabled (the default) this is
        // behaviorally identical to the advisory reminder the lifecycle path
        // emits. Stdin is injected so tests can pass `&mut std::io::empty()`.
        "post-tool-batch" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_post_tool_batch(&mut stdin, standard_output, standard_error)
        }

        // SessionStart re-asserts the keel MCP registration before the
        // normal lifecycle context render. This is the self-heal for a drifted
        // ~/.claude.json entry: install/update/repair re-register, but a binary
        // swapped in by any other path (manual copy, partial install,
        // __self-replace) leaves a previously-written entry untouched — so an
        // entry missing `alwaysLoad` would persist and the MCP tools would stay
        // deferred behind ToolSearch. Running the idempotent re-registration on
        // every session boot closes that window: it is a no-op on a healthy
        // config and silently repairs drift on the next launch. Best-effort —
        // a failure is reported to stderr but never changes the hook exit code,
        // because the SessionStart context render is load-bearing and MCP is not.
        // Routed here (not in run_hook_lifecycle) so the inner lifecycle unit
        // test stays free of any ~/.claude.json write.
        "session-start" => {
            maybe_self_heal_mcp_registration(standard_error);
            run_hook_lifecycle("session-start", standard_output, standard_error)
        }

        // SessionEnd reads stdin for `session_id` so the auto-capture can scope a
        // work summary to this session's edit-class observations and write it to
        // memory (the "after do, save to memory" half — so the next session
        // starts informed without the model remembering to write a note). Routed
        // here (not the stdin-blind lifecycle path) because the summary needs the
        // session id from the payload. The capture runs first, then the lifecycle
        // path still performs the existing SessionEnd side effects (system-map
        // refresh, store prunes, learning). Stdin is injected so tests can pass
        // `&mut std::io::empty()`.
        "session-end" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_session_end(&mut stdin, standard_output, standard_error)
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

fn run_hook_install(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook install");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
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
                "Installed Rust keel lifecycle hooks at {}",
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

fn run_hook_uninstall(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook uninstall");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
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
                            "Removed Rust keel hook from {}",
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
                    "No keel hook installed at {}",
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

fn run_hook_list(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook list");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,

        Err(error) => {
            let _ = writeln!(standard_error, "{error}");

            return 1;
        }
    };

    let hook_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);

    match fs::read_to_string(&hook_path) {
        Ok(text) => {
            // Redact secret-pattern values before printing. `settings.json`
            // routinely carries an `env` block with a live `ANTHROPIC_AUTH_TOKEN`
            // (and may hold API keys/passwords), and `hook list`/`show` output
            // lands in logs, screen shares, and subagent transcripts. Printing
            // verbatim would leak a live credential, so mask known-secret keys.
            let _ = writeln!(standard_output, "{}", redact_secrets_in_settings(&text));

            0
        }

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(
                standard_output,
                "No keel hook installed at {}",
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

/// True when a settings key name looks like it holds a secret. Case-insensitive
/// substring match on the conventional secret markers so `ANTHROPIC_AUTH_TOKEN`,
/// `OPENAI_API_KEY`, `*_SECRET`, and `*PASSWORD*` are all caught. The match is
/// deliberately broad (redacting a non-secret is harmless; leaking a secret is
/// not), but it is NOT exhaustive — keys like `DATABASE_URL` that can embed a
/// credential in a value are not caught here.
fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let keyword_match = ["token", "secret", "password", "passwd", "api_key", "apikey"]
        .iter()
        .any(|marker| lower.contains(marker));
    // `*_KEY` always; a bare `*key` suffix only when paired with an auth/api/
    // access marker, so `monkey`/`passkey` do not trigger a false redaction.
    let key_suffix_match = lower.ends_with("_key")
        || (lower.ends_with("key")
            && (lower.contains("auth") || lower.contains("api") || lower.contains("access")));
    keyword_match || key_suffix_match
}

/// Mask a secret value, preserving a short prefix so an operator can still
/// recognize which credential it is without exposing the whole token. Short
/// values are fully masked. Counts by characters (not bytes) and slices on a
/// char boundary so a multi-byte UTF-8 value can never panic.
fn mask_secret_value(value: &str) -> String {
    if value.chars().count() <= 4 {
        "****".to_string()
    } else {
        let prefix: String = value.chars().take(4).collect();
        format!("{prefix}…(redacted)")
    }
}

/// Walk a parsed settings document and replace every string value whose key
/// looks like a secret with a masked form. Recurses through objects and arrays
/// so an `env` block at any depth is covered. On parse failure the raw text is
/// NOT returned (it could contain a live token) — a suppression notice is
/// returned instead, so a malformed settings.json can never leak a credential
/// through `hook list`/`show`.
fn redact_secrets_in_settings(raw: &str) -> String {
    match serde_json::from_str::<JsonDocument>(raw) {
        Ok(mut document) => {
            redact_secrets_in_value(&mut document, false);
            // A re-serialization failure is implausible for a value we just
            // parsed, but if it happens we still must not fall back to the raw
            // (un-redacted) text — suppress instead of leaking.
            serde_json::to_string_pretty(&document).unwrap_or_else(|_| {
                "[settings.json could not be re-serialized — output suppressed to prevent secret leak]"
                    .to_string()
            })
        }
        Err(_) => "[settings.json is not valid JSON — output suppressed to prevent secret leak]"
            .to_string(),
    }
}

/// Recursive worker for [`redact_secrets_in_settings`]. `parent_key_is_secret`
/// carries down whether the immediate parent object key was itself a secret
/// marker, so a value reached via a secret key is masked even if it is nested.
fn redact_secrets_in_value(value: &mut JsonDocument, parent_key_is_secret: bool) {
    match value {
        JsonDocument::Object(map) => {
            for (key, child) in map.iter_mut() {
                // Once we are under a secret-named key, every descendant is
                // sensitive — OR the carry-down in so the chain survives an
                // intermediate object (e.g. {"api_key": {"primary": "..."}}),
                // not just a direct string or array.
                let key_is_secret = parent_key_is_secret || is_secret_key(key);
                if key_is_secret {
                    if let JsonDocument::String(secret) = child {
                        *secret = mask_secret_value(secret);
                        // Already masked this string; skip the recursion below
                        // so we do not walk into it a second time.
                        continue;
                    }
                }
                redact_secrets_in_value(child, key_is_secret);
            }
        }
        JsonDocument::Array(items) => {
            for item in items.iter_mut() {
                redact_secrets_in_value(item, parent_key_is_secret);
            }
        }
        JsonDocument::String(text) if parent_key_is_secret => {
            *text = mask_secret_value(text);
        }
        _ => {}
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
            ("rerunPrefix".into(), Value::String("keel run --".into())),
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

        "keel PreToolUse hook transparently rewrites noisy shell commands via `keel run -- <command>`. No manual rerun needed."

    );

    let _ = writeln!(
        standard_output,
        "the harness exposes hook events including: {}.",
        claude_hook_event_names().collect::<Vec<_>>().join(", ")
    );

    let _ = writeln!(

        standard_output,

        "keel installs managed entries for every supported lifecycle event; `PreToolUse` silently rewrites supported Bash commands with native compaction."

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
                "Unable to read the harness hook input: {error}"
            );

            return 1;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to decode the harness hook input: {error}"
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

            "allowRules": [
                format!("Bash({}:*)", rewrite.rewritten_command.split_whitespace().next().unwrap_or("keel")),
            ],

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
                "Unable to render the harness hook output: {error}"
            );

            0
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

        // PostToolUse must never fail loudly: a non-zero exit teaches the harness
        // Code that the post-tool hook itself is broken. Log to stderr and
        // exit 0 — the lifecycle event is observability, not a gate.
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: unable to decode hook input: {error}"
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
            "keel post-tool-use: tool-timings record failed: {error}"
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
            "keel post-tool-use: observation record failed: {error}"
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
                "keel post-tool-use: counter update failed: {error}"
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
                "keel post-tool-use-failure: unable to read hook input: {error}"
            );

            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use-failure: unable to decode hook input: {error}"
            );

            return 0;
        }
    };

    if let Err(error) = tool_timings::record_tool_timing("PostToolUseFailure", &input) {
        let _ = writeln!(
            standard_error,
            "keel post-tool-use-failure: tool-timings record failed: {error}"
        );
    }

    // Capture the FAILURE as its own behavioral observation. A failing tool call
    // is the Reflexion-style "what goes wrong here" signal: it clusters under a
    // distinct `… (failed)` signature so a recurring failure becomes its own
    // instinct and surfaces in the SessionStart digest, without polluting the
    // success patterns. Like the timing record, any error is logged and swallowed
    // — learning capture must never fail the hook.
    if let Err(error) = observation::record_failure_observation(&input) {
        let _ = writeln!(
            standard_error,
            "keel post-tool-use-failure: observation record failed: {error}"
        );
    }

    0
}

/// Notification handler.
///
/// CC 2.1.141 added a `terminalSequence` top-level field to hook JSON output
/// so hooks can emit desktop notifications, window titles, and bells without
/// a controlling terminal. The Notification event fires when the harness
/// raises a permission prompt or an idle "needs your attention" cue, so it
/// is the natural place to ring the BEL. Allowed payload per the docs is
/// OSC 0/1/2/9/99/777 and BEL — `\u{0007}` is the BEL and is in the
/// allowlist. `suppressOutput` keeps the transcript clean.
///
/// The handler is input-agnostic: the JSON output is the same regardless of
/// what stdin contains, so we don't read it. The harness does not require
/// the hook to drain the pipe.
fn run_hook_notification(standard_output: &mut dyn Write) -> u8 {
    let _ = writeln!(standard_output, "{NOTIFICATION_BELL_OUTPUT}");

    0
}

/// Hook JSON emitted by Notification. BEL is in the CC 2.1.141
/// `terminalSequence` allowlist and is JSON-escaped as `\u0007` per
/// RFC 8259 (control characters U+0000–U+001F MUST be escaped inside a
/// JSON string). The harness unescapes the value before writing it to the
/// terminal, which is what produces the audible bell. `suppressOutput`
/// hides this row from the transcript so the bell is the only side effect.
const NOTIFICATION_BELL_OUTPUT: &str = "{\"suppressOutput\":true,\"terminalSequence\":\"\\u0007\"}";

/// PermissionRequest handler.
///
/// Auto-approves Bash commands that invoke `keel` to reduce permission
/// prompt friction. For all other tool calls, returns 0 (no output) to let
/// the harness handle the permission dialog normally.
fn run_hook_permission_request(
    stdin: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let input_text = match read_stdin_text(stdin) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-request: {error}");
            return 0;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-request: decode: {error}");
            return 0;
        }
    };

    let tool_name = input
        .get("tool_name")
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    // Only auto-approve Bash calls to keel
    if tool_name != "Bash" {
        return 0;
    }

    let command = input
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    if !command.starts_with("keel ") && !command.starts_with("keel.exe ") {
        return 0;
    }

    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": {
                "behavior": "allow",
                "allowRules": ["Bash(keel *)"],
            },
        }
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-request: render: {error}");
            0
        }
    }
}

/// PermissionDenied handler.
///
/// Signals `retry: true` so the model knows it can retry the denied call.
/// This is useful when a permission was denied transiently by the auto-mode
/// classifier — the model gets explicit feedback that retrying is allowed.
fn run_hook_permission_denied(
    stdin: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let input_text = match read_stdin_text(stdin) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-denied: {error}");
            return 0;
        }
    };

    let _input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-denied: decode: {error}");
            return 0;
        }
    };

    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionDenied",
            "retry": true,
        }
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "keel permission-denied: render: {error}");
            0
        }
    }
}

/// SubagentStart handler.
///
/// Injects a compact iron law reminder into the subagent's context at spawn
/// time, so subagents start informed instead of blind. Uses
/// hookSpecificOutput.additionalContext per code.claude.com/docs/en/hooks.
fn run_hook_subagent_start(
    stdin: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let input_text = match read_stdin_text(stdin) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(standard_error, "keel subagent-start: {error}");
            return 0;
        }
    };

    let _input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,
        Err(error) => {
            let _ = writeln!(standard_error, "keel subagent-start: decode: {error}");
            return 0;
        }
    };

    let context = subagent_start_context();
    if context.trim().is_empty() {
        return 0;
    }

    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SubagentStart",
            "additionalContext": context,
        },
        "suppressOutput": true,
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "keel subagent-start: render: {error}");
            0
        }
    }
}

/// CwdChanged handler.
///
/// Refreshes the system map when the working directory changes so the
/// workspace pointer stays current. The refresh runs through the standard
/// lifecycle path which already handles CwdChanged via should_refresh_system_map.
fn run_hook_cwd_changed(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    run_hook_lifecycle("cwd-changed", standard_output, standard_error)
}

/// Read all of stdin into a String. Shared by handlers that parse hook JSON.
fn read_stdin_text(stdin: &mut dyn Read) -> Result<String, String> {
    let mut buf = String::new();
    stdin
        .read_to_string(&mut buf)
        .map_err(|e| format!("unable to read hook input: {e}"))?;
    Ok(buf)
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
    // to invoke `keel memory scope resolve` — these hooks fire it
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

    // PreCompact is the OTHER point the learning cycle must run. Working memory is
    // about to be summarized away; if we waited for SessionEnd, a long session's
    // observations accumulated before this compaction could be lost when the
    // window is rewritten. The cycle is an idempotent upsert (it re-reads the
    // observation window and refreshes instincts), so running it here AND at
    // SessionEnd never double-counts — it only ensures what was learned so far is
    // persisted before the context that produced it is compacted. Same off-switch
    // (`CLAUDE_SKILLS_LEARNING=off`) and same fail-open contract.
    if event.name == "PreCompact" {
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
                "Unable to render the harness lifecycle hook output: {error}"
            );

            0
        }
    }
}

/// Wrap `context` in the JSON payload the harness expects for `event`.
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
        let mut hook_output = serde_json::json!({
            "hookEventName": event.name,
            "additionalContext": context,
        });

        // SessionStart: add watchPaths for key files so FileChanged fires
        // when CLAUDE.md, Cargo.toml, or settings change during the session.
        if event.name == "SessionStart" {
            if let Ok(cwd) = std::env::current_dir() {
                let watch_files: Vec<String> = [
                    "CLAUDE.md",
                    "Cargo.toml",
                    "package.json",
                    ".claude/settings.json",
                ]
                .iter()
                .map(|f| display_path(&cwd.join(f)))
                .collect();
                hook_output["watchPaths"] = serde_json::json!(watch_files);
            }
        }

        serde_json::json!({
            "hookSpecificOutput": hook_output,
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
/// slug in hand but need to reason in the harness's PascalCase vocabulary.
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
        // before the next model turn. This is the home for the closeout /
        // reviewer-on-close reminder: it runs mid-turn before the next model
        // call, so it can nudge without ever forcing an extra turn. Stop is
        // deliberately NOT used for this — additionalContext on a Stop hook
        // means "keep going", which loops (see the "stop" dispatch arm).
        "post-tool-batch" => post_tool_batch_context(),

        // SubagentStart: inject a compact iron law reminder so spawned
        // subagents start with the core operating contract.
        "subagent-start" => subagent_start_context(),

        // Silenced events. Stop / SubagentStop are silenced because emitting
        // additionalContext on them forces the turn to continue (infinite
        // loop); they are also short-circuited to exit 0 in run_hook_command,
        // so this arm is a second line of defense. SessionEnd fires at session
        // termination. PostToolUse and PostToolUseFailure are owned by their
        // dedicated dispatch arms.
        "stop" | "subagent-stop" | "session-end" | "post-tool-use" | "post-tool-use-failure" => {
            String::new()
        }

        _ => String::new(),
    }
}

/// Compact SessionStart bootstrap contract.
///
/// This must land in the model's context *in full*. The harness truncates hook
/// `hookSpecificOutput.additionalContext` once it crosses ~10KB: the full text
/// is persisted to `<project>/tool-results/hook-…-additionalContext.txt` and the
/// model receives only a ~2KB preview plus a file pointer it never reads back.
/// The previous implementation injected the entire 27KB `using-keel/
/// SKILL.md` here, so in every project the bootstrap was silently truncated to
/// its first ~2KB — the model never saw the iron law's later rules, the MCP tool
/// list, the discipline pillars, or the skill catalog. Verified against live
/// session transcripts: the 27.6KB SessionStart additionalContext was replaced
/// by a 2KB preview while a 5.9KB UserPromptSubmit context landed intact.
///
/// The fix is to keep this block small enough to survive the cap. We drop the
/// ~8.5KB skill catalog enumeration (the harness already injects its own native
/// skill listing every session, so it was pure duplication) and the verbose
/// prose, keeping the operative contract: the iron law, the rationalization Red
/// Flags, the four discipline pillars, the always-on MCP tools, and the memory
/// writers. The full body still ships to disk as
/// `~/.claude/skills/using-keel/SKILL.md` (synced by `sync_skills`) and is
/// loadable on demand via `Skill("using-keel")` when the model wants the
/// complete catalog and routing rules. `~/.claude/CLAUDE.md` carries the same
/// compact contract through the hook-independent user-memory channel.
const COMPACT_BOOTSTRAP: &str = r#"# keel operating contract (loaded at SessionStart)

<EXTREMELY_IMPORTANT>
This contract governs **every project you work in**, not just keel itself.
**Trust the codebase, not your knowledge base.** Knowledge-base recall is stale. Memories drift. The repository in front of you is the source of truth.

## The Iron Law — before you respond to anything that could touch code, configuration, or architecture
1. **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing implementation before claiming behavior. Never propose changes against an imagined version of the file.
2. **Understand before building.** Restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code. Do not guess, do not assume, do not build against an imagined spec. The most expensive waste is not buggy code — it is correct code that solved the wrong problem. If the request is ambiguous in a way that changes what you build, ask before building, not after.
3. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool to invoke it BEFORE writing code or giving a final answer. This is not negotiable. You cannot rationalize your way out of it.
4. **Find the root cause.** Suspicion is a hypothesis, not a finding. Take the symptom as a starting point, trace it end-to-end against the running code with file:line evidence, and confirm the suspected target sits on that path before changing anything.
5. **Preserve existing data.** Never remove or replace an existing field, column, output, or record to fit a new format — ADD alongside, and ASK before dropping anything the user did not name. Data loss in an edit is destructive like `DROP TABLE`. Autonomy covers reversible choices, never data deletion or a changed data contract; when a request could mean "add" or "replace", ask before acting.

This is the **Iron Law** of keel. It is loaded into your context at SessionStart and applies to every prompt thereafter — if asked whether the Iron Law is in your context, the answer is yes: it is the four rules above.
</EXTREMELY_IMPORTANT>

## Red Flags (rationalizations to ignore)
- "I remember this codebase" → Memories drift. Read SYSTEM_MAP and the owning file before claiming behavior.
- "The user story is clear" → Stories are summaries, not specs. Find the root cause.
- "I get the gist, I'll start building" → The gist is not the spec. Restate the request and research what's needed; building on a guess ships the wrong thing.
- "I'll just code this quickly" → Skills tell you HOW. Check first.
- "Oh this may be the case" → Suspicion is a hypothesis, not a finding. Confirm the suspect sits on the symptom's traced path with file:line evidence before changing it.
- "Tests already passed earlier" → Re-run before claiming. No completion claims without fresh evidence.
- "I'll just remove this field to match the format" → ADD alongside; format copies style, not omissions. If you would note the removal after, ask before instead.
- "That hook reminder is wrapper noise" → It states the rule inline so it is self-contained in any repo. Re-read the diff against the rule before skipping.

## Code Implementation Discipline (every code-touching turn)
1. **Think Before Coding** — state assumptions, surface tradeoffs, and deep-dive any suspected target (read it, trace callers/callees against the failing trigger) before changing it.
2. **Simplicity First** — the minimum code that solves the problem. No speculative features, no abstractions for single-use code, no error handling for impossible scenarios.
3. **Surgical Changes** — touch only what the task requires. Match existing style. Every changed line traces directly to the request. Do not refactor unrelated code.
4. **Goal-Driven Execution** — turn vague tasks into verifiable goals before coding. Reproduce or trace the symptom from the user story end-to-end before naming a root cause.
5. **Short Comments** — one line is the default; comments say *why*, never *what*. No multi-paragraph narrative blocks or design history in the code body — that belongs in the brief or commit. A comment that takes longer to read than the code it describes gets cut.

## keel MCP tools — always available, prefer over guessing
- `system_map` — call before any claim about a repository's structure or layout ("what is this project", "where does X live") instead of reading files blind.
- `recall` — call before claiming what you remember or previously learned; full-text search over your durable memory and working briefs.
- `run_command` — run noisy shell commands (test, build, lint, logs, search) through it so compacted output enters context instead of the raw stream.

## Skills & subagents
40 specialist skills are installed under `~/.claude/skills/` (lifecycle, backend, cloud, security, `reviewer`, UI/UX, `preserve-existing-flow`, systematic-debugging, TDD, migrations, and more) — the harness lists them natively each session. Invoke by bare name, e.g. `Skill("reviewer")`. For the full catalog and routing rules, call `Skill("using-keel")`. 24 matching subagents in `.claude/agents/` handle delegated isolated-context work via the Agent tool. About to read or edit existing code? Invoke `preserve-existing-flow` first.

## Memory writes (when you learn something durable)
Working memory dies at compaction. To persist across sessions:
- `keel memory working-brief write` — when starting non-trivial work: capture the request, acceptance criteria, and files you expect to touch BEFORE coding so completion can be reconciled against it.
- `keel memory completion-gate check` — before claiming a task complete: returns the gate's verdict and points at any requirement with no evidence yet.
- SYSTEM_MAP auto-refreshes at session start, pre-compact, and session end — read it before repo-structure claims.

## The one thing to remember
**Understand before you build. Research first. Invoke relevant skills before responding. Find the root cause. The repository — not your training data — is the source of truth.**"#;

pub(crate) fn session_start_context() -> String {
    // SessionStart fires once per session and is the documented entry point
    // for delivering durable model context via
    // `hookSpecificOutput.additionalContext`. Per-prompt token cost is paid
    // at most once per session, so this is the right place to deliver the
    // bootstrap contract instead of restating it on every UserPromptSubmit.
    //
    // The bootstrap MUST be the compact contract, not the full 27KB
    // using-keel/SKILL.md. The harness truncates additionalContext above
    // ~10KB to a 2KB preview + a file pointer (verified against live
    // transcripts), so dumping the full skill here meant the model only ever saw
    // its first ~2KB. COMPACT_BOOTSTRAP keeps the operative contract under the
    // cap so it lands in full; the complete catalog ships to disk and is loadable
    // on demand via Skill("using-keel").
    //
    // Layout: compact bootstrap (iron law + Red Flags + discipline pillars + MCP
    // tools + memory writers), the runtime-resolved memory pointer that CLAUDE.md
    // cannot know in advance, the learned-instinct digest for the current
    // project (the always-on tier of the learning loop — what the user
    // reliably does here, surfaced without waiting for a skill match), and an
    // autonomous synthesis nudge so a freshly generated skill's deterministic
    // template gets upgraded to richer prose in the normal course of work
    // (no manual slash). The nudge self-clears once the agent refines the skill,
    // because the content-hash no-clobber guard then reports it as non-template.
    let mut context = format!("{COMPACT_BOOTSTRAP}\n\n{}", memory_scope_summary());
    // PUSH actual workspace memory content (map head + newest brief + most
    // recent note) so the agent starts informed instead of having to blind-search
    // with system_map/recall. Bounded and may be empty (fresh workspace); the
    // truncation-cap test guards the total SessionStart size.
    let workspace_digest = workspace_memory_digest();
    if !workspace_digest.trim().is_empty() {
        context.push_str("\n\n");
        context.push_str(&workspace_digest);
    }
    if let (Ok(claude_home), Ok(cwd)) = (resolve_claude_home(""), std::env::current_dir()) {
        let cwd = cwd.to_string_lossy();
        let instinct_digest = learning::project_instinct_digest(&claude_home, &cwd);
        if !instinct_digest.trim().is_empty() {
            context.push_str("\n\n");
            context.push_str(&instinct_digest);
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
    "Before compaction, preserve keel continuity: summarize active workflow stage, files changed, validation evidence, unresolved blockers, memory facts to save, and next review gate.".to_string()
}

pub(crate) fn post_compact_context() -> String {
    let mut context = format!(

        "After compaction, resume using keel automatically: reload workspace memory/system map, re-establish workflow proof state, and run review gates before final closeout.\n\n{}",

        memory_scope_summary()

    );
    // Re-PUSH the workspace digest after compaction: the original SessionStart
    // push has dropped out of the window, so the resumed turn would otherwise be
    // back to blind-searching. Same bounded content as session start.
    let digest = workspace_memory_digest();
    if !digest.trim().is_empty() {
        context.push_str("\n\n");
        context.push_str(&digest);
    }
    context
}

/// Per-prompt research-first iron law.
///
/// Compact by design: the schema lets us inject as much text as we want, but
/// every byte lands per prompt and is paid as input tokens. The full
/// bootstrap (skill catalog, Red Flags table, decision flow, four
/// implementation-discipline pillars) is delivered once via SessionStart;
/// this hook only restates the iron law, names the four pillars, advertises the
/// always-available keel MCP tools (so the model reaches for
/// `system_map`/`recall` instead of guessing about the repo or its memory),
/// adds the understand-before-building rule (research the request before writing
/// code — the lever that stops the model building the wrong thing), and the
/// one-line parallel-fan-out independence test so they stay top-of-mind on each
/// turn. Body weight is roughly 320 tokens before `memory_scope_summary()` —
/// within budget for a per-prompt injection but expensive enough that adding
/// more text needs a deliberate reason.
/// Per-prompt iron-law base text (no skill match, no compression hint).
/// Kept separate so the bridge `user-prompt` subcommand can compose the full
/// per-prompt context from flat fields without needing stdin parsing.
fn user_prompt_submit_core() -> String {
    format!(
        "Research-first: trust the codebase, not your knowledge base. Read SYSTEM_MAP and the owning module before claiming behavior. Invoke any relevant skill via the Skill tool BEFORE responding — even a 1% chance it applies means use it. Native keel MCP tools are always available — prefer them over guessing: `system_map` returns the workspace structural map (call it before any repo-structure or \"what is this project\" claim), `recall` runs full-text search over your saved memories and working briefs (call it before claiming what you remember or learned), and `run_command` routes noisy shell output through the compaction proxy. Understand before building: restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code — no guessing, no assuming, no building against an imagined spec. Researching first is what stops you building the wrong thing; the cost of an hour's research is always less than the cost of shipping the wrong feature. Preserve existing data: never remove or replace a field, column, output, or record to fit a new format — ADD alongside and ASK before dropping anything the user did not name; data loss in an edit is destructive, and autonomy covers reversible choices, not data deletion. Find the root cause, not just the surface symptom: suspicion is a hypothesis, not a finding — trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing it. No assumptions. No jumping from \"this may be the case\" to a patch. Implementation discipline applies on every code-touching turn — Think Before Coding (state assumptions, deep-dive any suspected target before changing it), Simplicity First (minimum code, no speculative features or abstractions), Surgical Changes (every changed line traces to the request), Goal-Driven Execution (reproduce or trace the symptom before naming a root cause; turn the task into a verifiable goal before coding). Parallel fan-out: only batch agents in the same message when all four hold — no shared inputs, no shared file or git-index writes, no need to cancel/steer one based on another's interim result, and the work fits the current task scope. If any check fails, dispatch sequentially. {}",
        memory_scope_summary()
    )
}

pub(crate) fn user_prompt_submit_context(prompt_text: &str) -> String {
    let mut base_context = user_prompt_submit_core();
    let claude_home = resolve_claude_home("").ok();

    // Inline the matched skill's own guidance when the prompt distinctively
    // matches one installed skill.
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

    // Point repo/structure and memory questions at the MCP tools.
    if !prompt_text.trim().is_empty() {
        if let Some(pointer) = mcp_tool_pointer_for_prompt(prompt_text) {
            base_context = format!("{pointer}\n\n{base_context}");
        }
    }

    // Point code-CHANGE prompts at the read-map/recall front.
    if !prompt_text.trim().is_empty() {
        if let Some(pointer) = work_intent_pointer_for_prompt(prompt_text) {
            base_context = format!("{pointer}\n\n{base_context}");
        }
    }

    base_context
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

/// Per-prompt pointer at the keel MCP tools for prompts that ask about
/// the repository layout or the agent's own memory.
///
/// The deterministic skill matcher (`utility::skill_match`) stays silent on
/// these prompts by design: "what is this project about", "how is the repo
/// structured", "what do you remember" carry no *distinctive domain token*, so
/// they clear no skill's score floor. That silence is correct for skill
/// routing, but it left a real gap — exactly these prompts are the ones the
/// model should answer by calling `system_map` (structure) or `recall`
/// (memory) instead of guessing or reading files conversationally. This pointer
/// fills that gap with a targeted reminder, fired only when the prompt matches
/// one of the two question shapes below. Returns `None` (no injection) for
/// everything else so the generic per-prompt context is unchanged.
fn mcp_tool_pointer_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();

    // Memory questions: the model should search its durable memory rather than
    // claim from conversation alone. Checked first because "what did you learn
    // about this project" mentions "project" too, and the recall answer is the
    // better one for a memory-shaped ask.
    const MEMORY_CUES: &[&str] = &[
        "what do you remember",
        "what did you learn",
        "what have you learned",
        "do you remember",
        "from memory",
        "your memory",
        "recall what",
        "what's in memory",
        "what is in memory",
    ];
    if MEMORY_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(
            "This prompt asks about your durable memory. Call the keel MCP `recall` tool (full-text search over your saved memories and working briefs) before answering — do not claim what you remember from conversation alone. Use `recall_status` if you need index health.",
        );
    }

    // Repo/structure questions: the model should consult the workspace map
    // rather than guess. The cues are phrased to catch the common shapes
    // ("what is this project", "how is this repo structured", "what does this
    // codebase do", "project overview", "explain the architecture") while
    // staying narrow enough not to fire on ordinary feature work that merely
    // mentions the word "project".
    const REPO_CUES: &[&str] = &[
        "what is this project",
        "what's this project",
        "what is this repo",
        "what's this repo",
        "what is this codebase",
        "what does this project",
        "what does this repo",
        "what does this codebase",
        "about this project",
        "about this repo",
        "about this codebase",
        "project overview",
        "repo structure",
        "repository structure",
        "project structure",
        "codebase structure",
        "how is this repo",
        "how is the repo",
        "how is this project",
        "how is the project",
        "how is this codebase",
        "explain the architecture",
        "explain this project",
        "explain the project",
        "explain the codebase",
    ];
    if REPO_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(
            "This prompt asks about the repository's structure or purpose. Call the keel MCP `system_map` tool to get the authoritative workspace structural map before answering — do not describe the repo layout from memory or guesswork. Read the owning files only after the map points you at them.",
        );
    }

    None
}

/// Per-prompt reminder for code-CHANGE prompts: read the map, recall prior work,
/// and write a working brief BEFORE editing existing code.
///
/// This closes the gap that let the front of the Iron Law go unenforced in
/// practice. `mcp_tool_pointer_for_prompt` above fires only on *question*-shaped
/// prompts ("what is this project", "what do you remember"). A *work* prompt
/// ("rework the X", "fix the Y", "add Z") carries a domain token, so the skill
/// matcher may fire — but nothing reminded the model to read SYSTEM_MAP, run
/// `recall`, or write a working brief first. Those are exactly the steps most
/// easily rationalized away under time pressure, and skipping them is what ships
/// the wrong thing.
///
/// Returns `Some(text)` when the prompt looks like a request to change the
/// codebase (edit/build/refactor/fix intent) and `None` otherwise, so it never
/// fires on pure questions, chit-chat, or read-only asks. Deliberately
/// conservative: a missed work prompt just loses a reminder (the default-on
/// brief gate is the hard backstop), while a false positive would add noise to
/// an ordinary question.
fn work_intent_pointer_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();

    // Unambiguous change-intent cues — safe to match as substrings because they
    // are imperative verbs or verb+object phrases that do not double as common
    // nouns in question phrasing. Each is a frequent opener for a code-change
    // request.
    const STRONG_CUES: &[&str] = &[
        "implement",
        "refactor",
        "rework",
        "rewrite",
        "add a ",
        "add an ",
        "add support",
        "change the",
        "update the",
        "modify",
        "migrate",
        "wire up",
        "integrate",
        "create a",
        "create an",
        "delete the",
        "remove the",
        "rename",
        "optimize",
        "extend the",
    ];
    if STRONG_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(WORK_INTENT_REMINDER);
    }

    // Verbs that ALSO read as nouns ("the build", "a fix", "the patch"). Treat
    // them as change-intent only when used as a verb — i.e. NOT immediately
    // preceded by an article. "fix the bug" fires; "is the build passing" and
    // "when is the fix landing" do not.
    const VERB_OR_NOUN_CUES: &[&str] = &["build ", "fix ", "fixes ", "patch "];
    if VERB_OR_NOUN_CUES
        .iter()
        .any(|cue| cue_used_as_verb(&lowered, cue))
    {
        return Some(WORK_INTENT_REMINDER);
    }

    None
}

/// The read-map / recall / write-brief / preserve-flow reminder injected for
/// code-change prompts. A `const` so both match arms above return the exact same
/// text and the test asserting its content has a single source of truth.
const WORK_INTENT_REMINDER: &str = "This prompt asks you to change the codebase. Before editing: (1) read the workspace SYSTEM_MAP (call the keel MCP `system_map` tool) and the owning file — never edit against an imagined version; (2) call `recall` to surface any prior work, decisions, or conventions on this topic; (3) write a working brief with `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"` capturing what the task actually asks and how completion is judged BEFORE you start (this also clears the default-on working-brief gate); (4) if you are about to edit existing code, invoke the `preserve-existing-flow` skill first. Understand before building — correct code that solved the wrong problem is the most expensive failure.";

/// True when `cue` (e.g. `"fix "`) appears in `lowered` used as a verb rather
/// than a noun — that is, at least one occurrence is NOT immediately preceded by
/// a determiner ("the", "a", "an", "this", "that"). This is what separates the
/// change request "fix the bug" from the question "is the fix ready". Whole-word
/// determiner matching avoids treating "breathe " (ends in "the") as an article.
fn cue_used_as_verb(lowered: &str, cue: &str) -> bool {
    const ARTICLES: &[&str] = &["the", "a", "an", "this", "that"];
    let mut search_start = 0;
    while let Some(relative) = lowered[search_start..].find(cue) {
        let index = search_start + relative;
        let preceding = lowered[..index].trim_end();
        let after_article = ARTICLES.iter().any(|article| {
            if !preceding.ends_with(article) {
                return false;
            }
            // Whole-word check: the char before the article must be a boundary,
            // so "breathe" (ends with "the") is not mistaken for the article.
            let article_start = preceding.len() - article.len();
            article_start == 0 || preceding.as_bytes()[article_start - 1] == b' '
        });
        if !after_article {
            return true;
        }
        search_start = index + cue.len();
    }
    false
}

/// UserPromptSubmit dispatcher that reads stdin and composes the per-prompt
/// `additionalContext`.
///
/// the harness delivers a JSON payload on stdin for this event with at least
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

    let prompt_text = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("prompt"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    let claude_home = resolve_claude_home("").ok();

    let base_context = user_prompt_submit_context(prompt_text);

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
                "Unable to render the harness lifecycle hook output: {error}"
            );
            0
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
    "Output compression is on for this turn — context is heavy. Read narrower line ranges (offset+limit) instead of whole files. Search before reading: use your host's search tool to locate the exact symbol, then read only the relevant window. Summarize logs and command output instead of pasting them in full. Skill: compression-discipline."
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
/// `keel` installs globally, so this hook fires in every host
/// repo — most of which have no `CLAUDE.md`, no `AGENTS.md`, and no
/// `reviewer` skill. The text therefore states the trivial/non-trivial
/// split inline so the rule is self-contained in any project, and treats
/// project-level convention files as an optional override rather than a
/// required citation. We still pre-empt the "wrapper noise" rationalization
/// because models that pattern-match generic reminders as noise have
/// rationalized past prior versions of this text.
fn post_tool_batch_context() -> String {
    "Closeout check: if this batch changed code with logic edits, multi-file changes, or public-API touches, route the diff through a reviewer pass before closing. Trivial work (docs, formatting, single-line fixes) skips this. Non-trivial code does not self-review. If this reminder feels redundant — re-read the diff and decide deliberately before skipping.".to_string()
}

/// SubagentStart context — injected into every spawned subagent so it starts
/// with the core operating contract instead of blind. Kept compact to avoid
/// burning subagent context on a wall of text.
fn subagent_start_context() -> String {
    "keel iron law for this subagent: (1) Read SYSTEM_MAP and the owning file before claiming behavior. (2) Understand before building — restate the request and research what is needed. (3) Invoke relevant skills if there is even a 1% chance one applies. (4) Find the root cause — trace with file:line evidence before changing anything. Trust the codebase, not your knowledge base. Native MCP tools available: system_map, recall, run_command.".to_string()
}

// ----- PostToolBatch enforcement gates (review gate + working-brief gate) -----
//
// The PostToolBatch hook fires after a batch of tool calls resolves, just
// before the next model turn. Two gates ride here, both DEFAULT-ON, because
// they are the only model-INDEPENDENT way to surface the Iron Law — the harness
// hooks cannot force a Skill()/MCP/tool call, but they CAN inject a reminder
// (and, opt-in, refuse to let a turn close) when a required artifact is missing:
//   * Review gate — fires when this session changed code but no reviewer pass
//     was recorded since the last edit (the BACK of the law: review before close).
//   * Working-brief gate — fires when this session changed code but no working
//     brief was written this session (the FRONT of the law: understand/plan before
//     building). This is the gate that would have caught the failure that motivated
//     it: editing files with no brief, no recall, no map read.
//
// FIRING BEHAVIOR — four modes per gate, selected by its env var (see
// `GateMode` / `gate_mode`):
//   * Escalate (DEFAULT) — the FIRST end-of-turn fire injects the gate message
//     via `hookSpecificOutput.additionalContext` (a non-blocking nudge: the agent
//     is TOLD to run the review / write the brief but the turn is not halted). If
//     the requirement is STILL unmet at a later end-of-turn, the gate escalates to
//     `decision: "block"`. This is the honest answer to "not optional": a hook
//     cannot force a Skill()/tool call, but it can refuse to let the turn close
//     cheaply. First contact never interrupts mid-task; persistent neglect does.
//   * Nudge (`…=nudge`, opt-down) — always a non-blocking reminder, never blocks.
//   * Block (`…=block`, opt-up) — emit `decision: "block"` on every fire so the harness
//     Code halts the turn until the requirement is met.
//   * Off (`…=off`/`0`/`false`/`no`, or `…_MAX_BLOCKS=0`) — disabled; only the
//     generic advisory reminder is emitted.
//
// SAFETY — this is what makes a default-on gate shippable without wedging or
// spamming anyone's session:
//   * Bounded. Each gate fires at most `…_MAX_BLOCKS` times per session (default
//     1 for Nudge/Block, 2 for Escalate so it can nudge once then block once) —
//     whether nudging or blocking — then permanently falls through to the generic
//     advisory. The issued counter strictly increases on every fire and
//     `decide_gate` stops firing once it reaches the cap, so neither a nudge-spam
//     loop nor an infinite Stop/PostToolBatch block loop is possible, regardless
//     of whether the model ever satisfies the gate. This forecloses the documented
//     stop-cascade hazard.
//   * Fail-open everywhere. No session id, no claude_home, unreadable telemetry,
//     or a serialization error all degrade to the advisory reminder, never to a
//     block. A telemetry hiccup can never wedge a turn.
//   * Switches preserved. `CLAUDE_SKILLS_REVIEW_GATE` and
//     `CLAUDE_SKILLS_BRIEF_GATE` each take `off` (disable), `nudge` (advisory-only),
//     `block` (always hard stop), or unset/anything else (the escalating default);
//     `…_MAX_BLOCKS=0` is a second kill switch.
//   * Clearable by actually doing the work. Running `keel review
//     pre-pr|pre-commit|gates` clears the review gate; writing a working brief
//     this session (`keel memory working-brief write`) clears the brief
//     gate. The gates reward the real action, not a token one — though a
//     determined model can still write a junk brief to clear the front gate, which
//     is the acknowledged ceiling of artifact-existence enforcement.
//
// Default-on-as-escalate rationale: these started opt-in and almost nobody flipped
// them on, so the law went unenforced. A revision made them default-on as a HARD
// BLOCK — enforced but stopped work mid-task, disruptive enough that users asked it
// to stop. The next revision made them a non-blocking NUDGE, which never disrupted
// but was free to ignore, so "not optional" was not actually true. The resolution
// is to ESCALATE: warn on first contact (no mid-task interruption), then refuse to
// close cheaply if the requirement is still unmet (real enforcement). Opt-down to
// `=nudge` for advisory-only, opt-up to `=block` for an immediate stop, `=off` to
// disable. Loop-proof and instantly disablable in every mode.

const REVIEW_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE";
const REVIEW_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE_MAX_BLOCKS";

/// Base fire cap for `Nudge`/`Block` modes when the operator did not set a custom
/// `…_MAX_BLOCKS`. One fire per session: the model gets a single reminder (a
/// non-blocking nudge under `=nudge`, or one hard block under `=block`), after
/// which the turn proceeds no matter what. The escalating default uses a higher
/// cap via [`default_max_blocks_for`] so it can nudge once then block once. A cap
/// of 0 disables firing entirely (a second escape hatch alongside the off-switch).
const GATE_DEFAULT_MAX_BLOCKS: u64 = 1;

/// Exposed for bridge `gate-status` so callers can compare counter values
/// against the cap without importing the private `default_max_blocks_for`.
pub(crate) fn default_max_blocks() -> u64 {
    GATE_DEFAULT_MAX_BLOCKS
}

/// Default per-session fire cap for a gate, chosen by mode. `Escalate` needs at
/// least 2 (fire 0 nudges, fire 1 blocks) or it could never escalate past the
/// opening nudge; every other mode keeps the historical single fire. An explicit
/// `…_MAX_BLOCKS` env var always overrides this.
fn default_max_blocks_for(mode: GateMode) -> u64 {
    match mode {
        GateMode::Escalate => 2,
        _ => GATE_DEFAULT_MAX_BLOCKS,
    }
}

/// How a PostToolBatch gate behaves when it fires (code changed, requirement
/// unmet, under the per-session cap). Three modes, parsed from the gate's env
/// var by [`gate_mode`]:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateMode {
    /// Fully disabled. The gate never fires; only the generic advisory reminder
    /// is emitted. Selected by `off` / `0` / `false` / `no`.
    Off,
    /// Inject the gate's message via `hookSpecificOutput.additionalContext` — the
    /// agent is *told* to run the review / write the brief, but the turn is never
    /// halted (no `decision` field). Still bounded by the per-session counter so
    /// the reminder shows at most `…_MAX_BLOCKS` time(s) and never spams every
    /// batch. Opt-down from the escalating default: select with `nudge`.
    Nudge,
    /// Escalated feed-forward. Emit an imperative reminder via
    /// `hookSpecificOutput.additionalContext` on every fire (up to the cap) so the
    /// agent is told in strong terms to satisfy the requirement — but the turn is
    /// NEVER halted (no `decision: "block"`). Select with `block` (case-insensitive).
    Block,
    /// DEFAULT. The honest answer to "not optional": a hook cannot force a
    /// `Skill()`/Agent call, but it can feed corrective context forward so the turn
    /// does not close cheaply. The FIRST fire is an advisory nudge (warn, do not
    /// interrupt mid-task); if the requirement is STILL unmet on a later
    /// end-of-turn the gate ESCALATES to an imperative reminder (still via
    /// `additionalContext`, never a blocking decision). Strictly bounded by the
    /// per-session counter, so the worst case is "one nudge, then one imperative,
    /// then advisory forever" — it can neither be ignored for free nor wedge the
    /// session, and it never stops the turn. Selected by an unset var
    /// or any unrecognized value, so a typo fails safe toward this default.
    Escalate,
}

/// Parse a gate's behavior from its env var. Default-on as an ESCALATING gate
/// (nudge first, block if still unmet).
///
/// Mapping (value trimmed, compared case-insensitively):
///   * `off` / `0` / `false` / `no` → [`GateMode::Off`]
///   * `nudge` → [`GateMode::Nudge`] (opt-down: warn only, never block)
///   * `block` → [`GateMode::Block`] (opt-up: block on every fire)
///   * unset, or anything else → [`GateMode::Escalate`] (the default)
///
/// A typo therefore lands on `Escalate` (warn first, then block if ignored),
/// not on silent disablement and not on an immediate surprise stop — the safest
/// failure direction for a gate whose whole point is to make the requirement
/// progressively harder to skip without ever wedging the session.
fn gate_mode(env_var: &str) -> GateMode {
    match std::env::var(env_var).ok().as_deref().map(str::trim) {
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => GateMode::Off,
            "nudge" => GateMode::Nudge,
            "block" => GateMode::Block,
            _ => GateMode::Escalate,
        },
        None => GateMode::Escalate,
    }
}

/// Review-gate behavior. Default `Nudge`; `CLAUDE_SKILLS_REVIEW_GATE=block`
/// restores the hard stop; `=off` (or `0`/`false`/`no`) disables it entirely.
fn review_gate_mode() -> GateMode {
    gate_mode(REVIEW_GATE_ENV_VAR)
}

fn review_gate_max_blocks() -> u64 {
    std::env::var(REVIEW_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(review_gate_mode()))
}

// ---- Honest-closeout story-gap gate (PostToolBatch) ----
//
// The end-of-turn moment where the user wants "I found these gaps, I'm not
// done" cannot live in a Stop/SubagentStop/SessionEnd hook — those events do not
// accept `hookSpecificOutput.additionalContext` per harness schema, so
// they cannot inject text into the model's context. PostToolBatch is the one
// end-of-turn event that can, so the honest-closeout gate rides here alongside
// the brief/review gates. It is scoped to user-story work: it fires only when the
// current workspace has an ACTIVE SPRINT (story records exist) whose stories are
// not all Done. With no sprint it is silent, so ordinary and question turns are
// untouched — exactly the "if based on user stories, else ignore" contract.
const STORY_CLOSEOUT_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_STORY_CLOSEOUT_GATE";
const STORY_CLOSEOUT_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_STORY_CLOSEOUT_GATE_MAX_BLOCKS";

/// Story-closeout gate behavior. Default `Nudge`;
/// `CLAUDE_SKILLS_STORY_CLOSEOUT_GATE=block` makes it a hard stop; `=off` disables.
fn story_closeout_gate_mode() -> GateMode {
    gate_mode(STORY_CLOSEOUT_GATE_ENV_VAR)
}

fn story_closeout_gate_max_blocks() -> u64 {
    std::env::var(STORY_CLOSEOUT_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(story_closeout_gate_mode()))
}

fn story_closeout_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("story-closeout-gate-blocks")
        .join(key)
}

/// Build the honest-closeout gate message naming each open story as a gap.
/// Keyed on the EMITTED decision (not the mode) so an escalating gate renders
/// the non-blocking phrasing on its first fire and the hard-stop phrasing once
/// it escalates. Both name the loop-back action, the clearing action (finish the
/// stories / mark them done), the bound, and the off-switch.
fn story_closeout_gate_message(
    decision: GateDecision,
    open: &[crate::utility::sprint::OpenStory],
) -> String {
    let mut gaps = String::new();
    for story in open {
        gaps.push_str(&format!(
            "\n  - [{}] {} :: {}",
            story.state, story.id, story.story
        ));
    }
    let preamble = match decision {
        GateDecision::Block => "Honest-closeout gate (CLAUDE_SKILLS_STORY_CLOSEOUT_GATE): sprint NOT complete — now a hard stop.",
        _ => "Honest-closeout reminder (CLAUDE_SKILLS_STORY_CLOSEOUT_GATE): sprint NOT complete.",
    };
    let tail = match decision {
        GateDecision::Block => "Do NOT present this work as done. Mark each open story with `keel sprint advance --id <id> --state done` after review; `keel sprint review` clears this gate when every story is Done. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_STORY_CLOSEOUT_GATE=nudge, =off.",
        _ => "State each open story as an honest gap before claiming done. Mark with `keel sprint advance --id <id> --state done` after review; `keel sprint review` clears when all Done. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_STORY_CLOSEOUT_GATE=nudge, =block, =off.",
    };
    format!("{preamble} Open stories (Definition of Done not met):{gaps}\n{tail}")
}

// ---- Research gate (PostToolBatch) ----
//
// Fires when a session edited code but used no web search or recall tool. The
// Iron Law demands "read first" and "understand before building" — editing
// without any research evidence means the model assumed from stale knowledge.
// Detection: scan tool_timings JSONL for the session and check whether any tool
// name contains "websearch", "web_fetch", "context7", or "recall".

const RESEARCH_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE";
const RESEARCH_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE_MAX_BLOCKS";

/// Research-gate behavior. Default `Escalate` (nudge first, block if ignored);
/// `CLAUDE_SKILLS_RESEARCH_GATE=nudge` keeps it advisory-only; `=off` disables.
fn research_gate_mode() -> GateMode {
    gate_mode(RESEARCH_GATE_ENV_VAR)
}

fn research_gate_max_blocks() -> u64 {
    std::env::var(RESEARCH_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(research_gate_mode()))
}

fn research_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("research-gate-blocks")
        .join(key)
}

/// Whether any research tool was called this session. Scans the tool_timings
/// JSONL for `session_id` and checks whether any record's tool_name contains
/// one of the research-tool substrings: "websearch", "web_fetch", "context7",
/// or "recall". Fail-open: any read/parse problem returns `true` so the gate
/// degrades to advisory.
fn session_has_research_tool(claude_home: &Path, session_id: &str) -> bool {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let Ok(body) = fs::read_to_string(&path) else {
        return true;
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<JsonDocument>(line) else {
            continue;
        };
        if row.get("session_id").and_then(JsonDocument::as_str) != Some(session_id) {
            continue;
        }
        let tool = row
            .get("tool_name")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if tool.contains("websearch")
            || tool.contains("web_fetch")
            || tool.contains("context7")
            || tool.contains("recall")
        {
            return true;
        }
    }
    false
}

fn research_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Research gate (CLAUDE_SKILLS_RESEARCH_GATE): code changed without web search or recall evidence — now a hard stop. Use websearch, context7, or recall before implementing. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_RESEARCH_GATE=nudge, =block, =off.".to_string(),
        _ => "Research gate (CLAUDE_SKILLS_RESEARCH_GATE): code changed without web search or recall evidence. Use websearch, context7, or recall before implementing. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_RESEARCH_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Story-first gate (PostToolBatch) ----
//
// Fires when a session edited code but no user stories were confirmed (no
// `user-story-confirmed` marker file exists for this session). The Iron Law
// demands understanding before building — editing without confirmed user stories
// means implementation may drift from what was requested. Detection: check for
// the marker file at <claude_home>/state/story-first/<session_id>.confirmed.

const STORY_FIRST_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_STORY_FIRST_GATE";
const STORY_FIRST_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_STORY_FIRST_GATE_MAX_BLOCKS";

/// Story-first gate behavior. Default `Escalate` (nudge first, block if
/// ignored); `CLAUDE_SKILLS_STORY_FIRST_GATE=nudge` keeps it advisory-only;
/// `=off` disables.
fn story_first_gate_mode() -> GateMode {
    gate_mode(STORY_FIRST_GATE_ENV_VAR)
}

fn story_first_gate_max_blocks() -> u64 {
    std::env::var(STORY_FIRST_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(story_first_gate_mode()))
}

fn story_first_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("story-first-gate-blocks")
        .join(key)
}

/// Path to the story-confirmed marker file for a session. When this file exists,
/// user stories were confirmed via `keel user-story lint` before implementation.
fn story_confirmed_marker_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("story-first")
        .join(format!("{key}.confirmed"))
}

/// Create the story-confirmed marker file indicating user stories were confirmed
/// for this session. Called from the user-story lint success path. Best-effort:
/// any failure is silently ignored — a missing marker only means the gate may
/// fire once more, which the per-session cap still bounds.
pub fn maybe_record_story_confirmed() {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    // We need a session_id but this is called from the user-story lint surface
    // which may not have one in context. Use a sentinel that will be checked by
    // the marker path. For now, read from the most recent tool-timings to find
    // the active session.
    let session_id = std::env::var("CLAUDE_SESSION_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            // Best-effort: read the most recent tool-timings row for any session.
            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let path = claude_home
                .join("state")
                .join("tool-timings")
                .join(format!("{date}.jsonl"));
            fs::read_to_string(&path)
                .ok()
                .and_then(|body| {
                    body.lines().rev().find_map(|line| {
                        let row = serde_json::from_str::<JsonDocument>(line).ok()?;
                        row.get("session_id")
                            .and_then(JsonDocument::as_str)
                            .map(String::from)
                    })
                })
                .unwrap_or_else(|| "no-session".to_string())
        });
    let marker = story_confirmed_marker_path(&claude_home, &session_id);
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, now_ms().to_string());
}

fn story_first_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Story-first gate (CLAUDE_SKILLS_STORY_FIRST_GATE): code changed without confirmed user stories — now a hard stop. Write stories with `keel user-story lint`, confirm with user, then implement. Bounded per session. Set CLAUDE_SKILLS_STORY_FIRST_GATE=nudge, =off.".to_string(),
        _ => "Story-first gate (CLAUDE_SKILLS_STORY_FIRST_GATE): code changed without confirmed user stories. Write stories with `keel user-story lint`, confirm with user, then implement. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_STORY_FIRST_GATE=nudge, =block, =off.".to_string(),
    }
}

const BRIEF_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE";
const BRIEF_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE_MAX_BLOCKS";

/// Grace window (ms) applied when deciding whether a working brief "belongs to"
/// the current session. A brief written as the session's very first action has a
/// file mtime a few ms BEFORE the session-start timestamp (which is taken from
/// the PostToolUse timing row recorded *after* that write tool completes), so a
/// zero-grace `mtime >= session_start` comparison would falsely reject correct
/// brief-first behavior. 60s comfortably covers tool-execution skew while still
/// rejecting prior-session briefs, which are minutes-to-hours older. Erring on
/// the generous side is deliberate: the gate fails open toward NOT blocking.
const BRIEF_GATE_SESSION_GRACE_MS: u64 = 60_000;

/// Working-brief gate behavior. Default `Nudge`; `CLAUDE_SKILLS_BRIEF_GATE=block`
/// restores the hard stop; `=off` (or `0`/`false`/`no`) disables it entirely.
fn brief_gate_mode() -> GateMode {
    gate_mode(BRIEF_GATE_ENV_VAR)
}

fn brief_gate_max_blocks() -> u64 {
    std::env::var(BRIEF_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(brief_gate_mode()))
}

fn brief_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("brief-gate-blocks")
        .join(key)
}

/// Working-brief gate message. `Nudge` (default) is framed as a non-blocking
/// reminder; `Block` (opt-in) is framed as a hard stop. Both name the clearing
/// action and the off-switch, and both reassure the reminder is bounded.
fn brief_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Working-brief gate (CLAUDE_SKILLS_BRIEF_GATE): code changed without a working brief — now a hard stop. Write one: `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"`. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_BRIEF_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Working-brief reminder (CLAUDE_SKILLS_BRIEF_GATE): code changed without a working brief. Write one: `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"`. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_BRIEF_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Memory-save gate (PostToolBatch) ----
//
// A session that changed code but saved nothing durable to memory gets nudged to
// record what it learned before it forgets mid-task — the symptom the user
// reported. Mirrors the brief gate's "happened this session" mtime shape, scoped
// to the surfaces a `keel memory ...` write lands in. Default-on as Escalate;
// bounded per session; fail-open.
const MEMORY_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE";
const MEMORY_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE_MAX_BLOCKS";

fn memory_gate_mode() -> GateMode {
    gate_mode(MEMORY_GATE_ENV_VAR)
}

fn memory_gate_max_blocks() -> u64 {
    std::env::var(MEMORY_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(memory_gate_mode()))
}

fn memory_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("memory-gate-blocks")
        .join(key)
}

/// Memory-save gate message, keyed on the emitted decision. Both variants name
/// the clearing action (research-cache record or maintenance working-buffer),
/// the bound, and the off-switch.
fn memory_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Memory-save gate (CLAUDE_SKILLS_MEMORY_GATE): code changed without saving to memory — now a hard stop. Record findings: `keel memory research-cache record --question \"...\" --answer \"...\"` or `keel memory maintenance append-working-buffer --note \"...\"`. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_MEMORY_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Memory-save reminder (CLAUDE_SKILLS_MEMORY_GATE): code changed without saving to memory. Record findings: `keel memory research-cache record --question \"...\" --answer \"...\"` or `keel memory maintenance append-working-buffer --note \"...\"`. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_MEMORY_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Sprint-start gate (PostToolBatch) ----
//
// Multi-story scope (a working brief with >=2 acceptance criteria) that has no
// sprint started yet gets nudged to plan one so each story is tracked to Done.
// Default-on as Escalate; bounded per session; fail-open.
const SPRINT_START_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_SPRINT_START_GATE";
const SPRINT_START_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_SPRINT_START_GATE_MAX_BLOCKS";

fn sprint_start_gate_mode() -> GateMode {
    gate_mode(SPRINT_START_GATE_ENV_VAR)
}

fn sprint_start_gate_max_blocks() -> u64 {
    std::env::var(SPRINT_START_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(sprint_start_gate_mode()))
}

fn sprint_start_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("sprint-start-gate-blocks")
        .join(key)
}

/// Sprint-start gate message, keyed on the emitted decision. Both variants name
/// the clearing action (`keel sprint plan`), the working-a-sprint skill, the
/// bound, and the off-switch.
fn sprint_start_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Sprint-start gate (CLAUDE_SKILLS_SPRINT_START_GATE): brief describes multi-story scope but no sprint started — now a hard stop. Run `keel sprint plan` to start the sprint, then use the working-a-sprint skill. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_SPRINT_START_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Sprint-start reminder (CLAUDE_SKILLS_SPRINT_START_GATE): brief describes multi-story scope but no sprint started. Run `keel sprint plan` to start, then use the working-a-sprint skill. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_SPRINT_START_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Learned-skill reminder gate (PostToolBatch) ----
//
// The learning loop generates template-state `learned-<project>` skills the agent
// has not loaded or refined. This reminder surfaces them so the captured
// conventions actually get applied. Default-on as Escalate for consistency, but
// the message stays advisory (load the skill, not "you must"); independent of edit
// count like the closeout gate. Bounded per session; fail-open.
const LEARNED_SKILL_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_LEARNED_SKILL_GATE";
const LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_LEARNED_SKILL_GATE_MAX_BLOCKS";

fn learned_skill_gate_mode() -> GateMode {
    gate_mode(LEARNED_SKILL_GATE_ENV_VAR)
}

fn learned_skill_gate_max_blocks() -> u64 {
    std::env::var(LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(learned_skill_gate_mode()))
}

fn learned_skill_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("learned-skill-gate-blocks")
        .join(key)
}

/// Learned-skill reminder message listing each pending learned skill as a
/// `Skill("...")` load action. Advisory in both variants (a reminder, not
/// enforcement); names the gate, the action, the bound, and the off-switch.
fn learned_skill_gate_message(
    decision: GateDecision,
    briefs: &[crate::runner::learning::SynthesisBrief],
) -> String {
    let mut actions = String::new();
    for brief in briefs {
        actions.push_str(&format!("\n  - Skill(\"{}\")", brief.skill_name));
    }
    let preamble = match decision {
        GateDecision::Block => "Learned-skill reminder (CLAUDE_SKILLS_LEARNED_SKILL_GATE): learned skill(s) not yet loaded — reminder repeated.",
        _ => "Learned-skill reminder (CLAUDE_SKILLS_LEARNED_SKILL_GATE): learned skill(s) not yet loaded.",
    };
    format!("{preamble} Load and refine:{actions}\nAdvisory, bounded per session, never halts the turn. Set CLAUDE_SKILLS_LEARNED_SKILL_GATE=nudge, =off.")
}

/// Newest mtime (ms) across the memory surfaces a session can write to, or `None`
/// when none exist. Scans research-cache records, working-brief files, and the
/// maintenance working buffer — the targets the gate's clearing actions write to.
fn newest_memory_write_ms(claude_home: &Path) -> Option<u64> {
    let candidates = [
        newest_file_mtime_in_dir(&claude_home.join("memory").join("research-cache")),
        newest_file_mtime_in_dir(&crate::utility::working_brief::brief_directory(claude_home)),
        file_mtime_ms(&claude_home.join("memory").join("working-buffer.md")),
    ];
    candidates.into_iter().flatten().max()
}

/// Newest file mtime (ms) directly under `directory`, or `None` when it is
/// missing/unreadable or has no files. Non-recursive: the record stores write
/// flat `<id>.json` files.
fn newest_file_mtime_in_dir(directory: &Path) -> Option<u64> {
    let entries = fs::read_dir(directory).ok()?;
    let mut newest: Option<u64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ms) = file_mtime_ms(&path) {
            newest = Some(newest.map_or(ms, |current| current.max(ms)));
        }
    }
    newest
}

/// Whether durable memory was written for this session. Mirrors
/// [`brief_written_this_session`]: an unknown session start reports satisfied
/// (never block a session we cannot time); otherwise satisfied iff the newest
/// memory write is at or after `session_start_ms` minus the shared grace. A
/// missing/unreadable surface counts as "no write" so the gate still fires.
fn memory_written_this_session(claude_home: &Path, session_start_ms: Option<u64>) -> bool {
    let Some(start) = session_start_ms else {
        return true;
    };
    match newest_memory_write_ms(claude_home) {
        Some(write_ms) => write_ms.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        None => false,
    }
}

/// Whether the most recent working brief applying to `workspace_cwd` describes
/// multi-story scope (>=2 acceptance criteria). Uses the same workspace-match
/// rule as [`newest_brief_mtime_ms`] (empty workspace applies anywhere). Fail-open:
/// an unreadable brief store yields `false` (not multi-story → silent).
fn workspace_brief_is_multi_story(claude_home: &Path, workspace_cwd: &str) -> bool {
    let Ok(briefs) = crate::utility::working_brief::list_briefs(claude_home) else {
        return false;
    };
    let current_key = sanitize_memory_key(workspace_cwd);
    // list_briefs is sorted oldest-first, so the last applicable brief is newest.
    let newest = briefs.into_iter().rev().find(|brief| {
        brief.workspace.trim().is_empty() || sanitize_memory_key(&brief.workspace) == current_key
    });
    match newest {
        Some(brief) => brief.acceptance_criteria.len() >= 2,
        None => false,
    }
}

/// Unix-ms modification time of `path`, or `None` on any error. Fail-open: an
/// unreadable mtime is treated by callers as "no usable timestamp" rather than
/// surfacing an error into the hook.
fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
}

/// Most recent working-brief file mtime (ms) for a brief that applies to
/// `workspace_cwd`, or `None` when no such brief exists. Scans
/// `<claude_home>/working-briefs/*.json` — the same directory the
/// `keel memory working-brief write` surface writes to.
///
/// A brief applies when its stored `workspace` matches `workspace_cwd` (compared
/// through [`sanitize_memory_key`] so path-separator and case differences
/// normalize out) OR its workspace is empty. Empty means a legacy brief written
/// before the field existed, or a write where the cwd could not be resolved —
/// either way it is treated as "applies anywhere" so the workspace scoping never
/// makes an older brief suddenly stop counting. Fail-open: a missing or
/// unreadable directory, or a brief that fails to parse, yields no match for
/// that entry rather than an error.
fn newest_brief_mtime_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let directory = crate::utility::working_brief::brief_directory(claude_home);
    let entries = fs::read_dir(&directory).ok()?;
    let current_key = sanitize_memory_key(workspace_cwd);
    let mut newest: Option<u64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(brief) = crate::utility::working_brief::parse_brief_text(&text) else {
            continue;
        };
        let applies = brief.workspace.trim().is_empty()
            || sanitize_memory_key(&brief.workspace) == current_key;
        if !applies {
            continue;
        }
        if let Some(ms) = file_mtime_ms(&path) {
            newest = Some(newest.map_or(ms, |current| current.max(ms)));
        }
    }
    newest
}

/// Earliest recorded tool-timing (ms) for `session_id`, i.e. an approximation
/// of when this session started doing work. Scans today's AND yesterday's
/// per-day JSONL so a session that began before midnight and continued past it
/// still resolves its true start, rather than taking today's first
/// post-midnight row (which would post-date a brief written late yesterday and
/// trigger one spurious block). Returns `None` when the session has no recorded
/// rows in that window (empty session id, older CC, or unreadable telemetry) so
/// the caller can fail open.
fn session_start_ms(claude_home: &Path, session_id: &str) -> Option<u64> {
    if session_id.trim().is_empty() {
        return None;
    }
    let today = chrono::Local::now().date_naive();
    let mut earliest: Option<u64> = None;
    // offset 0 = today, 1 = yesterday. Two days is enough to span one midnight
    // boundary; sessions longer than that are vanishingly rare and at worst pay
    // one bounded, clearable block.
    for offset in 0..2u64 {
        let Some(date) = today.checked_sub_days(chrono::Days::new(offset)) else {
            break;
        };
        let path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{}.jsonl", date.format("%Y-%m-%d")));
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(row) = serde_json::from_str::<JsonDocument>(line) else {
                continue;
            };
            if row.get("session_id").and_then(JsonDocument::as_str) != Some(session_id) {
                continue;
            }
            if let Some(ms) = row.get("recorded_at_ms").and_then(JsonDocument::as_u64) {
                earliest = Some(earliest.map_or(ms, |current| current.min(ms)));
            }
        }
    }
    earliest
}

/// Whether a working brief exists that plausibly covers this session's work in
/// `workspace_cwd`.
///
/// True when the newest brief applying to `workspace_cwd` (see
/// [`newest_brief_mtime_ms`] for the workspace-match rule) has an mtime at or
/// after `session_start_ms` minus [`BRIEF_GATE_SESSION_GRACE_MS`]. Fail-open in
/// two ways: when the session start is unknown (`None`, e.g. empty session id)
/// we report satisfied so the gate never blocks a session it cannot time; the
/// only way to be unsatisfied is a known session start with no applicable brief
/// recent enough to match it.
fn brief_written_this_session(
    claude_home: &Path,
    workspace_cwd: &str,
    session_start_ms: Option<u64>,
) -> bool {
    let Some(start) = session_start_ms else {
        return true;
    };
    match newest_brief_mtime_ms(claude_home, workspace_cwd) {
        Some(brief_ms) => brief_ms.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateDecision {
    /// Emit the normal generic advisory reminder (the gate did not fire).
    Advisory,
    /// Emit the gate-specific message via `hookSpecificOutput.additionalContext`
    /// — tell the agent to do the work, but do NOT halt the turn. Increments the
    /// per-session counter so it shows at most `max_blocks` time(s). Emitted by
    /// `Nudge` mode always, and by the default `Escalate` mode on its first fire.
    Nudge,
    /// Emit an imperative `additionalContext` reminder and increment the
    /// per-session counter — escalated feed-forward, NOT a turn halt (no
    /// `decision: "block"`). Emitted by `Block` mode always, and by the default
    /// `Escalate` mode once its opening nudge was issued and the requirement is
    /// still unmet.
    Block,
}

/// Pure decision core (no IO, no env) shared by the review gate and the
/// working-brief gate, so the termination guarantee is unit-testable in
/// isolation and identical for both.
///
/// `satisfied` is the gate-specific "requirement already met" signal: for the
/// review gate it means a reviewer pass ran after the last edit; for the brief
/// gate it means a working brief covers this session's work.
///
/// `mode` selects what a fired gate emits:
///   * [`GateMode::Nudge`] → always a non-blocking message ([`GateDecision::Nudge`]).
///   * [`GateMode::Block`] → always a hard stop ([`GateDecision::Block`]).
///   * [`GateMode::Escalate`] (default) → the FIRST fire (`blocks_issued == 0`)
///     is a [`GateDecision::Nudge`]; every later fire is a [`GateDecision::Block`].
///     This is the "warn once, then refuse to close cheaply" behavior that makes
///     skipping the requirement progressively harder without interrupting work
///     mid-task on first contact.
///   * [`GateMode::Off`] → never fires.
///
/// The cap check (`blocks_issued >= max_blocks`) is the termination proof: the
/// caller increments `blocks_issued` on every Nudge OR Block, so the value is
/// strictly monotonic across a session and the function returns `Advisory`
/// forever once the cap is reached. Escalate's default cap is 2 (one nudge + one
/// block), so its worst case is "nudge, then block, then advisory forever" — no
/// infinite loop is possible in any mode.
fn decide_gate(
    mode: GateMode,
    max_blocks: u64,
    blocks_issued: u64,
    edit_count: usize,
    satisfied: bool,
) -> GateDecision {
    if mode == GateMode::Off || max_blocks == 0 {
        return GateDecision::Advisory;
    }
    // No code changed this session — nothing to gate. Pure-research and
    // question-answering turns never fire a gate.
    if edit_count == 0 {
        return GateDecision::Advisory;
    }
    // The gate-specific requirement is already met — nothing to fire on.
    if satisfied {
        return GateDecision::Advisory;
    }
    // Hard cap: stop firing once we have issued the allowed number of
    // nudges/blocks. This is what guarantees the gate cannot loop or spam.
    if blocks_issued >= max_blocks {
        return GateDecision::Advisory;
    }
    match mode {
        GateMode::Nudge => GateDecision::Nudge,
        GateMode::Block => GateDecision::Block,
        // Escalate: warn on first contact, then refuse to close cheaply. The
        // monotonic `blocks_issued` is the escalation clock — fire 0 is the
        // nudge, every later fire is a block, all bounded by `max_blocks`.
        GateMode::Escalate => {
            if blocks_issued == 0 {
                GateDecision::Nudge
            } else {
                GateDecision::Block
            }
        }
        // Unreachable: handled by the early return above. Mapped to Advisory so a
        // future refactor that removes the early return fails safe, not loud.
        GateMode::Off => GateDecision::Advisory,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn read_counter_value(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn review_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("review-gate-blocks")
        .join(key)
}

/// Number of edit-class tool calls recorded for `session_id` today, plus the
/// timestamp of the most recent one and the cwd it ran in. `count == 0` means
/// no code changed this session. Fail-open: any read/parse problem yields a
/// zero-count result so the gate degrades to advisory.
struct SessionEditStats {
    count: usize,
    last_edit_ms: u64,
    last_cwd: String,
}

fn session_edit_stats(claude_home: &Path, session_id: &str) -> SessionEditStats {
    let mut stats = SessionEditStats {
        count: 0,
        last_edit_ms: 0,
        last_cwd: String::new(),
    };
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let Ok(body) = fs::read_to_string(&path) else {
        return stats;
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<JsonDocument>(line) else {
            continue;
        };
        if row.get("session_id").and_then(JsonDocument::as_str) != Some(session_id) {
            continue;
        }
        let tool = row
            .get("tool_name")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default();
        if !is_edit_class_tool(tool) {
            continue;
        }
        stats.count += 1;
        let ms = row
            .get("recorded_at_ms")
            .and_then(JsonDocument::as_u64)
            .unwrap_or(0);
        if ms >= stats.last_edit_ms {
            stats.last_edit_ms = ms;
            stats.last_cwd = row
                .get("cwd")
                .and_then(JsonDocument::as_str)
                .unwrap_or_default()
                .to_string();
        }
    }
    stats
}

/// Timestamp (ms) of the last recorded review for `workspace_cwd`, or `None`
/// when no review marker exists. Written by `record_review_gate_clear` from the
/// `keel review` surface.
fn review_marker_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let key = sanitize_memory_key(workspace_cwd);
    let path = claude_home
        .join("state")
        .join("review-gate")
        .join(format!("{key}.reviewed"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// Record that a reviewer pass ran for the current workspace, clearing the
/// review gate for edits made up to now. Called from the `keel review`
/// surface (pre-pr / pre-commit / gates). Best-effort: any failure is silently
/// ignored — a missing marker only means the gate may block once more, which
/// the per-session cap still bounds.
pub fn record_review_gate_clear() {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let key = sanitize_memory_key(&display_path(&cwd));
    let dir = claude_home.join("state").join("review-gate");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(format!("{key}.reviewed")), now_ms().to_string());
}

/// Review gate message, keyed on the EMITTED decision (not the mode) so an
/// escalating gate renders nudge phrasing on its first fire and block phrasing
/// once it escalates. `Block` is framed as a hard stop, `Nudge` as a
/// non-blocking reminder; both name the clearing action and the off-switch and
/// reassure the reminder is bounded. `Advisory` never reaches here.
fn review_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Review gate (CLAUDE_SKILLS_REVIEW_GATE): code changed without a reviewer pass — now a hard stop. Run `keel review pre-pr` or invoke the reviewer skill on the diff. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_REVIEW_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Review reminder (CLAUDE_SKILLS_REVIEW_GATE): code changed without a reviewer pass. Run `keel review pre-pr` or invoke the reviewer skill before closing. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_REVIEW_GATE=nudge, =block, =off.".to_string(),
    }
}

/// Emit the advisory PostToolBatch reminder (the default, gate-disabled path and
/// every fail-open branch). Mirrors the lifecycle render so the output is
/// identical to what `run_hook_lifecycle("post-tool-batch")` produces.
fn emit_post_tool_batch_advisory(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let Some(event) = event_by_name("PostToolBatch") else {
        return 0;
    };
    let payload = render_lifecycle_payload(event, &post_tool_batch_context());
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch advisory output: {error}"
            );
            1
        }
    }
}

/// Emit a NON-BLOCKING feed-forward PostToolBatch payload with IMPERATIVE tone.
///
/// Previously emitted `decision: "block"` which halted the turn. Now emits
/// `hookSpecificOutput.additionalContext` (identical shape to the nudge) but
/// with imperative language ("Do NOT present this work as done") so the gate
/// still asserts its requirement without stopping the turn. The per-session
/// counter and cap logic are unchanged — the monotonic termination guarantee
/// remains intact. Falls back to the advisory reminder on render failure.
fn emit_post_tool_batch_block(
    reason: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolBatch",
            "additionalContext": reason,
        },
        "suppressOutput": true,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch block output: {error}"
            );
            emit_post_tool_batch_advisory(standard_output, standard_error)
        }
    }
}

/// Emit a NON-BLOCKING PostToolBatch nudge: the gate's `message` is injected via
/// `hookSpecificOutput.additionalContext` so the agent is told to do the work,
/// but the turn is never halted (no `decision` field). This is the default
/// firing path — the fix for "stop mid-task": the agent gets the reminder and
/// keeps going. Falls back to the generic advisory reminder if rendering fails.
/// The caller increments the gate's counter BEFORE calling this so the
/// per-session cap advances even on a render error (the monotonic counter is the
/// termination guarantee).
fn emit_post_tool_batch_nudge(
    message: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolBatch",
            "additionalContext": message,
        },
        "suppressOutput": true,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch nudge output: {error}"
            );
            emit_post_tool_batch_advisory(standard_output, standard_error)
        }
    }
}

/// PostToolBatch dispatcher running the three default-on enforcement gates.
///
/// Reads stdin for `session_id` (the harness delivers the hook payload there,
/// same as UserPromptSubmit). Evaluates the working-brief gate FIRST (the front
/// of the Iron Law — understand before building), then the review gate (the
/// back — review before close), then the honest-closeout gate.
///
/// Each gate fires at most once per turn and is independently bounded by its own
/// per-session counter. The DEFAULT firing behavior is ESCALATE: the first fire
/// is a non-blocking nudge (the gate's message via
/// `hookSpecificOutput.additionalContext` — told to do the work, turn not halted,
/// no mid-task interruption), and if the requirement is still unmet on a later
/// turn the gate escalates to a `decision: "block"` hard stop. Setting a gate's
/// env var to `nudge` keeps it advisory-only; `block` blocks on every fire; `off`
/// disables it.
///
/// The worst case across a whole session is, per gate, one nudge then one block
/// (the escalate cap of 2), after which it falls through to the generic advisory
/// forever. When all gates are off this is byte-identical to the advisory reminder.
fn run_hook_post_tool_batch(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let review_mode = review_gate_mode();
    let brief_mode = brief_gate_mode();
    let closeout_mode = story_closeout_gate_mode();
    let memory_mode = memory_gate_mode();
    let sprint_mode = sprint_start_gate_mode();
    let learned_mode = learned_skill_gate_mode();
    let research_mode = research_gate_mode();
    let story_first_mode = story_first_gate_mode();
    let review_on = review_mode != GateMode::Off && review_gate_max_blocks() > 0;
    let brief_on = brief_mode != GateMode::Off && brief_gate_max_blocks() > 0;
    let closeout_on = closeout_mode != GateMode::Off && story_closeout_gate_max_blocks() > 0;
    let memory_on = memory_mode != GateMode::Off && memory_gate_max_blocks() > 0;
    let sprint_on = sprint_mode != GateMode::Off && sprint_start_gate_max_blocks() > 0;
    let learned_on = learned_mode != GateMode::Off && learned_skill_gate_max_blocks() > 0;
    let research_on = research_mode != GateMode::Off && research_gate_max_blocks() > 0;
    let story_first_on = story_first_mode != GateMode::Off && story_first_gate_max_blocks() > 0;

    // All gates off: skip stdin entirely and emit the advisory reminder. This
    // keeps the fully-disabled path cheap and side-effect-free.
    if !review_on
        && !brief_on
        && !closeout_on
        && !memory_on
        && !sprint_on
        && !learned_on
        && !research_on
        && !story_first_on
    {
        return emit_post_tool_batch_advisory(standard_output, standard_error);
    }

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

    // Fail-open: without a claude_home we cannot read telemetry or the gate
    // counters, so degrade to advisory rather than risk a wedged turn.
    let Ok(claude_home) = resolve_claude_home("") else {
        return emit_post_tool_batch_advisory(standard_output, standard_error);
    };

    let stats = session_edit_stats(&claude_home, session_id);

    // Brief and review gates only fire when code changed this session — pure
    // research and question turns never trip them. The honest-closeout gate is
    // evaluated separately below because it keys off sprint state, not edits: an
    // incomplete sprint matters at closeout even on a no-edit summary turn.
    if stats.count > 0 {
        // Brief gate FIRST (front of the law: understand/plan before building).
        if brief_on {
            let start = session_start_ms(&claude_home, session_id);
            let satisfied = brief_written_this_session(&claude_home, &stats.last_cwd, start);
            let blocks_path = brief_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                brief_mode,
                brief_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                // Increment FIRST so the cap counts down even if rendering fails or
                // the model ignores the message — the monotonic counter is the
                // termination guarantee and must advance before anything else.
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    brief_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Review gate (back of the law: review before close).
        if review_on {
            let reviewed = review_marker_ms(&claude_home, &stats.last_cwd)
                .map(|marker_ms| marker_ms >= stats.last_edit_ms)
                .unwrap_or(false);
            let blocks_path = review_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                review_mode,
                review_gate_max_blocks(),
                blocks_issued,
                stats.count,
                reviewed,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    review_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Memory-save gate THIRD (record what you learned before forgetting it).
        if memory_on {
            let start = session_start_ms(&claude_home, session_id);
            let satisfied = memory_written_this_session(&claude_home, start);
            let blocks_path = memory_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                memory_mode,
                memory_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    memory_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Research gate: fires when code changed but no web search or recall
        // tool was used this session. Satisfied when any research tool fired.
        if research_on {
            let satisfied = session_has_research_tool(&claude_home, session_id);
            let blocks_path = research_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                research_mode,
                research_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    research_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Sprint-start gate FOURTH (track multi-story scope as a sprint).
        if sprint_on {
            let multi_story = workspace_brief_is_multi_story(&claude_home, &stats.last_cwd);
            // Satisfied when a sprint EXISTS for the workspace (Some, whether open
            // or done). No sprint (Ok(None)) → unsatisfied → fire. Err → fail-open
            // (treat as satisfied so an unreadable store never blocks).
            let sprint_exists = matches!(
                crate::utility::sprint::open_stories_for_workspace(&claude_home, &stats.last_cwd),
                Ok(Some(_)) | Err(_)
            );
            // Only applicable to multi-story scope: a single-story (or no) brief
            // never needs a sprint, so report satisfied to keep the gate silent.
            let satisfied = !multi_story || sprint_exists;
            let blocks_path = sprint_start_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                sprint_mode,
                sprint_start_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    sprint_start_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Story-first gate: fires when code changed but no user stories were
        // confirmed for this session. Satisfied when the confirmed marker exists.
        if story_first_on {
            let marker = story_confirmed_marker_path(&claude_home, session_id);
            let satisfied = marker.exists();
            let blocks_path = story_first_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                story_first_mode,
                story_first_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    story_first_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }
    }

    // Honest-closeout gate (final honesty: do not present an incomplete sprint as
    // done). Scoped to user-story work: fires only when the workspace has an
    // ACTIVE sprint with open stories. Silent when there is no sprint, so ordinary
    // and question turns are untouched — independent of edit count by design.
    if closeout_on {
        if let Some(decision_and_message) =
            evaluate_story_closeout_gate(&claude_home, session_id, &stats, closeout_mode)
        {
            let (decision, message, blocks_path) = decision_and_message;
            let _ = increment_counter_file(&blocks_path);
            return emit_gate_decision(decision, message, standard_output, standard_error);
        }
    }

    // Learned-skill reminder (apply the loop's captured conventions). Independent
    // of edit count like the closeout gate: a pending learned skill matters even on
    // a no-edit turn. Silent when nothing is pending.
    if learned_on {
        if let Some(decision_and_message) =
            evaluate_learned_skill_gate(&claude_home, session_id, learned_mode)
        {
            let (decision, message, blocks_path) = decision_and_message;
            let _ = increment_counter_file(&blocks_path);
            return emit_gate_decision(decision, message, standard_output, standard_error);
        }
    }

    // No gate fired → advisory reminder.
    emit_post_tool_batch_advisory(standard_output, standard_error)
}

/// Decide whether the honest-closeout gate fires this turn, returning the
/// `(decision, message, counter_path)` when it does (so the caller increments the
/// counter and emits) or `None` to fall through. Fail-open: any error resolving
/// the workspace or reading the sprint store yields `None`.
///
/// Fires when the workspace has an active sprint (`Some(open)`) with at least one
/// open story, the gate is under its per-session cap, and the mode is not Off.
/// Stays silent when there is no sprint (`None`) or every story is Done
/// (`Some(empty)`).
fn evaluate_story_closeout_gate(
    claude_home: &Path,
    session_id: &str,
    stats: &SessionEditStats,
    mode: GateMode,
) -> Option<(GateDecision, String, PathBuf)> {
    // Resolve the workspace the same way the sprint CLI does so the slug — and
    // therefore the store directory — matches the records `sprint plan` wrote.
    // Prefer the cwd of the last edit (set when this session changed code), else
    // the repository root, else the process cwd.
    let workspace_root = if !stats.last_cwd.trim().is_empty() {
        stats.last_cwd.clone()
    } else {
        let repo_root: Option<PathBuf> = resolve_repository_root("")
            .ok()
            .or_else(|| std::env::current_dir().ok());
        match repo_root {
            Some(path) => display_path(&path),
            None => String::new(),
        }
    };
    if workspace_root.is_empty() {
        return None;
    }
    let open =
        match crate::utility::sprint::open_stories_for_workspace(claude_home, &workspace_root) {
            Ok(Some(open)) if !open.is_empty() => open,
            // No active sprint, sprint complete, or read error → silent / fail-open.
            _ => return None,
        };
    let blocks_path = story_closeout_gate_blocks_path(claude_home, session_id);
    let blocks_issued = read_counter_value(&blocks_path);
    // edit_count is passed as 1 (applicable): the gate's applicability is the
    // active incomplete sprint, not whether this specific turn edited code, so it
    // must fire on a no-edit closeout turn too. `satisfied` is false here because
    // we already filtered to a non-empty open-story set.
    let decision = decide_gate(
        mode,
        story_closeout_gate_max_blocks(),
        blocks_issued,
        1,
        false,
    );
    if decision == GateDecision::Advisory {
        return None;
    }
    Some((
        decision,
        story_closeout_gate_message(decision, &open),
        blocks_path,
    ))
}

/// Decide whether the learned-skill reminder fires this turn, returning the
/// `(decision, message, counter_path)` when it does or `None` to fall through.
/// Independent of edit count (passes 1 to `decide_gate`): applicability is the
/// existence of a pending template-state learned skill, like the closeout gate.
/// Fail-open: an empty brief set (nothing pending) yields `None`.
fn evaluate_learned_skill_gate(
    claude_home: &Path,
    session_id: &str,
    mode: GateMode,
) -> Option<(GateDecision, String, PathBuf)> {
    let briefs = crate::runner::learning::collect_synthesis_briefs(claude_home);
    if briefs.is_empty() {
        return None;
    }
    let blocks_path = learned_skill_gate_blocks_path(claude_home, session_id);
    let blocks_issued = read_counter_value(&blocks_path);
    let decision = decide_gate(
        mode,
        learned_skill_gate_max_blocks(),
        blocks_issued,
        1,
        false,
    );
    if decision == GateDecision::Advisory {
        return None;
    }
    Some((
        decision,
        learned_skill_gate_message(decision, &briefs),
        blocks_path,
    ))
}

/// Route a fired gate's [`GateDecision`] to the matching emitter: `Nudge` →
/// non-blocking `additionalContext`, `Block` → `decision: "block"`. `Advisory`
/// should never reach here (the caller only emits on a fired gate) but maps to
/// the generic advisory so the function is total and fails safe.
fn emit_gate_decision(
    decision: GateDecision,
    message: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    match decision {
        GateDecision::Nudge => emit_post_tool_batch_nudge(message, standard_output, standard_error),
        GateDecision::Block => emit_post_tool_batch_block(message, standard_output, standard_error),
        GateDecision::Advisory => emit_post_tool_batch_advisory(standard_output, standard_error),
    }
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
        let _ = writeln!(standard_error, "keel raw-output prune failed: {error}");
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
        let _ = writeln!(standard_error, "keel tool-timings prune failed: {error}");
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
        let _ = writeln!(standard_error, "keel observation prune failed: {error}");
    }
}

/// Run the autonomous learning cycle at session end: distill the session's
/// observations into instincts and evolve trusted clusters into generated
/// skills. Fully automatic — no slash command. Set
/// `CLAUDE_SKILLS_LEARNING=off` to disable. Errors are swallowed so a learning
/// failure can never fail the SessionEnd hook.
pub(crate) fn run_session_end_learning(standard_error: &mut dyn Write) {
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
            "keel learn: recorded {} instinct(s), generated {} skill(s) and {} agent(s)",
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
    matches!(
        event_name,
        "SessionStart" | "PreCompact" | "SessionEnd" | "CwdChanged"
    )
}

/// Idempotently re-assert the `keel` MCP registration at session start.
///
/// This is the drift self-heal: `register_mcp_server` writes `~/.claude.json`
/// only when the live entry differs from the desired one (which carries
/// `alwaysLoad: true`), so this costs nothing on a healthy config and silently
/// repairs an entry left stale by any binary-swap path that never re-ran
/// install/update/repair. Honors `CLAUDE_TARGET_OVERRIDE` through
/// `resolve_claude_home`, and only writes for a standard `~/.claude` home
/// (`self_heal_registration` guards that), so the suite's throwaway homes are
/// never touched.
///
/// Best-effort: every failure path is swallowed to stderr. The caller must not
/// change its exit code based on this — the SessionStart context render is the
/// load-bearing work; MCP registration is additive.
fn maybe_self_heal_mcp_registration(standard_error: &mut dyn Write) {
    if std::env::var(MCP_SELF_HEAL_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(_) => return,
    };
    match crate::manager::mcp_register::self_heal_registration(&claude_home) {
        // Skipped (non-standard home) or already current → nothing to report.
        None | Some(Ok(crate::manager::mcp_register::McpRegistration::AlreadyCurrent)) => {}
        Some(Ok(crate::manager::mcp_register::McpRegistration::Added)) => {
            let _ = writeln!(
                standard_error,
                "keel: registered keel MCP server in ~/.claude.json (alwaysLoad). Restart the harness to load the tools into context."
            );
        }
        Some(Ok(crate::manager::mcp_register::McpRegistration::Updated)) => {
            let _ = writeln!(
                standard_error,
                "keel: repaired drifted keel MCP entry in ~/.claude.json (alwaysLoad). Restart the harness to load the tools into context."
            );
        }
        Some(Err(error)) => {
            let _ = writeln!(standard_error, "keel: MCP self-heal skipped ({error})");
        }
    }
}

/// SessionEnd dispatch body with injectable stdin so the real arm ordering is
/// testable. Runs the auto-capture FIRST (it needs the `session_id` from the
/// payload), then delegates to the lifecycle path for the existing SessionEnd
/// side effects (system-map refresh, store prunes, learning). The ordering is
/// load-bearing — capture must read the observation log before the lifecycle
/// path's prune could touch it — so it is exercised end-to-end by a test that
/// drives this function with injected bytes, exactly as `run_hook_post_tool_batch`
/// is tested.
fn run_hook_session_end(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    maybe_capture_session_summary(standard_input, standard_error);
    run_hook_lifecycle("session-end", standard_output, standard_error)
}

/// Bridge-compatible SessionEnd: runs summary capture then learning WITHOUT
/// stdin parsing (the bridge passes the session id directly). Calls
/// `maybe_capture_session_summary_with_id` so the capture can scope itself
/// without reading the hook JSON payload.
pub(crate) fn run_bridge_session_end(
    claude_home: &std::path::Path,
    session_id: &str,
    standard_error: &mut dyn Write,
) {
    maybe_capture_session_summary_with_id(claude_home, session_id, standard_error);
    run_session_end_learning(standard_error);
}

/// Bridge-compatible variant of [`maybe_capture_session_summary`]: takes the
/// session id directly instead of reading it from the hook stdin payload.
fn maybe_capture_session_summary_with_id(
    _claude_home: &std::path::Path,
    session_id: &str,
    standard_error: &mut dyn Write,
) {
    if std::env::var(SESSION_CAPTURE_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    if session_id.trim().is_empty() {
        return;
    }

    let Some(summary) = build_session_summary(session_id) else {
        return;
    };

    let mut capture_stderr = Vec::new();
    let code = crate::utility::memory_families::run_memory_family_command(
        "memory",
        "research-cache",
        &[
            "record".to_string(),
            "--question".to_string(),
            summary.question,
            "--answer".to_string(),
            summary.answer,
            "--source".to_string(),
            format!("auto-capture session {session_id}"),
        ],
        &mut std::io::sink(),
        &mut capture_stderr,
    );
    if code != 0 {
        let _ = writeln!(
            standard_error,
            "keel: session auto-capture skipped ({})",
            String::from_utf8_lossy(&capture_stderr).trim()
        );
    }
}

/// Auto-capture a one-line work summary to memory at SessionEnd.
///
/// This is the "after do, save to memory" half of the contract: the next
/// session starts informed without the model having to remember to write a
/// note. It reuses the behavioral observation log (the same edit/command
/// signatures the learning loop reads) to summarize what this session actually
/// did, then writes it through `memory research-cache record` — the path that
/// now syncs the recall index (s4), so the summary is immediately recallable.
///
/// Silent on sessions that did no edit-class work: a pure research or question
/// turn produces no summary, so the memory store is not polluted with noise.
///
/// Best-effort by contract: every failure path returns without writing and
/// without changing the caller's exit code. The SessionEnd prunes and learning
/// cycle are the load-bearing work; this capture is additive.
fn maybe_capture_session_summary(standard_input: &mut dyn Read, standard_error: &mut dyn Write) {
    if std::env::var(SESSION_CAPTURE_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }

    // The session id arrives on stdin (the harness writes the hook payload then
    // closes the handle). Without it we cannot scope the summary to this
    // session, so fail open — silently skip rather than summarize the wrong work.
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
    if session_id.trim().is_empty() {
        return;
    }

    let Some(summary) = build_session_summary(session_id) else {
        return; // No edit-class work this session — stay silent.
    };

    // Write through the family command so the capture inherits the s4 index
    // sync. --question/--answer are the research-cache record fields; we frame
    // the summary as a recallable Q/A note. Errors are swallowed: a failed
    // capture must never wedge SessionEnd. stdout is discarded (the record id
    // print is noise here); only stderr is surfaced on failure.
    let mut capture_stderr = Vec::new();
    let code = crate::utility::memory_families::run_memory_family_command(
        "memory",
        "research-cache",
        &[
            "record".to_string(),
            "--question".to_string(),
            summary.question,
            "--answer".to_string(),
            summary.answer,
            "--source".to_string(),
            format!("auto-capture session {session_id}"),
        ],
        &mut std::io::sink(),
        &mut capture_stderr,
    );
    if code != 0 {
        let _ = writeln!(
            standard_error,
            "keel: session auto-capture skipped ({})",
            String::from_utf8_lossy(&capture_stderr).trim()
        );
    }
}

/// A session work summary framed as a recallable question/answer pair.
struct SessionSummary {
    question: String,
    answer: String,
}

/// Build a work summary for `session_id` from this session's edit-class
/// behavioral observations, or `None` when the session edited nothing.
///
/// Reads today's observation rows (the learning loop's source), filters to this
/// session's edit/command signatures, and renders a compact "what changed"
/// line: the working directory, the count of edits, the distinct file
/// extensions touched, and the distinct command signatures run. This is
/// deliberately low-cardinality (extensions and command verbs, not full paths)
/// so the note is a useful recall anchor without leaking long arguments.
fn build_session_summary(session_id: &str) -> Option<SessionSummary> {
    let rows = crate::runner::observation::iter_recent_rows(1).ok()?;
    let mut edit_count = 0usize;
    let mut command_count = 0usize;
    let mut extensions: Vec<String> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    let mut cwd = String::new();
    for row in rows {
        if row.session_id != session_id {
            continue;
        }
        if cwd.is_empty() && !row.cwd.is_empty() {
            cwd = row.cwd.clone();
        }
        if let Some(extension) = row.signature.strip_prefix("edit:") {
            edit_count += 1;
            let extension = extension.to_string();
            if !extensions.contains(&extension) {
                extensions.push(extension);
            }
        } else {
            command_count += 1;
            if !commands.contains(&row.signature) {
                commands.push(row.signature.clone());
            }
        }
    }

    // Only capture when the session actually edited code. Command-only sessions
    // (ran tests, browsed git) are not durable "work done" worth a memory note.
    if edit_count == 0 {
        return None;
    }

    // Use only the final path component, not the full cwd: the full path can
    // carry a username or other sensitive directory names, and the doc contract
    // promises low-cardinality anchors, not full paths. `file_name` handles
    // both `/` and `\` separators via the OS path parser.
    let workspace = if cwd.is_empty() {
        "unknown workspace".to_string()
    } else {
        std::path::Path::new(&cwd)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "unknown workspace".to_string())
    };
    let question = format!("What was done in {workspace} on {}?", today_date_string());
    let mut answer = format!(
        "Edited {edit_count} file(s) ({}).",
        if extensions.is_empty() {
            "no recorded extension".to_string()
        } else {
            extensions.join(", ")
        }
    );
    if command_count > 0 {
        answer.push_str(&format!(
            " Ran {command_count} command(s): {}.",
            commands.join(", ")
        ));
    }
    Some(SessionSummary { question, answer })
}

/// Local calendar date as `YYYY-MM-DD`, matching the observation-log naming so a
/// captured summary's date lines up with the rows it was built from.
fn today_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
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
            "keel lifecycle memory refresh failed: {}",
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

        None => "Workspace memory system map: unavailable; create it with keel memory scope resolve --create-missing --refresh-system-map before repo-structure claims.".to_string(),

    }
}

/// Byte budget for the whole pushed workspace digest. The SessionStart context
/// must stay under the ~9KB truncation ceiling (see the cap test); the compact
/// bootstrap + memory pointer already spend most of that, so the digest gets a
/// deliberately small slice. Sections are capped individually below this so the
/// digest degrades gracefully (drop the tail) rather than blowing the ceiling.
const WORKSPACE_DIGEST_MAX_BYTES: usize = 2200;
/// Per-section caps inside the digest. The system-map head is the most valuable
/// (it answers "what is this repo" without a tool call), so it gets the largest
/// share; the brief and memory note are one-liners pointing the model at detail
/// it can pull with `recall`/`brief_get` if needed.
const DIGEST_MAP_HEAD_MAX_BYTES: usize = 1400;
const DIGEST_BRIEF_MAX_BYTES: usize = 500;
const DIGEST_MEMORY_MAX_BYTES: usize = 300;

/// Build a bounded digest of ACTUAL workspace memory content to PUSH into
/// context at session start and post-compact — so the agent does not have to
/// blind-search (call `system_map`/`recall`) before it knows the basics.
///
/// Three sections, each independently capped and any of which may be empty:
///   1. The head of the workspace SYSTEM_MAP.md (the structural map), so
///      "what is this project / where does X live" is answered up front.
///   2. The newest working brief for this workspace (request + first acceptance
///      criterion), so a resumed or fresh session sees the active intent.
///   3. The most recent durable memory note (research-cache record, which now
///      includes the s5 SessionEnd auto-capture), so "what was last done here"
///      is visible without a recall call.
///
/// Returns an empty string when nothing is available (first run in a fresh
/// workspace), so the caller appends nothing. The whole digest is truncated to
/// [`WORKSPACE_DIGEST_MAX_BYTES`] on a line boundary as a final guard. This is
/// the PUSH half of the contract; the model can still PULL the full artifacts
/// with `system_map`, `recall`, and `brief_get` when it needs more than the head.
fn workspace_memory_digest() -> String {
    let Ok(claude_home) = resolve_claude_home("") else {
        return String::new();
    };
    let Ok(workspace_root) = std::env::current_dir() else {
        return String::new();
    };
    let workspace_display = display_path(&workspace_root);

    let mut sections: Vec<String> = Vec::new();

    // 1. System-map head.
    if let Some(map_path) = memory_system_map_path_for_workspace(&workspace_root) {
        if let Ok(map_body) = fs::read_to_string(&map_path) {
            let head = truncate_on_line_boundary(map_body.trim(), DIGEST_MAP_HEAD_MAX_BYTES);
            if !head.trim().is_empty() {
                sections.push(format!(
                    "## Workspace map (head; full map at {})\n{head}",
                    display_path(&map_path)
                ));
            }
        }
    }

    // 2. Newest working brief for THIS workspace (fall back to newest overall
    //    when none is workspace-tagged, since legacy briefs have no workspace).
    if let Ok(briefs) = crate::utility::working_brief::list_briefs(&claude_home) {
        let newest = briefs
            .iter()
            .rev()
            .find(|brief| brief.workspace == workspace_display)
            .or_else(|| briefs.last());
        if let Some(brief) = newest {
            let mut line = format!("## Active working brief ({})\n{}", brief.id, brief.request);
            if let Some(first_criterion) = brief.acceptance_criteria.first() {
                line.push_str(&format!("\nAcceptance: {first_criterion}"));
            }
            sections.push(truncate_on_line_boundary(&line, DIGEST_BRIEF_MAX_BYTES));
        }
    }

    // 3. Open work items for THIS workspace (the dependency-aware work graph).
    //    Pushing ready + blocked items every session is the anti-drift property:
    //    work discovered but not finished (including `discovered-from` captures)
    //    stays reachable across compaction instead of being dropped. Mirrors the
    //    sprint digest but at the finer-grained task-graph level.
    if let Ok(Some(open)) =
        crate::utility::work_graph::open_work_items_for_workspace(&claude_home, &workspace_display)
    {
        if !open.is_empty() {
            let mut lines = String::from(
                "## Open work items (keel work — finish or explicitly defer; do not drop)",
            );
            for item in open.iter().take(12) {
                let blockers = if item.open_blockers.is_empty() {
                    "ready".to_string()
                } else {
                    format!("blocked on {}", item.open_blockers.join(", "))
                };
                lines.push_str(&format!(
                    "\n  - [{}] {} :: {} ({blockers})",
                    item.status, item.id, item.title
                ));
            }
            sections.push(truncate_on_line_boundary(&lines, DIGEST_BRIEF_MAX_BYTES));
        }
    }

    // 4. Most recent durable memory note (includes s5 SessionEnd auto-capture).
    let store =
        crate::utility::record_store::RecordStore::new(&claude_home, "memory/research-cache");
    if let Ok(mut records) = store.list_records() {
        // Ids are time-ordered hex (rc-<millis:x>), so the last id is newest.
        records.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, fields)) = records.last() {
            let lookup = |key: &str| {
                fields
                    .iter()
                    .find(|(field_key, _)| field_key == key)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default()
            };
            let question = lookup("question");
            let answer = lookup("answer");
            if !question.is_empty() || !answer.is_empty() {
                let line = format!("## Most recent memory note\nQ: {question}\nA: {answer}");
                sections.push(truncate_on_line_boundary(&line, DIGEST_MEMORY_MAX_BYTES));
            }
        }
    }

    if sections.is_empty() {
        return String::new();
    }

    let header = "# Workspace memory (pushed so you need not blind-search; pull more with system_map/recall/brief_get)";
    let body = format!("{header}\n\n{}", sections.join("\n\n"));
    truncate_on_line_boundary(&body, WORKSPACE_DIGEST_MAX_BYTES)
}

/// Truncate `text` to at most `max_bytes` on a line boundary for the workspace
/// digest, appending a short elision marker. A thin wrapper over the shared,
/// UTF-8-safe `skill_match::truncate_on_line_boundary` so the digest and the
/// per-prompt skill brief share one correct implementation (the earlier local
/// copy sliced `&str` by raw byte index and panicked on a multibyte char at the
/// boundary — map/brief/note text routinely contains em-dashes and smart quotes).
fn truncate_on_line_boundary(text: &str, max_bytes: usize) -> String {
    crate::utility::skill_match::truncate_on_line_boundary(text, max_bytes, "\n…[truncated]")
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

pub(crate) fn sanitize_memory_key(value: &str) -> String {
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
    // invisible to anyone who ran `keel hook` to learn what was
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

    let _ = writeln!(standard_output, "Usage: keel hook [{joined}]");
}

fn run_hook_diagnose(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook diagnose");
    flag_set.string_flag("format", "text");
    flag_set.string_flag("claude-home", "");

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

    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
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

        let _ = writeln!(output, "keel hook diagnose");
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
    // macOS, filesystems are case-sensitive and `~/.claude/keel` is a
    // genuinely different file from `~/.the harness/keel` — lowercasing
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
    // claude_home/elsewhere/keel.exe) just because the file name
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
                // keel versions that haven't been re-installed yet.
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

/// One managed hook entry as the harness's `args` exec form (added in CC 2.1.139).
///
/// `command` is the bare executable path; `args` is the argv that follows. The harness
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

/// Human-readable summary of what `keel hook install` writes for the
/// PreToolUse event. Used by `keel hook diagnose` to surface the
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
            serde_json::json!(0.06),
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
/// full uninstall does not leave the harness firing hooks at a deleted binary.
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

/// True if `command_entry` is a managed keel hook (either the modern
/// args-form CC 2.1.139+ or any legacy single-string shape we shipped earlier).
///
/// Detection is permissive on purpose. `keel hook uninstall` runs
/// against arbitrary user settings that may have been written by an older
/// version of this binary, so we accept both shapes:
///
///   1. Args form: `{"command": "<exe>", "args": ["hook", "<slug>"]}` where
///      `<exe>` ends in `keel` (with or without `.exe`) and `<slug>`
///      matches a row in `HOOK_EVENTS`.
///
///   2. Legacy string form: `{"command": "<single-string>"}` where the string
///      mentions `keel` together with `hook <slug>` or
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

/// True if `command` resolves to the keel binary (case-insensitive
/// basename match — Windows file systems are case-insensitive and the args
/// form embeds the exact path string CC will invoke).
fn command_path_is_managed_executable(command: &str) -> bool {
    let basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase());

    matches!(basename.as_deref(), Some("keel") | Some("keel.exe"))
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

    let plain_managed = normalized.contains("keel")
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
        let hook_path = temp_hook_path("keel-hook-payload");

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

            "command": "keel hook instructions --format json"

          }

        ]

      }

    ]

  }

}

"#,
        )
        .unwrap();

        let rendered = build_hooks_payload(&hook_path, Path::new(r"C:\tools\keel.exe")).unwrap();

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
        let hook_path = temp_hook_path("keel-hook-command-prefix");

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
                // settings.json by `keel hook install`, so the
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
            Path::new(r"C:\Users\Example User\.claude\keel.exe")
        } else {
            Path::new("/home/example/.claude/keel")
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

        // Legacy single-string form (older keel versions): plain
        // string mentioning `keel` and a known slug. Detector must
        // still flag it so uninstall cleans up upgrades from older builds.
        let legacy_plain = serde_json::json!({
            "type": "command",
            "command": "keel hook session-start",
        });
        assert!(is_managed_hook_entry(&legacy_plain));

        // Legacy PowerShell-encoded form (Windows installs from older
        // keel versions). Hand-rolled snapshot of what the previous
        // encoder produced for `& 'keel' hook session-start` so we
        // don't depend on the deleted encoder. The base64 below decodes via
        // the still-present decode_powershell_encoded_command helper.
        let encoded_script = "& 'keel' hook session-start";
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
            r"C:\Users\Example\.claude\keel.exe"
        } else {
            "/home/example/.claude/keel"
        });
        let hook_path = temp_hook_path("keel-uninstall-roundtrip");
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
                            { "type": "command", "command": "keel", "args": ["hook", "stop"] },
                            { "type": "command", "command": "/usr/local/bin/my-own-stop.sh" }
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "keel", "args": ["hook", "post-tool-use"] }
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
        // NotebookEdit) at runtime, which would be unreachable if the harness
        // only delivered Bash events. The empty matcher also lets
        // `tool_timings::record_tool_timing` sample non-Bash tools so the
        // compression-discipline nudge fires when context fills with file
        // reads and edits, not only with shell output.
        let hook_path = temp_hook_path("keel-hook-matcher-scope");

        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let rendered = build_hooks_payload(&hook_path, Path::new("keel")).unwrap();

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
        // PostToolUse / SubagentStop / SessionEnd fire per tool call or
        // turn end and carry a per-prompt token cost that outweighs the
        // value of any per-call reminder. The operating contract belongs
        // in CLAUDE.md and SessionStart, both paid at most once per session.
        // These events must stay silent.
        //
        // Stop is deliberately NOT in this list: per the official docs it
        // supports additionalContext and we now use it for closeout context.
        // SubagentStart is also NOT here: it injects iron law context.
        // UserPromptSubmit and PostToolBatch emit their own context,
        // gated by their own dedicated tests below.
        for subcommand in [
            "post-tool-use",
            "post-tool-use-failure",
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

        // MCP-tool advertisement — every per-prompt injection must name the
        // always-available keel MCP tools so the model reaches for
        // `system_map`/`recall` instead of guessing about the repo or its
        // memory. This is the base-context half of the fix; the targeted
        // repo/memory-question pointer (tested separately) is the other half.
        assert!(
            context.contains("system_map"),
            "UserPromptSubmit must advertise the system_map MCP tool"
        );
        assert!(
            context.contains("recall"),
            "UserPromptSubmit must advertise the recall MCP tool"
        );
        assert!(
            context.contains("run_command"),
            "UserPromptSubmit must advertise the run_command MCP tool"
        );

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
    fn mcp_pointer_fires_for_repo_structure_questions() {
        // The skill matcher stays silent on these prompts (no distinctive
        // domain token), so this targeted pointer is the only thing that nudges
        // the model to call `system_map` instead of guessing the layout. Cover
        // the common phrasings.
        for prompt in [
            "what is this project about?",
            "so what's this repo for",
            "give me a project overview",
            "explain the architecture here",
            "how is this codebase structured",
            "what does this project do",
        ] {
            let pointer = mcp_tool_pointer_for_prompt(prompt).unwrap_or_else(|| {
                panic!("repo-question prompt should point at system_map: {prompt:?}")
            });
            assert!(
                pointer.contains("system_map"),
                "repo-question pointer must name system_map: {prompt:?}"
            );
        }
    }

    #[test]
    fn mcp_pointer_fires_for_memory_questions() {
        for prompt in [
            "what do you remember about this work",
            "what did you learn last session",
            "do you remember the auth refactor",
            "recall what we decided about pagination",
        ] {
            let pointer = mcp_tool_pointer_for_prompt(prompt).unwrap_or_else(|| {
                panic!("memory-question prompt should point at recall: {prompt:?}")
            });
            assert!(
                pointer.contains("recall"),
                "memory-question pointer must name recall: {prompt:?}"
            );
        }
    }

    #[test]
    fn mcp_pointer_silent_for_ordinary_work() {
        // Must not fire on ordinary feature/bugfix prompts — even ones that
        // mention "project" incidentally — or the reminder becomes noise on
        // every turn. Silence here is the correct, conservative default.
        for prompt in [
            "add a logout button to the navbar",
            "fix the failing pagination test",
            "refactor the project's auth module to use PKCE",
            "why is this function returning null",
            "",
            "   ",
        ] {
            assert_eq!(
                mcp_tool_pointer_for_prompt(prompt),
                None,
                "pointer must stay silent for ordinary work: {prompt:?}"
            );
        }
    }

    #[test]
    fn mcp_pointer_prefers_recall_for_memory_shaped_repo_question() {
        // "what did you learn about this project" mentions "project" but is a
        // memory ask — the recall answer is the right one, so the memory branch
        // must win over the repo branch.
        let pointer = mcp_tool_pointer_for_prompt("what did you learn about this project")
            .expect("memory-shaped prompt should match");
        assert!(
            pointer.contains("recall"),
            "memory-shaped prompt must prefer recall over system_map"
        );
        assert!(
            !pointer.contains("structural map"),
            "memory-shaped prompt must not fire the repo-structure pointer"
        );
    }

    #[test]
    fn work_intent_pointer_fires_on_code_change_prompts() {
        // The targeting fix: code-change prompts must get the read-map / recall /
        // write-brief / preserve-flow reminder. These are exactly the prompts the
        // question pointer stays silent on.
        for prompt in [
            "rework the github skills in this repo",
            "fix the failing pagination test",
            "refactor the auth module to use PKCE",
            "implement a logout endpoint",
            "add a retry to the upload client",
            "update the config loader to read TOML",
            "migrate the store to sqlite",
            "rename getUserName to getUsername",
        ] {
            let pointer = work_intent_pointer_for_prompt(prompt)
                .unwrap_or_else(|| panic!("work pointer must fire for: {prompt:?}"));
            assert!(
                pointer.contains("SYSTEM_MAP") && pointer.contains("working brief"),
                "work pointer must name the map and the brief: {prompt:?}"
            );
            assert!(
                pointer.contains("preserve-existing-flow"),
                "work pointer must route existing-code edits through preserve-existing-flow: {prompt:?}"
            );
        }
    }

    #[test]
    fn work_intent_pointer_silent_for_questions_and_chitchat() {
        // Must not fire on questions, read-only asks, or empty prompts — that
        // would turn the reminder into per-turn noise. Conservative by design.
        for prompt in [
            "why is this function returning null",
            "what does this module do",
            "explain how the gate works",
            "is the build passing",
            "thanks, that looks good",
            "",
            "   ",
        ] {
            assert_eq!(
                work_intent_pointer_for_prompt(prompt),
                None,
                "work pointer must stay silent for non-change prompts: {prompt:?}"
            );
        }
    }

    #[test]
    fn user_prompt_submit_injects_mcp_pointer_for_repo_question() {
        // End-to-end through the dispatcher: a repo-structure prompt on stdin
        // must surface the system_map pointer in the emitted additionalContext,
        // ahead of the base research-first context. This is the integration
        // half of the fix — the unit tests above prove the detector, this
        // proves it is actually wired into the per-prompt payload.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
        std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

        let payload = serde_json::json!({
            "session_id": "",
            "prompt": "what is this project about?"
        })
        .to_string();
        let payload_bytes = payload.into_bytes();
        let mut stdin: &[u8] = &payload_bytes;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

        match previous_mode {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
            None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
        }

        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

        let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");
        let context = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("additionalContext"))
            .and_then(JsonDocument::as_str)
            .expect("additionalContext present");

        assert!(
            context.contains("system_map"),
            "repo question must inject the system_map pointer; got: {context}"
        );
        assert!(
            context.contains("Research-first"),
            "base research-first context must still be present"
        );
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

        // Real the harness payload shape — UserPromptSubmit always carries a
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
            context.contains("logic edits")
                || context.contains("multi-file")
                || context.contains("public-API")
                || context.contains("code changed"),
            "PostToolBatch reminder must state what triggers a reviewer pass"
        );
        assert!(
            context.contains("Trivial")
                || context.contains("docs")
                || context.contains("formatting"),
            "PostToolBatch reminder must spell out the exempt trivial cases"
        );
        assert!(
            !context.contains("Routing Rules"),
            "PostToolBatch reminder must not cite a repo-specific section name; the rule is stated inline so it works across host repos"
        );
        assert!(
            context.contains("decide deliberately")
                || context.contains("deliberately before skipping"),
            "PostToolBatch reminder must encourage deliberate consideration before skipping"
        );

        let event_name = output
            .get("hookSpecificOutput")
            .and_then(|node| node.get("hookEventName"))
            .and_then(JsonDocument::as_str);

        assert_eq!(event_name, Some("PostToolBatch"));
    }

    // ----- Shared gate decision-core tests (review gate + brief gate) -----

    #[test]
    fn gate_disabled_decision_is_always_advisory() {
        // With a gate Off, no combination of inputs fires.
        for edit_count in [0usize, 1, 50] {
            for satisfied in [true, false] {
                assert_eq!(
                    decide_gate(GateMode::Off, 1, 0, edit_count, satisfied),
                    GateDecision::Advisory,
                    "Off gate must never fire (edits={edit_count}, satisfied={satisfied})"
                );
            }
        }
    }

    #[test]
    fn gate_max_blocks_zero_is_advisory() {
        // Cap of 0 is a second kill switch: enabled but never fires, in either mode.
        assert_eq!(
            decide_gate(GateMode::Nudge, 0, 0, 5, false),
            GateDecision::Advisory
        );
        assert_eq!(
            decide_gate(GateMode::Block, 0, 0, 5, false),
            GateDecision::Advisory
        );
    }

    #[test]
    fn gate_no_edits_is_advisory() {
        // Pure-research / question turns changed no code — never fire them.
        assert_eq!(
            decide_gate(GateMode::Nudge, 1, 0, 0, false),
            GateDecision::Advisory
        );
        assert_eq!(
            decide_gate(GateMode::Block, 1, 0, 0, false),
            GateDecision::Advisory
        );
    }

    #[test]
    fn gate_satisfied_is_advisory() {
        // The gate-specific requirement is already met (review ran / brief
        // written) — nothing to fire on, in either mode.
        assert_eq!(
            decide_gate(GateMode::Nudge, 1, 0, 5, true),
            GateDecision::Advisory
        );
        assert_eq!(
            decide_gate(GateMode::Block, 1, 0, 5, true),
            GateDecision::Advisory
        );
    }

    #[test]
    fn gate_fires_unsatisfied_edits_once() {
        // Enabled, code changed, requirement unmet, under the cap → fire.
        // Default Nudge mode yields a non-blocking nudge; Block mode yields a stop.
        assert_eq!(
            decide_gate(GateMode::Nudge, 1, 0, 5, false),
            GateDecision::Nudge,
            "default mode must NUDGE, never block — this is the no-stop fix"
        );
        assert_eq!(
            decide_gate(GateMode::Block, 1, 0, 5, false),
            GateDecision::Block,
            "block mode must restore the opt-in hard stop"
        );
    }

    #[test]
    fn gate_cannot_loop_terminates_at_cap() {
        // THE TERMINATION PROOF. Simulate the worst case: the gate stays enabled,
        // code stays changed, and the requirement is NEVER satisfied
        // (satisfied=false forever). Drive the loop exactly as the dispatcher
        // does — increment the issued counter on every fire — and assert it
        // stops firing once the cap is reached, no matter how many turns elapse.
        // If this ever looped in production it would spam (nudge) or wedge (block)
        // every project; this test is the guarantee that it cannot. Covers both
        // gates AND both modes because they share this exact decision core.
        for mode in [GateMode::Nudge, GateMode::Block] {
            for max_blocks in [1u64, 2, 3] {
                let mut blocks_issued = 0u64;
                let mut total_fires = 0u64;
                for _turn in 0..1000 {
                    match decide_gate(mode, max_blocks, blocks_issued, 5, false) {
                        GateDecision::Nudge | GateDecision::Block => {
                            blocks_issued += 1;
                            total_fires += 1;
                        }
                        GateDecision::Advisory => {}
                    }
                }
                assert_eq!(
                    total_fires, max_blocks,
                    "gate ({mode:?}) must fire exactly max_blocks ({max_blocks}) times across a long session, then fall through forever"
                );
            }
        }
    }

    #[test]
    fn run_hook_post_tool_batch_both_gates_off_matches_advisory_path() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _silenced = NewGatesSilenced::new();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        let mut gate_stdin = std::io::empty();
        let mut gate_out = Vec::new();
        let mut gate_err = Vec::new();
        let gate_code = run_hook_post_tool_batch(&mut gate_stdin, &mut gate_out, &mut gate_err);

        let mut adv_out = Vec::new();
        let mut adv_err = Vec::new();
        let adv_code = run_hook_lifecycle("post-tool-batch", &mut adv_out, &mut adv_err);

        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }

        assert_eq!(gate_code, 0);
        assert_eq!(adv_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&gate_out),
            String::from_utf8_lossy(&adv_out),
            "all-gates-off dispatcher output must match the advisory lifecycle path exactly"
        );
        assert!(
            !String::from_utf8_lossy(&gate_out).contains("\"decision\""),
            "disabled gates must never emit a decision field"
        );
    }

    #[test]
    fn gate_mode_parses_off_block_nudge_and_escalate_default() {
        // The default-on-as-escalate contract: unset → Escalate; explicit disable
        // tokens → Off; `nudge` → Nudge (opt-down); `block` → Block (opt-up);
        // anything else (including a typo) → Escalate (fail toward the default
        // that warns first and only then blocks).
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        const PROBE: &str = "CLAUDE_SKILLS_GATE_DEFAULT_PROBE";
        let previous = std::env::var(PROBE).ok();

        std::env::remove_var(PROBE);
        assert_eq!(
            gate_mode(PROBE),
            GateMode::Escalate,
            "unset must default to the escalating gate"
        );

        for disable in ["off", "0", "false", "no", "OFF", "  off  ", "False"] {
            std::env::set_var(PROBE, disable);
            assert_eq!(
                gate_mode(PROBE),
                GateMode::Off,
                "disable token {disable:?} must turn the gate Off"
            );
        }
        for nudge in ["nudge", "NUDGE", "  Nudge  "] {
            std::env::set_var(PROBE, nudge);
            assert_eq!(
                gate_mode(PROBE),
                GateMode::Nudge,
                "value {nudge:?} must select the advisory-only nudge"
            );
        }
        for block in ["block", "BLOCK", "  Block  "] {
            std::env::set_var(PROBE, block);
            assert_eq!(
                gate_mode(PROBE),
                GateMode::Block,
                "value {block:?} must select the always-block hard stop"
            );
        }
        for escalate in ["on", "1", "true", "yes", "wibble", "", "escalate"] {
            std::env::set_var(PROBE, escalate);
            assert_eq!(
                gate_mode(PROBE),
                GateMode::Escalate,
                "non-off, non-nudge, non-block value {escalate:?} must default to escalate"
            );
        }

        match previous {
            Some(value) => std::env::set_var(PROBE, value),
            None => std::env::remove_var(PROBE),
        }
    }

    #[test]
    fn gate_escalate_nudges_first_then_blocks() {
        // The core escalation contract: fire 0 (blocks_issued == 0) is a
        // non-blocking nudge; once that nudge is spent (blocks_issued == 1) the
        // SAME unmet requirement escalates to a hard block. Uses the escalate
        // default cap of 2 so both fires are under the cap.
        let max = default_max_blocks_for(GateMode::Escalate);
        assert_eq!(max, 2, "escalate default cap must allow nudge + block");
        assert_eq!(
            decide_gate(GateMode::Escalate, max, 0, 5, false),
            GateDecision::Nudge,
            "escalate first contact must NUDGE, not interrupt mid-task"
        );
        assert_eq!(
            decide_gate(GateMode::Escalate, max, 1, 5, false),
            GateDecision::Block,
            "escalate second fire must BLOCK once the nudge was ignored"
        );
        // Satisfying the requirement at any point stops the escalation cold.
        assert_eq!(
            decide_gate(GateMode::Escalate, max, 1, 5, true),
            GateDecision::Advisory,
            "meeting the requirement must halt escalation immediately"
        );
    }

    #[test]
    fn gate_escalate_terminates_at_cap() {
        // Termination proof for the escalating gate: driven turn-by-turn with the
        // requirement NEVER met, it fires exactly `max_blocks` times (one nudge
        // then blocks) and then falls through to advisory forever — never loops.
        let max = default_max_blocks_for(GateMode::Escalate);
        let mut blocks_issued = 0u64;
        let mut nudges = 0u64;
        let mut blocks = 0u64;
        for _turn in 0..1000 {
            match decide_gate(GateMode::Escalate, max, blocks_issued, 5, false) {
                GateDecision::Nudge => {
                    nudges += 1;
                    blocks_issued += 1;
                }
                GateDecision::Block => {
                    blocks += 1;
                    blocks_issued += 1;
                }
                GateDecision::Advisory => {}
            }
        }
        assert_eq!(
            nudges, 1,
            "escalate must nudge exactly once (the first fire)"
        );
        assert_eq!(
            blocks,
            max - 1,
            "escalate must block (cap - 1) times after the opening nudge"
        );
        assert_eq!(
            nudges + blocks,
            max,
            "total fires must equal the cap, then advisory forever"
        );
    }

    #[test]
    fn gate_mode_parses_off_block_and_nudge_default() {
        // Back-compat shim: the historical test name kept as a thin wrapper so a
        // grep for the old name still finds coverage. Delegates to the canonical
        // escalate-aware test above.
        gate_mode_parses_off_block_nudge_and_escalate_default();
    }

    #[test]
    fn review_gate_messages_name_the_switches() {
        // Operators must always be told how to change/disable the gate, right in
        // the message — keyed on the emitted decision (nudge vs block).
        let nudge = review_gate_message(GateDecision::Nudge);
        assert!(nudge.contains("CLAUDE_SKILLS_REVIEW_GATE"));
        assert!(nudge.contains("=block"));
        assert!(nudge.contains("=off"));
        assert!(nudge.contains("review pre-pr"));
        assert!(
            nudge.contains("does not stop the turn"),
            "nudge message must make clear it is non-blocking"
        );
        assert!(
            nudge.contains("escalate"),
            "nudge message must warn that an unmet requirement escalates"
        );

        let block = review_gate_message(GateDecision::Block);
        assert!(block.contains("CLAUDE_SKILLS_REVIEW_GATE"));
        assert!(block.contains("=off"));
        assert!(block.contains("review pre-pr"));
        assert!(
            block.contains("cannot loop") || block.contains("bounded"),
            "block message must reassure that the gate is bounded"
        );
        assert!(
            block.contains("hard stop"),
            "block message must make clear it now halts the turn"
        );
    }

    #[test]
    fn brief_gate_messages_name_the_switches_and_action() {
        // The brief-gate message must tell the model how to clear it (write a
        // brief) and how to change/disable it, keyed on the emitted decision.
        let nudge = brief_gate_message(GateDecision::Nudge);
        assert!(nudge.contains("CLAUDE_SKILLS_BRIEF_GATE"));
        assert!(nudge.contains("=block"));
        assert!(nudge.contains("=off"));
        assert!(
            nudge.contains("working-brief write"),
            "nudge message must name the brief-write surface that clears the gate"
        );
        assert!(
            nudge.contains("does not stop the turn"),
            "nudge message must make clear it is non-blocking"
        );
        assert!(
            nudge.contains("escalate"),
            "nudge message must warn that an unmet requirement escalates"
        );

        let block = brief_gate_message(GateDecision::Block);
        assert!(block.contains("CLAUDE_SKILLS_BRIEF_GATE"));
        assert!(block.contains("=off"));
        assert!(
            block.contains("working-brief write"),
            "block message must name the brief-write surface that clears the gate"
        );
        assert!(
            block.contains("cannot loop") || block.contains("bounded"),
            "block message must reassure that the gate is bounded"
        );
        assert!(
            block.contains("hard stop"),
            "block message must make clear it now halts the turn"
        );
    }

    #[test]
    fn brief_written_this_session_logic() {
        const WS_A: &str = "D:/Nasri/Project/alpha";
        const WS_B: &str = "D:/Nasri/Project/beta";

        // Unknown session start → satisfied (fail-open: never block a session we
        // cannot time).
        let claude_home = temp_brief_gate_home("unknown-start");
        assert!(
            brief_written_this_session(&claude_home, WS_A, None),
            "unknown session start must report satisfied"
        );

        // Known start, no brief on disk → not satisfied.
        assert!(
            !brief_written_this_session(&claude_home, WS_A, Some(now_ms())),
            "known start with no brief must be unsatisfied"
        );

        // A brief written ~now for WS_A covers a WS_A session starting ~now.
        let brief_a = crate::utility::working_brief::create_brief(
            "wb-gate-a".into(),
            "cover this session".into(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            WS_A.into(),
            "2026-06-06T00:00:00Z".into(),
        );
        crate::utility::working_brief::write_brief(&claude_home, &brief_a).expect("write brief a");
        assert!(
            brief_written_this_session(&claude_home, WS_A, Some(now_ms())),
            "a freshly written brief for this workspace must satisfy a session starting now"
        );

        // WORKSPACE SCOPING (the point of the fix): the WS_A brief must NOT
        // satisfy a session editing WS_B — a brief for one project does not
        // count for another.
        assert!(
            !brief_written_this_session(&claude_home, WS_B, Some(now_ms())),
            "a brief written for another workspace must not satisfy this one"
        );

        // BACKWARD COMPAT: a legacy brief with an empty workspace applies
        // anywhere, so it satisfies WS_B too (older briefs never start blocking).
        let brief_legacy = crate::utility::working_brief::create_brief(
            "wb-gate-legacy".into(),
            "legacy brief".into(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
            "2026-06-06T00:00:00Z".into(),
        );
        crate::utility::working_brief::write_brief(&claude_home, &brief_legacy)
            .expect("write legacy brief");
        assert!(
            brief_written_this_session(&claude_home, WS_B, Some(now_ms())),
            "an empty-workspace (legacy) brief must apply to any workspace"
        );

        // A brief far older than a session that starts well beyond the grace
        // window → not satisfied (prior-session brief does not count). Use a
        // fresh home so the briefs written above do not satisfy it.
        let stale_home = temp_brief_gate_home("stale");
        let stale_brief = crate::utility::working_brief::create_brief(
            "wb-gate-stale".into(),
            "old".into(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            WS_A.into(),
            "2026-06-06T00:00:00Z".into(),
        );
        crate::utility::working_brief::write_brief(&stale_home, &stale_brief)
            .expect("write stale brief");
        let far_future_start = now_ms().saturating_add(BRIEF_GATE_SESSION_GRACE_MS * 10);
        assert!(
            !brief_written_this_session(&stale_home, WS_A, Some(far_future_start)),
            "a brief older than (session_start - grace) must not satisfy the gate"
        );

        let _ = std::fs::remove_dir_all(&claude_home);
        let _ = std::fs::remove_dir_all(&stale_home);
    }

    struct NewGatesSilenced {
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl NewGatesSilenced {
        fn new() -> Self {
            let vars = [
                MEMORY_GATE_ENV_VAR,
                SPRINT_START_GATE_ENV_VAR,
                LEARNED_SKILL_GATE_ENV_VAR,
            ];
            let previous = vars
                .iter()
                .map(|&var| {
                    let prior = std::env::var(var).ok();
                    std::env::set_var(var, "off");
                    (var, prior)
                })
                .collect();
            Self { previous }
        }
    }

    impl Drop for NewGatesSilenced {
        fn drop(&mut self) {
            for (var, prior) in &self.previous {
                match prior {
                    Some(value) => std::env::set_var(var, value),
                    None => std::env::remove_var(var),
                }
            }
        }
    }

    fn temp_brief_gate_home(label: &str) -> std::path::PathBuf {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let directory = std::env::temp_dir().join(format!(
            "keel-brief-gate-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("create tempdir");
        directory
    }

    #[test]
    fn run_hook_post_tool_batch_brief_gate_nudges_in_nudge_mode_then_falls_through() {
        // END-TO-END through the real dispatcher in explicit NUDGE mode (the
        // opt-down). Two things proven here:
        //   1. NUDGE mode (BRIEF_GATE=nudge): a session that edited code with no
        //      working brief gets exactly one NON-BLOCKING nudge — additionalContext
        //      carrying the brief reminder, and crucially NO `decision` field, so
        //      the turn is never halted.
        //   2. The per-session counter still advances and the next call falls
        //      through to the generic advisory, so the nudge is bounded (no spam).
        //   3. Opt-up: with BRIEF_GATE=block a fresh session emits `decision:block`.
        // The escalate DEFAULT (nudge-then-block) is covered by the decide_gate
        // unit tests. The review gate is disabled throughout to isolate the brief gate.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-nudge");
        let _silenced = NewGatesSilenced::new();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "nudge");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        // Seed one edit-class timing row for this session so stats.count > 0 and
        // session_start_ms resolves. No brief is written → gate must fire.
        let session_id = "sess-e2e-nudge";
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let timings_dir = claude_home.join("state").join("tool-timings");
        std::fs::create_dir_all(&timings_dir).expect("create timings dir");
        let row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": session_id,
            "cwd": "D:/Nasri/Project/gate-e2e",
            "effort_level": "",
        });
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n"),
        )
        .expect("write timings row");

        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        // First call (default mode): must NUDGE — additionalContext with the brief
        // reminder and NO decision field (the turn is not halted).
        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default brief gate must NOT block (no decision field): {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
            "default brief gate must emit a non-blocking nudge naming the gate: {out1_text}"
        );

        // The counter must have advanced to 1 (the nudge is bounded like a block).
        let blocks_path = brief_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "brief-gate counter must advance to 1 after the nudge"
        );

        // Second call (same unsatisfied state): cap reached → generic advisory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            !out2_text.contains("\"decision\""),
            "second call must not block: {out2_text}"
        );
        assert!(
            out2_text.contains("Closeout check"),
            "second call must fall through to the generic advisory (cap reached): {out2_text}"
        );

        // Opt-in hard stop: a FRESH session with BRIEF_GATE=block must emit a real
        // decision:block. New session id so its counter starts at zero.
        std::env::set_var(BRIEF_GATE_ENV_VAR, "block");
        let block_session = "sess-e2e-block-optin";
        let block_row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": block_session,
            "cwd": "D:/Nasri/Project/gate-e2e",
            "effort_level": "",
        });
        // Append the new session's row alongside the first.
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n{block_row}\n"),
        )
        .expect("rewrite timings rows");
        let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            out3_text.contains("additionalContext")
                && out3_text.contains("now a hard stop")
                && out3_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
            "BRIEF_GATE=block must emit the feed-forward hard stop: {out3_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn run_hook_post_tool_batch_brief_gate_escalates_by_default_nudge_then_block() {
        // END-TO-END proof of the ESCALATE DEFAULT through the real dispatcher:
        // with BRIEF_GATE unset, a session that edited code with no working brief
        // gets a NON-BLOCKING nudge on the first end-of-turn, then a real
        // `decision:block` on the second (the requirement is still unmet), then
        // falls through to the generic advisory once the cap (2) is spent. This is
        // the "not optional" behavior — ignoring the nudge is no longer free.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-escalate");
        let _silenced = NewGatesSilenced::new();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off"); // isolate the brief gate
        std::env::remove_var(BRIEF_GATE_ENV_VAR); // default → escalate
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        let session_id = "sess-e2e-escalate";
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let timings_dir = claude_home.join("state").join("tool-timings");
        std::fs::create_dir_all(&timings_dir).expect("create timings dir");
        let row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": session_id,
            "cwd": "D:/Nasri/Project/escalate-e2e",
            "effort_level": "",
        });
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n"),
        )
        .expect("write timings row");

        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        // First call: non-blocking nudge (no decision field).
        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "escalate first fire must NUDGE, not block: {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
            "escalate first fire must emit a non-blocking nudge: {out1_text}"
        );

        // Second call (still no brief): escalate to a real hard block.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            out2_text.contains("additionalContext") && out2_text.contains("now a hard stop"),
            "escalate second fire must emit the feed-forward hard stop: {out2_text}"
        );

        // Third call: cap (2) spent → generic advisory, no decision field.
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            !out3_text.contains("\"decision\""),
            "third call must fall through to advisory (cap spent): {out3_text}"
        );
        assert!(
            out3_text.contains("Closeout check"),
            "third call must be the generic advisory: {out3_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn run_hook_post_tool_batch_review_gate_nudges_in_nudge_mode_then_falls_through() {
        // END-TO-END for the REVIEW gate in explicit NUDGE mode, symmetric to the
        // brief-gate test above. The review gate has distinct plumbing from the
        // brief gate (review_marker_ms, review_gate_blocks_path,
        // review_gate_message), so a regression there could silently re-introduce
        // a wrong `decision:block` even while the brief gate stays correct. This
        // isolates the review gate (brief gate off) and proves:
        //   1. NUDGE mode (REVIEW_GATE=nudge): a session that edited code with no
        //      reviewer marker gets a NON-BLOCKING nudge — additionalContext with
        //      the review reminder and NO `decision` field.
        //   2. The per-session counter advances and the next call falls through to
        //      the generic advisory (bounded, no spam).
        //   3. Opt-up: with REVIEW_GATE=block a fresh session emits decision:block.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-review-nudge");
        let _silenced = NewGatesSilenced::new();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off"); // isolate the review gate
        std::env::set_var(REVIEW_GATE_ENV_VAR, "nudge"); // explicit advisory-only mode
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        // Seed one edit-class timing row. No `.reviewed` marker is written → the
        // review gate sees an unreviewed edit and must fire.
        let session_id = "sess-e2e-review-nudge";
        let cwd = "D:/Nasri/Project/gate-review-e2e";
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let timings_dir = claude_home.join("state").join("tool-timings");
        std::fs::create_dir_all(&timings_dir).expect("create timings dir");
        let row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": session_id,
            "cwd": cwd,
            "effort_level": "",
        });
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n"),
        )
        .expect("write timings row");

        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        // First call (default mode): must NUDGE — additionalContext naming the
        // review gate, and NO decision field (turn not halted).
        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default review gate must NOT block (no decision field): {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_REVIEW_GATE"),
            "default review gate must emit a non-blocking nudge naming the gate: {out1_text}"
        );

        // Counter advances to 1 (the nudge is bounded like a block).
        let blocks_path = review_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "review-gate counter must advance to 1 after the nudge"
        );

        // Second call (same unsatisfied state): cap reached → generic advisory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            !out2_text.contains("\"decision\""),
            "second call must not block: {out2_text}"
        );
        assert!(
            out2_text.contains("Closeout check"),
            "second call must fall through to the generic advisory (cap reached): {out2_text}"
        );

        // Opt-in hard stop: a FRESH session with REVIEW_GATE=block must emit a
        // real decision:block. New session id so its counter starts at zero.
        std::env::set_var(REVIEW_GATE_ENV_VAR, "block");
        let block_session = "sess-e2e-review-block-optin";
        let block_row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": block_session,
            "cwd": cwd,
            "effort_level": "",
        });
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n{block_row}\n"),
        )
        .expect("rewrite timings rows");
        let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            out3_text.contains("additionalContext")
                && out3_text.contains("now a hard stop")
                && out3_text.contains("CLAUDE_SKILLS_REVIEW_GATE"),
            "REVIEW_GATE=block must emit the feed-forward hard stop: {out3_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    /// Seed the sprint store for `workspace_cwd` with the given (story, state)
    /// pairs, using the same slug + group the gate resolves, so the gate sees a
    /// real active sprint. Returns nothing; the records land under
    /// `<home>/sprint/<slug>/`.
    fn seed_sprint(claude_home: &std::path::Path, workspace_cwd: &str, stories: &[(&str, &str)]) {
        let slug = crate::utility::sprint::workspace_slug_for_test(workspace_cwd);
        let store =
            crate::utility::record_store::RecordStore::new(claude_home, &format!("sprint/{slug}"));
        for (index, (story, state)) in stories.iter().enumerate() {
            let id = format!("s{}", index + 1);
            let record: crate::utility::record_store::Record = vec![
                ("id".into(), id.clone()),
                ("story".into(), (*story).into()),
                ("state".into(), (*state).into()),
                ("note".into(), String::new()),
            ];
            store.write_record(&id, &record).expect("seed sprint story");
        }
    }

    /// Seed one edit-class timing row so `session_edit_stats` reports the given
    /// cwd and a non-zero count (the closeout gate resolves the workspace from the
    /// last edit's cwd).
    fn seed_edit_row(claude_home: &std::path::Path, session_id: &str, cwd: &str) {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let timings_dir = claude_home.join("state").join("tool-timings");
        std::fs::create_dir_all(&timings_dir).expect("create timings dir");
        let row = serde_json::json!({
            "recorded_at_ms": now_ms(),
            "event": "PostToolUse",
            "tool_name": "Edit",
            "duration_ms": 5u64,
            "session_id": session_id,
            "cwd": cwd,
            "effort_level": "",
        });
        std::fs::write(
            timings_dir.join(format!("{date}.jsonl")),
            format!("{row}\n"),
        )
        .expect("write timings row");
    }

    #[test]
    fn story_closeout_gate_nudges_when_sprint_incomplete_then_silent_without_sprint() {
        // The honest-closeout gate (story 1 + 2 + 3). Isolates it by disabling the
        // brief and review gates. Proves:
        //   1. Active sprint with an open story -> NON-BLOCKING nudge naming the gap
        //      (no `decision` field), and the counter advances (bounded).
        //   2. A different workspace with NO sprint -> the gate stays silent and the
        //      turn falls through to the generic advisory.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-closeout");
        let _silenced = NewGatesSilenced::new();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR); // default → escalate (first fire nudges)

        // Workspace WITH an incomplete sprint.
        let open_cwd = "D:/Nasri/Project/closeout-open";
        let session_id = "sess-closeout-open";
        seed_edit_row(&claude_home, session_id, open_cwd);
        seed_sprint(
            &claude_home,
            open_cwd,
            &[
                ("As a dev, I want A, so that X.", "done"),
                ("As a dev, I want B, so that Y.", "todo"),
            ],
        );
        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default closeout gate must NOT block: {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_STORY_CLOSEOUT_GATE")
                && out1_text.contains("s2"),
            "closeout nudge must name the open story s2 as a gap: {out1_text}"
        );
        let blocks_path = story_closeout_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "closeout counter must advance to 1 after the nudge"
        );

        // Workspace WITHOUT a sprint -> gate silent, generic advisory.
        let none_cwd = "D:/Nasri/Project/closeout-none";
        let none_session = "sess-closeout-none";
        seed_edit_row(&claude_home, none_session, none_cwd);
        let none_stdin = format!("{{\"session_id\":\"{none_session}\"}}");
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut none_stdin.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            !out2_text.contains("CLAUDE_SKILLS_STORY_CLOSEOUT_GATE"),
            "no sprint -> closeout gate must stay silent: {out2_text}"
        );
        assert!(
            out2_text.contains("Closeout check"),
            "no-sprint turn must fall through to the generic advisory: {out2_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn story_closeout_gate_blocks_when_opted_in_and_silent_when_complete() {
        // Proves the opt-in hard stop (=block) fires with `decision:block` on an
        // incomplete sprint, and that a fully-Done sprint never fires (silent).
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-closeout-block");
        let _silenced = NewGatesSilenced::new();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "block");

        // Incomplete sprint -> decision:block.
        let block_cwd = "D:/Nasri/Project/closeout-block";
        let block_session = "sess-closeout-block";
        seed_edit_row(&claude_home, block_session, block_cwd);
        seed_sprint(
            &claude_home,
            block_cwd,
            &[("As a dev, I want C, so that Z.", "blocked")],
        );
        let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("Do NOT")
                && out1_text.contains("now a hard stop"),
            "STORY_CLOSEOUT_GATE=block must emit the feed-forward hard stop: {out1_text}"
        );

        // Fully-Done sprint -> silent (generic advisory), even under =block.
        let done_cwd = "D:/Nasri/Project/closeout-done";
        let done_session = "sess-closeout-done";
        seed_edit_row(&claude_home, done_session, done_cwd);
        seed_sprint(
            &claude_home,
            done_cwd,
            &[("As a dev, I want D, so that W.", "done")],
        );
        let done_stdin = format!("{{\"session_id\":\"{done_session}\"}}");
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut done_stdin.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            !out2_text.contains("\"decision\""),
            "a fully-Done sprint must not fire the closeout gate: {out2_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    /// Seed a research-cache record file with a fresh mtime so the memory gate
    /// sees a durable write this session. Mirrors the `memory/research-cache`
    /// layout `keel memory research-cache record` writes to.
    fn seed_memory_write(claude_home: &std::path::Path) {
        let dir = claude_home.join("memory").join("research-cache");
        std::fs::create_dir_all(&dir).expect("create research-cache dir");
        std::fs::write(dir.join("rc-1.json"), "{\"id\":\"rc-1\"}").expect("write research record");
    }

    /// Seed the newest working brief for `workspace_cwd` with `criteria_count`
    /// acceptance criteria so the sprint-start gate's multi-story check resolves.
    fn seed_brief_with_criteria(
        claude_home: &std::path::Path,
        workspace_cwd: &str,
        criteria_count: usize,
    ) {
        let criteria: Vec<String> = (0..criteria_count)
            .map(|index| format!("Given X, When Y{index}, Then Z{index}."))
            .collect();
        let brief = crate::utility::working_brief::create_brief(
            format!("wb-sprint-{criteria_count}"),
            "multi-story request".into(),
            Vec::new(),
            criteria,
            Vec::new(),
            workspace_cwd.into(),
            "2026-06-06T00:00:00Z".into(),
        );
        crate::utility::working_brief::write_brief(claude_home, &brief).expect("write brief");
    }

    /// Seed a template-state generated learned skill plus the trusted instincts it
    /// was built from, so `collect_synthesis_briefs` reports one pending brief. The
    /// fnv1a-64 here matches the learning loop's marker hash so the skill reads as
    /// unrefined (template state). Returns the skill name.
    fn seed_pending_learned_skill(claude_home: &std::path::Path, project: &str) -> String {
        fn fnv1a_64(bytes: &[u8]) -> u64 {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash
        }
        let skill_name = format!("learned-{project}");
        let skill_dir = claude_home.join("skills").join(&skill_name);
        std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
        let content =
            format!("---\nname: {skill_name}\ngenerated: true\nprovenance: learned\n---\nbody\n");
        std::fs::write(skill_dir.join("SKILL.md"), &content).expect("write skill");
        let marker = serde_json::json!({
            "generator": "keel-learning",
            "generatedHash": fnv1a_64(content.as_bytes()).to_string(),
            "signatureSet": "cargo test\ngit commit",
            "project": project,
            "predictedSignatures": ["cargo test", "git commit"],
        });
        std::fs::write(
            skill_dir.join(".learning-meta.json"),
            serde_json::to_string_pretty(&marker).unwrap(),
        )
        .expect("write marker");
        let store = crate::utility::record_store::RecordStore::new(claude_home, "memory/instincts");
        for (index, trigger) in ["cargo test", "git commit"].iter().enumerate() {
            let id = format!("inst-{index}");
            let record: crate::utility::record_store::Record = vec![
                ("id".into(), id.clone()),
                ("trigger".into(), (*trigger).into()),
                ("guidance".into(), format!("always run {trigger}")),
                ("confidence".into(), "8".into()),
                ("observations".into(), "8".into()),
                ("sessions".into(), "2".into()),
                ("project".into(), project.into()),
                ("source".into(), "observed".into()),
            ];
            store.write_record(&id, &record).expect("seed instinct");
        }
        skill_name
    }

    #[test]
    fn memory_gate_messages_name_the_switches_and_action() {
        // The memory-gate message must name the clearing action (a memory write)
        // and how to change/disable it, keyed on the emitted decision.
        let nudge = memory_gate_message(GateDecision::Nudge);
        assert!(nudge.contains("CLAUDE_SKILLS_MEMORY_GATE"));
        assert!(nudge.contains("=block"));
        assert!(nudge.contains("=off"));
        assert!(
            nudge.contains("research-cache record")
                && nudge.contains("maintenance append-working-buffer"),
            "nudge message must name the memory-write surfaces that clear the gate"
        );
        assert!(nudge.contains("does not stop the turn"));
        assert!(nudge.contains("escalate"));

        let block = memory_gate_message(GateDecision::Block);
        assert!(block.contains("CLAUDE_SKILLS_MEMORY_GATE"));
        assert!(block.contains("=off"));
        assert!(block.contains("research-cache record"));
        assert!(block.contains("cannot loop") || block.contains("bounded"));
        assert!(block.contains("hard stop"));
    }

    #[test]
    fn sprint_start_gate_messages_name_the_switches_and_action() {
        let nudge = sprint_start_gate_message(GateDecision::Nudge);
        assert!(nudge.contains("CLAUDE_SKILLS_SPRINT_START_GATE"));
        assert!(nudge.contains("=block"));
        assert!(nudge.contains("=off"));
        assert!(
            nudge.contains("keel sprint plan") && nudge.contains("working-a-sprint"),
            "nudge message must name the sprint-plan action and the sprint skill"
        );
        assert!(nudge.contains("does not stop the turn"));
        assert!(nudge.contains("escalate"));

        let block = sprint_start_gate_message(GateDecision::Block);
        assert!(block.contains("CLAUDE_SKILLS_SPRINT_START_GATE"));
        assert!(block.contains("=off"));
        assert!(block.contains("keel sprint plan"));
        assert!(block.contains("cannot loop") || block.contains("bounded"));
        assert!(block.contains("hard stop"));
    }

    #[test]
    fn learned_skill_gate_message_names_switch_and_skill() {
        let briefs = vec![crate::runner::learning::SynthesisBrief {
            skill_name: "learned-demo".into(),
            skill_path: "/skills/learned-demo/SKILL.md".into(),
            project: "demo".into(),
            prompt: "...".into(),
        }];
        let nudge = learned_skill_gate_message(GateDecision::Nudge, &briefs);
        assert!(nudge.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"));
        assert!(nudge.contains("=off"));
        assert!(
            nudge.contains("Skill(\"learned-demo\")"),
            "message must name the learned skill as a load action: {nudge}"
        );
        assert!(
            nudge.contains("never halts the turn"),
            "learned-skill reminder is advisory, never a hard stop"
        );
    }

    #[test]
    fn memory_gate_nudges_when_no_memory_saved_then_satisfied_off_and_capped() {
        // END-TO-END for the memory-save gate. Isolates it by disabling the other
        // gates. Proves: fires when code changed but nothing saved; silent once a
        // memory write exists; silent when off; bounded per session.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-memory");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
        let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
        let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
        std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
        std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
        std::env::set_var(MEMORY_GATE_ENV_VAR, "nudge");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");

        let session_id = "sess-memory";
        let cwd = "D:/Nasri/Project/memory-e2e";
        seed_edit_row(&claude_home, session_id, cwd);
        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        // Fires: edited code, no memory write → non-blocking nudge naming the gate.
        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default memory gate must NOT block: {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
            "memory gate must emit a non-blocking nudge naming the gate: {out1_text}"
        );
        let blocks_path = memory_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "memory-gate counter must advance to 1 after the nudge"
        );

        // Cap reached (still no write): falls through to the generic advisory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            out2_text.contains("Closeout check")
                && !out2_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
            "second call must fall through to the generic advisory (cap reached): {out2_text}"
        );

        // Satisfied: a fresh session with a memory write present → silent.
        let saved_session = "sess-memory-saved";
        seed_edit_row(&claude_home, saved_session, cwd);
        seed_memory_write(&claude_home);
        let saved_stdin = format!("{{\"session_id\":\"{saved_session}\"}}");
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 = run_hook_post_tool_batch(&mut saved_stdin.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            !out3_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
            "a memory write this session must satisfy the gate (silent): {out3_text}"
        );

        // Off: a fresh session with no write but MEMORY_GATE=off → silent.
        std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
        let off_session = "sess-memory-off";
        seed_edit_row(&claude_home, off_session, cwd);
        let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
        let mut out4 = Vec::new();
        let mut err4 = Vec::new();
        let code4 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out4, &mut err4);
        let out4_text = String::from_utf8_lossy(&out4);
        assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
        assert!(
            !out4_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
            "MEMORY_GATE=off must keep the gate silent: {out4_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        for (var, prior) in [
            (REVIEW_GATE_ENV_VAR, previous_review),
            (BRIEF_GATE_ENV_VAR, previous_brief),
            (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
            (SPRINT_START_GATE_ENV_VAR, previous_sprint),
            (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
            (MEMORY_GATE_ENV_VAR, previous_memory),
            (RESEARCH_GATE_ENV_VAR, previous_research),
            (STORY_FIRST_GATE_ENV_VAR, previous_story_first),
        ] {
            match prior {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn sprint_start_gate_nudges_for_multi_story_without_sprint_then_satisfied_off_and_capped() {
        // END-TO-END for the sprint-start gate. Isolates it by disabling the other
        // gates. Proves: fires on multi-story scope with no sprint; silent once a
        // sprint exists; silent for single-story scope; silent when off; bounded.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-sprint-start");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
        let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
        let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
        std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
        std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
        std::env::set_var(SPRINT_START_GATE_ENV_VAR, "nudge");

        // Multi-story scope, no sprint → fire.
        let cwd = "D:/Nasri/Project/sprint-start-e2e";
        let session_id = "sess-sprint-start";
        seed_edit_row(&claude_home, session_id, cwd);
        seed_brief_with_criteria(&claude_home, cwd, 2);
        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default sprint-start gate must NOT block: {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
            "sprint-start gate must nudge on multi-story scope with no sprint: {out1_text}"
        );
        let blocks_path = sprint_start_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "sprint-start counter must advance to 1 after the nudge"
        );

        // Cap reached: falls through to the generic advisory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            out2_text.contains("Closeout check")
                && !out2_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
            "second call must fall through to the generic advisory (cap reached): {out2_text}"
        );

        // Satisfied: a multi-story workspace that already has a sprint → silent.
        let with_sprint_cwd = "D:/Nasri/Project/sprint-start-has-sprint";
        let with_sprint_session = "sess-sprint-has";
        seed_edit_row(&claude_home, with_sprint_session, with_sprint_cwd);
        seed_brief_with_criteria(&claude_home, with_sprint_cwd, 2);
        seed_sprint(
            &claude_home,
            with_sprint_cwd,
            &[("As a dev, I want A, so that X.", "todo")],
        );
        let with_sprint_stdin = format!("{{\"session_id\":\"{with_sprint_session}\"}}");
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 =
            run_hook_post_tool_batch(&mut with_sprint_stdin.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            !out3_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
            "an existing sprint must satisfy the gate (silent): {out3_text}"
        );

        // Single-story scope (one acceptance criterion) → not multi-story → silent.
        let single_cwd = "D:/Nasri/Project/sprint-start-single";
        let single_session = "sess-sprint-single";
        seed_edit_row(&claude_home, single_session, single_cwd);
        seed_brief_with_criteria(&claude_home, single_cwd, 1);
        let single_stdin = format!("{{\"session_id\":\"{single_session}\"}}");
        let mut out4 = Vec::new();
        let mut err4 = Vec::new();
        let code4 = run_hook_post_tool_batch(&mut single_stdin.as_bytes(), &mut out4, &mut err4);
        let out4_text = String::from_utf8_lossy(&out4);
        assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
        assert!(
            !out4_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
            "single-story scope must keep the gate silent: {out4_text}"
        );

        // Off: a fresh multi-story session but SPRINT_START_GATE=off → silent.
        std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
        let off_session = "sess-sprint-off";
        seed_edit_row(&claude_home, off_session, cwd);
        let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
        let mut out5 = Vec::new();
        let mut err5 = Vec::new();
        let code5 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out5, &mut err5);
        let out5_text = String::from_utf8_lossy(&out5);
        assert_eq!(code5, 0, "stderr: {}", String::from_utf8_lossy(&err5));
        assert!(
            !out5_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
            "SPRINT_START_GATE=off must keep the gate silent: {out5_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        for (var, prior) in [
            (REVIEW_GATE_ENV_VAR, previous_review),
            (BRIEF_GATE_ENV_VAR, previous_brief),
            (RESEARCH_GATE_ENV_VAR, previous_research),
            (STORY_FIRST_GATE_ENV_VAR, previous_story_first),
            (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
            (MEMORY_GATE_ENV_VAR, previous_memory),
            (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
            (SPRINT_START_GATE_ENV_VAR, previous_sprint),
        ] {
            match prior {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn learned_skill_gate_nudges_when_pending_then_silent_off_and_capped() {
        // END-TO-END for the learned-skill reminder. Isolates it by disabling the
        // other gates. Proves: fires when a template-state learned skill is pending
        // (independent of edits); silent when none pending; silent when off; bounded.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let claude_home = temp_brief_gate_home("e2e-learned");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
        let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
        let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
        std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
        std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
        std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "nudge");

        // Pending learned skill (no edit row needed — the gate is edit-independent).
        let skill_name = seed_pending_learned_skill(&claude_home, "learnedgate");
        let session_id = "sess-learned";
        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

        let mut out1 = Vec::new();
        let mut err1 = Vec::new();
        let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
        let out1_text = String::from_utf8_lossy(&out1);
        assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
        assert!(
            !out1_text.contains("\"decision\""),
            "default learned-skill gate must NOT block: {out1_text}"
        );
        assert!(
            out1_text.contains("additionalContext")
                && out1_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE")
                && out1_text.contains(&format!("Skill(\\\"{skill_name}\\\")")),
            "learned-skill gate must name the pending skill as a load action: {out1_text}"
        );
        let blocks_path = learned_skill_gate_blocks_path(&claude_home, session_id);
        assert_eq!(
            read_counter_value(&blocks_path),
            1,
            "learned-skill counter must advance to 1 after the nudge"
        );

        // Cap reached (skill still pending): falls through to the generic advisory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
        let out2_text = String::from_utf8_lossy(&out2);
        assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
        assert!(
            out2_text.contains("Closeout check")
                && !out2_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
            "second call must fall through to the generic advisory (cap reached): {out2_text}"
        );

        // Satisfied: a fresh home with no learned skills pending → silent.
        let empty_home = temp_brief_gate_home("e2e-learned-empty");
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &empty_home);
        let empty_session = "sess-learned-empty";
        let empty_stdin = format!("{{\"session_id\":\"{empty_session}\"}}");
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        let code3 = run_hook_post_tool_batch(&mut empty_stdin.as_bytes(), &mut out3, &mut err3);
        let out3_text = String::from_utf8_lossy(&out3);
        assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
        assert!(
            !out3_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
            "no pending learned skill must keep the gate silent: {out3_text}"
        );

        // Off: pending skill present but LEARNED_SKILL_GATE=off → silent.
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
        let off_session = "sess-learned-off";
        let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
        let mut out4 = Vec::new();
        let mut err4 = Vec::new();
        let code4 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out4, &mut err4);
        let out4_text = String::from_utf8_lossy(&out4);
        assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
        assert!(
            !out4_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
            "LEARNED_SKILL_GATE=off must keep the gate silent: {out4_text}"
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        for (var, prior) in [
            (REVIEW_GATE_ENV_VAR, previous_review),
            (BRIEF_GATE_ENV_VAR, previous_brief),
            (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
            (MEMORY_GATE_ENV_VAR, previous_memory),
            (SPRINT_START_GATE_ENV_VAR, previous_sprint),
            (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
        ] {
            match prior {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
        let _ = std::fs::remove_dir_all(&claude_home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }

    #[test]
    fn edit_counter_increments_and_resets_at_threshold() {
        // The counter file is the bridge between PostToolUse fires (one per
        // tool call) and the periodic SYSTEM_MAP refresh. Verify the file
        // round-trips correctly so the threshold check in run_hook_post_tool_use
        // sees the right value.
        let dir = std::env::temp_dir().join(format!("keel-edit-counter-{}", std::process::id()));
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
        // <repo>/using-keel/SKILL.md via include_str! and are what
        // make the model treat skill invocation as non-optional.
        assert!(
            context.contains("EXTREMELY_IMPORTANT"),
            "SessionStart must embed the bootstrap skill iron-law block"
        );
        assert!(
            context.contains("Trust the codebase, not your knowledge base"),
            "SessionStart must restate the trust-the-codebase rule"
        );
        // The four rules must be labeled with the literal phrase "Iron Law" in
        // the always-loaded SessionStart channel. Regression guard for the bug
        // where the contract WAS in context but never named: an agent scanning
        // its context for "iron law" found nothing because the bootstrap only
        // said "operating contract"/"EXTREMELY_IMPORTANT", so it answered "no
        // iron law in my context" even though the rules were right there. The
        // name is the lookup key the user (and the model) search for.
        assert!(
            context.contains("Iron Law"),
            "SessionStart must label the four rules with the literal phrase \"Iron Law\" so an agent asked whether the Iron Law is in context can find it by name"
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
            context.contains("keel memory working-brief write"),
            "SessionStart memory-writes block must name the working-brief write surface"
        );
        assert!(
            context.contains("keel memory completion-gate check"),
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
        // phrasing lives in using-keel/SKILL.md; this assertion
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
    fn session_start_context_stays_under_truncation_cap() {
        // The bug this guards: the harness truncates hook
        // `hookSpecificOutput.additionalContext` once it crosses ~10KB,
        // persisting the full text to a tool-results file and injecting only a
        // ~2KB preview + a file pointer the model never reads back. Verified
        // against live session transcripts — a 27.6KB SessionStart context was
        // replaced by a 2KB preview while a 5.9KB UserPromptSubmit context landed
        // intact. The previous implementation embedded the full 27KB
        // using-keel/SKILL.md here, so the operating contract was silently
        // truncated to its first ~2KB in every project: the model never saw the
        // later iron-law rules, the discipline pillars, the MCP tools, or the
        // memory writers.
        //
        // The contract MUST fit in full. We assert a conservative 9KB ceiling on
        // the UTF-8 byte length — below the observed ~10KB cap with headroom for
        // the appended runtime memory pointer. If a future edit grows the
        // bootstrap past this, it re-introduces the truncation bug, so the bound
        // fails loudly instead of shipping a contract the model cannot see.
        const TRUNCATION_CEILING_BYTES: usize = 9 * 1024;
        // Isolate the home so the base measures ONLY the compact bootstrap, not
        // this machine's accumulated instinct/synthesis digest for whatever
        // project the test happens to run in. session_start_context() appends
        // project_instinct_digest / project_synthesis_nudge from the resolved
        // ~/.claude home; without isolation a developer's real home (or this
        // repo's own growing instinct store, now that failures are captured too)
        // inflates the base so the worst-case assertion fails locally while
        // passing on a clean CI home. Point CLAUDE_TARGET_OVERRIDE at an empty
        // temp dir under the shared ENV_LOCK so the measurement is deterministic
        // everywhere.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let empty_home = temp_brief_gate_home("truncation-cap");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &empty_home);

        let context = session_start_context();

        match &previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = std::fs::remove_dir_all(&empty_home);

        let byte_len = context.len();
        assert!(
            byte_len < TRUNCATION_CEILING_BYTES,
            "SessionStart context is {byte_len} bytes, at/over the {TRUNCATION_CEILING_BYTES}-byte ceiling — the harness truncates additionalContext above ~10KB, so the operating contract would be cut off mid-way and the model would never see the full iron law. Trim the compact bootstrap or move detail into the on-demand Skill(\"using-keel\") body."
        );

        // Major-3 guard: in a fresh test env `workspace_memory_digest()` is
        // empty, so the line above only certifies the base context. But at
        // runtime the digest is appended and is independently bounded by
        // WORKSPACE_DIGEST_MAX_BYTES. Certify the WORST CASE — base context plus
        // a maxed-out digest — still clears the ceiling, so a future bootstrap
        // growth that would overflow once the digest is present fails loudly
        // here instead of silently truncating in production.
        let worst_case = byte_len + WORKSPACE_DIGEST_MAX_BYTES;
        assert!(
            worst_case < TRUNCATION_CEILING_BYTES,
            "SessionStart base ({byte_len} B) + a maxed workspace digest ({WORKSPACE_DIGEST_MAX_BYTES} B) = {worst_case} B would cross the {TRUNCATION_CEILING_BYTES}-byte ceiling. Shrink the bootstrap or WORKSPACE_DIGEST_MAX_BYTES so the pushed digest can never truncate the iron law."
        );
    }

    #[test]
    fn workspace_memory_digest_pushes_real_content_and_stays_bounded() {
        // s2: the digest must PUSH actual content (system-map head + newest
        // brief + most recent memory note), not just a pointer, and stay within
        // its byte budget so the SessionStart ceiling is never threatened.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "ws-digest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let base = std::env::temp_dir().join(unique);
        let claude_home = base.join(".claude");
        std::fs::create_dir_all(&claude_home).unwrap();

        // The digest reads the map by the same path helper the production code
        // uses, keyed off the current working directory. Drive cwd to a stable
        // workspace and seed a SYSTEM_MAP.md there.
        let workspace = base.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let previous_cwd = std::env::current_dir().ok();
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_current_dir(&workspace).unwrap();

        // 1. Seed the system map at the workspace-keyed reference path.
        if let Some(map_path) =
            memory_system_map_path_for_workspace(&std::env::current_dir().unwrap())
        {
            std::fs::create_dir_all(map_path.parent().unwrap()).unwrap();
            std::fs::write(
                &map_path,
                "# SYSTEM MAP\n\nThis repo is the WIDGET-FACTORY service.\nEntry: src/main.rs\n",
            )
            .unwrap();
        }

        // 2. Seed a working brief tagged for this workspace.
        let workspace_display = display_path(&std::env::current_dir().unwrap());
        let brief = crate::utility::working_brief::create_brief(
            "wb-digesttest".to_string(),
            "Ship the FROBNICATE endpoint".to_string(),
            vec![],
            vec!["frobnicate returns 200".to_string()],
            vec![],
            workspace_display,
            "2026-06-13T00:00:00Z".to_string(),
        );
        crate::utility::working_brief::write_brief(&claude_home, &brief).unwrap();

        // 3. Seed a recent memory note.
        crate::utility::memory_families::run_memory_family_command(
            "memory",
            "research-cache",
            &[
                "record".to_string(),
                "--question".to_string(),
                "What was last done in WIDGET-FACTORY?".to_string(),
                "--answer".to_string(),
                "Wired the GIZMO cache layer".to_string(),
            ],
            &mut std::io::sink(),
            &mut std::io::sink(),
        );

        let digest = workspace_memory_digest();

        // Restore env/cwd before assertions.
        if let Some(cwd) = previous_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }

        // Real content from all three sources is PUSHED, not pointed at.
        assert!(
            digest.contains("WIDGET-FACTORY"),
            "digest must embed the actual system-map head: {digest}"
        );
        assert!(
            digest.contains("FROBNICATE"),
            "digest must embed the actual working-brief request: {digest}"
        );
        assert!(
            digest.contains("GIZMO"),
            "digest must embed the actual most-recent memory note: {digest}"
        );
        // Bounded.
        assert!(
            digest.len() <= WORKSPACE_DIGEST_MAX_BYTES + 40,
            "digest length {} exceeds its byte budget",
            digest.len()
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn truncate_on_line_boundary_cuts_at_newline_and_marks_elision() {
        let text = "alpha line\nbeta line\ngamma line\ndelta line\n";
        // Cap mid-"gamma": must cut back to the end of "beta line".
        let cut = truncate_on_line_boundary(text, 25);
        assert!(cut.starts_with("alpha line\nbeta line"));
        assert!(cut.contains("[truncated]"));
        assert!(!cut.contains("gamma"));
        // Under cap → returned unchanged.
        assert_eq!(truncate_on_line_boundary("short", 100), "short");
    }

    #[test]
    fn truncate_on_line_boundary_does_not_panic_on_multibyte_at_cap() {
        // Blocker regression: the earlier local impl sliced `&str` by raw byte
        // index (`&text[..max_bytes]`), which panics when a multibyte char
        // straddles the cap. Workspace map / brief / note text routinely carries
        // em-dashes, ellipses, smart quotes, and arrows, so this is a real
        // SessionStart panic path. Build a single line (no newline in range, so
        // the char-boundary fallback is exercised) packed with em-dashes and set
        // a cap that lands inside one. Must return a truncated string, not panic.
        let text = "—".repeat(200); // each '—' is 3 UTF-8 bytes; no newline
        for cap in [10usize, 25, 31, 100, 199] {
            let out = truncate_on_line_boundary(&text, cap);
            // Did not panic, stayed within budget + the marker allowance, and
            // never split a char (valid UTF-8 by construction of the return type).
            assert!(out.len() <= cap + 32, "cap {cap}: len {}", out.len());
        }
        // A CJK line (3-byte chars) with the cut inside a character, too.
        let cjk = "你好世界你好世界你好世界"; // 12 chars × 3 bytes = 36 bytes
        let out = truncate_on_line_boundary(cjk, 10);
        assert!(out.contains("[truncated]"));
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
                    "{}: top-level-only events must not emit hookSpecificOutput — the official harness schema documents top-level decision fields only for this event",
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
    fn session_start_dispatch_self_heals_drifted_mcp_registration() {
        // End-to-end through the production entry point: a SessionStart hook
        // dispatched via run_hook_command must repair a drifted ~/.claude.json
        // (an entry missing `alwaysLoad`) without the user running
        // install/update/repair. This is the fix for the "binary swapped without
        // re-registering" drift vector. We isolate the home via
        // CLAUDE_TARGET_OVERRIDE pointed at a real `.claude` dir so the self-heal
        // is active (it skips non-`.claude` homes) yet never touches the real one.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "session-start-selfheal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let base = std::env::temp_dir().join(unique);
        // The home must be literally `.claude` for the self-heal to engage.
        let claude_home = base.join(".claude");
        std::fs::create_dir_all(&claude_home).unwrap();
        // ~/.claude.json lives beside the .claude dir (parent), per
        // mcp_config_path — resolve it the same way the production code does.
        let config_path = crate::manager::mcp_register::mcp_config_path(&claude_home);
        // Seed a DRIFTED entry: present but missing alwaysLoad.
        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"keel":{"type":"stdio","command":"old","args":["mcp","serve"],"env":{}}}}"#,
        )
        .unwrap();

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_self_heal = std::env::var(MCP_SELF_HEAL_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::remove_var(MCP_SELF_HEAL_ENV_VAR); // default → on

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_hook_command(&["session-start".to_string()], &mut stdout, &mut stderr);

        // Restore env before assertions so a failure cannot leak the override.
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_self_heal {
            Some(value) => std::env::set_var(MCP_SELF_HEAL_ENV_VAR, value),
            None => std::env::remove_var(MCP_SELF_HEAL_ENV_VAR),
        }

        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        // The SessionStart context still renders (load-bearing work survives).
        let output: JsonDocument =
            serde_json::from_slice(&stdout).expect("SessionStart still emits valid JSON");
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"], "SessionStart",
            "self-heal must not disturb the SessionStart context render"
        );
        // And the drifted entry is now repaired with alwaysLoad:true.
        let parsed: JsonDocument =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(
            parsed["mcpServers"]["keel"]["alwaysLoad"],
            serde_json::json!(true),
            "SessionStart dispatch must repair the drifted entry to carry alwaysLoad:true"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn session_start_dispatch_respects_self_heal_off_switch() {
        // The off switch must fully disable the write so an operator (or a test)
        // can opt out. With it off, a drifted entry stays drifted.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "session-start-selfheal-off-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let base = std::env::temp_dir().join(unique);
        let claude_home = base.join(".claude");
        std::fs::create_dir_all(&claude_home).unwrap();
        let config_path = crate::manager::mcp_register::mcp_config_path(&claude_home);
        let drifted = r#"{"mcpServers":{"keel":{"type":"stdio","command":"old","args":["mcp","serve"],"env":{}}}}"#;
        std::fs::write(&config_path, drifted).unwrap();

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_self_heal = std::env::var(MCP_SELF_HEAL_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::set_var(MCP_SELF_HEAL_ENV_VAR, "off");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_hook_command(&["session-start".to_string()], &mut stdout, &mut stderr);

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_self_heal {
            Some(value) => std::env::set_var(MCP_SELF_HEAL_ENV_VAR, value),
            None => std::env::remove_var(MCP_SELF_HEAL_ENV_VAR),
        }

        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        // The off switch left the drifted entry untouched, byte for byte.
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            drifted,
            "with the self-heal off, the drifted entry must be left exactly as-is"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn session_end_dispatch_auto_captures_work_summary_to_memory() {
        // s5: SessionEnd must auto-write a recallable work summary built from this
        // session's edit-class observations, and that write must be searchable
        // immediately (it routes through the research-cache record path, which
        // s4 made index-syncing). We isolate the home via CLAUDE_TARGET_OVERRIDE.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "session-end-capture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let claude_home = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&claude_home).unwrap();

        // Seed this session's observation rows: two edits and one command.
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let obs_dir = claude_home.join("state").join("observations");
        std::fs::create_dir_all(&obs_dir).unwrap();
        let session_id = "sess-capture-1";
        let cwd = "D:/Nasri/Project/capture-demo";
        let rows = format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Edit", "signature": "edit:rs", "detail": "src/lib.rs"}),
            serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Edit", "signature": "edit:md", "detail": "README.md"}),
            serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Bash", "signature": "cargo test", "detail": "cargo test"}),
        );
        std::fs::write(obs_dir.join(format!("{date}.jsonl")), rows).unwrap();

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_capture = std::env::var(SESSION_CAPTURE_ENV_VAR).ok();
        // SessionEnd's lifecycle path also runs learning; keep it off so the test
        // is scoped to the capture behavior only.
        let previous_learning = std::env::var("CLAUDE_SKILLS_LEARNING").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
        std::env::remove_var(SESSION_CAPTURE_ENV_VAR); // default → on
        std::env::set_var("CLAUDE_SKILLS_LEARNING", "off");

        let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");
        // Drive the REAL dispatch body with injected stdin (Major 2 fix): this
        // exercises the production "session-end" arm ordering — capture before
        // the lifecycle side effects — not just the helper in isolation.
        let code = run_hook_session_end(
            &mut stdin_json.as_bytes(),
            &mut std::io::sink(),
            &mut std::io::sink(),
        );
        assert_eq!(code, 0, "session-end dispatch must exit 0");

        // Restore env before assertions.
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_capture {
            Some(value) => std::env::set_var(SESSION_CAPTURE_ENV_VAR, value),
            None => std::env::remove_var(SESSION_CAPTURE_ENV_VAR),
        }
        match previous_learning {
            Some(value) => std::env::set_var("CLAUDE_SKILLS_LEARNING", value),
            None => std::env::remove_var("CLAUDE_SKILLS_LEARNING"),
        }

        // A research-cache record must now exist carrying the summary.
        let rc_dir = claude_home.join("memory").join("research-cache");
        let mut found_summary = false;
        if let Ok(entries) = std::fs::read_dir(&rc_dir) {
            for entry in entries.flatten() {
                let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if body.contains("Edited 2 file(s)")
                    && body.contains("rs")
                    && body.contains("md")
                    && body.contains("cargo test")
                {
                    found_summary = true;
                }
            }
        }
        assert!(
            found_summary,
            "SessionEnd must write a research-cache record summarizing the 2 edits + cargo test"
        );

        // And it must be immediately recallable (s4 index sync on the write path).
        let hit = crate::utility::recall::search_recall_index(&claude_home, "Edited file", 20)
            .expect("recall search runs");
        assert!(
            hit.map(|result| !result.hits.is_empty()).unwrap_or(false),
            "the auto-captured summary must be recallable right after SessionEnd"
        );

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn session_end_capture_is_silent_without_edits_and_respects_off_switch() {
        // Two guarantees: (1) a session that edited nothing produces no summary
        // (no memory pollution from research/question turns); (2) the off switch
        // fully disables capture even when edits exist.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!(
            "session-end-capture-off-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let claude_home = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&claude_home).unwrap();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let obs_dir = claude_home.join("state").join("observations");
        std::fs::create_dir_all(&obs_dir).unwrap();

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_capture = std::env::var(SESSION_CAPTURE_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

        // Case 1: command-only session (no edits) with capture ON → silent.
        std::env::remove_var(SESSION_CAPTURE_ENV_VAR);
        let read_only_session = "sess-readonly";
        std::fs::write(
            obs_dir.join(format!("{date}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({"recorded_at_ms": now_ms(), "session_id": read_only_session, "cwd": "D:/x", "tool_name": "Bash", "signature": "cargo test", "detail": "cargo test"})
            ),
        )
        .unwrap();
        maybe_capture_session_summary(
            &mut format!("{{\"session_id\":\"{read_only_session}\"}}").as_bytes(),
            &mut std::io::sink(),
        );
        assert!(
            !claude_home.join("memory").join("research-cache").exists()
                || std::fs::read_dir(claude_home.join("memory").join("research-cache"))
                    .map(|mut e| e.next().is_none())
                    .unwrap_or(true),
            "a no-edit session must write no summary"
        );

        // Case 2: edits exist but capture is OFF → still no summary.
        std::env::set_var(SESSION_CAPTURE_ENV_VAR, "off");
        let edit_session = "sess-edits-off";
        std::fs::write(
            obs_dir.join(format!("{date}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({"recorded_at_ms": now_ms(), "session_id": edit_session, "cwd": "D:/x", "tool_name": "Edit", "signature": "edit:rs", "detail": "src/lib.rs"})
            ),
        )
        .unwrap();
        maybe_capture_session_summary(
            &mut format!("{{\"session_id\":\"{edit_session}\"}}").as_bytes(),
            &mut std::io::sink(),
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_capture {
            Some(value) => std::env::set_var(SESSION_CAPTURE_ENV_VAR, value),
            None => std::env::remove_var(SESSION_CAPTURE_ENV_VAR),
        }

        let rc_dir = claude_home.join("memory").join("research-cache");
        let wrote_anything = std::fs::read_dir(&rc_dir)
            .map(|mut e| e.next().is_some())
            .unwrap_or(false);
        assert!(
            !wrote_anything,
            "with the off switch set, no summary must be written even when edits exist"
        );

        let _ = std::fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn hook_help_lists_every_official_event_slug() {
        // Regression guard: an earlier hand-maintained help string was
        // missing 14 of the 29 official slugs. Anyone running
        // `keel hook` to discover what's available saw a partial
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
        let claude_home =
            std::env::temp_dir().join(format!("keel-diagnose-healthy-{}", std::process::id()));
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
        let claude_home =
            std::env::temp_dir().join(format!("keel-diagnose-mismatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).unwrap();

        let executable = crate::runtime::installed_executable_path(&claude_home);
        std::fs::write(&executable, b"installed").unwrap();

        // settings.json points at a different binary (the historical
        // ~/.claude/keel.exe.stale-* leakage shape, where the hook
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
        let claude_home =
            std::env::temp_dir().join(format!("keel-diagnose-orphan-{}", std::process::id()));
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
        let claude_home =
            std::env::temp_dir().join(format!("keel-diagnose-text-{}", std::process::id()));
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
        // Stop and SubagentStop must always exit 0 AND emit no stdout. Two
        // distinct hazards this guards against:
        //   1. A non-zero exit makes the harness re-run the turn (stop cascade).
        //   2. Any stdout carrying hookSpecificOutput.additionalContext on a Stop
        //      hook means "keep going" — so emitting it makes the agent loop
        //      forever. This was the PR #121 regression; the dispatch arm now
        //      short-circuits both events to exit 0 with no output.
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
                "{subcommand} must emit no stdout (additionalContext would loop the turn); got: {}",
                String::from_utf8_lossy(&stdout)
            );
        }
    }

    #[test]
    fn notification_emits_bell_terminal_sequence() {
        // CC 2.1.141: Notification fires when the harness wants the user's
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
    fn hook_install_honors_claude_home_flag_and_never_touches_real_home() {
        // DEFECT 1 regression: `hook install` previously hardcoded
        // resolve_claude_home("") and so always wrote the real ~/.claude,
        // ignoring --claude-home. A probe that believed it was isolated
        // rewrote the user's live settings.json. This test pins the fix:
        // --claude-home must route the write to the requested dir, and the
        // env-resolved "real" home (CLAUDE_TARGET_OVERRIDE) must be untouched.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!("hook-install-isolation-{}", std::process::id());
        let base = std::env::temp_dir().join(unique);
        let isolated_home = base.join("isolated");
        let sentinel_real_home = base.join("sentinel-real");
        std::fs::create_dir_all(&isolated_home).unwrap();
        std::fs::create_dir_all(&sentinel_real_home).unwrap();

        // Point the env-resolved "real" home at a sentinel so we can prove the
        // install did NOT fall through to it.
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &sentinel_real_home);

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_hook_command(
            &[
                "install".to_string(),
                "--claude-home".to_string(),
                isolated_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );

        // Restore env before assertions so a failure does not leak override.
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }

        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let isolated_settings = isolated_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        let sentinel_settings = sentinel_real_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        assert!(
            isolated_settings.is_file(),
            "hook install must write settings.json under --claude-home"
        );
        assert!(
            !sentinel_settings.exists(),
            "hook install must NOT write the env-resolved real home when --claude-home is given"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hook_list_redacts_secret_env_values() {
        // WARN A regression: hook list/show printed settings.json verbatim,
        // leaking a live ANTHROPIC_AUTH_TOKEN in any captured output. The fix
        // masks secret-pattern keys while leaving non-secret structure intact.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let unique = format!("hook-list-redact-{}", std::process::id());
        let home = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&home).unwrap();
        let settings = home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        std::fs::write(
            &settings,
            r#"{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "sk-secret-token-value-123456",
    "OPENAI_API_KEY": "key-abcdefghijklmnop",
    "ANTHROPIC_BASE_URL": "https://api.example.com"
  },
  "hooks": {}
}"#,
        )
        .unwrap();

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_hook_command(
            &[
                "list".to_string(),
                "--claude-home".to_string(),
                home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let out = String::from_utf8_lossy(&stdout);

        // The raw secret values must be gone.
        assert!(
            !out.contains("sk-secret-token-value-123456"),
            "auth token must be redacted, got: {out}"
        );
        assert!(
            !out.contains("key-abcdefghijklmnop"),
            "api key must be redacted, got: {out}"
        );
        // A recognizable prefix is kept so operators can still identify it.
        assert!(out.contains("…(redacted)"), "masked marker missing: {out}");
        // Non-secret values stay readable.
        assert!(
            out.contains("https://api.example.com"),
            "non-secret base url must remain visible: {out}"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn is_secret_key_classifies_known_markers() {
        assert!(is_secret_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(is_secret_key("OPENAI_API_KEY"));
        assert!(is_secret_key("some_secret"));
        assert!(is_secret_key("DB_PASSWORD"));
        assert!(is_secret_key("ACCESS_KEY"));
        assert!(!is_secret_key("ANTHROPIC_BASE_URL"));
        assert!(!is_secret_key("matcher"));
        assert!(!is_secret_key("command"));
        // A bare `*key` suffix must NOT trigger without an auth/api/access
        // marker, so ordinary words are not falsely redacted.
        assert!(!is_secret_key("monkey"));
        assert!(!is_secret_key("passkey"));
    }

    #[test]
    fn mask_secret_value_handles_multibyte_utf8_without_panicking() {
        // Regression: slicing &value[..4] by byte offset panics if a multi-byte
        // char straddles offset 4. Mask by chars so this can never panic.
        let masked = mask_secret_value("sk-¥token-multibyte-value");
        assert!(masked.ends_with("…(redacted)"), "got: {masked}");
        assert!(
            !masked.contains("multibyte-value"),
            "tail must be hidden: {masked}"
        );
        // Short multi-byte value is fully masked, also without panicking.
        assert_eq!(mask_secret_value("¥¥"), "****");
    }

    #[test]
    fn redact_settings_suppresses_malformed_json_instead_of_leaking() {
        // Regression: the parse-failure path must NOT return the raw text — a
        // truncated/garbage settings.json could otherwise dump a live token.
        let malformed = r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-leaky-secret-123"} TRAILING GARBAGE"#;
        let out = redact_secrets_in_settings(malformed);
        assert!(
            !out.contains("sk-leaky-secret-123"),
            "malformed JSON must not leak the secret, got: {out}"
        );
        assert!(
            out.contains("suppressed"),
            "expected suppression notice: {out}"
        );
    }

    #[test]
    fn redact_settings_masks_secret_in_nested_object() {
        // A secret reached via a secret-named parent key, nested one level deep,
        // must still be masked (the parent_key_is_secret carry-down path).
        let nested = r#"{"credentials":{"value":"deep-secret-token-value"},"hooks":{}}"#;
        // "credentials" is not itself a marker, but "token" inside the value is
        // not how we detect it — detection is by KEY. Use a secret key wrapping
        // an object to exercise the carry-down.
        let by_secret_parent = r#"{"api_key":{"primary":"nested-secret-abcdef"},"hooks":{}}"#;
        let out = redact_secrets_in_settings(by_secret_parent);
        assert!(
            !out.contains("nested-secret-abcdef"),
            "secret under a secret-named parent key must be masked: {out}"
        );
        // A non-secret nested value stays visible.
        let out2 = redact_secrets_in_settings(nested);
        assert!(
            out2.contains("deep-secret-token-value"),
            "value under a non-secret key stays visible: {out2}"
        );
    }

    #[test]
    fn install_writes_default_skill_listing_budget_fraction() {
        let hook_path = temp_hook_path("keel-skill-budget-default");
        std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
        std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

        let executable = std::env::current_exe().unwrap();
        let rendered = build_hooks_payload(&hook_path, &executable).unwrap();
        let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

        assert_eq!(
            document
                .get("skillListingBudgetFraction")
                .and_then(JsonDocument::as_f64),
            Some(0.06),
        );

        let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
    }

    #[test]
    fn install_preserves_user_skill_listing_budget_fraction() {
        let hook_path = temp_hook_path("keel-skill-budget-preserve");
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
        let temp = compression_hint_tempdir("keel-hint-below-threshold");
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
        let temp = compression_hint_tempdir("keel-hint-at-threshold");
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
        let temp = compression_hint_tempdir("keel-hint-off");
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
        let temp = compression_hint_tempdir("keel-hint-force");
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
        let temp = compression_hint_tempdir("keel-hint-missing-jsonl");
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
        let temp = compression_hint_tempdir("keel-count-session-rows");
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
        let temp = compression_hint_tempdir("keel-hint-threshold-zero");
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

    #[test]
    fn research_gate_nudges_when_no_research_before_edit() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "nudge");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        let temp = std::env::temp_dir().join("keel-research-gate-test");
        let claude_home = temp.join("claude-home");
        let _ = std::fs::create_dir_all(claude_home.join("state").join("tool-timings"));
        let _ = std::fs::create_dir_all(claude_home.join("state").join("research-gate-blocks"));

        let session_id = "test-research-gate-session";
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let timings_path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{date}.jsonl"));
        let _ = std::fs::write(
            &timings_path,
            format!(
                "{{\"session_id\":\"{session_id}\",\"tool_name\":\"Write\",\"recorded_at_ms\":1000,\"cwd\":\"/tmp\"}}\n"
            ),
        );

        let decision = decide_gate(
            research_gate_mode(),
            research_gate_max_blocks(),
            0,
            1,
            session_has_research_tool(&claude_home, session_id),
        );
        assert_eq!(
            decision,
            GateDecision::Nudge,
            "research gate must nudge when code edited but no research tool found"
        );

        let nudge_msg = research_gate_message(GateDecision::Nudge);
        assert!(nudge_msg.contains("CLAUDE_SKILLS_RESEARCH_GATE"));
        assert!(nudge_msg.contains("does not stop the turn"));
        assert!(nudge_msg.contains("=off"));

        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn research_gate_off_matches_advisory_path() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");

        assert_eq!(
            decide_gate(GateMode::Off, 1, 0, 5, false),
            GateDecision::Advisory,
            "research gate off must always be Advisory"
        );

        let block_msg = research_gate_message(GateDecision::Block);
        assert!(block_msg.contains("hard stop"));
        let nudge_msg = research_gate_message(GateDecision::Nudge);
        assert!(nudge_msg.contains("does not stop the turn"));

        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
    }

    #[test]
    fn story_first_gate_nudges_when_no_stories_before_edit() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
        let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
        let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
        let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "nudge");
        std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
        std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
        std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
        std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

        let temp = std::env::temp_dir().join("keel-story-first-gate-test");
        let claude_home = temp.join("claude-home");
        let _ = std::fs::create_dir_all(claude_home.join("state").join("story-first"));
        let _ = std::fs::create_dir_all(claude_home.join("state").join("story-first-gate-blocks"));

        let session_id = "test-story-first-gate-session";
        let marker = story_confirmed_marker_path(&claude_home, session_id);
        assert!(
            !marker.exists(),
            "marker must not exist before being created"
        );

        let decision = decide_gate(
            story_first_gate_mode(),
            story_first_gate_max_blocks(),
            0,
            1,
            marker.exists(),
        );
        assert_eq!(
            decision,
            GateDecision::Nudge,
            "story-first gate must nudge when code edited but no stories confirmed"
        );

        let nudge_msg = story_first_gate_message(GateDecision::Nudge);
        assert!(nudge_msg.contains("CLAUDE_SKILLS_STORY_FIRST_GATE"));
        assert!(nudge_msg.contains("does not stop the turn"));
        assert!(nudge_msg.contains("=off"));

        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
        match previous_brief {
            Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
            None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
        }
        match previous_review {
            Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
            None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
        }
        match previous_research {
            Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
            None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
        }
        match previous_closeout {
            Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn story_first_gate_off_matches_advisory_path() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
        std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");

        assert_eq!(
            decide_gate(GateMode::Off, 1, 0, 5, false),
            GateDecision::Advisory,
            "story-first gate off must always be Advisory"
        );

        let block_msg = story_first_gate_message(GateDecision::Block);
        assert!(block_msg.contains("hard stop"));
        let nudge_msg = story_first_gate_message(GateDecision::Nudge);
        assert!(nudge_msg.contains("does not stop the turn"));

        match previous_story_first {
            Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
            None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
        }
    }
}
