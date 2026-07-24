//! Purpose: Register (and verify) the `keel` MCP server in the harness's
//!   user-scope config so the native installer no longer depends on the
//!   bootstrap shell script's `claude mcp add` step.
//! Caller: `manager::install::install_from_paths` (every install/update) and
//!   the `repair` command.
//! Dependencies: serde_json, crate::runtime for path + IO helpers.
//! Main Functions: register_mcp_server, mcp_config_path, mcp_server_entry.
//! Side Effects: Reads and writes `~/.claude.json`, preserving every unrelated
//!   key. Writes only when the `keel` entry is missing or stale, so a
//!   no-op install does not churn the file.
//!
//! Why `~/.claude.json` and not `~/.claude/settings.json`: the harness reads
//! user-scope MCP servers from the top-level `mcpServers` key of
//! `~/.claude.json`. `settings.json` does not support `mcpServers` at all —
//! writing it there is silently ignored (verified against
//! code.claude.com/docs/en/mcp and /settings). The bootstrap installers already
//! shell out to `claude mcp add --scope user`, which writes this same file;
//! doing it natively makes MCP survive installs/updates that never run the
//! shell script (manual release install, `keel update`, `repair`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::runtime::{display_path, installed_executable_path, read_text_if_exists, write_text};

/// MCP server key registered in the harness user config. Matches the name the
/// MCP server reports during `initialize` and the `.claude-plugin/plugin.json`
/// `mcpServers` key, so the plugin path and the native path agree.
pub const MCP_SERVER_KEY: &str = "keel";

/// Max age (seconds) of a lock file before it is considered stale (the holder
/// crashed) and reclaimed. Long enough that a normal registration (parse +
/// mutate + atomic write, well under a second) never trips it, short enough
/// that a crashed holder does not wedge subsequent registrations for long.
const CONFIG_LOCK_MAX_AGE_SECS: u64 = 30;

/// A best-effort cross-process lock for the read-modify-write of `~/.claude.json`.
///
/// `register_mcp_server` does read → parse → mutate → write without a lock, so
/// two concurrent writers (e.g. SessionStart self-heal racing a manual
/// `keel repair`) could both read the pre-mutation state and the later write
/// would clobber the earlier one's changes to unrelated keys. This lock uses an
/// O_EXCL sibling lockfile (`.claude.json.keel-lock`) for mutual exclusion.
///
/// Best-effort: a crash mid-registration leaves a stale lockfile, reclaimed
/// after `CONFIG_LOCK_MAX_AGE_SECS`. On any I/O error acquiring the lock, the
/// caller proceeds WITHOUT the lock (fail-open) rather than blocking the
/// install — the write itself is still atomic (temp+rename), so the worst case
/// is a lost update to unrelated keys under rare concurrency, not corruption.
struct ConfigLock {
    lock_path: PathBuf,
    held: bool,
}

impl ConfigLock {
    /// Acquire a lock for `config_path` (a sibling `.keel-lock` file). Returns a
    /// guard that releases on drop. If the lock is held by another process, this
    /// busy-waits briefly; if acquisition fails after the timeout, returns a
    /// non-holding guard (fail-open).
    fn acquire(config_path: &Path) -> Self {
        let lock_path = config_path.with_extension("json.keel-lock");
        // Try to reclaim a stale lock first.
        if let Ok(metadata) = fs::metadata(&lock_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() > CONFIG_LOCK_MAX_AGE_SECS {
                        let _ = fs::remove_file(&lock_path);
                    }
                }
            }
        }
        // Busy-wait up to ~2 seconds for the lock.
        let mut held = false;
        for _ in 0..40 {
            use std::io::ErrorKind;
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(_) => {
                    held = true;
                    break;
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => break, // fail-open on unexpected I/O error
            }
        }
        ConfigLock { lock_path, held }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

/// Outcome of a registration attempt, for caller-facing reporting.
#[derive(Debug, PartialEq, Eq)]
pub enum McpRegistration {
    /// The entry was missing and has been written.
    Added,
    /// The entry was present but stale (e.g. binary path changed) and updated.
    Updated,
    /// The entry already matched — nothing written.
    AlreadyCurrent,
}

