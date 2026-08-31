//! Hook lifecycle dispatch responsibility split.

use super::*;

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

        "git-hooks" => run_hook_git_hooks(&arguments[1..], standard_output, standard_error),

        "uninstall" => run_hook_uninstall(&arguments[1..], standard_output, standard_error),

        "list" | "show" => run_hook_list(&arguments[1..], standard_output, standard_error),

        "instructions" => run_hook_instructions(&arguments[1..], standard_output, standard_error),

        "diagnose" => run_hook_diagnose(&arguments[1..], standard_output, standard_error),

        // PreToolUse runs the transparent rewriter and emits hookSpecificOutput.
        "pre-tool-use" => run_hook_pre_tool_use(standard_output, standard_error),

        // PostToolUse counts edit-class tool calls and refreshes SYSTEM_MAP.md
        "post-tool-use" => run_hook_post_tool_use(standard_error),

        // PostToolUseFailure carries the same `duration_ms` field PostToolUse
        "post-tool-use-failure" => run_hook_post_tool_use_failure(standard_error),

        "stop" | "subagent-stop" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_stop(&mut stdin, standard_output, standard_error)
        }

        // Notification fires when the harness wants the user's attention
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
        "user-prompt-submit" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_user_prompt_submit(&mut stdin, standard_output, standard_error)
        }

        // PostToolBatch reads stdin for `session_id` so the optional review gate
        "post-tool-batch" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_post_tool_batch(&mut stdin, standard_output, standard_error)
        }

        // SessionStart re-asserts the keel MCP registration before the
        "session-start" => {
            maybe_self_heal_mcp_registration(standard_error);
            run_hook_lifecycle("session-start", standard_output, standard_error)
        }

        // SessionEnd reads stdin for `session_id` so the auto-capture can scope a
        "session-end" => {
            let mut stdin = std::io::stdin().lock();
            run_hook_session_end(&mut stdin, standard_output, standard_error)
        }

        // Every other slug is dispatched if and only if it appears in the canonical
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

pub(super) fn run_hook_notification(standard_output: &mut dyn Write) -> u8 {
    let _ = writeln!(standard_output, "{NOTIFICATION_BELL_OUTPUT}");

    0
}

/// Hook JSON emitted by Notification. BEL is in the CC 2.1.141
/// `terminalSequence` allowlist and is JSON-escaped as `\u0007` per
/// RFC 8259 (control characters U+0000–U+001F MUST be escaped inside a
/// JSON string). The harness unescapes the value before writing it to the
/// terminal, which is what produces the audible bell. `suppressOutput`
/// hides this row from the transcript so the bell is the only side effect.
pub(super) const NOTIFICATION_BELL_OUTPUT: &str =
    "{\"suppressOutput\":true,\"terminalSequence\":\"\\u0007\"}";

/// PermissionRequest handler.
///
/// Auto-approves Bash commands that invoke `keel` to reduce permission
/// prompt friction. For all other tool calls, returns 0 (no output) to let
/// the harness handle the permission dialog normally.
pub(super) fn run_hook_permission_request(
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
pub(super) fn run_hook_permission_denied(
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
pub(super) fn run_hook_subagent_start(
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
pub(super) fn run_hook_cwd_changed(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    run_hook_lifecycle("cwd-changed", standard_output, standard_error)
}
