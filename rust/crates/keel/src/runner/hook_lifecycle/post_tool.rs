//! Hook lifecycle post_tool responsibility split.

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
    run_hook_diagnose, run_hook_git_hooks, run_hook_install, run_hook_instructions,
    run_hook_lifecycle, run_hook_list, run_hook_notification, run_hook_permission_denied,
    run_hook_permission_request, run_hook_post_tool_batch, run_hook_pre_tool_use,
    run_hook_session_end, run_hook_subagent_start, run_hook_uninstall, run_hook_user_prompt_submit,
    run_iron_law_gate, run_session_end_learning, sanitize_memory_key, session_edit_stats,
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
    MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR, NOTIFICATION_BELL_OUTPUT,
    OBSERVATION_DEFAULT_RETENTION_DAYS, PLUGIN_MEMORY_RETENTION_DAYS, PLUGIN_REVIEW_STRICTNESS,
    PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL, RAW_OUTPUT_DEFAULT_RETENTION_DAYS, RESEARCH_GATE_ENV_VAR,
    RESEARCH_GATE_MAX_BLOCKS_ENV_VAR, REVIEW_GATE_ENV_VAR, REVIEW_GATE_MAX_BLOCKS_ENV_VAR,
    SESSION_CAPTURE_ENV_VAR, SYNTHESIS_NUDGE_MAX_BYTES, SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD,
    TIMINGS_DEFAULT_RETENTION_DAYS, USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP,
    WORKSPACE_DIGEST_MAX_BYTES, WORK_INTENT_REMINDER,
};

pub(super) fn run_hook_post_tool_use(standard_error: &mut dyn Write) -> u8 {
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
    match observation::record_observation(&input) {
        Ok(true) => {
            if let Ok(claude_home) = resolve_claude_home("") {
                learning::run_continuous_learning_if_due(&claude_home, standard_error);
            }
        }
        Ok(false) => {}
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use: observation record failed: {error}"
            );
        }
    }

    // Iron Law evidence: mark session satisfied when a keel research tool
    // (or balanced-mode host research tool) completes successfully.
    maybe_mark_iron_law_from_tool_event(&input);

    if !is_edit_class_tool(tool_name) {
        return 0;
    }

    // Comment-style lint: catch long/chatty comments at write time, not just review.
    // Advisory only (env-gated, fail-open). See run_post_tool_comment_lint.
    if let Some(nudge) = run_post_tool_comment_lint(tool_name, &input) {
        let _ = writeln!(standard_error, "{nudge}");
        let payload = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": nudge,
            },
            "suppressOutput": true,
        });
        if let Ok(rendered) = serde_json::to_string(&payload) {
            let _ = writeln!(std::io::stdout(), "{rendered}");
        }
    }

    // Graph context: after an edit, surface the blast radius (which files import
    // the edited file) so the next action is scoped by real edges, not grep.
    if let Some(context) = run_post_tool_graph_context(tool_name, &input) {
        let payload = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": context,
            },
            "suppressOutput": true,
        });
        if let Ok(rendered) = serde_json::to_string(&payload) {
            let _ = writeln!(std::io::stdout(), "{rendered}");
        }
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

/// Advisory comment-style lint for PostToolUse. Returns a nudge string when the
/// just-edited file introduced a blocking comment finding (over-length impl
/// comment, em/en dash, chatty/first-person wording), `None` otherwise.
///
/// Design constraints (it runs on every Edit/Write, a hot path):
/// - **Env-gated**: `CLAUDE_SKILLS_COMMENT_LINT_GATE=off` disables; anything
///   else (incl. unset) leaves it advisory-on. Matches the gate-mode convention.
/// - **Fail-open**: any error (no git repo, no cwd, parse failure) → `None`.
///   A comment lint must never break the PostToolUse hook.
/// - **Natural dedup**: the nudge stops firing once the comment is fixed
///   (findings clear from the working diff), so a repeated nudge means the
///   comment is still wrong, not spam.
/// - **Scoped to the edited file**: scans the working diff, filters findings to
///   the file just written so unrelated pre-existing comments are not flagged.
pub(super) fn run_post_tool_comment_lint(tool_name: &str, input: &JsonDocument) -> Option<String> {
    if std::env::var("CLAUDE_SKILLS_COMMENT_LINT_GATE").as_deref() == Ok("off") {
        return None;
    }
    // Only Edit/Write carry a file path we can scope to. Other edit-class tools
    // (apply_patch, str_replace) have no single file, so skip them to avoid noise.
    let edited_path = if matches!(tool_name, "Edit" | "Write" | "MultiEdit") {
        input
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
    } else {
        return None;
    };
    if edited_path.is_empty() {
        return None;
    }
    let repo_root = std::env::current_dir().ok()?;
    let findings = crate::comment_lint::lint_working_comments(&repo_root);
    if findings.is_empty() {
        return None;
    }
    // Scope to the file just edited (path may be absolute or repo-relative).
    let target = std::path::Path::new(edited_path);
    let target_str = target.to_string_lossy();
    let scoped: Vec<&crate::comment_lint::FileCommentFinding> = findings
        .iter()
        .filter(|f| target_str.ends_with(f.file.as_str()) || f.file.ends_with(target_str.as_ref()))
        .collect();
    if scoped.is_empty() {
        return None;
    }
    let blocking = crate::comment_lint::has_blocking(&findings);
    if !blocking {
        return None;
    }
    let rendered = crate::comment_lint::format_findings(
        &scoped.iter().map(|f| (*f).clone()).collect::<Vec<_>>(),
    );
    Some(format!(
        "keel comment-lint: blocking comment finding(s) in this edit — fix before moving on:\n{rendered}\nAdvisory; set CLAUDE_SKILLS_COMMENT_LINT_GATE=off to silence."
    ))
}

