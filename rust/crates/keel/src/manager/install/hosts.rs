// Installer host wiring.
use super::*;
use crate::runtime::{display_path, installed_executable_path, write_text};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedCopyStatus {
    Copied,
    AlreadyCurrent,
    PreservedCustom,
}

const MANAGED_HOST_FILE_MARKER: &str = "keel:managed-host-file";

fn legacy_managed_host_file(target: &Path, content: &str) -> bool {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match file_name {
        "keel-pi.ts" => content.contains("keel Pi Agent Extension"),
        "keel-antigravity.js" => content.contains("Keel Antigravity hook adapter"),
        "keel.ts" => {
            content.contains("@opencode-ai/plugin") && content.contains("../_shared/ts/bridge-core")
        }
        "keel-cursor.sh" => content.contains("keel Cursor adapter"),
        "keel-cmdc.ts" => content.contains("keel Command Code (cmdc) Mod"),
        "keel-codex.ts" => content.contains("keel Codex CLI Plugin"),
        "keel-codex.js" => content.contains("codex/keel-codex.ts"),
        "bridge-core.ts" => content.contains("resolveBinary") && content.contains("keel.exe"),
        "SKILL.md" => {
            content.contains("name: using-keel") && content.contains("Trust the codebase")
        }
        "common-discipline.md" => content.contains("Shared Discipline — Common Standards"),
        "subagent-iron-law.md" => content.contains("Subagent Iron Law — Read First"),
        "mcp-and-memory.md" => content.contains("MCP tools and memory writes"),
        "skill-and-agent-catalog.md" => content.contains("Skill and agent catalog"),
        _ => false,
    }
}

fn copy_managed_file(source: &Path, target: &Path) -> Result<ManagedCopyStatus, String> {
    let source_bytes =
        std::fs::read(source).map_err(|error| format!("read {}: {error}", display_path(source)))?;
    if let Ok(metadata) = std::fs::symlink_metadata(target) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(ManagedCopyStatus::PreservedCustom);
        }
        let target_bytes = std::fs::read(target)
            .map_err(|error| format!("read {}: {error}", display_path(target)))?;
        if target_bytes == source_bytes {
            return Ok(ManagedCopyStatus::AlreadyCurrent);
        }
        let source_text = String::from_utf8_lossy(&source_bytes);
        let target_text = String::from_utf8_lossy(&target_bytes);
        if !source_text.contains(MANAGED_HOST_FILE_MARKER)
            || (!target_text.contains(MANAGED_HOST_FILE_MARKER)
                && !legacy_managed_host_file(target, &target_text))
        {
            return Ok(ManagedCopyStatus::PreservedCustom);
        }
        std::fs::write(target, &source_bytes)
            .map_err(|error| format!("write {}: {error}", display_path(target)))?;
        return Ok(ManagedCopyStatus::Copied);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    std::fs::write(target, source_bytes)
        .map_err(|error| format!("write {}: {error}", display_path(target)))?;
    Ok(ManagedCopyStatus::Copied)
}

fn copy_bridge_core(repository_root: &Path, host_root: &Path) -> String {
    let source = repository_root
        .join("_shared")
        .join("ts")
        .join("bridge-core.ts");
    let target = host_root.join("_shared").join("ts").join("bridge-core.ts");
    if !source.is_file() {
        return "bridge core source absent".to_string();
    }
    match copy_managed_file(&source, &target) {
        Ok(ManagedCopyStatus::Copied) => format!("bridge core -> {}", display_path(&target)),
        Ok(ManagedCopyStatus::AlreadyCurrent) => "bridge core already current".to_string(),
        Ok(ManagedCopyStatus::PreservedCustom) => {
            "bridge core preserved (user-customized)".to_string()
        }
        Err(error) => format!("bridge core skipped ({error})"),
    }
}

fn copy_gateway_skill(repository_root: &Path, skills_root: &Path) -> String {
    let source = repository_root.join("using-keel");
    let target = skills_root.join("using-keel");
    if !source.join("SKILL.md").is_file() {
        return "gateway skill source absent".to_string();
    }

    let mut counts = [0usize; 3];
    if let Err(error) = copy_managed_tree(&source, &target, &mut counts) {
        return format!("gateway skill skipped ({error})");
    }
    let shared_source = repository_root.join("_shared");
    if shared_source.is_dir() {
        if let Err(error) =
            copy_managed_tree(&shared_source, &skills_root.join("_shared"), &mut counts)
        {
            return format!("gateway skill shared resources skipped ({error})");
        }
    }
    if counts[2] > 0 {
        format!(
            "gateway skill current ({} files copied, {} user-customized preserved)",
            counts[0], counts[2]
        )
    } else if counts[0] > 0 {
        format!(
            "gateway skill -> {} ({} files)",
            display_path(&target),
            counts[0]
        )
    } else {
        "gateway skill already current".to_string()
    }
}

