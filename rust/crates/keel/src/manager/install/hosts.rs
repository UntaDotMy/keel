// Installer host wiring.
use super::*;
use crate::runtime::{display_path, installed_executable_path, write_text};
use std::path::{Path, PathBuf};
pub(crate) fn maybe_register_mcp_server(claude_home: &Path) -> Option<String> {
    if !super::super::mcp_register::is_standard_claude_home(claude_home) {
        return None;
    }
    match super::super::mcp_register::register_mcp_server(claude_home) {
        Ok(super::super::mcp_register::McpRegistration::Added) => {
            Some("registered in ~/.claude.json".to_string())
        }
        Ok(super::super::mcp_register::McpRegistration::Updated) => {
            Some("updated in ~/.claude.json".to_string())
        }
        Ok(super::super::mcp_register::McpRegistration::AlreadyCurrent) => {
            Some("already current".to_string())
        }
        Err(error) => Some(format!("skipped ({error})")),
    }
}

/// Install the harness lifecycle hooks into `<claude_home>/settings.json`
/// during install, pointing them at the just-published binary.
///
/// Guarded on the standard `.claude` home name for the same reason as
/// `maybe_register_mcp_server`: the integration suite installs into throwaway
/// `--claude-home` directories, and writing a real settings.json into each
/// would race parallel tests sharing a temp-dir parent. Real installs into
/// `~/.claude` always get hooks.
///
/// Best-effort: a failure is reported in the summary but never fails the
/// install. The previous behavior left hook installation to the one-line
/// bootstrap scripts only, so a manual `keel install`, an `update`,
/// or a plugin-only setup produced skills+MCP with no hooks — meaning the
/// SessionStart bootstrap and per-prompt routing never fired. Folding it in
/// here makes the engagement rails load-bearing on every install path.
pub(crate) fn maybe_install_hooks(engagement_home: &Path, keel_home: &Path) -> Option<String> {
    if !is_standard_home(engagement_home) {
        return None;
    }
    let hook_path = engagement_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    // Point hooks at the installed binary, not the running build artifact.
    let executable = installed_executable_path(keel_home);
    match crate::runner::hook_lifecycle::build_hooks_payload(&hook_path, &executable) {
        Ok(payload) => match write_text(&hook_path, &payload) {
            Ok(()) => Some(format!("installed at {}", display_path(&hook_path))),
            Err(error) => Some(format!("skipped ({error})")),
        },
        Err(error) => Some(format!("skipped ({error})")),
    }
}

/// Wire keel into OpenCode: copy the bridge plugin into
/// `~/.config/opencode/plugins/` (plural — the directory OpenCode actually
/// loads, per opencode.ai/docs/plugins) and merge a `keel` MCP server
/// into `opencode.json` (merge, never clobber). Guarded on standard `.claude`
/// home; best-effort — a failure is reported in the summary, never fails install.
pub(crate) fn maybe_wire_opencode(
    repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }

    // Derive the owning home from `claude_home.parent()`, not process globals.
    let home: PathBuf = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("skipped (no home directory)".to_string()),
    };

    let plugin_dir = home.join(".config").join("opencode").join("plugins");
    if let Err(error) = std::fs::create_dir_all(&plugin_dir) {
        return Some(format!("plugin dir skipped ({error})"));
    }

    // Copy the OpenCode bridge plugin so its lifecycle wiring can run.
    let plugin_source = repository_root.join("opencode").join("keel.ts");
    let plugin_status = if plugin_source.is_file() {
        let plugin_target = plugin_dir.join("keel.ts");
        match std::fs::copy(&plugin_source, &plugin_target) {
            Ok(_) => format!("plugin -> {}", display_path(&plugin_target)),
            Err(error) => format!("plugin copy skipped ({error})"),
        }
    } else {
        "plugin source absent".to_string()
    };

    let opencode_config_path = home.join(".config").join("opencode").join("opencode.json");
    let binary = installed_executable_path(claude_home);
    let mcp_entry = serde_json::json!({
        "type": "local",
        "command": [display_path(&binary), "mcp", "serve"],
        "enabled": true,
    });

    let mcp_status = match merge_json_mcp(&opencode_config_path, "mcp", "keel", &mcp_entry, None) {
        Ok(JsonMcpMergeResult::Added) => {
            format!("MCP registered in {}", display_path(&opencode_config_path))
        }
        Ok(JsonMcpMergeResult::AlreadyCurrent) => "MCP already current".to_string(),
        Ok(JsonMcpMergeResult::Updated) => {
            format!("MCP updated in {}", display_path(&opencode_config_path))
        }
        Err(error) => format!("MCP skipped ({error})"),
    };

    Some(format!("{plugin_status}; {mcp_status}"))
}

