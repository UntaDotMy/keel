// Codex installer wiring.
use super::*;
use crate::runtime::{display_path, write_text};
use std::path::Path;
/// Result of merging the keel entry into the personal Codex marketplace.
#[derive(Debug)]
pub(crate) enum CodexMarketplaceResult {
    Added,
    AlreadyCurrent,
    Updated,
}

/// Marketplace name for the personal keel catalog. Codex keys enabled plugins
/// as `<plugin>@<marketplace>` in config.toml, so this constant is part of the
/// enablement key and must stay stable across installs.
pub(crate) const CODEX_PERSONAL_MARKETPLACE_NAME: &str = "personal-keel";

/// The marketplace entry that makes ~/.codex/plugins/keel discoverable. The
/// shape (source/policy/category) follows the Codex marketplace schema; the
/// `~` in `path` is expanded by Codex itself.
pub(crate) fn codex_marketplace_entry() -> serde_json::Value {
    serde_json::json!({
        "name": "keel",
        "source": { "source": "local", "path": "~/.codex/plugins/keel" },
        "policy": {
            "installation": "AVAILABLE",
            "authentication": "ON_INSTALL"
        },
        "category": "Productivity"
    })
}

/// Merge the keel entry into the personal Codex marketplace manifest
/// (`~/.agents/plugins/marketplace.json`). Codex discovers plugins only via a
/// marketplace; without this entry the copied plugin bundle never loads.
/// Preserves sibling plugin entries and any user-authored top-level metadata;
/// creates a fresh `personal-keel` catalog when the manifest is absent.
pub(crate) fn merge_codex_marketplace(
    marketplace_path: &Path,
) -> Result<CodexMarketplaceResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(marketplace_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if stripped.trim().is_empty() {
        serde_json::json!({
            "name": CODEX_PERSONAL_MARKETPLACE_NAME,
            "interface": { "displayName": "keel" }
        })
    } else {
        serde_json::from_str(stripped).map_err(|error| format!("parse error: {error}"))?
    };
    if document.get("plugins").is_none() {
        document["plugins"] = serde_json::json!([]);
    }
    let plugins = document["plugins"]
        .as_array_mut()
        .ok_or("plugins is not an array")?;
    let desired = codex_marketplace_entry();
    let had_keel = plugins
        .iter()
        .any(|entry| entry.get("name").and_then(|n| n.as_str()) == Some("keel"));
    if let Some(existing) = plugins
        .iter_mut()
        .find(|entry| entry.get("name").and_then(|n| n.as_str()) == Some("keel"))
    {
        if *existing == desired {
            return Ok(CodexMarketplaceResult::AlreadyCurrent);
        }
        *existing = desired;
    } else {
        plugins.push(desired);
    }
    if let Some(parent) = marketplace_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    let pretty = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("serialize error: {error}"))?;
    write_text(marketplace_path, &pretty)?;
    Ok(if had_keel {
        CodexMarketplaceResult::Updated
    } else {
        CodexMarketplaceResult::Added
    })
}

/// Result of ensuring the keel plugin is enabled in Codex config.toml.
#[derive(Debug)]
pub(crate) enum CodexEnableResult {
    Added,
    AlreadyEnabled,
    /// The user explicitly set `enabled = false`; install never overrides an
    /// intentional disable. Enable via Codex's `/plugins` UI or by editing the
    /// config key.
    UnchangedDisabled,
}

/// Result of registering the keel MCP server natively in Codex config.toml.
#[derive(Debug)]
pub(crate) enum CodexNativeMcpResult {
    Added,
    Updated,
    AlreadyCurrent,
}

/// The config.toml section Codex reads for this plugin's enablement:
/// `[plugins."keel@personal-keel"]` (plugin@marketplace).
pub(crate) const CODEX_PLUGIN_CONFIG_SECTION: &str = "[plugins.\"keel@personal-keel\"]";

/// The config.toml section for the native keel MCP server. A top-level
/// `[mcp_servers.<name>]` table is honored on every platform.
pub(crate) const CODEX_NATIVE_MCP_SECTION: &str = "[mcp_servers.keel]";