fn copy_managed_tree(source: &Path, target: &Path, counts: &mut [usize; 3]) -> Result<(), String> {
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("read {}: {error}", display_path(source)))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read directory entry: {error}"))?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "read file type for {}: {error}",
                display_path(&entry.path())
            )
        })?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_managed_tree(&source_path, &target_path, counts)?;
        } else if file_type.is_file() {
            match copy_managed_file(&source_path, &target_path)? {
                ManagedCopyStatus::Copied => counts[0] += 1,
                ManagedCopyStatus::AlreadyCurrent => counts[1] += 1,
                ManagedCopyStatus::PreservedCustom => counts[2] += 1,
            }
        }
    }
    Ok(())
}

const MANAGED_HOST_AGENTS_BEGIN: &str =
    "<!-- keel:begin (managed by keel install — edits inside this block are overwritten; edit outside it freely) -->";
const MANAGED_HOST_AGENTS_END: &str = "<!-- keel:end -->";

fn sync_host_agents_md(path: &Path, host: &str) -> Result<String, String> {
    let body = format!(
        "# keel operating contract (always-on)\n\nInstalled by keel for {host}. Before changing code, config, or architecture: read SYSTEM_MAP and the owning file; restate and research the request; use the keel MCP tools (`context_brief`, `system_map`, `recall`, `skill_route`, `skill_get`, `anvil`, `run_command`); trace the root cause; preserve user data; run affected tests and review before finishing. Do not trust training knowledge over current repository or official documentation."
    );
    let block = format!("{MANAGED_HOST_AGENTS_BEGIN}\n{body}\n{MANAGED_HOST_AGENTS_END}");
    let existing = crate::runtime::read_text_if_exists(path).unwrap_or_default();
    let stripped = existing.strip_prefix('\u{feff}').unwrap_or(&existing);
    let merged = super::super::install::codex::merge_managed_region(
        stripped,
        &block,
        MANAGED_HOST_AGENTS_BEGIN,
        MANAGED_HOST_AGENTS_END,
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

fn write_generated_managed_file(
    target: &Path,
    content: &str,
    ownership_marker: &str,
) -> Result<ManagedCopyStatus, String> {
    if let Ok(metadata) = std::fs::symlink_metadata(target) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(ManagedCopyStatus::PreservedCustom);
        }
        let existing = std::fs::read_to_string(target)
            .map_err(|error| format!("read {}: {error}", display_path(target)))?;
        if existing == content {
            return Ok(ManagedCopyStatus::AlreadyCurrent);
        }
        if !existing.contains(ownership_marker) {
            return Ok(ManagedCopyStatus::PreservedCustom);
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    write_text(target, content)?;
    Ok(ManagedCopyStatus::Copied)
}

pub(crate) fn maybe_wire_agents_gateway(
    repository_root: &Path,
    claude_home: &Path,
) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    let home = host_user_home(claude_home)?;
    Some(copy_gateway_skill(
        repository_root,
        &home.join(".agents").join("skills"),
    ))
}
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
    let _ = crate::manager::install::sync::backup_file_before_managed_overwrite(
        engagement_home,
        &hook_path,
        crate::hooks::claude::SETTINGS_FILE_NAME,
    );
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
    let home: PathBuf = match host_user_home(claude_home) {
        Some(path) => path,
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
        match copy_managed_file(&plugin_source, &plugin_target) {
            Ok(ManagedCopyStatus::Copied) => {
                format!("plugin -> {}", display_path(&plugin_target))
            }
            Ok(ManagedCopyStatus::AlreadyCurrent) => "plugin already current".to_string(),
            Ok(ManagedCopyStatus::PreservedCustom) => {
                "plugin skipped (user-customized)".to_string()
            }
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
        Err(error) => format!("MCP skipped ({error})"),
    };

    let core_status = copy_bridge_core(repository_root, &home.join(".config").join("opencode"));
    Some(format!("{plugin_status}; {core_status}; {mcp_status}"))
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
    let home = match host_user_home(claude_home) {
        Some(path) => path,
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
            let target = home.join(".cursor").join("hooks.json");
            match copy_managed_file(&hooks_json_source, &target) {
                Ok(ManagedCopyStatus::Copied) => status_parts.push("hooks.json copied".to_string()),
                Ok(ManagedCopyStatus::AlreadyCurrent) => {
                    status_parts.push("hooks.json already current".to_string())
                }
                Ok(ManagedCopyStatus::PreservedCustom) => {
                    status_parts.push("hooks.json skipped (user-customized)".to_string())
                }
                Err(error) => status_parts.push(format!("hooks.json copy failed ({error})")),
            }
        }
        if rewrite_script_source.is_file() {
            let target = hooks_dir.join("keel-cursor.sh");
            match copy_managed_file(&rewrite_script_source, &target) {
                Ok(ManagedCopyStatus::Copied) => {
                    status_parts.push("keel-cursor.sh copied".to_string())
                }
                Ok(ManagedCopyStatus::AlreadyCurrent) => {
                    status_parts.push("keel-cursor.sh already current".to_string())
                }
                Ok(ManagedCopyStatus::PreservedCustom) => {
                    status_parts.push("keel-cursor.sh skipped (user-customized)".to_string())
                }
                Err(error) => status_parts.push(format!("keel-cursor.sh copy failed ({error})")),
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
                "type": "stdio",
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
            Err(error) => status_parts.push(format!("MCP skipped ({error})")),
        }
    }

    if status_parts.is_empty() {
        Some("nothing to copy".to_string())
    } else {
        Some(status_parts.join("; "))
    }
}

/// Grok loads global hooks from `~/.grok/hooks/*.json` and, by default, merges
/// the managed hooks from `~/.claude/settings.json`. Reuse that compatible
/// source when it is current so every lifecycle event fires exactly once.
/// When Claude hook compatibility is explicitly disabled, install the native
/// Grok hook file so PreToolUse deny (Iron Law + Anvil) still fires.
/// Stop must call `keel hook stop` (silent). Grok treats Stop additionalContext
/// as "keep going"; wiring Stop to post-tool-batch loops until the host cap.
pub(crate) fn maybe_wire_grok(claude_home: &Path, detected: bool) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    let home = match host_user_home(claude_home) {
        Some(path) => path,
        None => return Some("skipped (no home directory)".to_string()),
    };
    if !detected {
        return Some("skipped (not detected)".to_string());
    }
    let grok_dir = grok_config_home(&home);
    let target = grok_dir.join("hooks").join("keel.json");
    let binary = installed_executable_path(claude_home);
    let config_path = grok_dir.join("config.toml");
    let use_claude_compat = grok_claude_hook_compatibility_enabled(&config_path)
        && claude_compatible_hooks_are_current(claude_home, &binary);
    let hook_status = if use_claude_compat {
        match std::fs::remove_file(&target) {
            Ok(()) => "hooks via Claude compatibility (duplicate native hooks removed)".to_string(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "hooks via Claude compatibility".to_string()
            }
            Err(error) => return Some(format!("duplicate native hook removal skipped ({error})")),
        }
    } else {
        let hooks_dir = target
            .parent()
            .expect("Grok hook target always has a parent directory");
        if let Err(error) = std::fs::create_dir_all(hooks_dir) {
            return Some(format!("hooks dir skipped ({error})"));
        }
        let payload = grok_hooks_payload(&binary);
        let rendered = match serde_json::to_string_pretty(&payload) {
            Ok(text) => text,
            Err(error) => return Some(format!("serialize skipped ({error})")),
        };
        match write_text(&target, &rendered) {
            Ok(()) => format!("native hooks -> {}", display_path(&target)),
            Err(error) => return Some(format!("hooks write skipped ({error})")),
        }
    };
    let mcp_status = match ensure_codex_native_mcp(&config_path, &binary) {
        Ok(CodexNativeMcpResult::Added) => "native MCP registered".to_string(),
        Ok(CodexNativeMcpResult::Updated) => "native MCP command updated".to_string(),
        Ok(CodexNativeMcpResult::AlreadyCurrent) => "native MCP already current".to_string(),
        Err(error) => format!("native MCP skipped ({error})"),
    };
    Some(format!("{hook_status}; {mcp_status}"))
}

