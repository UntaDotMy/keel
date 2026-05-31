//! Purpose: Register (and verify) the `claude_core` MCP server in Claude Code's
//!   user-scope config so the native installer no longer depends on the
//!   bootstrap shell script's `claude mcp add` step.
//! Caller: `manager::install::install_from_paths` (every install/update) and
//!   the `repair` command.
//! Dependencies: serde_json, crate::runtime for path + IO helpers.
//! Main Functions: register_mcp_server, mcp_config_path, mcp_server_entry.
//! Side Effects: Reads and writes `~/.claude.json`, preserving every unrelated
//!   key. Writes only when the `claude_core` entry is missing or stale, so a
//!   no-op install does not churn the file.
//!
//! Why `~/.claude.json` and not `~/.claude/settings.json`: Claude Code reads
//! user-scope MCP servers from the top-level `mcpServers` key of
//! `~/.claude.json`. `settings.json` does not support `mcpServers` at all —
//! writing it there is silently ignored (verified against
//! code.claude.com/docs/en/mcp and /settings). The bootstrap installers already
//! shell out to `claude mcp add --scope user`, which writes this same file;
//! doing it natively makes MCP survive installs/updates that never run the
//! shell script (manual release install, `claude-skills update`, `repair`).

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::runtime::{display_path, installed_executable_path, read_text_if_exists, write_text};

/// MCP server key registered in Claude Code user config. Matches the name the
/// MCP server reports during `initialize` and the `.claude-plugin/plugin.json`
/// `mcpServers` key, so the plugin path and the native path agree.
pub const MCP_SERVER_KEY: &str = "claude_core";

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

/// Resolve `~/.claude.json` from the resolved Claude home. Claude home is
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
pub fn mcp_server_entry(claude_home: &Path) -> Value {
    let executable = installed_executable_path(claude_home);
    json!({
        "type": "stdio",
        "command": display_path(&executable),
        "args": ["mcp", "serve"],
        "env": {},
    })
}

/// Ensure `~/.claude.json` contains a top-level `mcpServers.claude_core` entry
/// pointing at the installed binary. Preserves all other content. Returns the
/// registration outcome, or an error string on IO/parse failure.
///
/// Idempotent: re-running with an unchanged binary path is an
/// `AlreadyCurrent` no-op that does not rewrite the file.
pub fn register_mcp_server(claude_home: &Path) -> Result<McpRegistration, String> {
    let config_path = mcp_config_path(claude_home);
    let existing_text = read_text_if_exists(&config_path)?;

    // Parse the existing config, or start a fresh object. A corrupt/non-object
    // file is a hard error rather than a silent overwrite — clobbering a user's
    // ~/.claude.json (history, project state, auth) would be destructive.
    let mut document: Value = if existing_text.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing_text)
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

    if outcome == McpRegistration::AlreadyCurrent {
        return Ok(outcome);
    }

    servers.insert(MCP_SERVER_KEY.to_string(), desired_entry);

    let rendered = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("render {}: {error}", display_path(&config_path)))?;
    write_text(&config_path, &format!("{rendered}\n"))?;
    Ok(outcome)
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
            "claude-skills-mcpreg-{label}-{}-{nanos}",
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
        assert!(entry["command"].as_str().unwrap().contains("claude-skills"));

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
        // Use the literal key "claude_core" (== MCP_SERVER_KEY) so the json!
        // seed is unambiguous; the production code reads MCP_SERVER_KEY.
        let seed = json!({
            "mcpServers": {
                "claude_core": { "type": "stdio", "command": "old-path", "args": [], "env": {} }
            }
        });
        assert_eq!(seed["mcpServers"][MCP_SERVER_KEY]["command"], "old-path");
        fs::write(&config_path, serde_json::to_string_pretty(&seed).unwrap()).unwrap();
        let outcome = register_mcp_server(&claude_home).expect("register");
        assert_eq!(outcome, McpRegistration::Updated);
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }
}