/// Resolve `~/.claude.json` from the resolved harness home. harness home is
/// `<user-home>/.claude`, so its parent is the user home where `.claude.json`
/// lives. Falls back to joining `.claude.json` as a sibling of `claude_home`
/// when the parent cannot be determined (degenerate paths only).
pub fn mcp_config_path(claude_home: &Path) -> PathBuf {
    match claude_home.parent() {
        Some(home) => home.join(".claude.json"),
        None => claude_home.with_file_name(".claude.json"),
    }
}

/// Build the stdio MCP server entry pointing at the installed binary.
///
/// `alwaysLoad: true` is load-bearing for the keel contract. The harness
/// defers MCP tools behind a `ToolSearch` step by default — and that deferral is
/// *forced on* whenever tool search is enabled or `ANTHROPIC_BASE_URL` points at
/// a non-first-party gateway (a common keel setup). Deferred tools are
/// not in the model's context at session start: the model must search for them
/// before it can call them. That directly defeats the design premise that
/// `system_map`/`recall`/`run_command`/`recall_status` (and the `skill_*`,
/// `brief_*`, `memory_status`, `system_map_refresh` tools) are *always available*
/// and should be reached for instead of guessing. `alwaysLoad: true` is
/// per-server, so it pins every tool this server publishes into context upfront
/// regardless of the tool-search mode — the per-prompt "prefer the MCP tools"
/// guidance then points at tools that are actually loaded rather than ones the
/// model has to discover first. Supported by the harness v2.1.121+ (per
/// code.claude.com/docs/en/mcp); the tools are tiny, so the upfront context cost
/// is negligible.
pub fn mcp_server_entry(claude_home: &Path) -> Value {
    let executable = installed_executable_path(claude_home);
    json!({
        "type": "stdio",
        "command": display_path(&executable),
        "args": ["mcp", "serve"],
        // why: Claude Code holds stdin open for the whole session and sends no
        // keepalive frames, so the server's default 300s idle self-reap would drop
        // a *live attached* session's tools mid-session (they never return to model
        // context after the reconnect). Disable idle-reap for this registration; the
        // server still exits cleanly on stdin EOF at session end (ReaderEof). The
        // reap default stays on for hosts that may abandon the server without
        // closing stdin — the orphan case the reaper was added for.
        "env": { "KEEL_MCP_IDLE_TIMEOUT_SECS": "0" },
        "alwaysLoad": true,
    })
}

/// Ensure `~/.claude.json` contains a top-level `mcpServers.keel` entry
/// pointing at the installed binary. Preserves all other content. Returns the
/// registration outcome, or an error string on IO/parse failure.
///
/// Idempotent: re-running with an unchanged binary path is an
/// `AlreadyCurrent` no-op that does not rewrite the file.
pub fn register_mcp_server(claude_home: &Path) -> Result<McpRegistration, String> {
    let config_path = mcp_config_path(claude_home);
    // Hold a cross-process lock for the read-modify-write so two concurrent
    // keel processes (SessionStart self-heal racing a manual `keel repair`)
    // cannot both read the pre-mutation state and clobber each other's changes
    // to unrelated keys. Fail-open if the lock cannot be acquired.
    let _lock = ConfigLock::acquire(&config_path);
    let existing_text = read_text_if_exists(&config_path)?;

    // Parse the existing config, or start a fresh object. A corrupt/non-object
    // file is a hard error rather than a silent overwrite — clobbering a user's
    // ~/.claude.json (history, project state, auth) would be destructive.
    // why: a UTF-8 BOM (a common PowerShell artifact on Windows) is not ASCII
    // whitespace, so `trim` never removes it and `serde_json` hard-errors on it —
    // which would wedge registration and the SessionStart self-heal permanently.
    // The opencode/cursor merge paths already strip it; do the same here.
    let existing_trimmed = existing_text.trim_start_matches('\u{feff}');
    let mut document: Value = if existing_trimmed.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(existing_trimmed)
            .map_err(|error| format!("parse {}: {error}", display_path(&config_path)))?
    };
    let root = document.as_object_mut().ok_or_else(|| {
        format!(
            "{} is not a JSON object; refusing to overwrite",
            display_path(&config_path)
        )
    })?;

    let desired_entry = mcp_server_entry(claude_home);

    // Inspect the current entry to decide added/updated/no-op.
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers.as_object_mut().ok_or_else(|| {
        format!(
            "{}: mcpServers is not a JSON object; refusing to overwrite",
            display_path(&config_path)
        )
    })?;

    let outcome = match servers.get(MCP_SERVER_KEY) {
        Some(current) if current == &desired_entry => McpRegistration::AlreadyCurrent,
        Some(_) => McpRegistration::Updated,
        None => McpRegistration::Added,
    };

    // Drop sibling MCP servers whose command binary is missing. A dead
    // registration (e.g. leftover `claude_core` pointing at a removed
    // `claude-skills.exe`) makes Grok/Claude hang or fail MCP connect while
    // waiting on a process that can never start.
    let pruned = prune_missing_command_servers(servers);

    if outcome == McpRegistration::AlreadyCurrent && !pruned {
        return Ok(outcome);
    }

    servers.insert(MCP_SERVER_KEY.to_string(), desired_entry);

    let rendered = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("render {}: {error}", display_path(&config_path)))?;
    write_text(&config_path, &format!("{rendered}\n"))?;
    Ok(if outcome == McpRegistration::AlreadyCurrent && pruned {
        McpRegistration::Updated
    } else {
        outcome
    })
}