pub(crate) fn grok_claude_hook_compatibility_enabled(config_path: &Path) -> bool {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|document| {
            document
                .get("compat")
                .and_then(|compat| compat.get("claude"))
                .and_then(|claude| claude.get("hooks"))
                .and_then(toml::Value::as_bool)
        })
        .unwrap_or(true)
}

pub(crate) fn claude_compatible_hooks_are_current(keel_home: &Path, binary: &Path) -> bool {
    let settings_path = crate::runtime::claude_engagement_home(keel_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let current = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let expected = crate::runner::hook_lifecycle::build_hooks_payload(&settings_path, binary)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    current.is_some() && current == expected
}

pub(crate) fn grok_hooks_are_effective(grok_home: &Path, keel_home: &Path, binary: &Path) -> bool {
    let native_hook = grok_home.join("hooks").join("keel.json");
    let compat_enabled = grok_claude_hook_compatibility_enabled(&grok_home.join("config.toml"));
    if compat_enabled && claude_compatible_hooks_are_current(keel_home, binary) {
        !native_hook.exists()
    } else {
        grok_hooks_are_current(&native_hook, binary)
    }
}

pub(crate) fn grok_hooks_payload(binary: &Path) -> serde_json::Value {
    let command =
        crate::runner::shell_rewrite::platform_default_command_for_executable_args(binary, "hook");
    serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": format!("{command} session-start"), "timeout": 10 }] }],
            "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": format!("{command} user-prompt-submit"), "timeout": 10 }] }],
            "PreToolUse": [{ "hooks": [{ "type": "command", "command": format!("{command} pre-tool-use"), "timeout": 10 }] }],
            "PostToolUse": [{ "hooks": [{ "type": "command", "command": format!("{command} post-tool-use"), "timeout": 10 }] }],
            "PostToolUseFailure": [{ "hooks": [{ "type": "command", "command": format!("{command} post-tool-use-failure"), "timeout": 10 }] }],
            "PreCompact": [{ "hooks": [{ "type": "command", "command": format!("{command} pre-compact"), "timeout": 10 }] }],
            "PostCompact": [{ "hooks": [{ "type": "command", "command": format!("{command} post-compact"), "timeout": 10 }] }],
            "SessionEnd": [{ "hooks": [{ "type": "command", "command": format!("{command} session-end"), "timeout": 10 }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": format!("{command} stop"), "timeout": 10 }] }]
        }
    })
}

