//! Hook lifecycle facade: dispatches hook events and re-exports the stable runner APIs.
//! Implementations live in responsibility modules under this directory.

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

pub use dispatch::{run_hook_command, run_hook_command_with_stdin};
use git_hooks::run_hook_git_hooks;
#[cfg(test)]
pub use post_batch::completeness_marker_key;
pub(crate) use post_batch::gate_status_rows;
pub use post_batch::{
    completeness_marker_record, completeness_marker_record_for_workspace,
    completeness_scan_satisfies, record_completeness_gate_clear_for, record_review_gate_clear,
};
use post_batch::{
    file_mtime_ms, now_ms, run_hook_post_tool_batch, run_hook_stop, session_start_ms, GateMode,
    BRIEF_GATE_SESSION_GRACE_MS,
};
use post_tool::{run_hook_post_tool_use, run_hook_post_tool_use_failure};
pub use pre_tool::record_anvil_gate_clear;
#[cfg(test)]
pub(crate) use pre_tool::{emit_pretool_deny, iron_law_gate_decision, is_keel_research_tool_name};
pub(crate) use pre_tool::{
    is_host_shell_tool_name, maybe_mark_iron_law_from_parts, maybe_mark_iron_law_from_tool_event,
    pre_tool_gate_decision, tool_is_iron_law_gated,
};
use pre_tool::{run_hook_pre_tool_use, IRON_LAW_LEGACY_GATE_DIR, IRON_LAW_SATISFIED_DIR};
use prompt_submit::run_hook_user_prompt_submit;
pub(crate) use prompt_submit::user_prompt_submit_context;
use session_end::{
    memory_scope_summary, prune_observations_store, prune_raw_output_store,
    prune_state_marker_stores, refresh_memory_scope_for_current_directory, run_hook_session_end,
    truncate_on_line_boundary, workspace_memory_digest, INSTINCT_DIGEST_MAX_BYTES,
    SYNTHESIS_NUDGE_MAX_BYTES,
};
pub(crate) use session_end::{
    prune_tool_timings_store, run_bridge_session_end, run_session_end_learning, sanitize_memory_key,
};
use session_start::{
    maybe_self_heal_mcp_registration, post_tool_batch_context, run_hook_lifecycle,
    subagent_start_context,
};
pub(crate) use session_start::{
    post_compact_context, render_lifecycle_payload, session_start_context,
};
pub use settings::{build_hooks_payload, remove_managed_hook_payload_for_home};
use settings::{
    is_help_argument, render_hook_help, run_hook_diagnose, run_hook_install, run_hook_instructions,
    run_hook_list, run_hook_uninstall,
};
use state::{
    claude_hook_event_names, hook_session_id, hook_str, hook_tool_name, increment_counter_file,
    read_json_stdin_fail_open, read_stdin_text, reset_counter_file, system_map_edit_counter_path,
    system_map_refresh_threshold, user_config_or_env_u64, user_config_review_strictness,
    MANAGED_PRE_TOOL_USE_EVENT, MCP_SELF_HEAL_ENV_VAR, OBSERVATION_DEFAULT_RETENTION_DAYS,
    PLUGIN_MEMORY_RETENTION_DAYS, RAW_OUTPUT_DEFAULT_RETENTION_DAYS, REVIEW_GATE_ENV_VAR,
    REVIEW_GATE_MAX_BLOCKS_ENV_VAR, SESSION_CAPTURE_ENV_VAR, TIMINGS_DEFAULT_RETENTION_DAYS,
};
pub(crate) use state::{effective_tool_name, is_edit_class_tool};

#[cfg(test)]
mod tests;