/// Register the keel MCP server directly in `~/.codex/config.toml`.
///
/// why: Codex on Windows does not load the MCP server a plugin bundles
/// (upstream openai/codex#26693), so the plugin path alone leaves MCP empty
/// there. The native `[mcp_servers.keel]` table works everywhere, so install
/// writes it deterministically alongside the plugin. The edit is string-
/// surgical (parse with `toml` to decide, then append or rewrite the section)
/// so comments, ordering, and unrelated keys survive untouched. Creates the
/// file when absent.
pub(crate) fn ensure_codex_native_mcp(
    config_path: &Path,
    binary: &Path,
) -> Result<CodexNativeMcpResult, String> {
    let command = display_path(binary);
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    // Decide current state via a real TOML parse; on parse failure refuse to
    // touch the file rather than risk corrupting it.
    if !stripped.trim().is_empty() {
        let doc: toml::Value =
            toml::from_str(stripped).map_err(|error| format!("parse error: {error}"))?;
        if let Some(entry) = doc.get("mcp_servers").and_then(|m| m.get("keel")) {
            let current_command = entry.get("command").and_then(|v| v.as_str());
            let current_args = entry
                .get("args")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let args_match = current_args == ["mcp", "serve"];
            if current_command == Some(command.as_str()) && args_match {
                return Ok(CodexNativeMcpResult::AlreadyCurrent);
            }
        }
    }
    // Replace an existing section in place, or append a fresh one at the end.
    let lines: Vec<&str> = stripped.lines().collect();
    let header_pos = lines
        .iter()
        .position(|line| line.trim() == CODEX_NATIVE_MCP_SECTION);
    let new_text: String = if let Some(pos) = header_pos {
        // Drop the old section body (until the next table header), then
        // re-emit the section with the desired values.
        let mut end = lines.len();
        for (offset, line) in lines[pos + 1..].iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                end = pos + 1 + offset;
                break;
            }
        }
        let mut rebuilt: Vec<String> = Vec::with_capacity(lines.len());
        rebuilt.extend(lines[..pos].iter().map(|l| l.to_string()));
        rebuilt.push(CODEX_NATIVE_MCP_SECTION.to_string());
        rebuilt.push(format!("command = {}", toml_quote(&command)));
        rebuilt.push("args = [\"mcp\", \"serve\"]".to_string());
        rebuilt.extend(lines[end..].iter().map(|l| l.to_string()));
        rebuilt.join("\n")
    } else {
        let mut out = stripped.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(CODEX_NATIVE_MCP_SECTION);
        out.push('\n');
        out.push_str(&format!("command = {}\n", toml_quote(&command)));
        out.push_str("args = [\"mcp\", \"serve\"]\n");
        out
    };
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    write_text(config_path, &new_text)?;
    // Report Added vs Updated: Added when no prior section existed.
    if header_pos.is_some() {
        Ok(CodexNativeMcpResult::Updated)
    } else {
        Ok(CodexNativeMcpResult::Added)
    }
}

/// Quote a string as a TOML basic (double-quoted) string, escaping the
/// backslashes and double quotes that Windows paths carry.
pub(crate) fn toml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Sentinels delimiting the keel-managed region inside the user-global
/// `~/.codex/AGENTS.md`, mirroring the `~/.claude/CLAUDE.md` sentinels so the
/// uninstall path can strip exactly what install wrote.
pub(crate) const MANAGED_CODEX_AGENTS_BEGIN: &str =
    "<!-- keel:begin (managed by keel install — edits inside this block are overwritten; edit outside it freely) -->";
pub(crate) const MANAGED_CODEX_AGENTS_END: &str = "<!-- keel:end -->";

/// The always-on operating contract written into `~/.codex/AGENTS.md`.
///
/// why: Codex hooks do not fire on Windows and plugin-bundled hooks require
/// approval elsewhere, so the hook channel that carries the iron law on the
/// claude host is unreliable here. Codex loads the user-global AGENTS.md into
/// every session, making it the hook-independent surface. Kept compact because
/// it is paid on every session of every project.
const MANAGED_CODEX_AGENTS_BODY: &str = r#"# keel operating contract (always-on)

Installed by keel into `~/.codex/AGENTS.md` and loaded into every Codex session — independent of hooks. Applies to every project you work in, not just keel.

## Iron Law — for any request that could touch code, config, or architecture
1. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.
2. **Understand before building.** Restate what the request asks and research what is genuinely needed before writing code. No guessing, no building against an imagined spec.
3. **Request fidelity.** Implement only what the user asked. Do not invent features, APIs, files, refactors, or "nice extras" outside the request.
4. **Ask when unclear.** If the request is unclear, conflicting, or incomplete, stop and ask the user before coding. Do not decide silently.
5. **Never trust knowledge-base alone.** Read SYSTEM_MAP, owning files, and user stories here; nothing in training data is truth for this repo.
6. **Use the keel MCP tools.** Prefer `system_map`, `recall`, `run_command`, and the `skill_*` tools over guessing; they are always available.
7. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything.
8. **Preserve existing data.** Never remove or replace an existing field, column, output, or record to fit a new format — ADD alongside, and ASK before dropping anything the user did not name."#;