/// Remove *keel's own* legacy/dead sibling registrations whose `command` binary
/// is missing — e.g. a leftover `claude_core` / `claude-skills` entry from a
/// previous keel identity that now points at a removed binary and makes MCP
/// connect hang. Returns true when at least one entry was removed.
///
/// why: this is deliberately scoped to keel-family entries. An earlier version
/// pruned ANY sibling whose absolute command path was missing, which would
/// permanently delete a user's unrelated third-party MCP server the moment its
/// binary sat on an unmounted drive or was mid-rebuild — a preserve-existing-data
/// violation done silently on every SessionStart. Third-party servers are never
/// touched now. Bare PATH commands (no separator) are also left alone.
fn prune_missing_command_servers(servers: &mut Map<String, Value>) -> bool {
    let dead: Vec<String> = servers
        .iter()
        .filter(|(name, entry)| {
            if name.as_str() == MCP_SERVER_KEY {
                return false;
            }
            if !is_keel_family_server(name, entry) {
                return false;
            }
            let Some(command) = entry.get("command").and_then(Value::as_str) else {
                return false;
            };
            let command = command.trim();
            if command.is_empty() {
                return false;
            }
            // PATH-only basenames: do not prune (we cannot know PATH here).
            if !command.contains('/') && !command.contains('\\') && !command.contains(':') {
                return false;
            }
            !Path::new(command).is_file()
        })
        .map(|(name, _)| name.clone())
        .collect();
    let changed = !dead.is_empty();
    for name in dead {
        servers.remove(&name);
    }
    changed
}

/// Whether a sibling mcpServers entry belongs to a *keel* identity (past or
/// present) — matched on a known legacy server key or a keel-family command
/// binary. Only such entries are eligible for missing-binary pruning; an
/// unrelated third-party server is never removed by keel's self-heal.
fn is_keel_family_server(name: &str, entry: &Value) -> bool {
    const LEGACY_KEYS: &[&str] = &[
        "claude_core",
        "claude-core",
        "claude_skills",
        "claude-skills",
        "claudeskills",
        "claude-code-skills",
    ];
    let name_l = name.to_ascii_lowercase();
    if LEGACY_KEYS.contains(&name_l.as_str()) {
        return true;
    }
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    command.contains("claude-skills")
        || command.contains("claude_skills")
        || command.contains("claude_core")
        || command.contains("claude-core")
        || command.contains("keel.exe")
        || command.ends_with("/keel")
        || command.ends_with("\\keel")
}

