//! Hook lifecycle settings responsibility split.

#![allow(unused_imports)]

use super::{
    display_path, event_by_name, event_by_slug, fs, installed_executable_path, learning,
    observation, resolve_claude_home, resolve_repository_root, rewrite_command_text_for_shell,
    rewrite_shell_for_tool, tool_timings, utility, write_indented, write_text, BTreeMap, FlagSet,
    HookEvent, JsonDocument, JsonMap, Path, PathBuf, RawStore, Read, Value, Write, HOOK_EVENTS,
};

use super::{
    anvil_gate_enabled, anvil_satisfied_path, anvil_satisfied_this_session,
    anvil_workspace_marker_ms, append_compression_hint_when_forced, brief_gate_blocks_path,
    brief_gate_max_blocks, brief_gate_message, brief_gate_mode, brief_is_fresh,
    brief_written_this_session, build_session_summary, claude_hook_event_names,
    completeness_gate_blocks_path, completeness_gate_max_blocks, completeness_gate_message,
    completeness_gate_mode, completeness_marker_ms, completeness_scan_satisfies,
    compression_hint_text, count_session_tool_timing_rows, cue_used_as_verb, decide_gate,
    default_max_blocks_for, emit_gate_decision, emit_post_tool_batch_advisory,
    emit_post_tool_batch_block, emit_post_tool_batch_nudge, emit_pretool_deny,
    evaluate_learned_skill_gate, file_mtime_ms, gate_mode, gate_mode_value, gate_status_rows,
    hook_session_id, hook_str, hook_tool_name, increment_counter_file, iron_law_gate_decision,
    iron_law_gate_mode, iron_law_legacy_path, iron_law_marker_present, iron_law_satisfied_path,
    is_edit_class_tool, is_host_research_tool_name, is_host_shell_tool_name,
    is_keel_research_command, is_keel_research_tool_name, is_shell_tool_name,
    is_web_research_tool_name, learned_skill_gate_blocks_path, learned_skill_gate_max_blocks,
    learned_skill_gate_message, learned_skill_gate_mode, lifecycle_additional_context,
    mark_anvil_satisfied, mark_iron_law_satisfied, maybe_capture_session_summary,
    maybe_capture_session_summary_with_id, maybe_compression_hint, maybe_mark_iron_law_from_parts,
    maybe_mark_iron_law_from_tool_event, maybe_self_heal_mcp_registration,
    mcp_tool_pointer_for_prompt, memory_gate_blocks_path, memory_gate_max_blocks,
    memory_gate_message, memory_gate_mode, memory_scope_summary,
    memory_system_map_path_for_workspace, memory_written_this_session, newest_brief_mtime_ms,
    newest_file_mtime_in_dir, newest_memory_write_ms, now_ms, post_compact_context,
    post_tool_batch_context, pre_compact_context, prune_dir_files_older_than,
    prune_observations_store, prune_raw_output_store, prune_state_marker_stores,
    prune_tool_timings_store, read_counter_value, read_stdin_text, record_anvil_gate_clear,
    record_completeness_gate_clear_for, record_review_gate_clear,
    refresh_memory_scope_for_current_directory, render_lifecycle_payload,
    research_gate_blocks_path, research_gate_max_blocks, research_gate_message, research_gate_mode,
    reset_counter_file, review_gate_blocks_path, review_gate_max_blocks, review_gate_message,
    review_gate_mode, review_marker_ms, run_bridge_session_end, run_hook_command,
    run_hook_cwd_changed, run_hook_git_hooks, run_hook_lifecycle, run_hook_notification,
    run_hook_permission_denied, run_hook_permission_request, run_hook_post_tool_batch,
    run_hook_post_tool_use, run_hook_post_tool_use_failure, run_hook_pre_tool_use,
    run_hook_session_end, run_hook_subagent_start, run_hook_user_prompt_submit, run_iron_law_gate,
    run_post_tool_comment_lint, run_post_tool_graph_context, run_session_end_learning,
    sanitize_memory_key, session_edit_stats, session_has_iron_law_evidence,
    session_has_research_tool, session_start_context, session_start_ms, set_core_hooks_path,
    should_refresh_system_map, skill_pointer_fallback, skill_pointer_text, subagent_start_context,
    system_map_edit_counter_path, system_map_refresh_threshold, today_date_string,
    tool_input_command, tool_is_anvil_surface, tool_is_iron_law_gated, tool_satisfies_iron_law,
    truncate_on_line_boundary, user_config_or_env_u64, user_config_review_strictness,
    user_prompt_submit_context, user_prompt_submit_core, work_intent_pointer_for_prompt,
    workspace_memory_digest, GateDecision, GateMode, GateStatusRow, IronLawGateMode,
    SessionEditStats, SessionSummary, ANVIL_GATE_DENIAL, ANVIL_SATISFIED_DIR, BRIEF_GATE_ENV_VAR,
    BRIEF_GATE_MAX_BLOCKS_ENV_VAR, BRIEF_GATE_SESSION_GRACE_MS, COMPACT_BOOTSTRAP,
    COMPLETENESS_GATE_ENV_VAR, COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR,
    COMPRESSION_HINT_DEFAULT_THRESHOLD, DIGEST_BRIEF_MAX_BYTES, DIGEST_MAP_HEAD_MAX_BYTES,
    DIGEST_MEMORY_MAX_BYTES, GATE_DEFAULT_MAX_BLOCKS, INSTINCT_DIGEST_MAX_BYTES,
    IRON_LAW_GATE_DENIAL_BALANCED, IRON_LAW_GATE_DENIAL_STRICT, IRON_LAW_GATE_DENIAL_VERIFIED,
    IRON_LAW_GATE_ENV_VAR, IRON_LAW_LEGACY_GATE_DIR, IRON_LAW_SATISFIED_DIR,
    LEARNED_SKILL_GATE_ENV_VAR, LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR, MANAGED_PRE_TOOL_USE_EVENT,
    MCP_SELF_HEAL_ENV_VAR, MEMORY_GATE_ENV_VAR, MEMORY_GATE_MAX_BLOCKS_ENV_VAR,
    NOTIFICATION_BELL_OUTPUT, OBSERVATION_DEFAULT_RETENTION_DAYS, PLUGIN_MEMORY_RETENTION_DAYS,
    PLUGIN_REVIEW_STRICTNESS, PLUGIN_SYSTEM_MAP_REFRESH_INTERVAL,
    RAW_OUTPUT_DEFAULT_RETENTION_DAYS, RESEARCH_GATE_ENV_VAR, RESEARCH_GATE_MAX_BLOCKS_ENV_VAR,
    REVIEW_GATE_ENV_VAR, REVIEW_GATE_MAX_BLOCKS_ENV_VAR, SESSION_CAPTURE_ENV_VAR,
    SYNTHESIS_NUDGE_MAX_BYTES, SYSTEM_MAP_REFRESH_DEFAULT_THRESHOLD,
    TIMINGS_DEFAULT_RETENTION_DAYS, USER_PROMPT_DIGEST_MAX_BYTES, USER_PROMPT_ENFORCEMENT_STRIP,
    WORKSPACE_DIGEST_MAX_BYTES, WORK_INTENT_REMINDER,
};

