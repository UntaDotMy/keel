//! Hook lifecycle facade: dispatches hook events and re-exports the stable runner APIs.
//! Implementations live in responsibility modules under this directory.
#![allow(unused_imports)]

pub(crate) use crate::args::FlagSet;
pub(crate) use crate::hooks::claude::{event_by_name, event_by_slug, HookEvent, HOOK_EVENTS};
pub(crate) use crate::json::{write_indented, Value};
pub(crate) use crate::proxy::raw_store::RawStore;
pub(crate) use crate::runner::shell_rewrite::{
    rewrite_command_text_for_shell, rewrite_shell_for_tool,
};
pub(crate) use crate::runner::tool_timings;
pub(crate) use crate::runner::{learning, observation};
pub(crate) use crate::runtime::{
    display_path, installed_executable_path, resolve_claude_home, resolve_repository_root,
    write_text,
};
pub(crate) use crate::utility;
pub(crate) use serde_json::{Map as JsonMap, Value as JsonDocument};
pub(crate) use std::collections::BTreeMap;
pub(crate) use std::fs;
pub(crate) use std::io::{Read, Write};
pub(crate) use std::path::{Path, PathBuf};

mod dispatch;
mod git_hooks;
mod post_batch;
mod post_tool;
mod pre_tool;
mod prompt_submit;
mod session_end;
mod session_start;
mod settings;
mod state;