/// True when `claude_home` is a real `~/.claude` directory (its final path
/// component is literally `.claude`).
///
/// Auto-writing `~/.claude.json` is only meaningful next to a standard the harness
/// home. The integration suite and several hook tests install into throwaway
/// `--claude-home` / `CLAUDE_TARGET_OVERRIDE` directories that are NOT named
/// `.claude`; gating every automatic registration site on this predicate keeps
/// real installs and the SessionStart self-heal fully automatic while leaving
/// tests hermetic. `register_mcp_server` itself stays unconditional so explicit
/// operator recovery (`repair`) and tests with a real `.claude` home can always
/// force a write.
pub fn is_standard_claude_home(claude_home: &Path) -> bool {
    claude_home
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".claude")
        .unwrap_or(false)
}

/// Idempotently re-assert the `keel` MCP registration, but only for a
/// standard `~/.claude` home. Returns `None` when skipped (non-standard home),
/// or `Some(result)` carrying the [`register_mcp_server`] outcome.
///
/// This is the self-heal entry point shared by the SessionStart hook and the
/// post-`__self-replace` step. Its whole purpose is to close the drift window:
/// `register_mcp_server` writes only when the live entry differs from the
/// desired one (it no-ops as `AlreadyCurrent` otherwise), so calling this on
/// every session is free on a healthy config and silently repairs a drifted
/// entry — e.g. one written by an older binary that predates `alwaysLoad`, or
/// left stale by a binary swap that never re-ran `install`/`update`/`repair`.
///
/// Best-effort by contract: callers must treat a returned `Err` as advisory and
/// never fail their own operation on it (MCP is additive, not load-bearing for
/// the skill pack).
pub fn self_heal_registration(claude_home: &Path) -> Option<Result<McpRegistration, String>> {
    if !is_standard_claude_home(claude_home) {
        return None;
    }
    Some(register_mcp_server(claude_home))
}

/// Outcome of an unregistration attempt, for caller-facing reporting.
#[derive(Debug, PartialEq, Eq)]
pub enum McpUnregistration {
    /// The `keel` entry was present and has been removed.
    Removed,
    /// No `keel` entry existed — nothing to do.
    NotPresent,
}