pub(crate) fn grok_hooks_are_current(hook_path: &Path, binary: &Path) -> bool {
    std::fs::read_to_string(hook_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|value| value == grok_hooks_payload(binary))
}

pub(crate) fn grok_config_home(user_home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home.join(".grok"))
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
    let home = match host_user_home(claude_home) {
        Some(path) => path,
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
        match copy_managed_file(&extension_source, &target) {
            Ok(ManagedCopyStatus::Copied) => {
                status_parts.push(format!("keel-pi.ts -> {}", display_path(&target)))
            }
            Ok(ManagedCopyStatus::AlreadyCurrent) => {
                status_parts.push("keel-pi.ts already current".to_string())
            }
            Ok(ManagedCopyStatus::PreservedCustom) => {
                status_parts.push("keel-pi.ts skipped (user-customized)".to_string())
            }
            Err(error) => status_parts.push(format!("keel-pi.ts copy failed ({error})")),
        }
    }

    status_parts.push(copy_bridge_core(
        repository_root,
        &home.join(".pi").join("agent"),
    ));
    status_parts.push(copy_gateway_skill(
        repository_root,
        &home.join(".pi").join("agent").join("skills"),
    ));

    Some(status_parts.join("; "))
}

/// Wire Oh My Pi at its native user root. OMP intentionally uses a distinct
/// `~/.omp/agent` tree; treating the `omp` binary as legacy `pi` leaves the
/// extension, MCP server, instructions, and skills undiscoverable.
pub(crate) fn maybe_wire_omp(
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
    let home = match host_user_home(claude_home) {
        Some(path) => path,
        None => return Some("skipped (no home directory)".to_string()),
    };
    let omp_root = home.join(".omp").join("agent");
    let mut status_parts = Vec::new();

    let extension_source = repository_root.join("pi").join("keel-pi.ts");
    let extension_target = omp_root.join("extensions").join("keel-pi.ts");
    if extension_source.is_file() {
        match copy_managed_file(&extension_source, &extension_target) {
            Ok(ManagedCopyStatus::Copied) => {
                status_parts.push(format!("extension -> {}", display_path(&extension_target)))
            }
            Ok(ManagedCopyStatus::AlreadyCurrent) => {
                status_parts.push("extension already current".to_string())
            }
            Ok(ManagedCopyStatus::PreservedCustom) => {
                status_parts.push("extension preserved (user-customized)".to_string())
            }
            Err(error) => status_parts.push(format!("extension skipped ({error})")),
        }
    } else {
        status_parts.push("extension source absent".to_string());
    }
    status_parts.push(copy_bridge_core(repository_root, &omp_root));

    let binary = installed_executable_path(claude_home);
    let mcp_path = omp_root.join("mcp.json");
    let mcp_entry = serde_json::json!({
        "type": "stdio",
        "command": display_path(&binary),
        "args": ["mcp", "serve"]
    });
    match merge_json_mcp(&mcp_path, "mcpServers", "keel", &mcp_entry, None) {
        Ok(JsonMcpMergeResult::Added) => {
            status_parts.push(format!("MCP registered in {}", display_path(&mcp_path)))
        }
        Ok(JsonMcpMergeResult::AlreadyCurrent) => {
            status_parts.push("MCP already current".to_string())
        }
        Err(error) => status_parts.push(format!("MCP skipped ({error})")),
    }
    match sync_host_agents_md(&omp_root.join("AGENTS.md"), "Oh My Pi") {
        Ok(status) => status_parts.push(status),
        Err(error) => status_parts.push(format!("AGENTS.md skipped ({error})")),
    }
    status_parts.push(copy_gateway_skill(
        repository_root,
        &omp_root.join("skills"),
    ));
    Some(status_parts.join("; "))
}

