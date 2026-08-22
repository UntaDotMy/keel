//! Hook lifecycle state responsibility split.

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
    build_session_summary, collect_hook_diagnostics, command_path_is_managed_executable,
    completeness_gate_blocks_path, completeness_gate_max_blocks, completeness_gate_message,
    completeness_gate_mode, completeness_marker_ms, completeness_scan_satisfies,
    compression_hint_text, count_session_tool_timing_rows, cue_used_as_verb, decide_gate,
    decode_powershell_encoded_command, default_max_blocks_for, emit_gate_decision,
    emit_post_tool_batch_advisory, emit_post_tool_batch_block, emit_post_tool_batch_nudge,
    emit_pretool_deny, ensure_hooks_object, ensure_skill_listing_budget_fraction,
    evaluate_learned_skill_gate, file_mtime_ms, gate_mode, gate_mode_value, gate_status_rows,
    iron_law_gate_decision, iron_law_gate_mode, iron_law_legacy_path, iron_law_marker_present,
    iron_law_satisfied_path, is_help_argument, is_host_research_tool_name, is_host_shell_tool_name,
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
    prune_tool_timings_store, read_counter_value, read_hooks_document, record_anvil_gate_clear,
    record_completeness_gate_clear_for, record_review_gate_clear, redact_secrets_in_settings,
    redact_secrets_in_value, refresh_memory_scope_for_current_directory,
    remove_managed_hook_payload, remove_managed_hook_payload_for_home, remove_managed_hooks,
    render_hook_help, render_lifecycle_payload, research_gate_blocks_path,
    research_gate_max_blocks, research_gate_message, research_gate_mode,
    resolve_current_executable, review_gate_blocks_path, review_gate_max_blocks,
    review_gate_message, review_gate_mode, review_marker_ms, run_bridge_session_end,
    run_hook_command, run_hook_cwd_changed, run_hook_diagnose, run_hook_git_hooks,
    run_hook_install, run_hook_instructions, run_hook_lifecycle, run_hook_list,
    run_hook_notification, run_hook_permission_denied, run_hook_permission_request,
    run_hook_post_tool_batch, run_hook_post_tool_use, run_hook_post_tool_use_failure,
    run_hook_pre_tool_use, run_hook_session_end, run_hook_subagent_start, run_hook_uninstall,
    run_hook_user_prompt_submit, run_iron_law_gate, run_post_tool_comment_lint,
    run_post_tool_graph_context, run_session_end_learning, sanitize_memory_key, session_edit_stats,
    session_has_iron_law_evidence, session_has_research_tool, session_start_context,
    session_start_ms, set_core_hooks_path, settings_points_at_installed_executable,
    should_refresh_system_map, skill_pointer_fallback, skill_pointer_text, sort_hook_events,
    subagent_start_context, today_date_string, tool_input_command, tool_is_anvil_surface,
    tool_is_iron_law_gated, tool_satisfies_iron_law, truncate_on_line_boundary,
    user_prompt_submit_context, user_prompt_submit_core, work_intent_pointer_for_prompt,
    workspace_memory_digest, GateDecision, GateMode, GateStatusRow, HookDiagnostics,
    IronLawGateMode, ManagedHookEntry, SessionEditStats, SessionSummary, ANVIL_GATE_DENIAL,
    ANVIL_SATISFIED_DIR, BRIEF_GATE_ENV_VAR, BRIEF_GATE_MAX_BLOCKS_ENV_VAR,
    BRIEF_GATE_SESSION_GRACE_MS, COMPACT_BOOTSTRAP, COMPLETENESS_GATE_ENV_VAR,
    COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR, COMPRESSION_HINT_DEFAULT_THRESHOLD,
    DIGEST_BRIEF_MAX_BYTES, DIGEST_MAP_HEAD_MAX_BYTES, DIGEST_MEMORY_MAX_BYTES,
    GATE_DEFAULT_MAX_BLOCKS, INSTINCT_DIGEST_MAX_BYTES, IRON_LAW_GATE_DENIAL_BALANCED,
    IRON_LAW_GATE_DENIAL_STRICT, IRON_LAW_GATE_DENIAL_VERIFIED, IRON_LAW_GATE_ENV_VAR,
    IRON_LAW_LEGACY_GATE_DIR, IRON_LAW_SATISFIED_DIR, LEARNED_SKILL_GATE_ENV_VAR,
    LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR, MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR,
    NOTIFICATION_BELL_OUTPUT, RESEARCH_GATE_ENV_VAR, RESEARCH_GATE_MAX_BLOCKS_ENV_VAR,
    SYNTHESIS_NUDGE_MAX_BYTES, USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP,
    WORKSPACE_DIGEST_MAX_BYTES, WORK_INTENT_REMINDER,
};

pub(super) const RAW_OUTPUT_DEFAULT_RETENTION_DAYS: u64 = 14;

/// Tool-timings JSONL rows are tiny (one short line per tool call) compared
/// to raw-output directories, so a longer default retention is fine. 30 days
/// gives an analyzer a useful month-long sample without letting the directory
/// grow unbounded across long sessions. Tunable via
/// `CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS`; setting it to `0` disables the
/// SessionEnd prune.
pub(super) const TIMINGS_DEFAULT_RETENTION_DAYS: u64 = 30;

/// Behavioral observation JSONL rows feed the learning loop. They age out of
/// the loop's 7-day distillation window naturally, but the files are pruned on
/// a longer horizon so a late `learn --window 14` inspection still has data.
/// Tunable via `CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS`; `0` disables.
pub(super) const OBSERVATION_DEFAULT_RETENTION_DAYS: u64 = 30;