/// Remove the top-level `mcpServers.keel` entry from `~/.claude.json`,
/// the exact reverse of [`register_mcp_server`]. Preserves every unrelated key
/// and every sibling MCP server; deletes the now-empty `mcpServers` object so an
/// uninstall leaves no empty scaffolding behind. Returns the outcome, or an error
/// string on IO/parse failure.
///
/// Why this exists: `register_mcp_server` writes `keel` into `~/.claude.json`
/// on every install/update/repair. Without the matching removal, an uninstall
/// leaves a dangling MCP entry pointing at a deleted binary, which the harness
/// tries to spawn as an MCP server every session. Uninstall must reverse install.
///
/// Idempotent: a missing file, a file with no `mcpServers`, or a file with no
/// `keel` entry are all `NotPresent` no-ops that do not rewrite the file.
/// A corrupt/non-object config is a hard error rather than a silent overwrite —
/// clobbering a user's `~/.claude.json` would be destructive.
pub fn unregister_mcp_server(claude_home: &Path) -> Result<McpUnregistration, String> {
    let config_path = mcp_config_path(claude_home);
    // Same cross-process lock as register_mcp_server: the read-modify-write of
    // ~/.claude.json must not race a concurrent writer.
    let _lock = ConfigLock::acquire(&config_path);
    let existing_text = read_text_if_exists(&config_path)?;
    if existing_text.trim().is_empty() {
        return Ok(McpUnregistration::NotPresent);
    }

    let mut document: Value = serde_json::from_str(&existing_text)
        .map_err(|error| format!("parse {}: {error}", display_path(&config_path)))?;
    let root = document.as_object_mut().ok_or_else(|| {
        format!(
            "{} is not a JSON object; refusing to overwrite",
            display_path(&config_path)
        )
    })?;

    // No mcpServers object at all → nothing to remove.
    let Some(servers_value) = root.get_mut("mcpServers") else {
        return Ok(McpUnregistration::NotPresent);
    };
    let servers = servers_value.as_object_mut().ok_or_else(|| {
        format!(
            "{}: mcpServers is not a JSON object; refusing to overwrite",
            display_path(&config_path)
        )
    })?;

    if servers.remove(MCP_SERVER_KEY).is_none() {
        return Ok(McpUnregistration::NotPresent);
    }

    // Drop the now-empty mcpServers scaffolding so uninstall leaves a clean file.
    if servers.is_empty() {
        root.remove("mcpServers");
    }

    let rendered = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("render {}: {error}", display_path(&config_path)))?;
    write_text(&config_path, &format!("{rendered}\n"))?;
    Ok(McpUnregistration::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unique_home(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "keel-mcpreg-{label}-{}-{nanos}",
            std::process::id()
        ));
        // claude_home is <root>/.claude so the parent (<root>) is the synthetic
        // user home where .claude.json must land.
        let claude_home = root.join(".claude");
        fs::create_dir_all(&claude_home).expect("create claude home");
        claude_home
    }

    #[test]
    fn registers_into_fresh_config() {
        let claude_home = unique_home("fresh");
        let outcome = register_mcp_server(&claude_home).expect("register");
        assert_eq!(outcome, McpRegistration::Added);

        let config_path = mcp_config_path(&claude_home);
        let text = fs::read_to_string(&config_path).expect("read config");
        let parsed: Value = serde_json::from_str(&text).expect("valid json");
        let entry = &parsed["mcpServers"][MCP_SERVER_KEY];
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["args"], json!(["mcp", "serve"]));
        assert!(entry["command"].as_str().unwrap().contains("keel"));
        // alwaysLoad pins the server's tools into context upfront so they are not
        // deferred behind ToolSearch (the deferral that makes them "just hanging
        // around" rather than always-available). Lock it so a future edit can't
        // silently drop it and re-defer the contract tools.
        assert_eq!(entry["alwaysLoad"], json!(true));

        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn entry_disables_idle_reap_for_claude_code() {
        // Claude Code holds stdin open for the whole session, so the registered
        // entry must disable the idle self-reap or a live session loses its tools
        // after 5 idle minutes.
        let claude_home = unique_home("idle-reap");
        let entry = mcp_server_entry(&claude_home);
        assert_eq!(entry["env"]["KEEL_MCP_IDLE_TIMEOUT_SECS"], json!("0"));
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn second_run_is_a_noop() {
        let claude_home = unique_home("noop");
        assert_eq!(
            register_mcp_server(&claude_home).unwrap(),
            McpRegistration::Added
        );
        assert_eq!(
            register_mcp_server(&claude_home).unwrap(),
            McpRegistration::AlreadyCurrent
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn preserves_unrelated_keys() {
        let claude_home = unique_home("preserve");
        let config_path = mcp_config_path(&claude_home);
        // Seed a realistic ~/.claude.json with unrelated state.
        let seed = json!({
            "userID": "abc123",
            "numStartups": 42,
            "projects": { "/some/path": { "allowedTools": [] } },
            "mcpServers": {
                "other-server": { "type": "stdio", "command": "x", "args": [], "env": {} }
            }
        });
        fs::write(&config_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        let outcome = register_mcp_server(&claude_home).expect("register");
        assert_eq!(outcome, McpRegistration::Added);

        let text = fs::read_to_string(&config_path).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        // Unrelated keys survive.
        assert_eq!(parsed["userID"], "abc123");
        assert_eq!(parsed["numStartups"], 42);
        assert_eq!(parsed["projects"]["/some/path"]["allowedTools"], json!([]));
        // Sibling MCP server survives.
        assert_eq!(parsed["mcpServers"]["other-server"]["command"], "x");
        // Our entry is present.
        assert_eq!(parsed["mcpServers"][MCP_SERVER_KEY]["type"], "stdio");

        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn refuses_to_clobber_non_object_config() {
        let claude_home = unique_home("corrupt");
        let config_path = mcp_config_path(&claude_home);
        fs::write(&config_path, "[\"not an object\"]").unwrap();
        let result = register_mcp_server(&claude_home);
        assert!(result.is_err(), "must not overwrite a non-object config");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn updates_stale_entry() {
        let claude_home = unique_home("stale");
        let config_path = mcp_config_path(&claude_home);
        // Use the literal key "keel" (== MCP_SERVER_KEY) so the json!
        // seed is unambiguous; the production code reads MCP_SERVER_KEY.
        let seed = json!({
            "mcpServers": {
                "keel": { "type": "stdio", "command": "old-path", "args": [], "env": {} }
            }
        });
        assert_eq!(seed["mcpServers"][MCP_SERVER_KEY]["command"], "old-path");
        fs::write(&config_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();
        let outcome = register_mcp_server(&claude_home).expect("register");
        assert_eq!(outcome, McpRegistration::Updated);
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn unregister_reverses_register() {
        let claude_home = unique_home("unreg-roundtrip");
        assert_eq!(
            register_mcp_server(&claude_home).unwrap(),
            McpRegistration::Added
        );
        // Round-trip: the entry register wrote is the entry unregister removes.
        assert_eq!(
            unregister_mcp_server(&claude_home).unwrap(),
            McpUnregistration::Removed
        );
        let config_path = mcp_config_path(&claude_home);
        let text = fs::read_to_string(&config_path).expect("read config");
        let parsed: Value = serde_json::from_str(&text).expect("valid json");
        // The keel entry is gone and the now-empty mcpServers scaffolding
        // is dropped so uninstall leaves a clean file.
        assert!(parsed.get("mcpServers").is_none());
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn unregister_preserves_sibling_servers_and_keys() {
        let claude_home = unique_home("unreg-preserve");
        let config_path = mcp_config_path(&claude_home);
        let seed = json!({
            "userID": "abc123",
            "projects": { "/some/path": { "allowedTools": [] } },
            "mcpServers": {
                "keel": { "type": "stdio", "command": "x", "args": ["mcp", "serve"], "env": {}, "alwaysLoad": true },
                "other-server": { "type": "stdio", "command": "y", "args": [], "env": {} }
            }
        });
        fs::write(&config_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        assert_eq!(
            unregister_mcp_server(&claude_home).unwrap(),
            McpUnregistration::Removed
        );

        let text = fs::read_to_string(&config_path).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        // Unrelated keys survive.
        assert_eq!(parsed["userID"], "abc123");
        assert_eq!(parsed["projects"]["/some/path"]["allowedTools"], json!([]));
        // Sibling MCP server survives; mcpServers object is kept because it is
        // not empty.
        assert_eq!(parsed["mcpServers"]["other-server"]["command"], "y");
        // Our entry is gone.
        assert!(parsed["mcpServers"].get(MCP_SERVER_KEY).is_none());
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn unregister_is_noop_when_absent() {
        let claude_home = unique_home("unreg-absent");
        // No file at all.
        assert_eq!(
            unregister_mcp_server(&claude_home).unwrap(),
            McpUnregistration::NotPresent
        );
        // File exists but has no keel entry.
        let config_path = mcp_config_path(&claude_home);
        fs::write(
            &config_path,
            r#"{"mcpServers":{"other":{"type":"stdio","command":"z","args":[],"env":{}}}}"#,
        )
        .unwrap();
        assert_eq!(
            unregister_mcp_server(&claude_home).unwrap(),
            McpUnregistration::NotPresent
        );
        // The unrelated server is untouched.
        let text = fs::read_to_string(&config_path).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["mcpServers"]["other"]["command"], "z");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn unregister_refuses_to_clobber_non_object_config() {
        let claude_home = unique_home("unreg-corrupt");
        let config_path = mcp_config_path(&claude_home);
        fs::write(&config_path, "[\"not an object\"]").unwrap();
        assert!(
            unregister_mcp_server(&claude_home).is_err(),
            "must not overwrite a non-object config"
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn register_prunes_only_keel_legacy_dead_siblings() {
        let claude_home = unique_home("prune-dead");
        let config_path = mcp_config_path(&claude_home);
        // Seed: (a) a keel-legacy dead sibling pointing at a missing binary — must
        // be pruned; (b) a THIRD-PARTY dead sibling pointing at a missing absolute
        // path — must SURVIVE (keel never deletes a user's other MCP server); and
        // (c) a PATH-only basename that must survive (we cannot know PATH here).
        fs::write(
            &config_path,
            r#"{
              "mcpServers": {
                "claude_core": {
                  "type": "stdio",
                  "command": "C:\\nonexistent\\claude-skills.exe",
                  "args": ["mcp", "serve"],
                  "env": {}
                },
                "third-party": {
                  "type": "stdio",
                  "command": "C:\\nonexistent\\missing-mcp.exe",
                  "args": ["mcp", "serve"],
                  "env": {}
                },
                "path-tool": {
                  "type": "stdio",
                  "command": "npx",
                  "args": ["-y", "some-mcp"],
                  "env": {}
                }
              }
            }"#,
        )
        .unwrap();
        let outcome = register_mcp_server(&claude_home).unwrap();
        assert!(matches!(
            outcome,
            McpRegistration::Added | McpRegistration::Updated
        ));
        let text = fs::read_to_string(&config_path).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert!(
            parsed["mcpServers"].get("claude_core").is_none(),
            "keel-legacy missing-binary sibling must be pruned: {parsed}"
        );
        assert_eq!(
            parsed["mcpServers"]["third-party"]["command"],
            json!("C:\\nonexistent\\missing-mcp.exe"),
            "a third-party server must NEVER be deleted, even with a missing binary"
        );
        assert_eq!(
            parsed["mcpServers"]["path-tool"]["command"],
            json!("npx"),
            "PATH basenames must not be pruned"
        );
        assert!(parsed["mcpServers"].get(MCP_SERVER_KEY).is_some());
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn self_heal_repairs_drifted_entry_then_is_a_noop() {
        // The whole point of the SessionStart self-heal: an entry written by an
        // older binary that predates `alwaysLoad` (or left stale by a binary
        // swap) must be repaired to carry `alwaysLoad: true`, and a second pass
        // must no-op rather than churn the file.
        let claude_home = unique_home("selfheal-drift");
        let config_path = mcp_config_path(&claude_home);
        // Seed a DRIFTED entry: present, but missing alwaysLoad — exactly the
        // shape `claude mcp add` or a pre-alwaysLoad binary would leave behind.
        let seed = json!({
            "mcpServers": {
                "keel": { "type": "stdio", "command": "old", "args": ["mcp", "serve"], "env": {} }
            }
        });
        fs::write(&config_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        // First self-heal repairs it.
        let outcome = self_heal_registration(&claude_home).expect("standard home → Some");
        assert_eq!(outcome.expect("repair ok"), McpRegistration::Updated);
        let text = fs::read_to_string(&config_path).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed["mcpServers"][MCP_SERVER_KEY]["alwaysLoad"],
            json!(true),
            "self-heal must write alwaysLoad:true onto a drifted entry"
        );

        // Second self-heal is a no-op — the entry already matches.
        let again = self_heal_registration(&claude_home).expect("standard home → Some");
        assert_eq!(again.expect("noop ok"), McpRegistration::AlreadyCurrent);

        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn self_heal_skips_non_standard_home() {
        // Test/integration homes are throwaway dirs NOT named `.claude`. The
        // self-heal must skip them entirely (return None, write nothing) so the
        // suite never races on or pollutes a real ~/.claude.json.
        let standard = unique_home("selfheal-standard");
        // A sibling dir that is NOT `.claude`.
        let non_standard = standard.parent().unwrap().join("not-dot-claude");
        fs::create_dir_all(&non_standard).unwrap();

        assert!(is_standard_claude_home(&standard), "`.claude` is standard");
        assert!(
            !is_standard_claude_home(&non_standard),
            "a non-`.claude` dir is not standard"
        );
        assert!(
            self_heal_registration(&non_standard).is_none(),
            "self-heal must skip a non-standard home"
        );
        // And it wrote nothing beside the non-standard home.
        assert!(
            !mcp_config_path(&non_standard).exists(),
            "self-heal must not create a .claude.json beside a non-standard home"
        );

        let _ = fs::remove_dir_all(standard.parent().unwrap());
    }
}
