// Installer MCP JSON helpers.
use crate::runtime::write_text;
use std::path::Path;
/// Rewrite the Codex MCP `keel` server's `command` to the absolute binary
/// path. Handles both the wrapped `{"mcp_servers": {"keel": {...}}}` shape
/// that keel ships and a direct `{"keel": {...}}` shape. Returns true when the
/// document was mutated (and should be persisted), false when the command was
pub(crate) fn rewrite_codex_mcp_command(doc: &mut serde_json::Value, absolute: &str) -> bool {
    // Prefer the wrapped {"mcp_servers": {"keel": {...}}} shape keel ships;
    // fall back to a direct {"keel": {...}} object for robustness.
    let servers = doc.get_mut("mcp_servers").and_then(|v| v.as_object_mut());
    let servers = match servers {
        Some(s) => s,
        None => match doc.as_object_mut() {
            Some(s) => s,
            None => return false,
        },
    };
    let Some(server) = servers.get_mut("keel").and_then(|v| v.as_object_mut()) else {
        return false;
    };
    let current = server.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if current == absolute {
        return false;
    }
    server["command"] = serde_json::Value::String(absolute.to_string());
    true
}

/// Rewrite a single MCP server entry's `command` to the absolute binary path.
/// Codex's merged document goes through [`rewrite_codex_mcp_command`]; the
/// Cursor and Pi wiring extract the `keel` entry value from the shipped
/// template first, so they rewrite the entry itself before merging. A bare
/// `keel` command fails on PATH-less hosts (the common case on Windows, where
/// install does not modify PATH), so the resolved installed path must land in
pub(crate) fn rewrite_mcp_entry_command(entry: &mut serde_json::Value, absolute: &str) -> bool {
    let Some(server) = entry.as_object_mut() else {
        return false;
    };
    let current = server.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if current == absolute {
        return false;
    }
    server["command"] = serde_json::Value::String(absolute.to_string());
    true
}

#[derive(Debug)]
pub(crate) enum JsonMcpMergeResult {
    Added,
    AlreadyCurrent,
    Updated,
}

/// Merge one MCP server entry into a host JSON container.
///
/// The standard hosts use `mcpServers`; OpenCode uses `mcp`. Pi may provide
/// top-level template defaults, which seed only absent user keys.
pub(crate) fn merge_json_mcp(
    config_path: &Path,
    container_key: &str,
    server_key: &str,
    entry: &serde_json::Value,
    template_defaults: Option<&serde_json::Value>,
) -> Result<JsonMcpMergeResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if stripped.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(stripped).map_err(|error| format!("parse error: {error}"))?
    };
    if let Some(defaults) = template_defaults.and_then(serde_json::Value::as_object) {
        for (key, value) in defaults {
            if document.get(key).is_none() {
                document[key.clone()] = value.clone();
            }
        }
    }
    if document.get(container_key).is_none() {
        document[container_key] = serde_json::json!({});
    }
    let current = document[container_key]
        .as_object_mut()
        .ok_or_else(|| format!("{container_key} is not an object"))?;
    if let Some(existing) = current.get(server_key) {
        if existing == entry {
            return Ok(JsonMcpMergeResult::AlreadyCurrent);
        }
        current.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(JsonMcpMergeResult::Updated);
    }
    current.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(JsonMcpMergeResult::Added)
}

/// Remove one MCP entry while preserving sibling data and malformed documents.
pub(crate) fn remove_json_mcp_entry(path: &Path, container_key: &str) -> usize {
    if !path.is_file() {
        return 0;
    }
    let Ok(text) = crate::runtime::read_text_if_exists(path) else {
        return 0;
    };
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(mut document) = serde_json::from_str::<serde_json::Value>(stripped) else {
        return 0;
    };
    let Some(container) = document
        .get_mut(container_key)
        .and_then(serde_json::Value::as_object_mut)
    else {
        return 0;
    };
    if container.remove("keel").is_none() {
        return 0;
    }
    if container.is_empty() {
        if let Some(object) = document.as_object_mut() {
            object.remove(container_key);
        }
    }
    let Ok(rendered) = serde_json::to_string_pretty(&document) else {
        return 0;
    };
    if write_text(path, &rendered).is_ok() {
        1
    } else {
        0
    }
}
