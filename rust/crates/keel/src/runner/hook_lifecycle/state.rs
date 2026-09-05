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
        Ok(_) if !text.trim().is_empty() => {
            serde_json::from_str(&text).ok().map(normalize_hook_input)
        }
        _ => None,
    }
}

/// Add Claude-compatible aliases for the Grok fields consumed by lifecycle state.
/// Existing snake_case values win. Deliberately avoid copying `toolResult` and
/// other potentially large fields that no current consumer reads.
pub(super) fn normalize_hook_input(mut input: JsonDocument) -> JsonDocument {
    let Some(object) = input.as_object_mut() else {
        return input;
    };

    for (snake_case, camel_case) in [
        ("session_id", "sessionId"),
        ("tool_name", "toolName"),
        ("tool_input", "toolInput"),
        ("duration_ms", "durationMs"),
    ] {
        if !object.contains_key(snake_case) {
            if let Some(value) = object.get(camel_case).cloned() {
                object.insert(snake_case.to_string(), value);
            }
        }
    }

    input
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
            // Google Antigravity edit-class tool names.
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
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
    let tool_name = hook_str(input, &["tool_name", "toolName"]);
    let tool_input = input
        .get("tool_input")
        .or_else(|| input.get("toolInput"))
        .or_else(|| input.get("input"));
    let path = tool_input
        .and_then(|value| {
            value
                .get("path")
                .or_else(|| value.get("file_path"))
                .or_else(|| value.get("filePath"))
        })
        .and_then(JsonDocument::as_str)
        .unwrap_or("");
    effective_tool_name(tool_name, path)
}

pub(crate) fn effective_tool_name<'a>(tool_name: &'a str, path: &'a str) -> &'a str {
    if tool_name.eq_ignore_ascii_case("write") {
        if let Some(device) = path.strip_prefix("xd://") {
            if device.starts_with("mcp__keel_")
                && device
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return device;
            }
        }
    }
    tool_name
}

pub(super) fn hook_session_id(input: &JsonDocument) -> &str {
    let value = hook_str(input, &["session_id", "sessionId"]);
    if value.is_empty() {
        // why: hook handlers are short-lived processes, so a per-process random fallback
        // would mint a new key per call; id-less callers intentionally share "default".
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
    let workspace_key = crate::utility::system_map::workspace_key(&display_path(&workspace_root));

    Some(
        claude_home
            .join("state")
            .join("system-map-edit-counter")
            .join(workspace_key),
    )
}

const COUNTER_LOCK_ATTEMPTS: usize = 200;
const COUNTER_LOCK_RETRY_MS: u64 = 5;
const COUNTER_LOCK_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// A small inter-process lock for the read-modify-write counter files.
///
/// Hook handlers are separate short-lived processes, so an in-process mutex
/// cannot protect the counter. create_new gives us an atomic claim on every
/// supported host. The bounded retry and stale-lock cleanup keep a killed hook
/// from wedging all future gate updates.
struct CounterFileLock {
    path: PathBuf,
}

impl Drop for CounterFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn counter_lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("counter"));
    path.with_file_name(format!("{name}.lock"))
}

fn remove_stale_counter_lock(path: &Path) {
    if let Ok(owner) = fs::read_to_string(path.join("owner")) {
        if let Ok(process_id) = owner.trim().parse::<u32>() {
            match crate::runtime::process_is_alive(process_id) {
                Some(true) => return,
                Some(false) => {
                    let _ = fs::remove_dir_all(path);
                    return;
                }
                None => {}
            }
        }
    }
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let Ok(age) = modified.elapsed() else {
        return;
    };
    if age >= COUNTER_LOCK_STALE_AFTER {
        if metadata.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path); // stale-lock recovery cleanup is best effort
        }
    }
}

fn acquire_counter_lock(path: &Path) -> std::io::Result<CounterFileLock> {
    let lock_path = counter_lock_path(path);
    for attempt in 0..COUNTER_LOCK_ATTEMPTS {
        match fs::create_dir(&lock_path) {
            Ok(()) => {
                if let Err(error) =
                    fs::write(lock_path.join("owner"), std::process::id().to_string())
                {
                    let _ = fs::remove_dir_all(&lock_path);
                    return Err(error);
                }
                return Ok(CounterFileLock { path: lock_path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                remove_stale_counter_lock(&lock_path);
                if attempt + 1 < COUNTER_LOCK_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(COUNTER_LOCK_RETRY_MS));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                remove_stale_counter_lock(&lock_path);
                if attempt + 1 < COUNTER_LOCK_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(COUNTER_LOCK_RETRY_MS));
                }
            }
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!("counter lock remained held: {}", display_path(&lock_path)),
    ))
}

fn read_counter_value_locked(path: &Path) -> std::io::Result<u64> {
    match fs::read_to_string(path) {
        Ok(text) => text.trim().parse::<u64>().map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid counter {}: {error}", display_path(path)),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

pub(super) fn increment_counter_file(path: &Path) -> std::io::Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = acquire_counter_lock(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("lock {}: {error}", display_path(path)),
        )
    })?;
    let current = read_counter_value_locked(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("read {}: {error}", display_path(path)),
        )
    })?;
    let next = current.saturating_add(1);
    let next_str = next.to_string();

    let mut write_err = None;
    for attempt in 0..5 {
        match crate::runtime::write_text(path, &next_str) {
            Ok(()) => return Ok(next),
            Err(error) => {
                write_err = Some(format!("write {}: {error}", display_path(path)));
                std::thread::sleep(std::time::Duration::from_millis(2 * (attempt + 1)));
            }
        }
    }

    Err(std::io::Error::other(
        write_err.expect("counter write retries always record an error"),
    ))
}

pub(super) fn reset_counter_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = acquire_counter_lock(path)?;
    crate::runtime::write_text(path, "0").map_err(std::io::Error::other)
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