fn json_object_child_mut<'a>(
    parent: &'a mut serde_json::Value,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, String> {
    if parent.get(key).is_none() {
        parent[key] = serde_json::json!({});
    }
    parent
        .get_mut(key)
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("{key} is not an object"))
}

fn zcode_hook_group(binary: &Path, subcommand: &str) -> serde_json::Value {
    let status_message = format!("Keel managed lifecycle hook: {subcommand}");
    serde_json::json!({
        "matcher": "*",
        "hooks": [{
            "type": "process",
            "command": display_path(binary),
            "args": ["hook", subcommand],
            "enabled": true,
            "timeoutMs": 10000,
            "statusMessage": status_message
        }]
    })
}

fn upsert_zcode_hook(
    events: &mut serde_json::Map<String, serde_json::Value>,
    event: &str,
    binary: &Path,
    subcommand: &str,
) -> Result<(), String> {
    let value = events
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    let entries = value
        .as_array_mut()
        .ok_or_else(|| format!("hooks.events.{event} is not an array"))?;
    let desired = zcode_hook_group(binary, subcommand);
    let desired_status = format!("Keel managed lifecycle hook: {subcommand}");
    if let Some(existing) = entries.iter_mut().find(|entry| {
        entry
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("statusMessage")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| {
                            status == desired_status || status == "Keel managed lifecycle hook"
                        })
                })
            })
    }) {
        *existing = desired;
    } else {
        entries.push(desired);
    }
    Ok(())
}

fn merge_zcode_config(path: &Path, binary: &Path) -> Result<String, String> {
    let original = crate::runtime::read_text_if_exists(path).unwrap_or_default();
    let mut document: serde_json::Value = if original.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(original.strip_prefix('\u{feff}').unwrap_or(&original))
            .map_err(|error| format!("parse {}: {error}", display_path(path)))?
    };
    if !document.is_object() {
        return Err("root is not an object".to_string());
    }

    {
        let mcp = json_object_child_mut(&mut document, "mcp")?;
        let servers_value = mcp
            .entry("servers".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let servers = servers_value
            .as_object_mut()
            .ok_or("mcp.servers is not an object")?;
        let keel_value = servers
            .entry("keel".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let keel = keel_value
            .as_object_mut()
            .ok_or("mcp.servers.keel is not an object")?;
        keel.insert(
            "command".to_string(),
            serde_json::Value::String(display_path(binary)),
        );
        keel.insert("args".to_string(), serde_json::json!(["mcp", "serve"]));
    }

    {
        let hooks = json_object_child_mut(&mut document, "hooks")?;
        if !hooks.contains_key("enabled") {
            hooks.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
        let events_value = hooks
            .entry("events".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let events = events_value
            .as_object_mut()
            .ok_or("hooks.events is not an object")?;
        for (event, subcommand) in [
            ("SessionStart", "session-start"),
            ("UserPromptSubmit", "user-prompt-submit"),
            ("PreToolUse", "pre-tool-use"),
            ("PostToolUse", "post-tool-use"),
            ("PostToolUseFailure", "post-tool-use-failure"),
            ("Stop", "stop"),
        ] {
            upsert_zcode_hook(events, event, binary, subcommand)?;
        }
        // ZCode has no SessionEnd, so Stop runs completion before session capture and learning.
        upsert_zcode_hook(events, "Stop", binary, "session-end")?;
    }

    let rendered = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("serialize {}: {error}", display_path(path)))?;
    if rendered == original.trim_end() {
        return Ok("config already current".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    write_text(path, &rendered)?;
    Ok(format!("config updated at {}", display_path(path)))
}

pub(crate) fn maybe_wire_zcode(
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
    let home = match host_user_home(claude_home) {
        Some(path) => path,
        None => return Some("skipped (no home directory)".to_string()),
    };
    let zcode_root = home.join(".zcode");
    let binary = installed_executable_path(claude_home);
    let mut status_parts = Vec::new();
    match merge_zcode_config(&zcode_root.join("cli").join("config.json"), &binary) {
        Ok(status) => status_parts.push(status),
        Err(error) => status_parts.push(format!("config skipped ({error})")),
    }
    match sync_host_agents_md(&zcode_root.join("AGENTS.md"), "ZCode") {
        Ok(status) => status_parts.push(status),
        Err(error) => status_parts.push(format!("AGENTS.md skipped ({error})")),
    }
    status_parts.push(copy_gateway_skill(
        repository_root,
        &zcode_root.join("skills"),
    ));
    Some(status_parts.join("; "))
}

const ANTIGRAVITY_ADAPTER_FILE: &str = "keel-antigravity.js";

fn antigravity_hook_command(event: &str) -> String {
    // why: Antigravity cwd is the hooks.json dir and does not strip quotes, so a
    // quoted absolute path becomes pluginDir\"C:\...\js" (MODULE_NOT_FOUND).
    format!("node {ANTIGRAVITY_ADAPTER_FILE} {event}")
}

pub(crate) fn antigravity_hooks_payload() -> serde_json::Value {
    serde_json::json!({
        "keel": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{"type": "command", "command": antigravity_hook_command("pre-tool-use"), "timeout": 10}]
            }],
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [{"type": "command", "command": antigravity_hook_command("post-tool-use"), "timeout": 10}]
            }],
            "PreInvocation": [
                {"type": "command", "command": antigravity_hook_command("pre-invocation"), "timeout": 10}
            ],
            "Stop": [
                {"type": "command", "command": antigravity_hook_command("stop"), "timeout": 10}
            ]
        }
    })
}

