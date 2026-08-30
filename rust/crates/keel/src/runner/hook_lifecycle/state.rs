//! Hook lifecycle state responsibility split.

use super::*;

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