pub(super) const MANAGED_PRE_TOOL_USE_EVENT: &str = "PreToolUse";

/// SYSTEM_MAP.md is rebuilt every N edit-class tool calls so the workspace
/// pointer stays in sync with the repo without paying refresh cost on every
/// tool call. Tunable via `CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL`; setting
/// it to `0` disables the periodic refresh.
pub(super) const SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD: u64 = 10;

/// Env var that disables the SessionStart MCP-registration self-heal. Unset (the
/// default) keeps the self-heal on; set to `off` to skip it (used by tests that
/// must not touch any `~/.claude.json`, and as an operator escape hatch). Any
/// other value leaves the self-heal enabled.
pub(super) const MCP_SELF_HEAL_ENV_VAR: &str = "CLAUDE_SKILLS_MCP_SELF_HEAL";

/// Env var that disables the SessionEnd auto-capture of a session work summary
/// to memory. Unset (the default) keeps it on; set to `off` to skip it. Any
/// other value leaves it enabled. The capture is silent on sessions that did no
/// edit-class work, so research/question-only turns never write a summary.
pub(super) const SESSION_CAPTURE_ENV_VAR: &str = "CLAUDE_SKILLS_SESSION_CAPTURE";

/// Iterate canonical hook event names. Single-line wrapper around the table so
/// existing for-loops keep their `for event in claude_hook_event_names()` shape
/// without caring that the source is a typed row table.
pub(super) fn claude_hook_event_names() -> impl Iterator<Item = &'static str> {
    HOOK_EVENTS.iter().map(|event| event.name)
}

pub(super) fn read_stdin_text(stdin: &mut dyn Read) -> Result<String, String> {
    let mut buf = String::new();
    stdin
        .read_to_string(&mut buf)
        .map_err(|e| format!("unable to read hook input: {e}"))?;
    Ok(buf)
}

/// Parse optional hook JSON using the fail-open semantics shared by event handlers.
pub(super) fn read_json_stdin_fail_open(stdin: &mut dyn Read) -> Option<JsonDocument> {
    let mut text = String::new();
    match stdin.read_to_string(&mut text) {
        Ok(_) if !text.trim().is_empty() => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

pub(crate) fn is_edit_class_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "edit"
            | "write"
            | "multiedit"
            | "notebookedit"
            | "apply_patch"
            | "str_replace"
            | "strreplace"
            | "patch"
            // Grok maps Claude Edit/Write/MultiEdit onto search_replace.
            | "search_replace"
            | "searchreplace"
    )
}

/// Read a hook string from Claude snake_case or Grok/Cursor camelCase.
pub(super) fn hook_str<'a>(input: &'a JsonDocument, keys: &[&str]) -> &'a str {
    for key in keys {
        if let Some(value) = input.get(*key).and_then(JsonDocument::as_str) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    ""
}

pub(super) fn hook_tool_name(input: &JsonDocument) -> &str {
    hook_str(input, &["tool_name", "toolName"])
}

pub(super) fn hook_session_id(input: &JsonDocument) -> &str {
    let value = hook_str(input, &["session_id", "sessionId"]);
    if value.is_empty() {
        "default"
    } else {
        value
    }
}

pub(super) fn system_map_refresh_threshold() -> u64 {
    user_config_or_env_u64(
        PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL,
        "CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL",
        SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD,
    )
}

pub(super) fn system_map_edit_counter_path() -> Option<PathBuf> {
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

pub(super) fn increment_counter_file(path: &Path) -> std::io::Result<u64> {
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

pub(super) fn reset_counter_file(path: &Path) -> std::io::Result<()> {
    fs::write(path, "0")
}

pub(super) const REVIEW_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE";

pub(super) const REVIEW_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_REVIEW_GATE_MAX_BLOCKS";

// The plugin manifest (.claude-plugin/plugin.json `userConfig`) declares three

pub(super) const PLUGIN_REVIEW_STRICTNESS: &str = "CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS";

pub(super) const PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL: &str =
    "CLAUDE_PLUGIN_OPTION_SYSTEM_MAP_REFRESH_INTERVAL";

pub(super) const PLUGIN_MEMORY_RETENTION_DAYS: &str = "CLAUDE_PLUGIN_OPTION_MEMORY_RETENTION_DAYS";

/// Map the harness userConfig vocabulary (`advisory`/`strict`/`off`) onto the
/// `GateMode` vocabulary (`nudge`/`block`/`off`). Unrecognized values fall
/// through to the caller's default (None) so a typo does not silently disable.
pub(super) fn user_config_review_strictness() -> Option<GateMode> {
    let value = std::env::var(PLUGIN_REVIEW_STRICTNESS)
        .ok()?
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "advisory" | "nudge" => Some(GateMode::Nudge),
        "strict" | "block" => Some(GateMode::Block),
        "off" | "0" | "false" | "no" => Some(GateMode::Off),
        _ => None,
    }
}

/// Read a numeric userConfig knob, falling back to the explicit operator env
/// var, then to `default`. Used by the system-map refresh interval and the
/// memory retention days. Empty/unparseable userConfig falls through to the
/// operator var/default rather than erroring.
pub(super) fn user_config_or_env_u64(plugin_var: &str, operator_var: &str, default: u64) -> u64 {
    if let Ok(value) = std::env::var(plugin_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            if let Ok(parsed) = trimmed.parse::<u64>() {
                return parsed;
            }
        }
    }
    std::env::var(operator_var)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}