pub(super) fn run_hook_install(
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

    let hook_path = crate::runtime::claude_engagement_home(&claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);

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

/// Rewrite a git config so `[core]` carries exactly one `hooksPath = <value>`.
///
/// The earlier implementation deleted any line containing the literal string
/// `core.hooksPath` (a form that never appears inside a config file) and then
/// appended `hooksPath` at the end of the file — so the key landed inside
/// whatever section happened to be last (e.g. a `[branch "..."]` block) and
/// accumulated one stray line per run. This version parses section headers so
/// the key is removed from every section it appears in (git only honors it
/// under `[core]`, and the appended duplicates this command itself wrote are
/// the primary source of strays), then re-inserted exactly once under
/// `[core]`. Idempotent: a config already carrying the desired value comes
/// back unchanged apart from the line endings the join normalizes.
pub(super) fn run_hook_uninstall(
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

    let hook_path = crate::runtime::claude_engagement_home(&claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);

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

pub(super) fn run_hook_list(
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

    let hook_path = crate::runtime::claude_engagement_home(&claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);

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
pub(super) fn is_secret_key(key: &str) -> bool {
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
pub(super) fn mask_secret_value(value: &str) -> String {
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
pub(super) fn redact_secrets_in_settings(raw: &str) -> String {
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
pub(super) fn redact_secrets_in_value(value: &mut JsonDocument, parent_key_is_secret: bool) {
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

pub(super) fn run_hook_instructions(
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

pub(super) fn render_hook_help(standard_output: &mut dyn Write) {
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
        "git-hooks",
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

pub(super) fn run_hook_diagnose(
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

pub(super) struct HookDiagnostics {
    claude_home: PathBuf,
    installed_executable: PathBuf,
    installed_executable_present: bool,
    settings_path: PathBuf,
    settings_present: bool,
    pub(crate) settings_parses: Option<bool>,
    managed_hook_command: Option<String>,
    pub(crate) settings_points_at_installed: Option<bool>,
    pub(crate) orphan_executable_siblings: Vec<PathBuf>,
}

impl HookDiagnostics {
    pub(crate) fn healthy(&self) -> bool {
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

    pub(crate) fn render_text(&self, output: &mut dyn Write) {
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

pub(super) fn collect_hook_diagnostics(claude_home: &Path) -> HookDiagnostics {
    let installed_executable = installed_executable_path(claude_home);
    let installed_executable_present = installed_executable.is_file();
    let settings_path = crate::runtime::claude_engagement_home(claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);
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

pub(super) fn settings_points_at_installed_executable(
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

pub(super) fn is_help_argument(argument: &str) -> bool {
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

pub(super) fn resolve_current_executable() -> Result<PathBuf, String> {
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

pub(super) fn ensure_skill_listing_budget_fraction(
    document: &mut JsonDocument,
) -> Result<(), String> {
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
    let hook_path = crate::runtime::claude_engagement_home(claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);
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

pub(super) fn ensure_hooks_object(document: &mut JsonDocument) -> Result<(), String> {
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

pub(super) fn append_managed_hooks(
    document: &mut JsonDocument,
    executable: &Path,
) -> Result<(), String> {
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

        let mut hook_def = serde_json::json!({
            "type": "command",
            "command": entry.command,
            "args": entry.args,
            "statusMessage": event.status
        });
        // why: PostToolUse/PostToolUseFailure record timings + observations and must
        // not add keel-spawn latency to every tool call — run them async, matching
        // the plugin hooks.json and the CLAUDE.md contract ("runs async").
        if matches!(event.name, "PostToolUse" | "PostToolUseFailure") {
            hook_def["async"] = serde_json::json!(true);
        }

        event_array.push(serde_json::json!({
            "matcher": event.matcher,
            "hooks": [hook_def]
        }));
    }

    sort_hook_events(hooks);

    Ok(())
}

pub(super) fn sort_hook_events(hooks: &mut JsonMap<String, JsonDocument>) {
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

pub(super) fn is_managed_args_form(command_entry: &JsonDocument) -> bool {
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
pub(super) fn command_path_is_managed_executable(command: &str) -> bool {
    let basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase());

    matches!(basename.as_deref(), Some("keel") | Some("keel.exe"))
}

pub fn is_managed_hook_command(command: &str) -> bool {
    is_managed_hook_command_with_depth(command, 0)
}

pub(super) fn is_managed_hook_command_with_depth(command: &str, depth: usize) -> bool {
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

pub(super) fn base64_decode(value: &str) -> Option<Vec<u8>> {
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

pub(super) fn decode_powershell_encoded_command(command: &str) -> Option<String> {
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