/// Graph context for PostToolUse: after an edit, report the edited file's blast
/// radius (the in-repo files that import it) so the agent's next step is scoped
/// by real dependency edges instead of a grep loop.
///
/// Design constraints (runs on every edit, a hot path):
/// - Env-gated: `CLAUDE_SKILLS_GRAPH_CONTEXT_GATE=off` disables; on by default.
/// - Fail-open: any error (no graph artifact, unreadable JSON, no cwd) -> `None`.
///   A context nudge must never break the PostToolUse hook.
/// - Cheap: reads the cached per-workspace code-graph artifact; it never builds
///   the graph here (building walks the whole tree, too slow for a hot path). If
///   no artifact exists yet, the nudge says how to build it once.
/// - Bounded: caps the dependent list so a wide blast radius cannot flood context.
pub(super) fn run_post_tool_graph_context(tool_name: &str, input: &JsonDocument) -> Option<String> {
    if std::env::var("CLAUDE_SKILLS_GRAPH_CONTEXT_GATE").as_deref() == Ok("off") {
        return None;
    }
    // Only Edit/Write/MultiEdit carry a single file path.
    let edited_path = if matches!(tool_name, "Edit" | "Write" | "MultiEdit") {
        input
            .get("tool_input")
            .and_then(|ti| ti.get("file_path"))
            .and_then(JsonDocument::as_str)
            .unwrap_or_default()
    } else {
        return None;
    };
    if edited_path.is_empty() {
        return None;
    }
    let repo_root = std::env::current_dir().ok()?;
    let artifact = crate::utility::code_graph::cached_artifact_path(&repo_root, "")?;
    // Staleness guard: a graph older than the file just edited is stale for this
    // edit and would mislead, so stay silent rather than inject wrong dependents.
    let artifact_mtime = file_mtime_ms(&artifact)?;
    let edited_mtime = file_mtime_ms(std::path::Path::new(edited_path))?;
    if artifact_mtime < edited_mtime {
        return None;
    }
    let graph = crate::utility::code_graph::CodeGraph::from_json_file(&artifact)?;
    // Normalize the edited path to the graph's workspace-relative forward-slash id.
    let relative = std::path::Path::new(edited_path)
        .strip_prefix(&repo_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| edited_path.replace('\\', "/"));
    let impacted = graph.impact_of(std::slice::from_ref(&relative));
    if impacted.is_empty() {
        return None;
    }
    const MAX_LISTED: usize = 8;
    let listed: Vec<&str> = impacted
        .iter()
        .take(MAX_LISTED)
        .map(String::as_str)
        .collect();
    let more = impacted.len().saturating_sub(MAX_LISTED);
    let mut line = format!(
        "keel graph: `{relative}` is imported by {} file(s): {}",
        impacted.len(),
        listed.join(", ")
    );
    if more > 0 {
        line.push_str(&format!(" (+{more} more)"));
    }
    line.push_str(". Verify these still compile/behave before closeout.");
    Some(line)
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
pub(super) fn run_hook_post_tool_use_failure(standard_error: &mut dyn Write) -> u8 {
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
    match observation::record_failure_observation(&input) {
        Ok(true) => {
            if let Ok(claude_home) = resolve_claude_home("") {
                learning::run_continuous_learning_if_due(&claude_home, standard_error);
            }
        }
        Ok(false) => {}
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "keel post-tool-use-failure: observation record failed: {error}"
            );
        }
    }

    0
}