pub(crate) fn maybe_wire_cursor(
    repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }
    let home = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("no home directory".to_string()),
    };
    let mut status_parts: Vec<String> = Vec::new();

    // Copy .cursorrules
    let cursorrules_source = repository_root.join("cursor").join(".cursorrules");
    if !cursorrules_source.is_file() {
        status_parts.push("cursorrules source absent".to_string());
    } else {
        let cursorrules_target = home.join(".cursorrules");
        if cursorrules_target.is_file() {
            let source_bytes = std::fs::read(&cursorrules_source).unwrap_or_default();
            let target_bytes = std::fs::read(&cursorrules_target).unwrap_or_default();
            if source_bytes != target_bytes {
                status_parts.push("cursorrules skipped (user-customized)".to_string());
            } else {
                status_parts.push("cursorrules already current".to_string());
            }
        } else {
            match std::fs::copy(&cursorrules_source, &cursorrules_target) {
                Ok(_) => status_parts.push(format!(
                    "cursorrules -> {}",
                    display_path(&cursorrules_target)
                )),
                Err(error) => status_parts.push(format!("cursorrules copy failed ({error})")),
            }
        }
    }

    // Copy compaction reroute hooks (preToolUse + keel-cursor.sh)
    let hooks_json_source = repository_root
        .join("cursor")
        .join("hooks")
        .join("hooks.json");
    let rewrite_script_source = repository_root
        .join("cursor")
        .join("hooks")
        .join("keel-cursor.sh");
    if hooks_json_source.is_file() || rewrite_script_source.is_file() {
        let hooks_dir = home.join(".cursor").join("hooks");
        let _ = std::fs::create_dir_all(&hooks_dir);
        if hooks_json_source.is_file() {
            let target = hooks_dir.join("hooks.json");
            match std::fs::copy(&hooks_json_source, &target) {
                Ok(_) => status_parts.push("hooks.json copied".to_string()),
                Err(e) => status_parts.push(format!("hooks.json copy failed ({e})")),
            }
        }
        if rewrite_script_source.is_file() {
            let target = hooks_dir.join("keel-cursor.sh");
            match std::fs::copy(&rewrite_script_source, &target) {
                Ok(_) => status_parts.push("keel-cursor.sh copied".to_string()),
                Err(e) => status_parts.push(format!("keel-cursor.sh copy failed ({e})")),
            }
        }
    }

    // Cursor MCP: merge the `keel` entry into ~/.cursor/mcp.json. Cursor loads
    let cursor_mcp_source = repository_root.join("cursor").join("mcp.json");
    if cursor_mcp_source.is_file() {
        let mcp_target = home.join(".cursor").join("mcp.json");
        if let Some(parent) = mcp_target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mcp_entry = match std::fs::read_to_string(&cursor_mcp_source) {
            Ok(text) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
                parsed
                    .get("mcpServers")
                    .and_then(|s| s.get("keel"))
                    .cloned()
                    .unwrap_or(serde_json::json!({}))
            }
            Err(_) => serde_json::json!({}),
        };
        let binary = installed_executable_path(claude_home);
        let mut mcp_entry = if mcp_entry.is_null() {
            serde_json::json!({
                "command": display_path(&binary),
                "args": ["mcp", "serve"],
            })
        } else {
            mcp_entry
        };
        // The shipped cursor/mcp.json templates a bare PATH-dependent `keel`
        rewrite_mcp_entry_command(&mut mcp_entry, &display_path(&binary));
        match merge_json_mcp(&mcp_target, "mcpServers", "keel", &mcp_entry, None) {
            Ok(JsonMcpMergeResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(JsonMcpMergeResult::AlreadyCurrent) => {
                status_parts.push("MCP already current".to_string())
            }
            Ok(JsonMcpMergeResult::Updated) => {
                status_parts.push(format!("MCP updated in {}", display_path(&mcp_target)))
            }
            Err(error) => status_parts.push(format!("MCP skipped ({error})")),
        }
    }

    if status_parts.is_empty() {
        Some("nothing to copy".to_string())
    } else {
        Some(status_parts.join("; "))
    }
}