/// Write (or refresh) the keel managed block inside `~/.codex/AGENTS.md`,
/// preserving any user content outside it. Creates the file when absent.
pub(crate) fn sync_codex_agents_md(path: &Path) -> Result<String, String> {
    let block = format!(
        "{MANAGED_CODEX_AGENTS_BEGIN}\n{MANAGED_CODEX_AGENTS_BODY}\n{MANAGED_CODEX_AGENTS_END}"
    );
    let existing = crate::runtime::read_text_if_exists(path).unwrap_or_default();
    let stripped = existing.strip_prefix('\u{feff}').unwrap_or(&existing);
    let merged = merge_managed_region(
        stripped,
        &block,
        MANAGED_CODEX_AGENTS_BEGIN,
        MANAGED_CODEX_AGENTS_END,
    );
    if merged == stripped {
        return Ok("AGENTS.md already current".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    write_text(path, &merged)?;
    Ok(format!("AGENTS.md written to {}", display_path(path)))
}

/// Splice `block` into `existing` content between `begin`/`end` sentinels,
/// preserving user content. If the region already exists it is replaced in
/// place; otherwise the block is prepended so the contract reads first. Pure
/// (no IO) so the splice logic is unit-testable.
pub(crate) fn merge_managed_region(existing: &str, block: &str, begin: &str, end: &str) -> String {
    if let (Some(start), Some(end_idx)) = (existing.find(begin), existing.find(end)) {
        if end_idx > start {
            let end_full = end_idx + end.len();
            let before = &existing[..start];
            let after = &existing[end_full..];
            return format!("{before}{block}{after}");
        }
    }
    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{block}\n\n{existing}")
    }
}

/// Strip the keel managed block from a file, preserving user content. Returns
/// the stripped text, or `None` when the block is absent. A file that is only
/// the block collapses to empty so the caller can delete it.
pub(crate) fn strip_managed_region(existing: &str, begin: &str, end: &str) -> Option<String> {
    let start = existing.find(begin)?;
    let end_idx = existing.find(end)?;
    if end_idx <= start {
        return None;
    }
    let end_full = end_idx + end.len();
    let before = existing[..start].trim_end();
    let after = existing[end_full..].trim_start();
    let mut stripped = if before.is_empty() {
        after.to_string()
    } else if after.is_empty() {
        format!("{before}\n")
    } else {
        format!("{before}\n{after}")
    };
    if stripped.trim().is_empty() {
        stripped = String::new();
    }
    Some(stripped)
}

/// Ensure `[plugins."keel@personal-keel"] enabled = true` is present in
/// `~/.codex/config.toml`. The edit is string-surgical (parse with `toml` to
/// decide, then append or insert lines) so comments, ordering, and unrelated
/// keys survive untouched. An explicit user `enabled = false` is respected.
/// Creates the file when absent.
pub(crate) fn ensure_codex_plugin_enabled(config_path: &Path) -> Result<CodexEnableResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    // Decide current state via a real TOML parse; on parse failure refuse to
    // touch the file rather than risk corrupting it.
    if !stripped.trim().is_empty() {
        let doc: toml::Value =
            toml::from_str(stripped).map_err(|error| format!("parse error: {error}"))?;
        let enabled = doc
            .get("plugins")
            .and_then(|p| p.get("keel@personal-keel"))
            .and_then(|entry| entry.get("enabled"))
            .and_then(|v| v.as_bool());
        match enabled {
            Some(true) => return Ok(CodexEnableResult::AlreadyEnabled),
            Some(false) => return Ok(CodexEnableResult::UnchangedDisabled),
            None => {}
        }
    }
    // Need `enabled = true`: insert under an existing section header, else
    // append the whole section at the end of the file.
    let header = CODEX_PLUGIN_CONFIG_SECTION;
    let lines: Vec<&str> = stripped.lines().collect();
    let mut new_text: String = if let Some(pos) = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed == header || trimmed == "[plugins.'keel@personal-keel']"
    }) {
        let mut rebuilt: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        rebuilt.insert(pos + 1, "enabled = true".to_string());
        rebuilt.join("\n")
    } else {
        let mut out = stripped.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(header);
        out.push('\n');
        out.push_str("enabled = true\n");
        out
    };
    if new_text.is_empty() {
        new_text = format!("{header}\nenabled = true\n");
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    write_text(config_path, &new_text)?;
    Ok(CodexEnableResult::Added)
}