pub use dispatch::run_hook_command;
use dispatch::{
    run_hook_cwd_changed, run_hook_notification, run_hook_permission_denied,
    run_hook_permission_request, run_hook_subagent_start, NOTIFICATION_BELL_OUTPUT,
};
use git_hooks::{run_hook_git_hooks, set_core_hooks_path};
use post_batch::{
    brief_gate_blocks_path, brief_gate_max_blocks, brief_gate_message, brief_gate_mode,
    brief_written_this_session, completeness_gate_blocks_path, completeness_gate_max_blocks,
    completeness_gate_message, completeness_gate_mode, completeness_marker_ms, decide_gate,
    default_max_blocks_for, emit_gate_decision, emit_post_tool_batch_advisory,
    emit_post_tool_batch_block, emit_post_tool_batch_nudge, evaluate_learned_skill_gate,
    file_mtime_ms, gate_mode, gate_mode_value, learned_skill_gate_blocks_path,
    learned_skill_gate_max_blocks, learned_skill_gate_message, learned_skill_gate_mode,
    memory_gate_blocks_path, memory_gate_max_blocks, memory_gate_message, memory_gate_mode,
    memory_written_this_session, newest_brief_mtime_ms, newest_file_mtime_in_dir,
    newest_memory_write_ms, now_ms, read_counter_value, research_gate_blocks_path,
    research_gate_max_blocks, research_gate_message, research_gate_mode, review_gate_blocks_path,
    review_gate_max_blocks, review_gate_message, review_gate_mode, review_marker_ms,
    run_hook_post_tool_batch, session_edit_stats, session_has_research_tool, session_start_ms,
    GateDecision, GateMode, SessionEditStats, BRIEF_GATE_ENV_VAR, BRIEF_GATE_MAX_BLOCKS_ENV_VAR,
    BRIEF_GATE_SESSION_GRACE_MS, COMPLETENESS_GATE_ENV_VAR, COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR,
    GATE_DEFAULT_MAX_BLOCKS, LEARNED_SKILL_GATE_ENV_VAR, LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR,
    MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR, RESEARCH_GATE_ENV_VAR,
    RESEARCH_GATE_MAX_BLOCKS_ENV_VAR,
};
pub use post_batch::{
    completeness_scan_satisfies, record_completeness_gate_clear_for, record_review_gate_clear,
};
pub(crate) use post_batch::{gate_status_rows, GateStatusRow};
use post_tool::{
    run_hook_post_tool_use, run_hook_post_tool_use_failure, run_post_tool_comment_lint,
    run_post_tool_graph_context,
};
pub use pre_tool::record_anvil_gate_clear;
use pre_tool::{
    anvil_gate_enabled, anvil_satisfied_path, anvil_satisfied_this_session,
    anvil_workspace_marker_ms, emit_pretool_deny, iron_law_gate_mode, iron_law_legacy_path,
    iron_law_marker_present, iron_law_satisfied_path, is_host_research_tool_name,
    is_host_shell_tool_name, is_keel_research_tool_name, is_shell_tool_name,
    is_web_research_tool_name, run_hook_pre_tool_use, run_iron_law_gate,
    session_has_iron_law_evidence, tool_input_command, tool_is_anvil_surface, ANVIL_GATE_DENIAL,
    ANVIL_SATISFIED_DIR, IRON_LAW_GATE_DENIAL_BALANCED, IRON_LAW_GATE_DENIAL_STRICT,
    IRON_LAW_GATE_DENIAL_VERIFIED, IRON_LAW_GATE_ENV_VAR, IRON_LAW_LEGACY_GATE_DIR,
    IRON_LAW_SATISFIED_DIR,
};
pub(crate) use pre_tool::{
    iron_law_gate_decision, is_keel_research_command, mark_anvil_satisfied,
    mark_iron_law_satisfied, maybe_mark_iron_law_from_parts, maybe_mark_iron_law_from_tool_event,
    tool_is_iron_law_gated, tool_satisfies_iron_law, IronLawGateMode,
};
pub(crate) use prompt_submit::user_prompt_submit_context;
use prompt_submit::{
    append_compression_hint_when_forced, compression_hint_text, count_session_tool_timing_rows,
    cue_used_as_verb, maybe_compression_hint, mcp_tool_pointer_for_prompt,
    run_hook_user_prompt_submit, skill_pointer_fallback, skill_pointer_text,
    user_prompt_submit_core, work_intent_pointer_for_prompt, COMPRESSION_HINT_DEFAULT_THRESHOLD,
    USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP, WORK_INTENT_REMINDER,
};
use session_end::{
    brief_is_fresh, build_session_summary, maybe_capture_session_summary,
    maybe_capture_session_summary_with_id, memory_scope_summary,
    memory_system_map_path_for_workspace, prune_dir_files_older_than, prune_observations_store,
    prune_raw_output_store, prune_state_marker_stores, refresh_memory_scope_for_current_directory,
    run_hook_session_end, today_date_string, truncate_on_line_boundary, workspace_memory_digest,
    SessionSummary, DIGEST_BRIEF_MAX_BYTES, DIGEST_MAP_HEAD_MAX_BYTES, DIGEST_MEMORY_MAX_BYTES,
    INSTINCT_DIGEST_MAX_BYTES, SYNTHESIS_NUDGE_MAX_BYTES, WORKSPACE_DIGEST_MAX_BYTES,
};
pub(crate) use session_end::{
    prune_tool_timings_store, run_bridge_session_end, run_session_end_learning, sanitize_memory_key,
};
use session_start::{
    lifecycle_additional_context, maybe_self_heal_mcp_registration, post_tool_batch_context,
    pre_compact_context, run_hook_lifecycle, should_refresh_system_map, subagent_start_context,
    COMPACT_BOOTSTRAP,
};
pub(crate) use session_start::{
    post_compact_context, render_lifecycle_payload, session_start_context,
};
use settings::{
    append_managed_hooks, base64_decode, collect_hook_diagnostics,
    command_path_is_managed_executable, decode_powershell_encoded_command, ensure_hooks_object,
    ensure_skill_listing_budget_fraction, is_help_argument, is_managed_args_form,
    is_managed_hook_command_with_depth, is_secret_key, mask_secret_value,
    redact_secrets_in_settings, redact_secrets_in_value, render_hook_help,
    resolve_current_executable, run_hook_diagnose, run_hook_install, run_hook_instructions,
    run_hook_list, run_hook_uninstall, settings_points_at_installed_executable, sort_hook_events,
    HookDiagnostics,
};
pub use settings::{
    build_hooks_payload, is_managed_hook_command, is_managed_hook_entry, managed_hook_command,
    managed_hook_entry, read_hooks_document, remove_managed_hook_payload,
    remove_managed_hook_payload_for_home, remove_managed_hooks, ManagedHookEntry,
};
pub(crate) use state::is_edit_class_tool;
use state::{
    claude_hook_event_names, hook_session_id, hook_str, hook_tool_name, increment_counter_file,
    read_json_stdin_fail_open, read_stdin_text, reset_counter_file, system_map_edit_counter_path,
    system_map_refresh_threshold, user_config_or_env_u64, user_config_review_strictness,
    MANAGED_PRE_TOOL_USE_EVENT, MCP_SELF_HEAL_ENV_VAR, OBSERVATION_DEFAULT_RETENTION_DAYS,
    PLUGIN_MEMORY_RETENTION_DAYS, PLUGIN_REVIEW_STRICTNESS, PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL,
    RAW_OUTPUT_DEFAULT_RETENTION_DAYS, REVIEW_GATE_ENV_VAR, REVIEW_GATE_MAX_BLOCKS_ENV_VAR,
    SESSION_CAPTURE_ENV_VAR, SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD, TIMINGS_DEFAULT_RETENTION_DAYS,
};

#[cfg(test)]
mod tests;