/// Grok loads global hooks from `~/.grok/hooks/*.json`. Claude-compat also
/// scans `~/.claude/settings.json`, but that scan can be turned off. Write a
/// native Grok hook file so PreToolUse deny (Iron Law + Anvil) always fires.
/// Stop must call `keel hook stop` (silent). Grok treats Stop additionalContext
/// as "keep going"; wiring Stop to post-tool-batch loops until the host cap.
pub(crate) fn maybe_wire_grok(claude_home: &Path) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    let home = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("skipped (no home directory)".to_string()),
    };
    let grok_dir = home.join(".grok");
    if !grok_dir.is_dir() {
        return Some("skipped (not detected)".to_string());
    }
    let hooks_dir = grok_dir.join("hooks");
    if let Err(error) = std::fs::create_dir_all(&hooks_dir) {
        return Some(format!("hooks dir skipped ({error})"));
    }
    let target = hooks_dir.join("keel.json");
    let binary = installed_executable_path(claude_home);
    let command = format!("{} hook", display_path(&binary));
    let payload = serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": format!("{command} session-start"), "timeout": 10 }] }],
            "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": format!("{command} user-prompt-submit"), "timeout": 10 }] }],
            "PreToolUse": [{ "hooks": [{ "type": "command", "command": format!("{command} pre-tool-use"), "timeout": 10 }] }],
            "PostToolUse": [{ "hooks": [{ "type": "command", "command": format!("{command} post-tool-use"), "timeout": 10 }] }],
            "PostToolUseFailure": [{ "hooks": [{ "type": "command", "command": format!("{command} post-tool-use-failure"), "timeout": 10 }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": format!("{command} stop"), "timeout": 10 }] }]
        }
    });
    let rendered = match serde_json::to_string_pretty(&payload) {
        Ok(text) => text,
        Err(error) => return Some(format!("serialize skipped ({error})")),
    };
    match std::fs::write(&target, rendered) {
        Ok(()) => Some(format!("hooks -> {}", display_path(&target))),
        Err(error) => Some(format!("hooks write skipped ({error})")),
    }
}