/// Remove the `keel` entry from the personal Codex marketplace manifest.
/// Preserves sibling entries and other keys; deletes the manifest only when it
/// becomes an empty catalog that install itself created shape for.
pub(crate) fn remove_codex_marketplace_entry(marketplace_path: &Path) -> usize {
    if !marketplace_path.is_file() {
        return 0;
    }
    let Ok(text) = crate::runtime::read_text_if_exists(marketplace_path) else {
        return 0;
    };
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(stripped) else {
        return 0;
    };
    let Some(plugins) = doc.get_mut("plugins").and_then(|v| v.as_array_mut()) else {
        return 0;
    };
    let before = plugins.len();
    plugins.retain(|entry| entry.get("name").and_then(|n| n.as_str()) != Some("keel"));
    if plugins.len() == before {
        return 0;
    }
    if plugins.is_empty() {
        // The catalog holds no plugins anymore; the whole file is keel's.
        return remove_path_if_exists_counted(marketplace_path).unwrap_or(0);
    }
    let _ = write_text(
        marketplace_path,
        &serde_json::to_string_pretty(&doc).unwrap_or_else(|_| stripped.to_string()),
    );
    1
}
/// drops the header line plus every key line until the next section header.
pub(crate) fn remove_codex_plugin_section(config_path: &Path) -> usize {
    if !config_path.is_file() {
        return 0;
    }
    let Ok(text) = crate::runtime::read_text_if_exists(config_path) else {
        return 0;
    };
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = stripped.lines().collect();
    let header_pos = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed == CODEX_PLUGIN_CONFIG_SECTION || trimmed == "[plugins.'keel@personal-keel']"
    });
    let Some(pos) = header_pos else {
        return 0;
    };
    // Find the end of the section: the next line that starts a new table.
    let mut end = lines.len();
    for (offset, line) in lines[pos + 1..].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            end = pos + 1 + offset;
            break;
        }
    }
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..pos]);
    kept.extend_from_slice(&lines[end..]);
    // Collapse the blank-line seam the removal may leave at the splice point.
    while kept.len() >= 2 {
        let n = kept.len();
        if kept[n - 1].trim().is_empty() && kept[n - 2].trim().is_empty() {
            kept.remove(n - 1);
        } else {
            break;
        }
    }
    let mut new_text = kept.join("\n");
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    let _ = write_text(config_path, &new_text);
    1
}

/// Remove the `[mcp_servers.keel]` section from Codex config.toml without
/// disturbing any other section, key, or comment. Mirrors the plugin-section
/// removal: drops the header line plus every key line until the next header.
pub(crate) fn remove_codex_native_mcp_section(config_path: &Path) -> usize {
    if !config_path.is_file() {
        return 0;
    }
    let Ok(text) = crate::runtime::read_text_if_exists(config_path) else {
        return 0;
    };
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = stripped.lines().collect();
    let Some(pos) = lines
        .iter()
        .position(|line| line.trim() == CODEX_NATIVE_MCP_SECTION)
    else {
        return 0;
    };
    let mut end = lines.len();
    for (offset, line) in lines[pos + 1..].iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            end = pos + 1 + offset;
            break;
        }
    }
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..pos]);
    kept.extend_from_slice(&lines[end..]);
    while kept.len() >= 2 {
        let n = kept.len();
        if kept[n - 1].trim().is_empty() && kept[n - 2].trim().is_empty() {
            kept.remove(n - 1);
        } else {
            break;
        }
    }
    let mut new_text = kept.join("\n");
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    let _ = write_text(config_path, &new_text);
    1
}

/// Strip the keel managed block from `~/.codex/AGENTS.md`, preserving any user
/// content outside it. Deletes the file only if it becomes empty. Returns the
/// number of paths changed/removed (0 or 1); a missing file or one without the
/// managed block is a no-op.
pub(crate) fn remove_codex_managed_agents_md(path: &Path) -> usize {
    if !path.is_file() {
        return 0;
    }
    let Ok(text) = crate::runtime::read_text_if_exists(path) else {
        return 0;
    };
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Some(stripped_content) = strip_managed_region(
        stripped,
        MANAGED_CODEX_AGENTS_BEGIN,
        MANAGED_CODEX_AGENTS_END,
    ) else {
        return 0;
    };
    if stripped_content.trim().is_empty() {
        remove_path_if_exists_counted(path).unwrap_or(0)
    } else {
        match write_text(path, &stripped_content) {
            Ok(()) => 1,
            Err(_) => 0,
        }
    }
}
