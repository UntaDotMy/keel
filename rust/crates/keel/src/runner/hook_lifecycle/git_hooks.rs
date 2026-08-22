//! Hook lifecycle git_hooks responsibility split.

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
    review_marker_ms, run_bridge_session_end, run_hook_command, run_hook_cwd_changed,
    run_hook_diagnose, run_hook_install, run_hook_instructions, run_hook_lifecycle, run_hook_list,
    run_hook_notification, run_hook_permission_denied, run_hook_permission_request,
    run_hook_post_tool_batch, run_hook_post_tool_use, run_hook_post_tool_use_failure,
    run_hook_pre_tool_use, run_hook_session_end, run_hook_subagent_start, run_hook_uninstall,
    run_hook_user_prompt_submit, run_iron_law_gate, run_post_tool_comment_lint,
    run_post_tool_graph_context, run_session_end_learning, sanitize_memory_key, session_edit_stats,
    session_has_iron_law_evidence, session_has_research_tool, session_start_context,
    session_start_ms, settings_points_at_installed_executable, should_refresh_system_map,
    skill_pointer_fallback, skill_pointer_text, sort_hook_events, subagent_start_context,
    system_map_edit_counter_path, system_map_refresh_threshold, today_date_string,
    tool_input_command, tool_is_anvil_surface, tool_is_iron_law_gated, tool_satisfies_iron_law,
    truncate_on_line_boundary, user_config_or_env_u64, user_config_review_strictness,
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
    LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR, MANAGED_PRE_TOOL_USE_EVENT, MCP_SELF_HEAL_ENV_VAR,
    MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR, NOTIFICATION_BELL_OUTPUT,
    OBSERVATION_DEFAULT_RETENTION_DAYS, PLUGIN_MEMORY_RETENTION_DAYS, PLUGIN_REVIEW_STRICTNESS,
    PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL, RAW_OUTPUT_DEFAULT_RETENTION_DAYS, RESEARCH_GATE_ENV_VAR,
    RESEARCH_GATE_MAX_BLOCKS_ENV_VAR, REVIEW_GATE_ENV_VAR, REVIEW_GATE_MAX_BLOCKS_ENV_VAR,
    SESSION_CAPTURE_ENV_VAR, SYNTHESIS_NUDGE_MAX_BYTES, SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD,
    TIMINGS_DEFAULT_RETENTION_DAYS, USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP,
    WORKSPACE_DIGEST_MAX_BYTES, WORK_INTENT_REMINDER,
};

pub(super) fn set_core_hooks_path(git_config: &str, value: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut core_insert_at: Option<usize> = None;

    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed.eq_ignore_ascii_case("[core]") {
                // Insert position = right after this header line.
                core_insert_at = Some(lines.len() + 1);
            }
            lines.push(line.to_string());
            continue;
        }
        // A hooksPath key line in any section is dropped; a single canonical
        // entry is re-inserted under [core] below.
        if let Some((key, _)) = trimmed.split_once('=') {
            if key.trim().eq_ignore_ascii_case("hookspath") {
                continue;
            }
        }
        lines.push(line.to_string());
    }

    let entry = format!("\thooksPath = {value}");
    if let Some(index) = core_insert_at {
        lines.insert(index, entry);
    } else {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[core]".to_string());
        lines.push(entry);
    }

    let mut joined = lines.join("\n");
    if git_config.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

pub(super) fn run_hook_git_hooks(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("hook git-hooks");
    flag_set.string_flag("repo-root", "");

    let mut args = arguments.to_vec();
    if args.first().map(|s| s.as_str()) == Some("install") {
        args.remove(0);
    }

    if let Err(parse_error) = flag_set.parse(&args) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let repo_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let githooks_dir = repo_root.join(".githooks");

    if !githooks_dir.exists() {
        let _ = writeln!(
            standard_error,
            "No .githooks directory found in {}",
            display_path(&repo_root)
        );
        return 1;
    }

    let hooks = ["pre-commit", "pre-push"];

    for hook_name in &hooks {
        let hook_path = githooks_dir.join(hook_name);
        if !hook_path.exists() {
            let _ = writeln!(standard_error, "Hook file not found: {}", hook_name);
            continue;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = match fs::metadata(&hook_path) {
                Ok(metadata) => metadata.permissions(),
                Err(e) => {
                    let _ = writeln!(
                        standard_error,
                        "Failed to read permissions for {}: {}",
                        hook_name, e
                    );
                    continue;
                }
            };
            perms.set_mode(0o755);
            if let Err(e) = fs::set_permissions(&hook_path, perms) {
                let _ = writeln!(
                    standard_error,
                    "Failed to make {} executable: {}",
                    hook_name, e
                );
                continue;
            }
        }

        let _ = writeln!(
            standard_output,
            "  {}",
            hook_path
                .strip_prefix(&repo_root)
                .unwrap_or(&hook_path)
                .display()
        );
    }

    let git_config_path = repo_root.join(".git").join("config");
    let hooks_path_value = ".githooks";

    if git_config_path.exists() {
        // why: a failed read (perms, AV lock) must never turn into an
        // unconditional overwrite that replaces a real config with a stub.
        let git_config = match fs::read_to_string(&git_config_path) {
            Ok(text) => text,
            Err(error) => {
                let _ = writeln!(
                    standard_error,
                    "Refusing to edit {}: unreadable ({error})",
                    display_path(&git_config_path)
                );
                return 1;
            }
        };
        let updated_config = set_core_hooks_path(&git_config, hooks_path_value);
        if let Err(e) = fs::write(&git_config_path, &updated_config) {
            let _ = writeln!(standard_error, "Failed to update .git/config: {}", e);
            return 1;
        }
    } else {
        let _ = writeln!(
            standard_error,
            "Warning: .git/config not found. Git hooks may not work."
        );
    }

    let _ = writeln!(
        standard_output,
        "Installed git hooks: {}",
        hooks
            .iter()
            .filter(|h| githooks_dir.join(h).exists())
            .copied()
            .collect::<Vec<_>>()
            .join(", ")
    );

    0
}