pub(crate) fn maybe_wire_pi(
    repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }
    let agents_source = repository_root.join("pi").join("AGENTS.md");
    let mcp_source = repository_root.join("pi").join(".mcp.json");
    if !agents_source.is_file() && !mcp_source.is_file() {
        return Some("source absent".to_string());
    }
    let home = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("no home directory".to_string()),
    };
    let mut status_parts: Vec<String> = Vec::new();
    if agents_source.is_file() {
        let agents_target = home.join(".pi").join("agent").join("AGENTS.md");
        if let Some(parent) = agents_target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if agents_target.is_file() {
            let source_bytes = std::fs::read(&agents_source).unwrap_or_default();
            let target_bytes = std::fs::read(&agents_target).unwrap_or_default();
            if source_bytes == target_bytes {
                status_parts.push("AGENTS.md already current".to_string());
            } else {
                status_parts.push("AGENTS.md skipped (user-customized)".to_string());
            }
        } else {
            match std::fs::copy(&agents_source, &agents_target) {
                Ok(_) => {
                    status_parts.push(format!("AGENTS.md -> {}", display_path(&agents_target)))
                }
                Err(error) => status_parts.push(format!("AGENTS.md copy failed ({error})")),
            }
        }
    }
    if mcp_source.is_file() {
        // Pi reads MCP config from the global or project-scoped path.
        let mcp_target = home.join(".pi").join("agent").join("mcp.json");
        if let Some(parent) = mcp_target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (mcp_entry, template_defaults) = match std::fs::read_to_string(&mcp_source) {
            Ok(text) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
                let entry = parsed
                    .get("mcpServers")
                    .and_then(|s| s.get("keel"))
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                // Top-level template keys (e.g. `settings`) ride along as
                // defaults; existing user values always win in the merge.
                let mut defaults = serde_json::json!({});
                if let Some(obj) = parsed.as_object() {
                    for (key, value) in obj {
                        if key != "mcpServers" {
                            defaults[key.clone()] = value.clone();
                        }
                    }
                }
                (entry, defaults)
            }
            Err(_) => (serde_json::json!({}), serde_json::json!({})),
        };
        let binary = installed_executable_path(claude_home);
        let mut mcp_entry = if mcp_entry.is_null() {
            serde_json::json!({
                "command": display_path(&binary),
                "args": ["mcp", "serve"],
                "lifecycle": "lazy",
                "directTools": true,
            })
        } else {
            mcp_entry
        };
        // The shipped pi/.mcp.json templates a bare PATH-dependent `keel`
        rewrite_mcp_entry_command(&mut mcp_entry, &display_path(&binary));
        match merge_json_mcp(
            &mcp_target,
            "mcpServers",
            "keel",
            &mcp_entry,
            Some(&template_defaults),
        ) {
            Ok(JsonMcpMergeResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(JsonMcpMergeResult::AlreadyCurrent) => {
                status_parts.push("MCP already current".to_string())
            }
            Ok(JsonMcpMergeResult::Updated) => {
                status_parts.push(format!("MCP updated in {}", display_path(&mcp_target)))
            }
            Err(error) => status_parts.push(format!("MCP skipped ({error})")),
        }
    }
    let extension_source = repository_root.join("pi").join("keel-pi.ts");
    if extension_source.is_file() {
        // Pi auto-discovers extensions from ~/.pi/agent/extensions/*.ts
        // Pi extensions may be global or project-scoped; do not use ~/.pi/extensions/.
        let extensions_dir = home.join(".pi").join("agent").join("extensions");
        let _ = std::fs::create_dir_all(&extensions_dir);
        let target = extensions_dir.join("keel-pi.ts");
        match std::fs::copy(&extension_source, &target) {
            Ok(_) => status_parts.push(format!("keel-pi.ts -> {}", display_path(&target))),
            Err(e) => status_parts.push(format!("keel-pi.ts copy failed ({e})")),
        }
    }

    Some(status_parts.join("; "))
}

/// Wire the Command Code (cmdc) adapter: copy the mod into
/// `~/.commandcode/mods/` and merge the `keel` MCP entry (never clobber).
pub(crate) fn maybe_wire_commandcode(
    repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }
    let home: PathBuf = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("no home directory".to_string()),
    };
    let mut status_parts: Vec<String> = Vec::new();

    // Copy the mod into the personal mods directory so it loads next session.
    let mod_source = repository_root.join("commandcode").join("keel-cmdc.ts");
    if mod_source.is_file() {
        let mods_dir = home.join(".commandcode").join("mods");
        if let Err(error) = std::fs::create_dir_all(&mods_dir) {
            status_parts.push(format!("mods dir skipped ({error})"));
        } else {
            let target = mods_dir.join("keel-cmdc.ts");
            match std::fs::copy(&mod_source, &target) {
                Ok(_) => status_parts.push(format!("keel-cmdc.ts -> {}", display_path(&target))),
                Err(error) => status_parts.push(format!("keel-cmdc.ts copy failed ({error})")),
            }
        }
    } else {
        status_parts.push("mod source absent".to_string());
    }

    // Merge the keel MCP entry into ~/.commandcode/mcp.json.
    let mcp_source = repository_root.join("commandcode").join("mcp.json");
    if mcp_source.is_file() {
        let mcp_target = home.join(".commandcode").join("mcp.json");
        if let Some(parent) = mcp_target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mcp_entry = match std::fs::read_to_string(&mcp_source) {
            Ok(text) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
                parsed
                    .get("mcpServers")
                    .and_then(|s| s.get("keel"))
                    .cloned()
                    .unwrap_or(serde_json::json!({}))
            }
            Err(_) => serde_json::json!({}),
        };
        let binary = installed_executable_path(claude_home);
        let mut mcp_entry = if mcp_entry.is_null() {
            serde_json::json!({
                "command": display_path(&binary),
                "args": ["mcp", "serve"],
            })
        } else {
            mcp_entry
        };
        // The shipped commandcode/mcp.json templates a bare PATH-dependent
        // `keel` command; rewrite it to the absolute installed binary path.
        rewrite_mcp_entry_command(&mut mcp_entry, &display_path(&binary));
        match merge_json_mcp(&mcp_target, "mcpServers", "keel", &mcp_entry, None) {
            Ok(JsonMcpMergeResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(JsonMcpMergeResult::AlreadyCurrent) => {
                status_parts.push("MCP already current".to_string())
            }
            Ok(JsonMcpMergeResult::Updated) => {
                status_parts.push(format!("MCP updated in {}", display_path(&mcp_target)))
            }
            Err(error) => status_parts.push(format!("MCP skipped ({error})")),
        }
    }

    if status_parts.is_empty() {
        Some("nothing to copy".to_string())
    } else {
        Some(status_parts.join("; "))
    }
}