fn wire_antigravity_plugin(
    repository_root: &Path,
    claude_home: &Path,
    plugin: &Path,
) -> Vec<String> {
    let adapter_source = repository_root
        .join("antigravity")
        .join(ANTIGRAVITY_ADAPTER_FILE);
    let adapter_target = plugin.join(ANTIGRAVITY_ADAPTER_FILE);
    let mut status_parts = Vec::new();
    if adapter_source.is_file() {
        match copy_managed_file(&adapter_source, &adapter_target) {
            Ok(ManagedCopyStatus::Copied) => status_parts.push("adapter copied".to_string()),
            Ok(ManagedCopyStatus::AlreadyCurrent) => {
                status_parts.push("adapter already current".to_string())
            }
            Ok(ManagedCopyStatus::PreservedCustom) => {
                status_parts.push("adapter preserved (user-customized)".to_string())
            }
            Err(error) => status_parts.push(format!("adapter skipped ({error})")),
        }
    } else {
        status_parts.push("adapter source absent".to_string());
    }

    let binary = installed_executable_path(claude_home);
    let manifest = serde_json::to_string_pretty(&serde_json::json!({
        "name": "keel",
        "description": "Keel managed integration for Antigravity"
    }))
    .unwrap_or_default();
    let mcp = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "keel": {
                "command": display_path(&binary),
                "args": ["mcp", "serve"]
            }
        }
    }))
    .unwrap_or_default();
    let hooks = serde_json::to_string_pretty(&antigravity_hooks_payload()).unwrap_or_default();
    let rule = "# Keel operating contract\n\nThis rule is managed by Keel. Before changing code, config, or architecture, use the Keel MCP tools to read the system map and owning files, route and load relevant skills, compile and dry-run Anvil, preserve existing data, trace root cause, test, and review. Trust current repository evidence and official documentation over model memory.\n";
    for (path, content, marker) in [
        (
            plugin.join("plugin.json"),
            manifest.as_str(),
            "Keel managed integration",
        ),
        (plugin.join("mcp_config.json"), mcp.as_str(), "\"keel\""),
        (plugin.join("hooks.json"), hooks.as_str(), "\"keel\""),
        (
            plugin.join("rules").join("keel.md"),
            rule,
            "managed by Keel",
        ),
    ] {
        match write_generated_managed_file(&path, content, marker) {
            Ok(ManagedCopyStatus::Copied) => status_parts.push(format!(
                "{} written",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
            Ok(ManagedCopyStatus::AlreadyCurrent) => {}
            Ok(ManagedCopyStatus::PreservedCustom) => status_parts.push(format!(
                "{} preserved (user-customized)",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
            Err(error) => status_parts.push(format!(
                "{} skipped ({error})",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }
    status_parts.push(copy_gateway_skill(repository_root, &plugin.join("skills")));
    status_parts
}

pub(crate) fn maybe_wire_antigravity(
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
    let home = match host_user_home(claude_home) {
        Some(path) => path,
        None => return Some("skipped (no home directory)".to_string()),
    };
    let gemini = home.join(".gemini");
    let ide_root = gemini.join("config");
    let cli_root = gemini.join("antigravity-cli");
    let ide_present = ide_root.is_dir() || which::which("antigravity").is_ok();
    let cli_present = cli_root.is_dir() || which::which("agy").is_ok();
    let mut targets = Vec::new();
    if ide_present || !cli_present {
        targets.push((
            "IDE",
            ide_root.clone(),
            ide_root.join("plugins").join("keel"),
        ));
    }
    if cli_present {
        targets.push((
            "CLI",
            cli_root.clone(),
            cli_root.join("plugins").join("keel"),
        ));
    }
    let mut status_parts = Vec::new();
    let binary = installed_executable_path(claude_home);
    let mcp_entry = serde_json::json!({
        "command": display_path(&binary),
        "args": ["mcp", "serve"]
    });
    for (label, root, plugin) in targets {
        let mut details = wire_antigravity_plugin(repository_root, claude_home, &plugin);
        let global_mcp = root.join("mcp_config.json");
        let global_mcp_status =
            match merge_json_mcp(&global_mcp, "mcpServers", "keel", &mcp_entry, None) {
                Ok(JsonMcpMergeResult::Added) => {
                    format!("global MCP registered in {}", display_path(&global_mcp))
                }
                Ok(JsonMcpMergeResult::AlreadyCurrent) => "global MCP already current".to_string(),
                Err(error) => format!("global MCP skipped ({error})"),
            };
        details.push(global_mcp_status);
        status_parts.push(format!("{label}: {}", details.join("; ")));
    }
    match sync_host_agents_md(&home.join(".gemini").join("GEMINI.md"), "Antigravity") {
        Ok(status) => status_parts.push(status),
        Err(error) => status_parts.push(format!("GEMINI.md skipped ({error})")),
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
    let home: PathBuf = match host_user_home(claude_home) {
        Some(path) => path,
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
            match copy_managed_file(&mod_source, &target) {
                Ok(ManagedCopyStatus::Copied) => {
                    status_parts.push(format!("keel-cmdc.ts -> {}", display_path(&target)))
                }
                Ok(ManagedCopyStatus::AlreadyCurrent) => {
                    status_parts.push("keel-cmdc.ts already current".to_string())
                }
                Ok(ManagedCopyStatus::PreservedCustom) => {
                    status_parts.push("keel-cmdc.ts skipped (user-customized)".to_string())
                }
                Err(error) => status_parts.push(format!("keel-cmdc.ts copy failed ({error})")),
            }
        }
    } else {
        status_parts.push("mod source absent".to_string());
    }
    status_parts.push(copy_bridge_core(
        repository_root,
        &home.join(".commandcode"),
    ));

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
    let home: PathBuf = match host_user_home(claude_home) {
        Some(path) => path,
        None => return Some("no home directory".to_string()),
    };
    let plugin_target = home.join(".codex").join("plugins").join("keel");
    if let Err(error) = std::fs::create_dir_all(&plugin_target) {
        return Some(format!("plugin dir failed ({error})"));
    }
    let mut copied = 0;
    let mut preserved = 0;
    let mut mcp_copy_status = None;
    for entry in [
        "hooks/hooks.json",
        "keel-codex.ts",
        "keel-codex.js",
        ".codex-plugin/plugin.json",
        ".mcp.json",
        "config.toml",
        "task-context.template.md",
        "agents/planner.toml",
        "agents/code-explorer.toml",
        "agents/implementer.toml",
        "agents/reviewer.toml",
        "agents/pusher.toml",
    ] {
        let source = codex_source_dir.join(entry);
        let target = plugin_target.join(entry);
        if source.is_file() {
            match copy_managed_file(&source, &target) {
                Ok(status @ ManagedCopyStatus::Copied)
                | Ok(status @ ManagedCopyStatus::AlreadyCurrent) => {
                    if status == ManagedCopyStatus::Copied {
                        copied += 1;
                    }
                    if entry == ".mcp.json" {
                        mcp_copy_status = Some(status);
                    }
                }
                Ok(ManagedCopyStatus::PreservedCustom) => {
                    preserved += 1;
                    if entry == ".mcp.json" {
                        mcp_copy_status = Some(ManagedCopyStatus::PreservedCustom);
                    }
                }
                Err(_) => {}
            }
        }
    }
    // Also copy custom agents directly into ~/.codex/agents/ for native discovery.
    let codex_agents_source = codex_source_dir.join("agents");
    let codex_agents_target = home.join(".codex").join("agents");
    if codex_agents_source.is_dir() {
        let _ = std::fs::create_dir_all(&codex_agents_target);
        for agent_file in [
            "planner.toml",
            "code-explorer.toml",
            "implementer.toml",
            "reviewer.toml",
            "pusher.toml",
        ] {
            let src = codex_agents_source.join(agent_file);
            let tgt = codex_agents_target.join(agent_file);
            if src.is_file() {
                match copy_managed_file(&src, &tgt) {
                    Ok(ManagedCopyStatus::Copied) => {
                        copied += 1;
                    }
                    Ok(ManagedCopyStatus::PreservedCustom) => {
                        preserved += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    // Codex resolves the MCP `command` via PATH only. Rewrite the shipped
    // entry after an owned copy; never mutate a conflicting user file.
    let mcp_target = plugin_target.join(".mcp.json");
    let binary = installed_executable_path(claude_home);
    let mcp_status = match mcp_copy_status {
        Some(ManagedCopyStatus::PreservedCustom) => "MCP preserved (user-customized)".to_string(),
        Some(ManagedCopyStatus::Copied) | Some(ManagedCopyStatus::AlreadyCurrent) => {
            if mcp_target.is_file() {
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
            }
        }
        None => "MCP absent".to_string(),
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
        let marketplace_ready = match merge_codex_marketplace(&marketplace_path) {
            Ok(CodexMarketplaceResult::Added) => {
                wire_status.push(format!(
                    "marketplace entry added in {}",
                    display_path(&marketplace_path)
                ));
                true
            }
            Ok(CodexMarketplaceResult::AlreadyCurrent) => {
                wire_status.push("marketplace entry already current".to_string());
                true
            }
            Ok(CodexMarketplaceResult::Updated) => {
                wire_status.push(format!(
                    "marketplace entry updated in {}",
                    display_path(&marketplace_path)
                ));
                true
            }
            Err(error) => {
                wire_status.push(format!("marketplace skipped ({error})"));
                false
            }
        };
        let codex_config = home_dir.join(".codex").join("config.toml");
        let plugin_enabled = match ensure_codex_plugin_enabled(&codex_config) {
            Ok(CodexEnableResult::Added) => {
                wire_status.push(format!("plugin enabled in {}", display_path(&codex_config)));
                true
            }
            Ok(CodexEnableResult::AlreadyEnabled) => {
                wire_status.push("plugin already enabled".to_string());
                true
            }
            Ok(CodexEnableResult::UnchangedDisabled) => {
                wire_status.push("plugin disabled by user (enable via Codex /plugins)".to_string());
                false
            }
            Err(error) => {
                wire_status.push(format!("enablement skipped ({error})"));
                false
            }
        };
        match ensure_codex_agents_enabled(&codex_config) {
            Ok(CodexEnableResult::Added) => {
                wire_status.push("codex agents configuration enabled".to_string());
            }
            Ok(CodexEnableResult::AlreadyEnabled) => {}
            Ok(CodexEnableResult::UnchangedDisabled) => {
                wire_status.push("codex agents disabled by user in config.toml".to_string());
            }
            Err(error) => {
                wire_status.push(format!("codex agents config skipped ({error})"));
            }
        }
        if marketplace_ready && plugin_enabled {
            if codex_plugin_installation(home_dir) == CodexPluginInstallation::Current {
                wire_status.push("plugin installation already current".to_string());
            } else {
                match install_codex_plugin(home_dir) {
                    Ok(status) => wire_status.push(status),
                    Err(error) => wire_status.push(format!(
                        "plugin installation pending ({error}); run `codex plugin add keel@personal-keel --json`"
                    )),
                }
            }
        }
        // Native MCP registration is retained as a direct, cross-platform tool
        // path even when the plugin hook cache is unavailable or awaiting trust.
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
        // Always-on contract while plugin hooks await explicit user review.
        let codex_agents = home_dir.join(".codex").join("AGENTS.md");
        match sync_codex_agents_md(&codex_agents) {
            Ok(status) => wire_status.push(status),
            Err(error) => wire_status.push(format!("AGENTS.md skipped ({error})")),
        }
    }

    Some(format!(
        "{copied} files ({preserved} preserved) -> {}; {mcp_status}; {}",
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

    let home = match host_user_home(claude_home) {
        Some(path) => path,
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
        Err(error) => Some(format!("MCP skipped ({error})")),
    }
}
