//! Hook lifecycle dispatch responsibility split.

#![allow(unused_imports)]

use super::{
    display_path, event_by_name, event_by_slug, fs, installed_executable_path, learning,
    observation, resolve_claude_home, resolve_repository_root, rewrite_command_text_for_shell,
    rewrite_shell_for_tool, tool_timings, utility, write_indented, write_text, BTreeMap, FlagSet,
    HookEvent, JsonDocument, JsonMap, Path, PathBuf, RawStore, Read, Value, Write, HOOK_EVENTS,
};

use super::{
    anvil_gate_enabled, anvil_satisfied_path, anvil_satisfied_this_session,
    anvil_workspace_marker_ms, append_compression_hint_when_forced, append_managed_hooks,
    base64_decode, brief_gate_blocks_path, brief_gate_max_blocks, brief_gate_message,
    brief_gate_mode, brief_is_fresh, brief_written_this_session, build_hooks_payload,
    build_session_summary, claude_hook_event_names, collect_hook_diagnostics,
    command_path_is_managed_executable, completeness_gate_blocks_path,
    completeness_gate_max_blocks, completeness_gate_message, completeness_gate_mode,
    completeness_marker_ms, completeness_scan_satisfies, compression_hint_text,
    count_session_tool_timing_rows, cue_used_as_verb, decide_gate,
    decode_powershell_encoded_command, default_max_blocks_for, emit_gate_decision,
    emit_post_tool_batch_advisory, emit_post_tool_batch_block, emit_post_tool_batch_nudge,
    emit_pretool_deny, ensure_hooks_object, ensure_skill_listing_budget_fraction,
    evaluate_learned_skill_gate, file_mtime_ms, gate_mode, gate_mode_value, gate_status_rows,
    hook_session_id, hook_str, hook_tool_name, increment_counter_file, iron_law_gate_decision,
    iron_law_gate_mode, iron_law_legacy_path, iron_law_marker_present, iron_law_satisfied_path,
    is_edit_class_tool, is_help_argument, is_host_research_tool_name, is_host_shell_tool_name,
    is_keel_research_command, is_keel_research_tool_name, is_managed_args_form,
    is_managed_hook_command, is_managed_hook_command_with_depth, is_managed_hook_entry,
    is_secret_key, is_shell_tool_name, is_web_research_tool_name, learned_skill_gate_blocks_path,
    learned_skill_gate_max_blocks, learned_skill_gate_message, learned_skill_gate_mode,
    lifecycle_additional_context, managed_hook_command, managed_hook_entry, mark_anvil_satisfied,
    mark_iron_law_satisfied, mask_secret_value, maybe_capture_session_summary,
    maybe_capture_session_summary_with_id, maybe_compression_hint, maybe_mark_iron_law_from_parts,
    maybe_mark_iron_law_from_tool_event, maybe_self_heal_mcp_registration,
    mcp_tool_pointer_for_prompt, memory_gate_blocks_path, memory_gate_max_blocks,
    memory_gate_message, memory_gate_mode, memory_scope_summary,
    memory_system_map_path_for_workspace, memory_written_this_session, newest_brief_mtime_ms,
    newest_file_mtime_in_dir, newest_memory_write_ms, now_ms, post_compact_context,
    post_tool_batch_context, pre_compact_context, prune_dir_files_older_than,
    prune_observations_store, prune_raw_output_store, prune_state_marker_stores,
    prune_tool_timings_store, read_counter_value, read_hooks_document, read_stdin_text,
    record_anvil_gate_clear, record_completeness_gate_clear_for, record_review_gate_clear,
    redact_secrets_in_settings, redact_secrets_in_value,
    refresh_memory_scope_for_current_directory, remove_managed_hook_payload,
    remove_managed_hook_payload_for_home, remove_managed_hooks, render_hook_help,
    render_lifecycle_payload, research_gate_blocks_path, research_gate_max_blocks,
    research_gate_message, research_gate_mode, reset_counter_file, resolve_current_executable,
    review_gate_blocks_path, review_gate_max_blocks, review_gate_message, review_gate_mode,
    review_marker_ms, run_bridge_session_end, run_hook_diagnose, run_hook_git_hooks,
    run_hook_install, run_hook_instructions, run_hook_lifecycle, run_hook_list,
    run_hook_post_tool_batch, run_hook_post_tool_use, run_hook_post_tool_use_failure,
    run_hook_pre_tool_use, run_hook_session_end, run_hook_uninstall, run_hook_user_prompt_submit,
    run_iron_law_gate, run_post_tool_comment_lint, run_post_tool_graph_context,
    run_session_end_learning, sanitize_memory_key, session_edit_stats,
    session_has_iron_law_evidence, session_has_research_tool, session_start_context,
    session_start_ms, set_core_hooks_path, settings_points_at_installed_executable,
    should_refresh_system_map, skill_pointer_fallback, skill_pointer_text, sort_hook_events,
    subagent_start_context, system_map_edit_counter_path, system_map_refresh_threshold,
    today_date_string, tool_input_command, tool_is_anvil_surface, tool_is_iron_law_gated,
    tool_satisfies_iron_law, truncate_on_line_boundary, user_config_or_env_u64,
    user_config_review_strictness, user_prompt_submit_context, user_prompt_submit_core,
    work_intent_pointer_for_prompt, workspace_memory_digest, GateDecision, GateMode, GateStatusRow,
    HookDiagnostics, IronLawGateMode, ManagedHookEntry, SessionEditStats, SessionSummary,
    ANVIL_GATE_DENIAL, ANVIL_SATISFIED_DIR, BRIEF_GATE_ENV_VAR, BRIEF_GATE_MAX_BLOCKS_ENV_VAR,
    BRIEF_GATE_SESSION_GRACE_MS, COMPACT_BOOTSTRAP, COMPLETENESS_GATE_ENV_VAR,
    COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR, COMPRESSION_HINT_DEFAULT_THRESHOLD,
    DIGEST_BRIEF_MAX_BYTES, DIGEST_MAP_HEAD_MAX_BYTES, DIGEST_MEMORY_MAX_BYTES,
    GATE_DEFAULT_MAX_BLOCKS, INSTINCT_DIGEST_MAX_BYTES, IRON_LAW_GATE_DENIAL_BALANCED,
    IRON_LAW_GATE_DENIAL_STRICT, IRON_LAW_GATE_DENIAL_VERIFIED, IRON_LAW_GATE_ENV_VAR,
    IRON_LAW_LEGACY_GATE_DIR, IRON_LAW_SATISFIED_DIR, LEARNED_SKILL_GATE_ENV_VAR,
    LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR, MANAGED_PRE_TOOL_USE_EVENT, MCP_SELF_HEAL_ENV_VAR,
    MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR, OBSERVATION_DEFAULT_RETENTION_DAYS,
    PLUGIN_MEMORY_RETENTION_DAYS, PLUGIN_REVIEW_STRICTNESS, PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL,
    RAW_OUTPUT_DEFAULT_RETENTION_DAYS, RESEARCH_GATE_ENV_VAR, RESEARCH_GATE_MAX_BLOCKS_ENV_VAR,
    REVIEW_GATE_ENV_VAR, REVIEW_GATE_MAX_BLOCKS_ENV_VAR, SESSION_CAPTURE_ENV_VAR,
    SYNTHESIS_NUDGE_MAX_BYTES, SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD,
    TIMINGS_DEFAULT_RETENTION_DAYS, USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP,
    WORKSPACE_DIGEST_MAX_BYTES, WORK_INTENT_REMINDER,
};

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

        // Stop and SubagentStop must never return a non-zero exit code, and must
        "stop" | "subagent-stop" => 0,

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