pub(crate) fn maybe_wire_codex(
    repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }
    let codex_source_dir = repository_root.join("codex");
    if !codex_source_dir.is_dir() {
        return Some("source absent".to_string());
    }
    let home: PathBuf = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("no home directory".to_string()),
    };
    let plugin_target = home.join(".codex").join("plugins").join("keel");
    if let Err(error) = std::fs::create_dir_all(&plugin_target) {
        return Some(format!("plugin dir failed ({error})"));
    }
    let mut copied = 0;
    for entry in [
        "hooks/hooks.json",
        "keel-codex.ts",
        ".codex-plugin/plugin.json",
        ".mcp.json",
    ] {
        let source = codex_source_dir.join(entry);
        let target = plugin_target.join(entry);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if source.is_file() && std::fs::copy(&source, &target).is_ok() {
            copied += 1;
        }
    }
    // Codex resolves the MCP `command` via PATH only. The shipped .mcp.json
    let mcp_target = plugin_target.join(".mcp.json");
    let binary = installed_executable_path(claude_home);
    let mcp_status = if mcp_target.is_file() {
        match std::fs::read_to_string(&mcp_target)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        {
            Some(mut doc) => {
                let absolute = display_path(&binary);
                let mutated = rewrite_codex_mcp_command(&mut doc, &absolute);
                if mutated {
                    if let Ok(pretty) = serde_json::to_string_pretty(&doc) {
                        let _ = write_text(&mcp_target, &pretty);
                    }
                }
                if mutated {
                    "MCP command -> absolute".to_string()
                } else {
                    "MCP command unchanged".to_string()
                }
            }
            None => "MCP unparseable".to_string(),
        }
    } else {
        "MCP absent".to_string()
    };
    // Codex discovers plugins ONLY via a marketplace manifest and loads only
    // plugins enabled in config.toml; copying files alone never wires them.
    let home_dir = plugin_target
        .parent() // plugins/
        .and_then(|p| p.parent()) // .codex/
        .and_then(|p| p.parent()) // user home
        .map(Path::to_path_buf);
    let mut wire_status: Vec<String> = Vec::new();
    if let Some(home_dir) = &home_dir {
        let marketplace_path = home_dir
            .join(".agents")
            .join("plugins")
            .join("marketplace.json");
        match merge_codex_marketplace(&marketplace_path) {
            Ok(CodexMarketplaceResult::Added) => wire_status.push(format!(
                "marketplace entry added in {}",
                display_path(&marketplace_path)
            )),
            Ok(CodexMarketplaceResult::AlreadyCurrent) => {
                wire_status.push("marketplace entry already current".to_string())
            }
            Ok(CodexMarketplaceResult::Updated) => wire_status.push(format!(
                "marketplace entry updated in {}",
                display_path(&marketplace_path)
            )),
            Err(error) => wire_status.push(format!("marketplace skipped ({error})")),
        }
        let codex_config = home_dir.join(".codex").join("config.toml");
        match ensure_codex_plugin_enabled(&codex_config) {
            Ok(CodexEnableResult::Added) => {
                wire_status.push(format!("plugin enabled in {}", display_path(&codex_config)))
            }
            Ok(CodexEnableResult::AlreadyEnabled) => {
                wire_status.push("plugin already enabled".to_string())
            }
            Ok(CodexEnableResult::UnchangedDisabled) => {
                wire_status.push("plugin disabled by user (enable via Codex /plugins)".to_string())
            }
            Err(error) => wire_status.push(format!("enablement skipped ({error})")),
        }
        // Native MCP registration: Codex on Windows never loads a plugin's
        // bundled MCP (openai/codex#26693), and [mcp_servers.keel] works everywhere.
        let binary = installed_executable_path(claude_home);
        match ensure_codex_native_mcp(&codex_config, &binary) {
            Ok(CodexNativeMcpResult::Added) => wire_status.push(format!(
                "native MCP registered in {}",
                display_path(&codex_config)
            )),
            Ok(CodexNativeMcpResult::Updated) => wire_status.push(format!(
                "native MCP command updated in {}",
                display_path(&codex_config)
            )),
            Ok(CodexNativeMcpResult::AlreadyCurrent) => {
                wire_status.push("native MCP already current".to_string())
            }
            Err(error) => wire_status.push(format!("native MCP skipped ({error})")),
        }
        // Always-on contract: Codex hooks are unreliable (absent on Windows),
        // and the user-global AGENTS.md is the hook-independent surface.
        let codex_agents = home_dir.join(".codex").join("AGENTS.md");
        match sync_codex_agents_md(&codex_agents) {
            Ok(status) => wire_status.push(status),
            Err(error) => wire_status.push(format!("AGENTS.md skipped ({error})")),
        }
    }

    Some(format!(
        "{copied} files -> {}; {mcp_status}; {}",
        display_path(&plugin_target),
        wire_status.join("; ")
    ))
}
/// Remove the `[plugins."keel@personal-keel"]` section from Codex config.toml
/// without disturbing any other section, key, or comment. String-surgical:
pub(crate) fn claude_desktop_config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(target_os = "windows") {
        home.join("AppData").join("Roaming").join("Claude")
    } else if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("Claude")
    } else {
        home.join(".config").join("Claude")
    };
    dir.join("claude_desktop_config.json")
}

/// Register keel's MCP server for Claude Desktop (Cowork) in
/// `claude_desktop_config.json` — the only integration Claude Desktop actually
/// supports.
///
/// why: Claude Desktop has NO lifecycle-hook system and NO JS plugin API (verified
/// against the docs and open parity issues), so the hook-based automation the CLI
/// and Claude Code plugin deliver (iron-law gate, compaction, gates, learning)
/// cannot run there. The prior implementation copied a dead TS plugin into
/// `~/.claude/plugins/keel-cowork/` (which Desktop never scans) and merged MCP into
/// `~/.claude/settings.json` (which Desktop never reads). This MCP-only honest
/// wiring registers the server where Desktop actually looks; skills are added via
/// Desktop's account-synced Customize UI, not the filesystem.
pub(crate) fn maybe_wire_cowork(
    _repository_root: &Path,
    claude_home: &Path,
    detected: bool,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    if !detected {
        return Some("skipped (not detected)".to_string());
    }

    let home = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("skipped (no home directory)".to_string()),
    };

    let config_path = claude_desktop_config_path(&home);
    if let Some(parent) = config_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Some(format!("MCP skipped (create config dir: {error})"));
        }
    }

    let binary = installed_executable_path(claude_home);
    let mcp_entry = serde_json::json!({
        "type": "stdio",
        "command": display_path(&binary),
        "args": ["mcp", "serve"],
        "description": "Keel CLI tools for Anvil, memory, recall, and review"
    });

    match merge_json_mcp(&config_path, "mcpServers", "keel", &mcp_entry, None) {
        Ok(JsonMcpMergeResult::Added) => Some(format!(
            "MCP registered in {} (Desktop supports MCP tools only — no hooks)",
            display_path(&config_path)
        )),
        Ok(JsonMcpMergeResult::AlreadyCurrent) => Some("MCP already current".to_string()),
        Ok(JsonMcpMergeResult::Updated) => {
            Some(format!("MCP updated in {}", display_path(&config_path)))
        }
        Err(error) => Some(format!("MCP skipped ({error})")),
    }
}
