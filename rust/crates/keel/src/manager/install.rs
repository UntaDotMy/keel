//! Purpose: Install, sync, update, and uninstall logic for keel manager.
//! Caller: commands.rs via run_install_command, run_update_command, run_uninstall_command.
//! Dependencies: std::fs, std::io, std::path, std::process, std::thread, std::time, keel_platform, crate::args, crate::runtime.
//! Main Functions: install_from_flags, install_from_paths, sync_root_files, sync_skills, sync_shared_resources, sync_agents, sync_subagent_definitions, sync_commands, publish_native_executable, run_update_command, run_uninstall_command.
//! Side Effects: Copies managed skill-pack files, writes harness home config/state, publishes the Rust binary, runs git commands, and removes managed files during uninstall.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use keel_platform::detect_current_target;

use crate::args::FlagSet;
use crate::runtime::{
    agent_profiles_directory, agents_directory, commands_directory, config_path,
    discover_repository_layout, display_path, executable_file_name, git_short_head,
    installed_executable_path, is_default_keel_home, is_standard_keel_home,
    legacy_claude_executable_path, legacy_state_directory, read_text_if_exists,
    remove_path_if_exists, repository_layout_is_complete, resolve_claude_home,
    resolve_repository_root, run_command, skills_directory, state_directory,
    update_cache_directory, write_lines, write_text, RepositoryLayout, SKILL_SYNC_DIRECTORIES,
};

use super::agent_config::{parse_agent_config, render_agent_toml, unix_timestamp};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlatformName {
    Opencode,
    Codex,
    Pi,
    Cursor,
    Cowork,
    Commandcode,
}

impl PlatformName {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "opencode" => Some(Self::Opencode),
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            "cowork" | "desktop" => Some(Self::Cowork),
            "commandcode" | "cmdc" | "command-code" => Some(Self::Commandcode),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct InstallOverrides {
    pub force: BTreeSet<PlatformName>,
    pub skip: BTreeSet<PlatformName>,
}

fn parse_overrides(with: &str, without: &str) -> InstallOverrides {
    let mut overrides = InstallOverrides::default();
    for name in with.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(p) = PlatformName::parse(name) {
            overrides.force.insert(p);
        }
    }
    for name in without.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(p) = PlatformName::parse(name) {
            overrides.skip.insert(p);
        }
    }
    overrides
}

fn apply_overrides(
    mut detected: super::platform_detect::DetectedPlatforms,
    overrides: &InstallOverrides,
) -> super::platform_detect::DetectedPlatforms {
    if overrides.force.contains(&PlatformName::Opencode) {
        detected.opencode = true;
    }
    if overrides.force.contains(&PlatformName::Codex) {
        detected.codex = true;
    }
    if overrides.force.contains(&PlatformName::Pi) {
        detected.pi = true;
    }
    if overrides.force.contains(&PlatformName::Cursor) {
        detected.cursor = true;
    }
    if overrides.force.contains(&PlatformName::Cowork) {
        detected.cowork = true;
    }
    if overrides.force.contains(&PlatformName::Commandcode) {
        detected.commandcode = true;
    }
    if overrides.skip.contains(&PlatformName::Opencode) {
        detected.opencode = false;
    }
    if overrides.skip.contains(&PlatformName::Codex) {
        detected.codex = false;
    }
    if overrides.skip.contains(&PlatformName::Pi) {
        detected.pi = false;
    }
    if overrides.skip.contains(&PlatformName::Cursor) {
        detected.cursor = false;
    }
    if overrides.skip.contains(&PlatformName::Cowork) {
        detected.cowork = false;
    }
    if overrides.skip.contains(&PlatformName::Commandcode) {
        detected.commandcode = false;
    }
    detected
}

/// True when `home` is a real user-level install root: the legacy `.claude`
/// home or the host-neutral `.keel` home. Every host-wiring gate keys off
/// this so adapters register for both layouts; non-standard roots (test temp
/// dirs, custom `--claude-home` overrides) keep wiring hermetic.
fn is_standard_home(home: &Path) -> bool {
    home.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".claude" || name == crate::runtime::KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false)
}

#[derive(Default)]
pub struct InstallSummary {
    pub synced_skills: usize,
    pub synced_agents: usize,
    pub synced_subagent_definitions: usize,
    pub synced_commands: usize,
    pub synced_root_files: usize,
    pub synced_shared_resources: usize,
    pub removed_stale_files: usize,
    pub removed_executable_orphans: usize,
    pub published_executable: bool,
    /// Human-readable outcome of the MCP server registration step, or `None`
    /// when registration was skipped (non-standard `--claude-home`) or failed
    /// non-fatally. Registration never blocks an install.
    pub mcp_registration: Option<String>,
    /// Human-readable outcome of the harness lifecycle hook installation
    /// step, or `None` when it was skipped (non-standard `--claude-home`) or
    /// failed non-fatally. Like MCP registration, hook install never blocks an
    /// install — but without it, a manual or plugin-only user gets skills+MCP
    /// and no hooks, so the SessionStart bootstrap and per-prompt routing never
    /// fire. Folding it in here makes hooks load-bearing on every install path,
    /// not only the one-line bootstrap scripts.
    pub hooks_installation: Option<String>,
    /// Human-readable outcome of writing the always-on operating contract into
    /// the user-global `~/.claude/CLAUDE.md`, or `None` when skipped
    /// (non-standard `--claude-home`). This file is loaded natively into every
    /// session as user memory — the one channel that does not depend on hook
    /// `additionalContext` reaching the model — so the iron law and tool/skill
    /// contract land even when the hook channel is dropped by a gateway/proxy.
    pub user_claude_md: Option<String>,
    /// Human-readable outcome of the OpenCode plugin-path / MCP-registration
    /// step, or `None` when skipped (non-standard `--claude-home`). Best-effort:
    /// a failure is reported in the summary but never fails the install.
    pub opencode_wiring: Option<String>,
    /// Human-readable outcome of copying the `.cursorrules` file into the
    /// project root during install, or `None` when skipped. Best-effort.
    pub cursor_wiring: Option<String>,
    /// Human-readable outcome of copying the Codex adapter files into the
    /// project during install, or `None` when skipped. Best-effort.
    pub codex_wiring: Option<String>,
    /// Human-readable outcome of copying the Pi Agent adapter files into the
    /// project root during install, or `None` when skipped. Best-effort.
    pub pi_wiring: Option<String>,
    /// Human-readable outcome of wiring the Command Code (cmdc) mod into
    /// `~/.commandcode/mods/` + MCP registration, or `None` when skipped.
    /// Best-effort.
    pub commandcode_wiring: Option<String>,
    /// Human-readable outcome of installing the Cowork (Claude Desktop) plugin
    /// files, or `None` when skipped. Best-effort.
    pub cowork_wiring: Option<String>,
    /// Writes `~/.grok/hooks/keel.json` so Grok PreToolUse fires natively.
    pub grok_wiring: Option<String>,
    /// Human-readable outcome of migrating keel-owned data out of a legacy
    /// `~/.claude` install into the host-neutral root, or `None` when there
    /// was nothing to migrate. Data-preserving by construction.
    pub migration_report: Option<String>,
    /// Human-readable outcome of putting the keel home on the user PATH so
    /// every shell and host can invoke `keel` without a full path.
    pub path_wiring: Option<String>,
}

struct FileTracker<'a> {
    claude_home: &'a Path,
    files: BTreeSet<String>,
}

impl<'a> FileTracker<'a> {
    fn new(claude_home: &'a Path) -> Self {
        Self {
            claude_home,
            files: BTreeSet::new(),
        }
    }

    fn record(&mut self, target: &Path) {
        if let Ok(relative) = target.strip_prefix(self.claude_home) {
            let normalized = relative.to_string_lossy().replace('\\', "/");
            if !normalized.is_empty() {
                self.files.insert(normalized);
            }
        }
    }
}

fn read_inventory_set(path: &Path) -> BTreeSet<String> {
    super::verify::read_inventory_lines(path)
        .into_iter()
        .collect()
}

pub fn install_from_flags(
    build_version: &str,
    flag_set: &FlagSet,
) -> Result<InstallSummary, String> {
    let repository_root = resolve_install_repository_root(flag_set.string_value("repo-root"))?;
    let claude_home = resolve_claude_home(flag_set.string_value("claude-home"))?;
    let overrides = parse_overrides(
        flag_set.string_value("with"),
        flag_set.string_value("without"),
    );
    let no_purge = flag_set.bool_value("no-purge");
    let purge_stale = flag_set.bool_value("purge-stale");
    install_from_paths(
        build_version,
        &repository_root,
        &claude_home,
        &overrides,
        install_purge_stale_enabled(no_purge, purge_stale),
    )
}

pub fn resolve_install_repository_root(flag_value: &str) -> Result<PathBuf, String> {
    if !flag_value.trim().is_empty() {
        return resolve_repository_root(flag_value);
    }
    let candidates = [
        std::env::current_dir().ok(),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
    ];
    resolve_install_repository_root_from_candidates(&candidates)
}

pub fn resolve_install_repository_root_from_candidates(
    candidates: &[Option<PathBuf>],
) -> Result<PathBuf, String> {
    for candidate in candidates.iter().flatten() {
        if repository_layout_is_complete(candidate) {
            return Ok(candidate.clone());
        }
    }
    Err("Repository root not found. Use --repo-root to specify the path.".to_string())
}

pub fn install_from_paths(
    build_version: &str,
    repository_root: &Path,
    claude_home: &Path,
    overrides: &InstallOverrides,
    purge_stale: bool,
) -> Result<InstallSummary, String> {
    // Two-home split: `claude_home` is the neutral root (~/.keel); the
    // engagement home (~/.claude) is where the harness reads its artifacts.
    let engagement_home = crate::runtime::claude_engagement_home(claude_home);
    let layout = discover_repository_layout(repository_root)?;
    // Migrate BEFORE scaffolding: ensure_*_directories would create empty root
    // dirs that migration mistakes for existing destinations.
    fs::create_dir_all(claude_home)
        .map_err(|error| format!("create {}: {error}", display_path(claude_home)))?;
    // Move keel-owned data out of a legacy ~/.claude install into the neutral
    // root; data-preserving, keel-owned names only, never overwrites.
    let migration_report = migrate_from_legacy_claude_home(claude_home, &engagement_home);
    migrate_legacy_state_directory(claude_home);
    if engagement_home != claude_home {
        migrate_legacy_state_directory(&engagement_home);
    }
    ensure_claude_home_directories(claude_home)?;
    ensure_claude_home_directories(&engagement_home)?;
    remove_deprecated_config_keys(claude_home)?;

    let previous_files = read_inventory_set(&managed_files_inventory_path(claude_home));
    let previous_skills = read_inventory_set(&managed_skills_inventory_path(claude_home));
    let previous_shared_resources =
        read_inventory_set(&managed_shared_resources_inventory_path(claude_home));
    let mut tracker = FileTracker::new(&engagement_home);

    let synced_root_files = sync_root_files(&layout, &engagement_home, &mut tracker)?;
    let synced_skills = sync_skills(&layout, &engagement_home, &mut tracker)?;
    let synced_shared_resources = sync_shared_resources(&layout, &engagement_home, &mut tracker)?;
    let synced_agents = sync_agents(&layout, &engagement_home, &mut tracker)?;
    let synced_subagent_definitions =
        sync_subagent_definitions(&layout, &engagement_home, &mut tracker)?;
    let synced_commands = sync_commands(&layout, &engagement_home, &mut tracker)?;
    // why: native install previously skipped output-styles, so a native-install
    // user never got them (they shipped only via the plugin manifest). Deliver them
    // to ~/.claude/output-styles/; the tracker records each file so uninstall
    // reverses it like every other managed artifact.
    let _synced_output_styles = sync_output_styles(&layout, &engagement_home, &mut tracker)?;

    // Deleted first-party surfaces (sprint / user-story / workflow) always
    // leave the engagement home, even when --purge-stale is off. Otherwise an
    // old install keeps teaching a loop that no longer exists.
    let mut removed_stale_files = remove_dropped_first_party_artifacts(claude_home, Some(&layout));
    removed_stale_files += remove_orphans(
        &engagement_home,
        &previous_files,
        &previous_skills,
        &previous_shared_resources,
        &layout,
        &tracker,
        purge_stale,
    )?;

    write_managed_config(claude_home)?;
    let published_executable = publish_native_executable(repository_root, claude_home)?;
    // Sweep both homes: the legacy binary parks as a `.stale-*` sibling in
    // ~/.claude (Windows cannot delete a mapped image), so sweep there too.
    let mut removed_executable_orphans = remove_executable_orphans(claude_home)?;
    if engagement_home != claude_home {
        removed_executable_orphans += remove_executable_orphans(&engagement_home).unwrap_or(0);
    }
    write_install_metadata(build_version, repository_root, claude_home)?;
    write_inventories(&layout, claude_home, &tracker)?;
    remove_update_temp_trees(claude_home, &engagement_home);
    // Put the keel home on PATH: best-effort, idempotent, and ONLY for the
    // user's default `~/.keel`. The previous guard used `is_standard_keel_home`
    // (basename-only), so test fixtures like `<tmp>/keel-home-split-<pid>/.keel`
    // passed it and every test install appended a dead temp dir to the real
    // user PATH. `is_default_keel_home` compares against the resolved user home,
    // so fixtures never touch PATH. When we DO install to the default home, also
    // sweep any stale `keel-home-split-*\.keel` entries a buggy older build left.
    let path_wiring = if published_executable && is_default_keel_home(claude_home) {
        purge_stale_temp_keel_path_entries();
        Some(ensure_keel_home_on_path(claude_home))
    } else {
        None
    };
    let mcp_registration = maybe_register_mcp_server(&engagement_home);
    let hooks_installation = maybe_install_hooks(&engagement_home, claude_home);
    let user_claude_md = maybe_sync_user_claude_md(&engagement_home);
    let detection_home = engagement_home.parent().unwrap_or(&engagement_home);
    let detected = super::platform_detect::PlatformDetector::new(detection_home).detect();
    let detected = apply_overrides(detected, overrides);
    let opencode_wiring = maybe_wire_opencode(repository_root, claude_home, detected.opencode);
    let cursor_wiring = maybe_wire_cursor(repository_root, claude_home, detected.cursor);
    let codex_wiring = maybe_wire_codex(repository_root, claude_home, detected.codex);
    let pi_wiring = maybe_wire_pi(repository_root, claude_home, detected.pi);
    let cowork_wiring = maybe_wire_cowork(repository_root, claude_home, detected.cowork);
    let commandcode_wiring =
        maybe_wire_commandcode(repository_root, claude_home, detected.commandcode);
    let grok_wiring = maybe_wire_grok(claude_home);
    Ok(InstallSummary {
        synced_skills,
        synced_agents,
        synced_subagent_definitions,
        synced_commands,
        synced_root_files,
        synced_shared_resources,
        removed_stale_files,
        removed_executable_orphans,
        published_executable,
        mcp_registration,
        hooks_installation,
        user_claude_md,
        opencode_wiring,
        cursor_wiring,
        codex_wiring,
        pi_wiring,
        commandcode_wiring,
        cowork_wiring,
        grok_wiring,
        migration_report,
        path_wiring,
    })
}

/// Register the `keel` MCP server in `~/.claude.json` during install,
/// but only when the target harness home is a real `.claude` directory under a
/// user home. The integration test suite installs into throwaway
/// `--claude-home` directories that are NOT named `.claude`; auto-writing a
/// `.claude.json` beside each of those would (a) be meaningless and (b) race
/// parallel tests on a shared temp-dir parent. Guarding on the directory name
/// keeps real installs fully automatic while leaving tests hermetic. The
/// `repair` command registers unconditionally for explicit operator recovery.
///
/// Registration is best-effort: a failure is reported in the summary but never
/// fails the install (MCP is additive, not load-bearing for the skill pack).
fn maybe_register_mcp_server(claude_home: &Path) -> Option<String> {
    if !super::mcp_register::is_standard_claude_home(claude_home) {
        return None;
    }
    match super::mcp_register::register_mcp_server(claude_home) {
        Ok(super::mcp_register::McpRegistration::Added) => {
            Some("registered in ~/.claude.json".to_string())
        }
        Ok(super::mcp_register::McpRegistration::Updated) => {
            Some("updated in ~/.claude.json".to_string())
        }
        Ok(super::mcp_register::McpRegistration::AlreadyCurrent) => {
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
fn maybe_install_hooks(engagement_home: &Path, keel_home: &Path) -> Option<String> {
    if !is_standard_home(engagement_home) {
        return None;
    }
    let hook_path = engagement_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    // Point the hooks at the binary we just published into the keel home, not at
    // the currently-running executable (which during `update` is the freshly
    // built release artifact in the repo target dir, and during a release-bundle
    // install is the extracted temp binary). The published path is the stable
    // location the harness will invoke for the lifetime of the install.
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

    // Derive the home that owns THIS .claude from claude_home's parent, not from
    // the process environment. This keeps `cargo test` hermetic (a temp .claude
    // home wires into the temp parent, never the developer's real ~/.config) and
    // makes the wiring honest for non-default --claude-home installs.
    let home: PathBuf = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return Some("skipped (no home directory)".to_string()),
    };

    let plugin_dir = home.join(".config").join("opencode").join("plugins");
    if let Err(error) = std::fs::create_dir_all(&plugin_dir) {
        return Some(format!("plugin dir skipped ({error})"));
    }

    // Copy the bridge plugin source into the OpenCode plugins directory so the
    // bridge actually runs. Without this the dir + MCP entry exist but no plugin
    // file loads, and none of the lifecycle wiring fires.
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

    let mcp_status = match merge_opencode_mcp(&opencode_config_path, "keel", &mcp_entry) {
        Ok(OpencodeMcpResult::Added) => {
            format!("MCP registered in {}", display_path(&opencode_config_path))
        }
        Ok(OpencodeMcpResult::AlreadyCurrent) => "MCP already current".to_string(),
        Ok(OpencodeMcpResult::Updated) => {
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
    // MCP servers from this file (https://cursor.com/docs/mcp). Merge, never
    // clobber — preserve the user's other MCP servers. No alwaysLoad equivalent;
    // Cursor loads MCP servers on demand.
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
        // command; rewrite it to the absolute installed binary path exactly as
        // the codex path does (bare `keel` only works when PATH is wired).
        rewrite_mcp_entry_command(&mut mcp_entry, &display_path(&binary));
        match merge_cursor_mcp(&mcp_target, "keel", &mcp_entry) {
            Ok(CursorMcpResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(CursorMcpResult::AlreadyCurrent) => {
                status_parts.push("MCP already current".to_string())
            }
            Ok(CursorMcpResult::Updated) => {
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
        // Pi loads MCP config from ~/.pi/agent/mcp.json (global) or
        // .pi/mcp.json (project) — NOT ~/.config/mcp/mcp.json. See
        // https://pi.dev/docs/latest/extensions and the settings.md reference.
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
        // command; rewrite it to the absolute installed binary path exactly as
        // the codex path does (bare `keel` only works when PATH is wired).
        rewrite_mcp_entry_command(&mut mcp_entry, &display_path(&binary));
        match merge_pi_mcp(&mcp_target, "keel", &mcp_entry, &template_defaults) {
            Ok(PiMcpResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(PiMcpResult::AlreadyCurrent) => status_parts.push("MCP already current".to_string()),
            Ok(PiMcpResult::Updated) => {
                status_parts.push(format!("MCP updated in {}", display_path(&mcp_target)))
            }
            Err(error) => status_parts.push(format!("MCP skipped ({error})")),
        }
    }
    let extension_source = repository_root.join("pi").join("keel-pi.ts");
    if extension_source.is_file() {
        // Pi auto-discovers extensions from ~/.pi/agent/extensions/*.ts
        // (global) or .pi/extensions/*.ts (project) — NOT ~/.pi/extensions/.
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
        match merge_commandcode_mcp(&mcp_target, "keel", &mcp_entry) {
            Ok(CommandcodeMcpResult::Added) => {
                status_parts.push(format!("MCP registered in {}", display_path(&mcp_target)))
            }
            Ok(CommandcodeMcpResult::AlreadyCurrent) => {
                status_parts.push("MCP already current".to_string())
            }
            Ok(CommandcodeMcpResult::Updated) => {
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
    // uses a bare `keel`, which fails with "program not found" when
    // ~/.claude is not on PATH (the common case on Windows, where install
    // does not modify PATH). Rewrite the copied file's command to the
    // absolute binary path, mirroring how OpenCode/Cursor/pi template the
    // resolved path into their MCP config at install time.
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

/// Result of merging the keel entry into the personal Codex marketplace.
#[derive(Debug)]
enum CodexMarketplaceResult {
    Added,
    AlreadyCurrent,
    Updated,
}

/// Marketplace name for the personal keel catalog. Codex keys enabled plugins
/// as `<plugin>@<marketplace>` in config.toml, so this constant is part of the
/// enablement key and must stay stable across installs.
const CODEX_PERSONAL_MARKETPLACE_NAME: &str = "personal-keel";

/// The marketplace entry that makes ~/.codex/plugins/keel discoverable. The
/// shape (source/policy/category) follows the Codex marketplace schema; the
/// `~` in `path` is expanded by Codex itself.
fn codex_marketplace_entry() -> serde_json::Value {
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
fn merge_codex_marketplace(marketplace_path: &Path) -> Result<CodexMarketplaceResult, String> {
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
enum CodexEnableResult {
    Added,
    AlreadyEnabled,
    /// The user explicitly set `enabled = false`; install never overrides an
    /// intentional disable. Enable via Codex's `/plugins` UI or by editing the
    /// config key.
    UnchangedDisabled,
}

/// Result of registering the keel MCP server natively in Codex config.toml.
#[derive(Debug)]
enum CodexNativeMcpResult {
    Added,
    Updated,
    AlreadyCurrent,
}

/// The config.toml section Codex reads for this plugin's enablement:
/// `[plugins."keel@personal-keel"]` (plugin@marketplace).
const CODEX_PLUGIN_CONFIG_SECTION: &str = "[plugins.\"keel@personal-keel\"]";

/// The config.toml section for the native keel MCP server. A top-level
/// `[mcp_servers.<name>]` table is honored on every platform.
const CODEX_NATIVE_MCP_SECTION: &str = "[mcp_servers.keel]";

/// Register the keel MCP server directly in `~/.codex/config.toml`.
///
/// why: Codex on Windows does not load the MCP server a plugin bundles
/// (upstream openai/codex#26693), so the plugin path alone leaves MCP empty
/// there. The native `[mcp_servers.keel]` table works everywhere, so install
/// writes it deterministically alongside the plugin. The edit is string-
/// surgical (parse with `toml` to decide, then append or rewrite the section)
/// so comments, ordering, and unrelated keys survive untouched. Creates the
/// file when absent.
fn ensure_codex_native_mcp(
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
fn toml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Sentinels delimiting the keel-managed region inside the user-global
/// `~/.codex/AGENTS.md`, mirroring the `~/.claude/CLAUDE.md` sentinels so the
/// uninstall path can strip exactly what install wrote.
const MANAGED_CODEX_AGENTS_BEGIN: &str =
    "<!-- keel:begin (managed by keel install — edits inside this block are overwritten; edit outside it freely) -->";
const MANAGED_CODEX_AGENTS_END: &str = "<!-- keel:end -->";

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
fn sync_codex_agents_md(path: &Path) -> Result<String, String> {
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
fn merge_managed_region(existing: &str, block: &str, begin: &str, end: &str) -> String {
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
fn strip_managed_region(existing: &str, begin: &str, end: &str) -> Option<String> {
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
fn ensure_codex_plugin_enabled(config_path: &Path) -> Result<CodexEnableResult, String> {
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
fn remove_codex_marketplace_entry(marketplace_path: &Path) -> usize {
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

/// Top-level names under a legacy `~/.claude` home that keel owns (creates
/// and reads) and that the claude harness never reads. These move to the
/// host-neutral root during migration; harness-owned engagement surfaces stay.
const MIGRATION_DATA_NAMES: &[&str] = &[
    "working-briefs",
    "memories",
    "memory",
    "sprint",
    "state",
    // NOTE: `agent-profiles` is NOT migrated. Install re-syncs it into the
    // engagement home every run, so moving it would only churn.
    ".claude-skill-manager",
    "state",
    "cache",
    "workflow",
    "anvil",
    "raw-output",
    "config.toml",
    "command-compaction-events.jsonl",
    "recall-index.sqlite3",
];

/// Migrate keel-owned data out of a legacy `~/.claude` install into the
/// host-neutral root and remove the old binary placement. Runs on every
/// install/update; a no-op once the old home holds nothing keel-owned.
///
/// Data-preserving by construction: each name moves only when the
/// destination is absent (existing destination wins, never overwritten), and
/// a move failure degrades to "skipped", never an install error.
fn migrate_from_legacy_claude_home(keel_home: &Path, engagement_home: &Path) -> Option<String> {
    if !is_standard_keel_home(keel_home) || engagement_home == keel_home {
        return None;
    }
    let legacy = engagement_home;
    if !legacy.is_dir() {
        return None;
    }
    let mut moved = 0usize;
    let mut skipped = 0usize;
    for name in MIGRATION_DATA_NAMES {
        let source = legacy.join(name);
        if !source.exists() {
            continue;
        }
        let destination = keel_home.join(name);
        if destination.exists() && destination.is_dir() && source.is_dir() {
            // File-level merge: destination files win on exact-path conflicts;
            // everything else moves over, and nothing is ever deleted.
            let (merged, conflicts) = merge_tree_preserving(&source, &destination);
            moved += merged;
            if conflicts > 0 {
                skipped += 1;
            }
            if is_empty_directory(&source) {
                let _ = fs::remove_dir(&source);
            }
            continue;
        }
        if destination.exists() {
            // Type mismatch (file vs directory) or a destination file: never
            // overwrite; leave both copies for the operator.
            skipped += 1;
            continue;
        }
        if move_path_preserving(&source, &destination) {
            moved += 1;
        } else {
            skipped += 1;
        }
    }
    // SQLite WAL sidecars must travel with the database, or a mid-transaction
    // migration loses committed-but-unmerged rows.
    for suffix in ["-wal", "-shm"] {
        let source = legacy.join(format!("recall-index.sqlite3{suffix}"));
        if source.exists() {
            let destination = keel_home.join(format!("recall-index.sqlite3{suffix}"));
            if !destination.exists() && move_path_preserving(&source, &destination) {
                moved += 1;
            }
        }
    }
    let binary_outcome = remove_legacy_binary(keel_home);
    if moved == 0 && skipped == 0 && binary_outcome.is_empty() {
        return None;
    }
    let mut report = format!("moved {moved} item(s) from {}", display_path(legacy));
    if skipped > 0 {
        report.push_str(&format!(
            ", skipped {skipped} (destination exists or move failed)"
        ));
    }
    if !binary_outcome.is_empty() {
        report.push_str(&format!("; {binary_outcome}"));
    }
    Some(report)
}

/// True when `directory` exists, is a directory, and holds no entries.
fn is_empty_directory(directory: &Path) -> bool {
    directory.is_dir()
        && fs::read_dir(directory)
            .map(|entries| entries.count() == 0)
            .unwrap_or(false)
}

/// Merge a legacy directory tree into an existing destination directory
/// without ever deleting destination content. Returns `(moved, conflicts)`
/// where `moved` counts files/directories relocated and `conflicts` counts
/// exact-path collisions (a source and destination file with the same
/// relative path). Destination files win every conflict; conflicting source
/// files stay in place for the operator to reconcile.
fn merge_tree_preserving(source: &Path, destination: &Path) -> (usize, usize) {
    let mut moved = 0usize;
    let mut conflicts = 0usize;
    let Ok(entries) = fs::read_dir(source) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let child_source = entry.path();
        let child_destination = destination.join(entry.file_name());
        if child_source.is_dir() {
            if child_destination.is_dir() {
                // Recurse: merge nested directories level by level.
                let (child_moved, child_conflicts) =
                    merge_tree_preserving(&child_source, &child_destination);
                moved += child_moved;
                conflicts += child_conflicts;
                if is_empty_directory(&child_source) {
                    let _ = fs::remove_dir(&child_source);
                }
            } else if child_destination.exists() {
                conflicts += 1;
            } else if fs::rename(&child_source, &child_destination).is_ok()
                || copy_tree(&child_source, &child_destination)
            {
                moved += 1;
            } else {
                conflicts += 1;
            }
        } else if child_destination.is_file() {
            // Exact-path conflict: byte-identical copies are provable
            // duplicates (safe to remove); otherwise the destination wins.
            if files_are_identical(&child_source, &child_destination) {
                if fs::remove_file(&child_source).is_ok() {
                    moved += 1;
                } else {
                    conflicts += 1;
                }
            } else {
                conflicts += 1;
            }
        } else if child_destination.exists() {
            conflicts += 1;
        } else if fs::rename(&child_source, &child_destination).is_ok()
            || copy_tree(&child_source, &child_destination)
        {
            moved += 1;
        } else {
            conflicts += 1;
        }
    }
    (moved, conflicts)
}

/// True when both paths are files with identical bytes. Any read error
/// conservatively answers `false` so a never-read file is treated as a
/// genuine conflict and never deleted.
fn files_are_identical(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Move a file or directory tree. Rename first (same volume, atomic); on
/// cross-volume failure fall back to copy-then-remove. Never overwrites the
/// destination; callers check that before calling.
fn move_path_preserving(source: &Path, destination: &Path) -> bool {
    if fs::rename(source, destination).is_ok() {
        return true;
    }
    if !copy_tree(source, destination) {
        return false;
    }
    // Verify the copy landed before touching the source.
    if !destination.exists() {
        return false;
    }
    if source.is_dir() {
        fs::remove_dir_all(source).is_ok()
    } else {
        fs::remove_file(source).is_ok()
    }
}

/// Recursive copy for files and directories (best-effort: per-entry failures
/// propagate as a false result rather than partial-success lies).
fn copy_tree(source: &Path, destination: &Path) -> bool {
    if source.is_file() {
        if let Some(parent) = destination.parent() {
            if fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        return fs::copy(source, destination).is_ok();
    }
    if !source.is_dir() {
        return false;
    }
    if fs::create_dir_all(destination).is_err() {
        return false;
    }
    let Ok(entries) = fs::read_dir(source) else {
        return false;
    };
    for entry in entries.flatten() {
        let child_source = entry.path();
        let child_destination = destination.join(entry.file_name());
        if !copy_tree(&child_source, &child_destination) {
            return false;
        }
    }
    true
}

/// Remove the legacy `~/.claude/keel[.exe]` binary. On Windows a running
/// image cannot be deleted, so a failed delete parks the image under the
/// `.stale-<ts>` sibling name that `find_executable_orphans` sweeps on the
/// next install (rename works on running images; delete does not).
fn remove_legacy_binary(keel_home: &Path) -> String {
    let Some(old_binary) = legacy_claude_executable_path(keel_home) else {
        return String::new();
    };
    if !old_binary.exists() {
        return String::new();
    }
    match remove_path_if_exists(&old_binary) {
        Ok(()) => format!("removed legacy binary {}", display_path(&old_binary)),
        Err(_) => {
            #[cfg(windows)]
            {
                // Park the running image; orphan sweep deletes it later.
                let mut stale_name = old_binary
                    .file_name()
                    .map(|n| n.to_owned())
                    .unwrap_or_default();
                stale_name.push(format!(".stale-{}", unix_timestamp()));
                let stale = old_binary.with_file_name(stale_name);
                if fs::rename(&old_binary, &stale).is_ok() {
                    return format!(
                        "legacy binary parked as {} (in use; swept next install)",
                        display_path(&stale)
                    );
                }
            }
            format!(
                "legacy binary removal deferred: {}",
                display_path(&old_binary)
            )
        }
    }
}

/// Rename leftover `.claude-skill-manager` to `state` so inventories live
/// under the keel-owned name. No-op when `state` already exists.
fn migrate_legacy_state_directory(home: &Path) {
    let current = state_directory(home);
    let legacy = legacy_state_directory(home);
    if current.exists() || !legacy.exists() {
        return;
    }
    let _ = fs::rename(&legacy, &current);
}

/// Delete transient update extract trees. Inventories in `state/` stay.
fn remove_update_temp_trees(keel_home: &Path, engagement_home: &Path) {
    let _ = remove_path_if_exists(&update_cache_directory(keel_home));
    let _ = remove_path_if_exists(&legacy_state_directory(keel_home).join("bin"));
    if engagement_home != keel_home {
        let _ = remove_path_if_exists(&update_cache_directory(engagement_home));
        let _ = remove_path_if_exists(&legacy_state_directory(engagement_home).join("bin"));
        let _ = remove_path_if_exists(&legacy_state_directory(engagement_home));
    }
    let staged = installed_executable_path(keel_home);
    let mut staged_name = staged
        .file_name()
        .map(|name| name.to_owned())
        .unwrap_or_default();
    staged_name.push(".new");
    let _ = remove_path_if_exists(&staged.with_file_name(staged_name));
}

/// First-party skill directories deleted from the pack. Always removed from
/// the engagement home on install/update/uninstall unless the current source
/// pack still ships that name.
const DROPPED_FIRST_PARTY_SKILLS: &[&str] = &["running-a-sprint", "writing-user-stories"];

/// First-party slash-command files deleted from `commands/`. Same rule as
/// [`DROPPED_FIRST_PARTY_SKILLS`].
const DROPPED_FIRST_PARTY_COMMANDS: &[&str] = &["sprint.md", "user-story.md", "workflow.md"];

/// Remove deleted first-party skills/commands from the engagement home.
///
/// `--purge-stale` only deletes names that were in a prior inventory. An old
/// install that copied `sprint.md` before inventories existed (or after a
/// failed inventory write) keeps teaching the deleted loop. This list is the
/// product-cutover owner: always run, skip a name only when the current pack
/// still contains it.
fn remove_dropped_first_party_artifacts(
    claude_home: &Path,
    layout: Option<&RepositoryLayout>,
) -> usize {
    let keep_skills: BTreeSet<String> = layout
        .map(|value| {
            value
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let keep_commands = current_pack_command_names(layout);
    let mut removed = 0;
    for name in DROPPED_FIRST_PARTY_SKILLS {
        if keep_skills.iter().any(|skill| skill == name) {
            continue;
        }
        removed +=
            remove_path_if_exists_counted(&skills_directory(claude_home).join(name)).unwrap_or(0);
    }
    for name in DROPPED_FIRST_PARTY_COMMANDS {
        if keep_commands.iter().any(|command| command == name) {
            continue;
        }
        removed +=
            remove_path_if_exists_counted(&commands_directory(claude_home).join(name)).unwrap_or(0);
    }
    removed
}

fn current_pack_command_names(layout: Option<&RepositoryLayout>) -> BTreeSet<String> {
    let Some(layout) = layout else {
        return BTreeSet::new();
    };
    let mut names = BTreeSet::new();
    let source = layout.root_path.join("commands");
    let Ok(entries) = fs::read_dir(&source) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
            names.insert(name.to_string());
        }
    }
    names
}

/// Drop keel-owned leftovers that still sit in the old `~/.claude` home.
fn remove_legacy_keel_leftovers(keel_home: &Path, engagement_home: &Path) -> usize {
    if engagement_home == keel_home {
        return 0;
    }
    let mut removed = 0;
    for name in MIGRATION_DATA_NAMES {
        if let Ok(count) = remove_path_if_exists_counted(&engagement_home.join(name)) {
            removed += count;
        }
    }
    for suffix in ["-wal", "-shm"] {
        if let Ok(count) = remove_path_if_exists_counted(
            &engagement_home.join(format!("recall-index.sqlite3{suffix}")),
        ) {
            removed += count;
        }
    }
    if let Some(old_binary) = legacy_claude_executable_path(keel_home) {
        if let Ok(count) = remove_path_if_exists_counted(&old_binary) {
            removed += count;
        }
    }
    removed += remove_executable_orphans(engagement_home).unwrap_or(0);
    removed
}

/// Marker guarding the keel PATH export appended to unix shell rc files.
#[cfg(not(windows))]
const KEEL_PATH_MARKER: &str = "# keel PATH (managed by the keel installer)";

/// Ensure the keel home directory is on the user's PATH so every shell and
/// every host can invoke `keel` without a full path. Best-effort and
/// idempotent: existing PATH entries and rc files are never duplicated or
/// clobbered; failures report a status string instead of failing the install.
///
/// Windows: appends to the per-user `HKCU\Environment\Path` via `reg.exe`.
/// Unix: appends a marker-guarded `export PATH=` line to each existing
/// `~/.bashrc` / `~/.zshrc` / `~/.profile` (creating `~/.profile` when none
/// exists).
pub fn ensure_keel_home_on_path(keel_home: &Path) -> String {
    let dir = display_path(keel_home);
    if path_already_contains(keel_home) {
        return format!("PATH already contains {dir}");
    }
    #[cfg(windows)]
    {
        match windows_append_user_path(keel_home) {
            Ok(()) => format!("added {dir} to user PATH (HKCU\\Environment)"),
            Err(error) => format!("PATH update skipped ({error}); add {dir} manually"),
        }
    }
    #[cfg(not(windows))]
    {
        match unix_append_path_export(keel_home) {
            Ok(files) => {
                if files.is_empty() {
                    format!("PATH update skipped (no shell rc file found); add {dir} manually")
                } else {
                    format!("added {dir} to PATH via {}", files.join(", "))
                }
            }
            Err(error) => format!("PATH update skipped ({error}); add {dir} manually"),
        }
    }
}

/// True when the current-process PATH already lists `keel_home`.
fn path_already_contains(keel_home: &Path) -> bool {
    let Some(path_value) = std::env::var_os("PATH") else {
        return false;
    };
    for entry in std::env::split_paths(&path_value) {
        if entry == keel_home {
            return true;
        }
    }
    false
}

/// Remove dead `keel-home-split-*\.keel` entries that a buggy older build
/// appended to the persistent user PATH during test installs. Only touches
/// entries that (a) live under a directory whose name starts with
/// `keel-home-split-` and (b) no longer exist on disk, so a legitimate live
/// install is never removed. Best-effort: failures are swallowed because this
/// is cleanup, not the install itself.
#[cfg(windows)]
fn purge_stale_temp_keel_path_entries() {
    let Ok((current, _expand)) = windows_read_user_path() else {
        return;
    };
    let kept: Vec<&str> = current
        .split(';')
        .filter(|entry| !is_stale_temp_keel_entry(entry.trim()))
        .collect();
    let new_value = kept.join(";");
    if new_value == current {
        return;
    }
    let value_type = if new_value.contains('%') {
        "REG_EXPAND_SZ"
    } else {
        "REG_SZ"
    };
    let _ = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            value_type,
            "/d",
            &new_value,
            "/f",
        ])
        .status();
}

#[cfg(not(windows))]
fn purge_stale_temp_keel_path_entries() {
    // Unix fixtures appended a marker-guarded export line to rc files. Sweep
    // the marker + following export when the referenced dir is a dead temp.
    let Ok(user_home) = crate::runtime::resolve_user_home() else {
        return;
    };
    for rc in [
        user_home.join(".bashrc"),
        user_home.join(".zshrc"),
        user_home.join(".profile"),
    ] {
        let Ok(text) = read_text_if_exists(&rc) else {
            continue;
        };
        let mut out: Vec<&str> = Vec::new();
        let mut lines = text.lines().peekable();
        let mut changed = false;
        while let Some(line) = lines.next() {
            if line.trim() == KEEL_PATH_MARKER {
                if let Some(next) = lines.peek() {
                    if let Some(dir) = next
                        .trim()
                        .strip_prefix("export PATH=\"")
                        .and_then(|rest| rest.strip_suffix(":$PATH\""))
                    {
                        if is_stale_temp_keel_entry(dir) {
                            lines.next();
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            out.push(line);
        }
        if changed {
            let mut joined = out.join("\n");
            if !joined.is_empty() && !joined.ends_with('\n') {
                joined.push('\n');
            }
            let _ = write_text(&rc, &joined);
        }
    }
}

/// True when `entry` is a dead temp-dir keel home left by a test install:
/// its parent directory name starts with `keel-home-split-`, its own name is
/// the standard `.keel`, and the directory no longer exists.
fn is_stale_temp_keel_entry(entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    let path = PathBuf::from(entry);
    let is_keel = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == crate::runtime::KEEL_HOME_DIRECTORY_NAME)
        .unwrap_or(false);
    let is_temp_parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("keel-home-split-"))
        .unwrap_or(false);
    is_keel && is_temp_parent && !path.exists()
}

/// Read the per-user PATH value from `HKCU\Environment` (empty when absent).
#[cfg(windows)]
fn windows_read_user_path() -> Result<(String, bool), String> {
    let output = std::process::Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .output()
        .map_err(|error| format!("run reg.exe: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current = String::new();
    let mut has_expand = false;
    if output.status.success() {
        for line in stdout.lines() {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("Path") else {
                continue;
            };
            let rest = rest.trim_start();
            if rest.starts_with("REG_EXPAND_SZ") {
                has_expand = true;
                current = rest
                    .trim_start_matches("REG_EXPAND_SZ")
                    .trim_start()
                    .to_string();
            } else if rest.starts_with("REG_SZ") {
                current = rest.trim_start_matches("REG_SZ").trim_start().to_string();
            }
        }
    }
    Ok((current, has_expand))
}

#[cfg(windows)]
fn windows_append_user_path(keel_home: &Path) -> Result<(), String> {
    let dir = display_path(keel_home);
    let (current, has_expand) = windows_read_user_path()?;
    // Case-insensitive duplicate guard: Windows PATH entries ignore case.
    let lower_current = current.to_lowercase();
    let lower_home = dir.to_lowercase();
    for entry in lower_current.split(';') {
        if entry.trim() == lower_home.trim() {
            return Ok(());
        }
    }
    let new_value = if current.trim().is_empty() {
        dir.to_string()
    } else if current.ends_with(';') {
        format!("{current}{dir}")
    } else {
        format!("{current};{dir}")
    };
    // REG_EXPAND_SZ when the existing value uses %VAR% expansion, so reg.exe
    // does not silently convert it to a REG_SZ and break expansion.
    let value_type = if has_expand || new_value.contains('%') {
        "REG_EXPAND_SZ"
    } else {
        "REG_SZ"
    };
    let status = std::process::Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            value_type,
            "/d",
            &new_value,
            "/f",
        ])
        .status()
        .map_err(|error| format!("run reg.exe add: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg.exe add exited with {status}"))
    }
}

#[cfg(not(windows))]
fn unix_append_path_export(keel_home: &Path) -> Result<Vec<String>, String> {
    let user_home = crate::runtime::resolve_user_home()?;
    let export_line = format!(
        "{KEEL_PATH_MARKER}\nexport PATH=\"{dir}:$PATH\"\n",
        dir = display_path(keel_home)
    );
    let candidates = [
        user_home.join(".bashrc"),
        user_home.join(".zshrc"),
        user_home.join(".profile"),
    ];
    let mut touched = Vec::new();
    for rc in &candidates {
        if !rc.is_file() {
            continue;
        }
        let text = read_text_if_exists(rc).unwrap_or_default();
        // Marker-guarded: never append twice, never touch unmanaged lines.
        if text.contains(KEEL_PATH_MARKER) {
            continue;
        }
        let mut updated = text.clone();
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&export_line);
        write_text(rc, &updated)?;
        touched.push(display_path(rc));
    }
    if touched.is_empty() && !candidates.iter().any(|rc| rc.is_file()) {
        // No rc file at all: create ~/.profile so sh-based sessions pick it up.
        let profile = user_home.join(".profile");
        write_text(&profile, &export_line)?;
        touched.push(display_path(&profile));
    }
    Ok(touched)
}

/// Remove the `[plugins."keel@personal-keel"]` section from Codex config.toml
/// without disturbing any other section, key, or comment. String-surgical:
/// drops the header line plus every key line until the next section header.
fn remove_codex_plugin_section(config_path: &Path) -> usize {
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
fn remove_codex_native_mcp_section(config_path: &Path) -> usize {
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
fn remove_codex_managed_agents_md(path: &Path) -> usize {
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

/// The Claude Desktop MCP config file, derived from the user's home so it is both
/// testable (a temp home yields a temp path) and correct in production (APPDATA
/// defaults to `%USERPROFILE%\AppData\Roaming`).
fn claude_desktop_config_path(home: &Path) -> PathBuf {
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

    match merge_cowork_mcp(&config_path, "keel", &mcp_entry) {
        Ok(CoworkMcpResult::Added) => Some(format!(
            "MCP registered in {} (Desktop supports MCP tools only — no hooks)",
            display_path(&config_path)
        )),
        Ok(CoworkMcpResult::AlreadyCurrent) => Some("MCP already current".to_string()),
        Ok(CoworkMcpResult::Updated) => {
            Some(format!("MCP updated in {}", display_path(&config_path)))
        }
        Err(error) => Some(format!("MCP skipped ({error})")),
    }
}

/// Merge the `keel` entry into Cowork's settings.json under `mcpServers`.
fn merge_cowork_mcp(
    config_path: &Path,
    server_key: &str,
    entry: &serde_json::Value,
) -> Result<CoworkMcpResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if stripped.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(stripped).map_err(|error| format!("parse error: {error}"))?
    };

    if document.get("mcpServers").is_none() {
        document["mcpServers"] = serde_json::json!({});
    }
    let servers = document["mcpServers"]
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    let desired =
        serde_json::to_string_pretty(entry).map_err(|error| format!("serialize error: {error}"))?;

    if let Some(existing) = servers.get(server_key) {
        let existing_str = serde_json::to_string_pretty(existing)
            .map_err(|error| format!("serialize error: {error}"))?;
        if existing_str == desired {
            return Ok(CoworkMcpResult::AlreadyCurrent);
        }
        servers.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(CoworkMcpResult::Updated);
    }

    servers.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(CoworkMcpResult::Added)
}

/// Rewrite the Codex MCP `keel` server's `command` to the absolute binary
/// path. Handles both the wrapped `{"mcp_servers": {"keel": {...}}}` shape
/// that keel ships and a direct `{"keel": {...}}` shape. Returns true when the
/// document was mutated (and should be persisted), false when the command was
/// already the absolute path or the expected structure was absent.
fn rewrite_codex_mcp_command(doc: &mut serde_json::Value, absolute: &str) -> bool {
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
/// the merged config. Returns true when the entry was mutated.
fn rewrite_mcp_entry_command(entry: &mut serde_json::Value, absolute: &str) -> bool {
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
enum OpencodeMcpResult {
    Added,
    AlreadyCurrent,
    Updated,
}

#[derive(Debug)]
enum PiMcpResult {
    Added,
    AlreadyCurrent,
    Updated,
}

enum CursorMcpResult {
    Added,
    AlreadyCurrent,
    Updated,
}

#[derive(Debug)]
enum CommandcodeMcpResult {
    Added,
    AlreadyCurrent,
    Updated,
}

#[derive(Debug)]
enum CoworkMcpResult {
    Added,
    AlreadyCurrent,
    Updated,
}

/// Merge the `keel` entry into `~/.cursor/mcp.json` under `mcpServers`. Merge,
/// never clobber — preserve the user's other MCP servers. BOM-tolerant. Cursor
/// uses the standard `{"mcpServers": {<name>: {command, args, env}}}` shape.
fn merge_cursor_mcp(
    config_path: &std::path::Path,
    server_key: &str,
    entry: &serde_json::Value,
) -> Result<CursorMcpResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if stripped.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(stripped).map_err(|error| format!("parse error: {error}"))?
    };

    if document.get("mcpServers").is_none() {
        document["mcpServers"] = serde_json::json!({});
    }
    let current = document["mcpServers"]
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    let desired =
        serde_json::to_string_pretty(entry).map_err(|error| format!("serialize error: {error}"))?;

    if let Some(existing) = current.get(server_key) {
        let existing_str = serde_json::to_string_pretty(existing)
            .map_err(|error| format!("serialize error: {error}"))?;
        if existing_str == desired {
            return Ok(CursorMcpResult::AlreadyCurrent);
        }
        current.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(CursorMcpResult::Updated);
    }

    current.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(CursorMcpResult::Added)
}

/// Merge the `keel` entry into `~/.commandcode/mcp.json` under `mcpServers`.
/// Merge, never clobber; preserves the user's other MCP servers. BOM-tolerant.
/// Command Code uses the same standard `{"mcpServers": {<name>: {command, args}}}`
/// shape as Cursor.
fn merge_commandcode_mcp(
    config_path: &std::path::Path,
    server_key: &str,
    entry: &serde_json::Value,
) -> Result<CommandcodeMcpResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let stripped = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if stripped.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(stripped).map_err(|error| format!("parse error: {error}"))?
    };

    if document.get("mcpServers").is_none() {
        document["mcpServers"] = serde_json::json!({});
    }
    let current = document["mcpServers"]
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    let desired =
        serde_json::to_string_pretty(entry).map_err(|error| format!("serialize error: {error}"))?;

    if let Some(existing) = current.get(server_key) {
        let existing_str = serde_json::to_string_pretty(existing)
            .map_err(|error| format!("serialize error: {error}"))?;
        if existing_str == desired {
            return Ok(CommandcodeMcpResult::AlreadyCurrent);
        }
        current.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(CommandcodeMcpResult::Updated);
    }

    current.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(CommandcodeMcpResult::Added)
}

fn merge_pi_mcp(
    config_path: &std::path::Path,
    server_key: &str,
    entry: &serde_json::Value,
    template_defaults: &serde_json::Value,
) -> Result<PiMcpResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    let existing_text = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if existing_text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing_text).map_err(|error| format!("parse error: {error}"))?
    };

    if document.get("mcpServers").is_none() {
        document["mcpServers"] = serde_json::json!({});
    }
    // Template top-level keys (e.g. `settings`) seed the document only where
    // the user has no value of their own — never clobber existing keys.
    if let Some(defaults) = template_defaults.as_object() {
        for (key, value) in defaults {
            if document.get(key).is_none() {
                document[key.clone()] = value.clone();
            }
        }
    }
    let current = document["mcpServers"]
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    let desired =
        serde_json::to_string_pretty(entry).map_err(|error| format!("serialize error: {error}"))?;

    if let Some(existing) = current.get(server_key) {
        let existing_str = serde_json::to_string_pretty(existing)
            .map_err(|error| format!("serialize error: {error}"))?;
        if existing_str == desired {
            return Ok(PiMcpResult::AlreadyCurrent);
        }
        current.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(PiMcpResult::Updated);
    }

    current.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(PiMcpResult::Added)
}

fn merge_opencode_mcp(
    config_path: &std::path::Path,
    server_key: &str,
    entry: &serde_json::Value,
) -> Result<OpencodeMcpResult, String> {
    let existing_text = crate::runtime::read_text_if_exists(config_path).unwrap_or_default();
    // Editors and PowerShell on Windows commonly write a UTF-8 BOM; serde_json
    // rejects a leading BOM as "expected value at line 1 column 1". Strip it.
    let existing_text = existing_text
        .strip_prefix('\u{feff}')
        .unwrap_or(&existing_text);
    let mut document: serde_json::Value = if existing_text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing_text).map_err(|error| format!("parse error: {error}"))?
    };

    if document.get("mcp").is_none() {
        document["mcp"] = serde_json::json!({});
    }
    let current_mcp = document["mcp"]
        .as_object_mut()
        .ok_or("mcp is not an object")?;

    let desired =
        serde_json::to_string_pretty(entry).map_err(|error| format!("serialize error: {error}"))?;

    if let Some(existing) = current_mcp.get(server_key) {
        let existing_str = serde_json::to_string_pretty(existing)
            .map_err(|error| format!("serialize error: {error}"))?;
        if existing_str == desired {
            return Ok(OpencodeMcpResult::AlreadyCurrent);
        }
        current_mcp.insert(server_key.to_string(), entry.clone());
        write_text(
            config_path,
            &serde_json::to_string_pretty(&document)
                .map_err(|error| format!("serialize error: {error}"))?,
        )?;
        return Ok(OpencodeMcpResult::Updated);
    }

    current_mcp.insert(server_key.to_string(), entry.clone());
    write_text(
        config_path,
        &serde_json::to_string_pretty(&document)
            .map_err(|error| format!("serialize error: {error}"))?,
    )?;
    Ok(OpencodeMcpResult::Added)
}

/// Sentinels delimiting the keel-managed region inside the user-global
/// `~/.claude/CLAUDE.md`. Everything between them is owned by the installer and
/// rewritten on every install; everything outside is the user's own content and
/// is preserved verbatim.
const MANAGED_CLAUDE_MD_BEGIN: &str =
    "<!-- keel:begin (managed by keel install — edits inside this block are overwritten; edit outside it freely) -->";
const MANAGED_CLAUDE_MD_END: &str = "<!-- keel:end -->";

/// The always-on operating contract written into `~/.claude/CLAUDE.md`.
///
/// Why this exists: every other keel surface (the SessionStart bootstrap,
/// the per-prompt iron law, skill pointers, MCP-tool nudges) is delivered through
/// the harness's hook `additionalContext` channel. When that channel does not
/// reach the model — e.g. a gateway/proxy that drops injected context — the agent
/// sees none of keel. `~/.claude/CLAUDE.md` is loaded natively into every
/// session as user memory, the same hook-independent channel that carries the
/// base system prompt, so this block lands even when hooks do not. Kept compact
/// because it is paid on every session of every project.
const MANAGED_CLAUDE_MD_BODY: &str = r#"# keel operating contract (always-on)

Installed by keel into `~/.claude/CLAUDE.md` and loaded into **every** harness session as user memory — independent of hooks. Applies to every project you work in, not just keel.

## Iron Law — for any request that could touch code, config, or architecture
1. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.
2. **Understand before building.** Restate what the request asks and research what is genuinely needed before writing code. No guessing, no building against an imagined spec.
3. **Request fidelity.** Implement only what the user asked. Do not invent features, APIs, files, refactors, or "nice extras" outside the request.
4. **Ask when unclear.** If the request is unclear, conflicting, incomplete, or you feel drift risk (multiple valid designs, unknown project conventions, scare that you will invent scope), **stop and ask the user** before coding. Do not decide silently. Do not "just pick one and go."
5. **Never trust knowledge-base alone.** Training data and generic patterns are not this project's structure, stories, or implementation path. Read SYSTEM_MAP, owning files, and user stories here. Each project has its own layout and conventions — nothing is hardcoded in your memory as truth for this repo.
6. **Invoke relevant skills.** If there is even a 1% chance a keel skill applies, use the Skill tool BEFORE writing code or giving a final answer.
7. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything.
8. **Preserve existing data.** Never remove or replace an existing field, column, output, or record to fit a new format — ADD alongside, and ASK before dropping anything the user did not name. Data loss in an edit is destructive like `DROP TABLE`; if you would flag a removal *after* acting, ask *before* instead. Autonomy covers reversible choices, never data deletion.
9. **Memory-first navigation.** Prefer `system_map` + `recall` + working briefs over listing the whole tree. If context already names the path, open that path; do not rediscover the repo with blind `ls`/`find`/full-tree greps.
10. **Useful comments only.** Never write summary comments that restate the code. Prefer structured contracts (`@param`, `@returns`, `# Errors`, `// why:`) or no comment.

## keel MCP tools — always available, prefer over guessing
- `system_map` — call when you lack the workspace structural map this turn (e.g. "what is this project", "where does X live") instead of reading files blind. **Call it at most once per turn** — if you already called it this turn, the result is in your context; reuse it and read the owning files it points you at. Call it again only if you have since created, moved, or deleted files and the in-context map is now stale.
- `recall` — call when you need to surface a prior decision, working brief, or learning from durable memory instead of claiming from conversation. **Call it at most once per turn** — if you already called it this turn, reuse the result. Call it again only if you have since written new memory this turn and need to confirm it landed.
- `run_command` — run noisy shell commands (test, build, lint, logs, search) through it so compacted output enters context instead of the raw stream.

**No tool-call loops.** These tools answer "what is the structure" and "what do I remember". They do not change between calls within a single turn unless *you* changed something. Re-calling them with no intervening change is a loop — re-read the result already in your context instead.

**No blind exploration.** If SYSTEM_MAP or recall already points at the module, edit or read that file next. Broad directory listings and full-repo scans are last resorts after scoped memory fails.

## Skills
keel installs specialist skills under `~/.claude/skills/` (lifecycle, backend, cloud, security, reviewer, UI/UX, debugging, TDD, migrations, and more). Invoke by bare name with the Skill tool, e.g. `Skill("reviewer")`. The `using-keel` skill carries the full catalog and routing rules."#;

fn managed_claude_md_block() -> String {
    format!("{MANAGED_CLAUDE_MD_BEGIN}\n{MANAGED_CLAUDE_MD_BODY}\n{MANAGED_CLAUDE_MD_END}")
}

/// Splice `block` into `existing` CLAUDE.md content, preserving user content.
///
/// If a managed region already exists (both sentinels present, in order), its
/// contents are replaced in place. Otherwise the block is prepended (so the
/// contract is the first thing the model reads) and any pre-existing user
/// content is kept below it. Pure (no IO) so the splice logic is unit-testable.
fn merge_managed_claude_md(existing: &str, block: &str) -> String {
    if let (Some(start), Some(end_idx)) = (
        existing.find(MANAGED_CLAUDE_MD_BEGIN),
        existing.find(MANAGED_CLAUDE_MD_END),
    ) {
        if end_idx > start {
            let end_full = end_idx + MANAGED_CLAUDE_MD_END.len();
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

/// Remove the managed region from `existing`, preserving user content. Pure
/// (no IO). Returns the content with the block (and the blank lines it
/// introduced) stripped; an all-managed file collapses to empty so the caller
/// can delete it.
fn strip_managed_claude_md(existing: &str) -> String {
    if let (Some(start), Some(end_idx)) = (
        existing.find(MANAGED_CLAUDE_MD_BEGIN),
        existing.find(MANAGED_CLAUDE_MD_END),
    ) {
        if end_idx > start {
            let end_full = end_idx + MANAGED_CLAUDE_MD_END.len();
            let before = existing[..start].trim_end();
            let after = existing[end_full..].trim_start();
            return match (before.is_empty(), after.is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!("{after}\n"),
                (false, true) => format!("{before}\n"),
                (false, false) => format!("{before}\n\n{after}\n"),
            };
        }
    }
    existing.to_string()
}

/// Write/refresh the keel managed block in `~/.claude/CLAUDE.md`.
///
/// Guarded on the standard `.claude` home name for the same reason as
/// `maybe_register_mcp_server` / `maybe_install_hooks`: the integration suite
/// installs into throwaway `--claude-home` dirs and must stay hermetic. Real
/// installs into `~/.claude` always get the file. Best-effort: a failure is
/// reported in the summary but never fails the install.
fn maybe_sync_user_claude_md(claude_home: &Path) -> Option<String> {
    if !is_standard_home(claude_home) {
        return None;
    }
    let path = claude_home.join("CLAUDE.md");
    let existing = read_text_if_exists(&path).unwrap_or_default();
    let merged = merge_managed_claude_md(&existing, &managed_claude_md_block());
    if merged == existing {
        return Some("already current".to_string());
    }
    match write_text(&path, &merged) {
        Ok(()) => Some(format!("written to {}", display_path(&path))),
        Err(error) => Some(format!("skipped ({error})")),
    }
}

/// Strip the keel managed block from `~/.claude/CLAUDE.md` on uninstall,
/// preserving any user content outside it. Deletes the file only if it becomes
/// empty. Returns the number of paths changed/removed (0 or 1). A missing file
/// or a file with no managed block is a no-op.
fn remove_managed_user_claude_md(claude_home: &Path) -> Result<usize, String> {
    let path = claude_home.join("CLAUDE.md");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(0),
    };
    if !existing.contains(MANAGED_CLAUDE_MD_BEGIN) {
        return Ok(0);
    }
    let stripped = strip_managed_claude_md(&existing);
    if stripped.trim().is_empty() {
        remove_path_if_exists_counted(&path)
    } else {
        write_text(&path, &stripped)?;
        Ok(1)
    }
}

fn managed_files_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-files.txt")
}

fn managed_skills_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-skills.txt")
}

fn managed_agents_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-agents.txt")
}

/// Inventory of top-level shared-resource directory names (currently `_shared`)
/// that the installer has staged into `<claude_home>/skills/`. Tracked
/// separately from `managed-skills.txt` because shared resources have no
/// `SKILL.md` and follow directory-level orphan cleanup, not file-level.
fn managed_shared_resources_inventory_path(claude_home: &Path) -> PathBuf {
    state_directory(claude_home).join("managed-shared-resources.txt")
}

/// Top-level names under `<claude_home>` that hold **user / harness data**.
/// Install and orphan cleanup must never delete or rewrite these as "stale".
/// (Grok sessions live under `~/.grok/`, which install never touches.)
const PROTECTED_USER_DATA_TOP_LEVEL: &[&str] = &[
    "sessions",
    "projects",
    "file-history",
    "memories",
    "memory",
    "working-briefs",
    "orchestration",
    "workflow",
    "tasks",
    "teams",
    "backups",
    "cache",
    "raw-output",
    "session-env",
    "shell-snapshots",
    "history.jsonl",
    "settings.json",
    "recall-index.sqlite3",
    "command-compaction-events.jsonl",
    "statsig",
    "todos",
    "plugins",
    "ide",
    "stats-cache",
    "transcripts",
];

/// Relative paths install is allowed to remove as "stale managed" content.
/// Anything else is refused even if it appears in an old inventory (corruption
/// or a bug must never turn orphan cleanup into a home-directory wipe).
fn is_allowed_managed_orphan_relative(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/");
    let rel = normalized.trim_start_matches("./");
    if rel.is_empty() || rel == "." {
        return false;
    }
    // Path traversal or absolute-like components → refuse.
    if rel.starts_with('/')
        || rel.starts_with("..")
        || rel.contains("/../")
        || rel.ends_with("/..")
        || rel.contains(':')
    {
        return false;
    }
    let first = rel.split('/').next().unwrap_or("");
    if PROTECTED_USER_DATA_TOP_LEVEL
        .iter()
        .any(|p| first.eq_ignore_ascii_case(p) || rel.eq_ignore_ascii_case(p))
    {
        return false;
    }
    // Never orphan-delete the always-on CLAUDE.md (merge-only surface).
    if rel.eq_ignore_ascii_case("CLAUDE.md") {
        return false;
    }
    // Never remove user-learned skills (not part of the ship pack).
    if rel.starts_with("skills/learned-") || rel == "skills/learned" {
        return false;
    }
    // Managed pack surfaces only.
    if matches!(
        first,
        "skills" | "agents" | "agent-profiles" | "commands" | "docs"
    ) {
        return true;
    }
    // Root guidance files from ROOT_GUIDANCE_RELATIVE_PATHS (file names only at root).
    if !rel.contains('/') && matches!(rel, "AGENTS.md" | "00-skill-routing-and-escalation.md") {
        return true;
    }
    false
}

/// Resolve `claude_home/relative` and ensure the result stays under `claude_home`.
fn resolve_managed_path_under_home(claude_home: &Path, relative: &str) -> Option<PathBuf> {
    if !is_allowed_managed_orphan_relative(relative) {
        return None;
    }
    let candidate = claude_home.join(relative);
    // Best-effort containment: after join, path must still start with claude_home.
    let home_canon = claude_home
        .canonicalize()
        .unwrap_or_else(|_| claude_home.to_path_buf());
    let cand_parent = candidate.parent().unwrap_or(&candidate);
    let parent_canon = cand_parent
        .canonicalize()
        .unwrap_or_else(|_| cand_parent.to_path_buf());
    if parent_canon.starts_with(&home_canon) || candidate.starts_with(claude_home) {
        Some(candidate)
    } else {
        None
    }
}

/// Whether orphan purge is enabled.
///
/// **Default is off** so a one-line reinstall never deletes user-adjacent data.
/// Opt in with `--purge-stale` or `KEEL_INSTALL_PURGE_STALE=1`. Even when on,
/// every delete still passes the protect/allowlist (sessions, projects, memories,
/// history, settings, learned skills, path traversal, etc. are never removed).
/// `--no-purge` always wins and disables deletes.
fn install_purge_stale_enabled(flag_no_purge: bool, flag_purge_stale: bool) -> bool {
    if flag_no_purge {
        return false;
    }
    if flag_purge_stale {
        return true;
    }
    match std::env::var("KEEL_INSTALL_PURGE_STALE") {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true" || t == "yes" || t == "on"
        }
        Err(_) => false,
    }
}

fn remove_orphans(
    claude_home: &Path,
    previous_files: &BTreeSet<String>,
    previous_skills: &BTreeSet<String>,
    previous_shared_resources: &BTreeSet<String>,
    layout: &RepositoryLayout,
    tracker: &FileTracker,
    purge_stale: bool,
) -> Result<usize, String> {
    if !purge_stale {
        return Ok(0);
    }
    let mut removed = 0;
    for relative in previous_files.difference(&tracker.files) {
        let Some(absolute) = resolve_managed_path_under_home(claude_home, relative) else {
            // Refuse: protected user data, traversal, or non-managed surface.
            continue;
        };
        if absolute.is_file() {
            removed += remove_path_if_exists_counted(&absolute)?;
        }
    }
    let current_skills: BTreeSet<String> = layout.skills.iter().map(|s| s.name.clone()).collect();
    for orphan_skill in previous_skills.difference(&current_skills) {
        // Never purge user-learned skill directories.
        if orphan_skill.starts_with("learned-") {
            continue;
        }
        let rel = format!("skills/{orphan_skill}");
        let Some(skill_directory) = resolve_managed_path_under_home(claude_home, &rel) else {
            continue;
        };
        removed += remove_path_if_exists_counted(&skill_directory)?;
    }
    let current_shared_resources: BTreeSet<String> =
        layout.shared_resource_directories.iter().cloned().collect();
    for orphan_shared in previous_shared_resources.difference(&current_shared_resources) {
        if orphan_shared.contains("..")
            || orphan_shared.contains('/')
            || orphan_shared.contains('\\')
        {
            continue;
        }
        let rel = format!("skills/{orphan_shared}");
        let Some(shared_directory) = resolve_managed_path_under_home(claude_home, &rel) else {
            continue;
        };
        removed += remove_path_if_exists_counted(&shared_directory)?;
    }
    Ok(removed)
}

pub fn write_install_summary(summary: &InstallSummary, output: &mut dyn Write) {
    let _ = writeln!(output, "Native Rust install complete");
    let _ = writeln!(output);
    let _ = writeln!(output, "Summary:");
    let _ = writeln!(output, "  Synced skills: {}", summary.synced_skills);
    let _ = writeln!(output, "  Synced agents: {}", summary.synced_agents);
    let _ = writeln!(
        output,
        "  Synced subagent definitions: {}",
        summary.synced_subagent_definitions
    );
    let _ = writeln!(output, "  Synced commands: {}", summary.synced_commands);
    let _ = writeln!(output, "  Synced root files: {}", summary.synced_root_files);
    let _ = writeln!(
        output,
        "  Synced shared resources: {}",
        summary.synced_shared_resources
    );
    let _ = writeln!(
        output,
        "  Removed stale files: {} (managed pack only; sessions/projects/memories/history never purged; default install leaves orphans unless --purge-stale)",
        summary.removed_stale_files
    );
    let _ = writeln!(
        output,
        "  Removed executable orphans: {}",
        summary.removed_executable_orphans
    );
    let _ = writeln!(
        output,
        "  Published executable: {}",
        summary.published_executable
    );
    if let Some(mcp_status) = &summary.mcp_registration {
        let _ = writeln!(output, "  MCP server: {mcp_status}");
    }
    if let Some(hooks_status) = &summary.hooks_installation {
        let _ = writeln!(output, "  Lifecycle hooks: {hooks_status}");
    }
    if let Some(claude_md_status) = &summary.user_claude_md {
        let _ = writeln!(output, "  User CLAUDE.md: {claude_md_status}");
    }
    if let Some(opencode_status) = &summary.opencode_wiring {
        let _ = writeln!(output, "  OpenCode wiring: {opencode_status}");
    }
    if let Some(cursor_status) = &summary.cursor_wiring {
        let _ = writeln!(output, "  Cursor wiring: {cursor_status}");
    }
    if let Some(codex_status) = &summary.codex_wiring {
        let _ = writeln!(output, "  Codex wiring: {codex_status}");
    }
    if let Some(pi_status) = &summary.pi_wiring {
        let _ = writeln!(output, "  Pi Agent wiring: {pi_status}");
    }
    if let Some(commandcode_status) = &summary.commandcode_wiring {
        let _ = writeln!(output, "  Command Code wiring: {commandcode_status}");
    }
    if let Some(cowork_status) = &summary.cowork_wiring {
        let _ = writeln!(output, "  Cowork wiring: {cowork_status}");
    }
    if let Some(grok_status) = &summary.grok_wiring {
        let _ = writeln!(output, "  Grok wiring: {grok_status}");
    }
    if let Some(migration) = &summary.migration_report {
        let _ = writeln!(output, "  Legacy migration: {migration}");
    }
    if let Some(path_status) = &summary.path_wiring {
        let _ = writeln!(output, "  PATH: {path_status}");
    }
}

fn ensure_claude_home_directories(claude_home: &Path) -> Result<(), String> {
    for directory in [
        claude_home,
        &skills_directory(claude_home),
        &agents_directory(claude_home),
        &agent_profiles_directory(claude_home),
        &state_directory(claude_home),
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("create {}: {error}", display_path(directory)))?;
    }
    Ok(())
}

fn uninstall_managed_files(claude_home: &Path) -> Result<usize, String> {
    let mut removed_count = 0;
    // Managed engagement files live in the engagement home (even when the
    // root is ~/.keel), so inventory-relative paths resolve there.
    let engagement_home = crate::runtime::claude_engagement_home(claude_home);
    let file_inventory = read_inventory_set(&managed_files_inventory_path(claude_home));
    for relative in &file_inventory {
        let absolute = engagement_home.join(relative);
        if absolute.is_file() {
            removed_count += remove_path_if_exists_counted(&absolute)?;
        }
    }
    let installed_skills = read_inventory_set(&managed_skills_inventory_path(claude_home));
    for skill_name in &installed_skills {
        let skill_path = skills_directory(claude_home).join(skill_name);
        removed_count += remove_path_if_exists_counted(&skill_path)?;
    }
    let installed_shared_resources =
        read_inventory_set(&managed_shared_resources_inventory_path(claude_home));
    for shared_name in &installed_shared_resources {
        let shared_path = skills_directory(claude_home).join(shared_name);
        removed_count += remove_path_if_exists_counted(&shared_path)?;
    }
    // `managed-agents.txt` stores bare agent names (no extension), one per
    // YAML config under `<skill>/agents/*.yaml`. Each name maps to a managed
    // agent profile under `<home>/agent-profiles/<name>.toml` plus an
    // optional matching directory entry under `<home>/agents/<name>` from
    // earlier installer revisions. Subagent `.md` definitions installed by
    // `sync_subagent_definitions` are tracked separately via
    // `managed-files.txt` and removed by the per-file inventory loop above.
    let installed_agents = read_inventory_set(&managed_agents_inventory_path(claude_home));
    for agent_name in &installed_agents {
        let agent_path = agents_directory(claude_home).join(agent_name);
        removed_count += remove_path_if_exists_counted(&agent_path)?;
        let profile_path = agent_profiles_directory(claude_home).join(format!("{agent_name}.toml"));
        removed_count += remove_path_if_exists_counted(&profile_path)?;
    }
    for inventory in [
        managed_files_inventory_path(claude_home),
        managed_skills_inventory_path(claude_home),
        managed_agents_inventory_path(claude_home),
        managed_shared_resources_inventory_path(claude_home),
        // install-metadata.txt records repo/installed version; install writes it
        // (install.rs), so uninstall must remove it or doctor/verify report a
        // stale "installed" state against a now-deleted binary.
        crate::manager::verify::install_metadata_path(claude_home),
    ] {
        let _ = remove_path_if_exists_counted(&inventory)?;
    }
    Ok(removed_count)
}

fn remove_deprecated_config_keys(claude_home: &Path) -> Result<(), String> {
    let config_file = config_path(claude_home);
    if !config_file.is_file() {
        return Ok(());
    }
    let original_text = read_text_if_exists(&config_file).unwrap_or_default();
    let updated_text = remove_managed_block(&original_text);
    if updated_text != original_text {
        write_text(&config_file, &updated_text)?;
    }
    Ok(())
}

fn remove_wired_adapters(claude_home: &Path) -> usize {
    let mut removed = 0;
    let home = match claude_home.parent() {
        Some(path) => path.to_path_buf(),
        None => return 0,
    };

    let plugin_file = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    removed += remove_path_if_exists_counted(&plugin_file).unwrap_or(0);

    // OpenCode MCP: install merges `mcp.keel` into opencode.json (merge_opencode_mcp).
    // Uninstall must remove that entry or OpenCode keeps spawning the now-deleted
    // keel binary every session. Mirrors the Pi mcp.json entry-removal below.
    let opencode_config = home.join(".config").join("opencode").join("opencode.json");
    if opencode_config.is_file() {
        if let Ok(text) = crate::runtime::read_text_if_exists(&opencode_config) {
            let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(stripped) {
                let mutated = if let Some(mcp) = doc.get_mut("mcp").and_then(|v| v.as_object_mut())
                {
                    if mcp.remove("keel").is_some() {
                        if mcp.is_empty() {
                            doc.as_object_mut().map(|o| o.remove("mcp"));
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if mutated {
                    let _ = write_text(
                        &opencode_config,
                        &serde_json::to_string_pretty(&doc)
                            .unwrap_or_else(|_| stripped.to_string()),
                    );
                    removed += 1;
                }
            }
        }
    }

    let codex_dir = home.join(".codex").join("plugins").join("keel");
    if codex_dir.is_dir() {
        removed += remove_path_if_exists_counted(&codex_dir).unwrap_or(0);
    }

    // Codex discovery surfaces written by maybe_wire_codex: leaving either
    // behind makes Codex try to load a plugin whose files no longer exist.
    let codex_marketplace = home
        .join(".agents")
        .join("plugins")
        .join("marketplace.json");
    removed += remove_codex_marketplace_entry(&codex_marketplace);
    let codex_config = home.join(".codex").join("config.toml");
    removed += remove_codex_plugin_section(&codex_config);
    // Native MCP entry + managed AGENTS.md block written by maybe_wire_codex;
    // a stale entry would spawn the deleted keel binary every session.
    removed += remove_codex_native_mcp_section(&codex_config);
    removed += remove_codex_managed_agents_md(&home.join(".codex").join("AGENTS.md"));

    let cursorrules = home.join(".cursorrules");
    if cursorrules.is_file() {
        if let Ok(content) = std::fs::read_to_string(&cursorrules) {
            if content.starts_with("# keel Iron Law for Cursor") {
                removed += remove_path_if_exists_counted(&cursorrules).unwrap_or(0);
            }
        }
    }

    // Cowork (Claude Desktop): remove the keel MCP entry from
    // claude_desktop_config.json (where MCP-only wiring registers it), and clean up
    // the legacy ~/.claude/plugins/keel-cowork/ dir that older installs copied a
    // now-retired TS plugin into.
    let desktop_config = claude_desktop_config_path(&home);
    if desktop_config.is_file() {
        if let Ok(text) = crate::runtime::read_text_if_exists(&desktop_config) {
            let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(stripped) {
                let mutated = if let Some(servers) =
                    doc.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                {
                    if servers.remove("keel").is_some() {
                        if servers.is_empty() {
                            doc.as_object_mut().map(|o| o.remove("mcpServers"));
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if mutated {
                    let _ = write_text(
                        &desktop_config,
                        &serde_json::to_string_pretty(&doc)
                            .unwrap_or_else(|_| stripped.to_string()),
                    );
                    removed += 1;
                }
            }
        }
    }
    let legacy_cowork_dir = home.join(".claude").join("plugins").join("keel-cowork");
    if legacy_cowork_dir.is_dir() {
        removed += remove_path_if_exists_counted(&legacy_cowork_dir).unwrap_or(0);
    }

    // Cursor hooks: install writes ~/.cursor/hooks/{hooks.json,keel-cursor.sh}.
    // Uninstall must remove both or Cursor keeps invoking a hook that shells to
    // the now-deleted keel binary on every tool call.
    for hook_file in [
        home.join(".cursor").join("hooks").join("hooks.json"),
        home.join(".cursor").join("hooks").join("keel-cursor.sh"),
    ] {
        removed += remove_path_if_exists_counted(&hook_file).unwrap_or(0);
    }

    // Cursor MCP: install merges `keel` into ~/.cursor/mcp.json (merge_cursor_mcp).
    // Uninstall must remove that entry or Cursor keeps spawning the now-deleted
    // keel binary. Mirrors the OpenCode/Pi MCP entry-removal.
    let cursor_mcp = home.join(".cursor").join("mcp.json");
    if cursor_mcp.is_file() {
        if let Ok(text) = crate::runtime::read_text_if_exists(&cursor_mcp) {
            let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(stripped) {
                let mutated = if let Some(servers) =
                    doc.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                {
                    if servers.remove("keel").is_some() {
                        if servers.is_empty() {
                            doc.as_object_mut().map(|o| o.remove("mcpServers"));
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if mutated {
                    let _ = write_text(
                        &cursor_mcp,
                        &serde_json::to_string_pretty(&doc)
                            .unwrap_or_else(|_| stripped.to_string()),
                    );
                    removed += 1;
                }
            }
        }
    }

    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    if agents_md.is_file() {
        if let Ok(content) = std::fs::read_to_string(&agents_md) {
            if content.starts_with("# keel Iron Law for Pi Agent") {
                removed += remove_path_if_exists_counted(&agents_md).unwrap_or(0);
            }
        }
    }

    // Pi MCP config: check the correct location (~/.pi/agent/mcp.json) and
    // the legacy wrong location (~/.config/mcp/mcp.json) from older installs,
    // so uninstall cleans up both. See maybe_wire_pi for the path rationale.
    for mcp_json in [
        home.join(".pi").join("agent").join("mcp.json"),
        home.join(".config").join("mcp").join("mcp.json"),
    ] {
        if mcp_json.is_file() {
            if let Ok(text) = crate::runtime::read_text_if_exists(&mcp_json) {
                if let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(servers) = doc.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                    {
                        if servers.remove("keel").is_some() {
                            let _ = write_text(
                                &mcp_json,
                                &serde_json::to_string_pretty(&doc).unwrap_or(text.clone()),
                            );
                            removed += 1;
                        }
                    }
                }
            }
        }
    }

    // Remove the Pi extension from both the correct auto-discovery path
    // (~/.pi/agent/extensions/) and the legacy wrong path (~/.pi/extensions/).
    for ext in [
        home.join(".pi")
            .join("agent")
            .join("extensions")
            .join("keel-pi.ts"),
        home.join(".pi").join("extensions").join("keel-pi.ts"),
    ] {
        removed += remove_path_if_exists_counted(&ext).unwrap_or(0);
    }

    // Command Code (cmdc): remove the mod + MCP entry installed by
    // maybe_wire_commandcode, so nothing spawns the deleted keel binary.
    let cmdc_mod = home.join(".commandcode").join("mods").join("keel-cmdc.ts");
    removed += remove_path_if_exists_counted(&cmdc_mod).unwrap_or(0);

    let cmdc_mcp = home.join(".commandcode").join("mcp.json");
    if cmdc_mcp.is_file() {
        if let Ok(text) = crate::runtime::read_text_if_exists(&cmdc_mcp) {
            let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(stripped) {
                let mutated = if let Some(servers) =
                    doc.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                {
                    if servers.remove("keel").is_some() {
                        if servers.is_empty() {
                            doc.as_object_mut().map(|o| o.remove("mcpServers"));
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if mutated {
                    let _ = write_text(
                        &cmdc_mcp,
                        &serde_json::to_string_pretty(&doc)
                            .unwrap_or_else(|_| stripped.to_string()),
                    );
                    removed += 1;
                }
            }
        }
    }

    removed
}

fn copy_file_if_changed(source: &Path, target: &Path) -> Result<bool, String> {
    if target.is_file() {
        let source_bytes =
            fs::read(source).map_err(|error| format!("read {}: {error}", display_path(source)))?;
        let target_bytes =
            fs::read(target).map_err(|error| format!("read {}: {error}", display_path(target)))?;
        if source_bytes == target_bytes {
            return Ok(false);
        }
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    fs::copy(source, target).map_err(|error| {
        format!(
            "copy {} to {}: {error}",
            display_path(source),
            display_path(target)
        )
    })?;
    Ok(true)
}

fn write_text_if_changed(path: &Path, content: &str) -> Result<bool, String> {
    if path.is_file() {
        let existing = read_text_if_exists(path).unwrap_or_default();
        if existing == content {
            return Ok(false);
        }
    }
    write_text(path, content)?;
    Ok(true)
}

fn sync_directory_delta(
    source_directory: &Path,
    target_directory: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    if !source_directory.is_dir() {
        return Ok(0);
    }
    fs::create_dir_all(target_directory)
        .map_err(|error| format!("create {}: {error}", display_path(target_directory)))?;
    let mut changed = 0usize;
    for entry_result in fs::read_dir(source_directory)
        .map_err(|error| format!("read {}: {error}", display_path(source_directory)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        let target_path = target_directory.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            format!("read file type for {}: {error}", display_path(&source_path))
        })?;
        if file_type.is_dir() {
            changed += sync_directory_delta(&source_path, &target_path, tracker)?;
        } else if file_type.is_file() {
            if copy_file_if_changed(&source_path, &target_path)? {
                changed += 1;
            }
            tracker.record(&target_path);
        }
    }
    Ok(changed)
}

fn remove_path_if_exists_counted(path: &Path) -> Result<usize, String> {
    if !path.exists() {
        return Ok(0);
    }
    remove_path_if_exists(path)?;
    Ok(1)
}

fn sync_root_files(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut synced_count = 0;
    for root_file_name in &layout.root_files {
        let source_path = layout.root_path.join(root_file_name);
        let target_path = claude_home.join(root_file_name);
        // If the user already has a different copy, snapshot it under backups/
        // before overwrite so install never silently destroys their edits.
        if target_path.is_file() {
            let _ = backup_file_before_managed_overwrite(claude_home, &target_path, root_file_name);
        }
        if copy_file_if_changed(&source_path, &target_path)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

/// Best-effort snapshot of an existing file before managed overwrite.
/// Writes to `<claude_home>/backups/install-<ts>/<relative>`. Errors are
/// returned but callers treat backup as best-effort.
fn backup_file_before_managed_overwrite(
    claude_home: &Path,
    target_path: &Path,
    relative_name: &str,
) -> Result<(), String> {
    let existing = match fs::read(target_path) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return Ok(()),
    };
    let stamp = unix_timestamp();
    let backup_root = claude_home.join("backups").join(format!("install-{stamp}"));
    let backup_path = backup_root.join(relative_name.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create backup dir {}: {e}", display_path(parent)))?;
    }
    fs::write(&backup_path, existing)
        .map_err(|e| format!("backup {}: {e}", display_path(&backup_path)))?;
    Ok(())
}

fn sync_skills(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut synced_count = 0;
    for skill in &layout.skills {
        let target_skill_directory = skills_directory(claude_home).join(&skill.name);
        let target_skill_file = target_skill_directory.join("SKILL.md");
        if copy_file_if_changed(&skill.skill_path.join("SKILL.md"), &target_skill_file)? {
            synced_count += 1;
        }
        tracker.record(&target_skill_file);
        for relative_directory in SKILL_SYNC_DIRECTORIES {
            let source_directory = skill.skill_path.join(relative_directory);
            let target_directory = target_skill_directory.join(relative_directory);
            synced_count += sync_directory_delta(&source_directory, &target_directory, tracker)?;
        }
    }
    Ok(synced_count)
}

/// Copy cross-skill resource directories (currently `_shared/`) verbatim into
/// `<claude_home>/skills/<name>/`. Skill files reference these via relative
/// paths like `_shared/common-discipline.md`; without this step, the path
/// resolves to a missing file at runtime even though the source lives in the
/// repo. Files are recorded in the tracker so per-file orphan cleanup picks
/// up renames and deletions just like skill references do. Returns the count
/// of files actually written this run (zero on a no-op re-install), so the
/// install summary reflects real churn rather than a constant.
fn sync_shared_resources(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut changed = 0usize;
    for shared_directory_name in &layout.shared_resource_directories {
        let source_directory = layout.root_path.join(shared_directory_name);
        let target_directory = skills_directory(claude_home).join(shared_directory_name);
        changed += sync_directory_delta(&source_directory, &target_directory, tracker)?;
    }
    Ok(changed)
}

fn sync_agents(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut synced_count = 0;
    for skill in &layout.skills {
        for agent_config in &skill.agent_configs {
            let parsed = parse_agent_config(agent_config)?;
            let toml_content = render_agent_toml(&parsed, &agent_config.agent_name)?;
            let target_path = agent_profiles_directory(claude_home)
                .join(format!("{}.toml", agent_config.agent_name));
            if write_text_if_changed(&target_path, &toml_content)? {
                synced_count += 1;
            }
            tracker.record(&target_path);
        }
    }
    Ok(synced_count)
}

/// Copy the harness subagent definitions from `<repo>/.claude/agents/*.md`
/// into `<claude_home>/agents/<name>.md` so they load globally for any host
/// repo. Without this step the subagent `.md` files only resolve when the harness
/// Code spawns inside the keel checkout itself, because the harness
/// reads project-scoped `.claude/agents/` only from the active project root.
///
/// Each copied file is recorded via the tracker so the existing per-file
/// orphan sweep removes renamed or deleted definitions on the next install,
/// and the uninstall path reaches them via `managed-files.txt`.
fn sync_subagent_definitions(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let source_directory = layout.root_path.join(".claude").join("agents");
    if !source_directory.is_dir() {
        return Ok(0);
    }
    let target_directory = agents_directory(claude_home);
    fs::create_dir_all(&target_directory)
        .map_err(|error| format!("create {}: {error}", display_path(&target_directory)))?;
    let mut synced_count = 0;
    for entry_result in fs::read_dir(&source_directory)
        .map_err(|error| format!("read {}: {error}", display_path(&source_directory)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        if source_path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = match source_path.file_name() {
            Some(name) => name.to_owned(),
            None => continue,
        };
        let target_path = target_directory.join(&file_name);
        if copy_file_if_changed(&source_path, &target_path)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

/// Copy custom slash-command definitions from `<repo>/commands/*.md` into
/// `<claude_home>/commands/<name>.md` so `/keel:<name>` commands resolve
/// globally for any host repo. The harness reads project-scoped
/// `.claude/commands/` only from the active project root, so without this step
/// the commands ship only through the plugin install path, never the native
/// `keel install`.
///
/// Mirrors `sync_subagent_definitions`: only `.md` files are copied, each is
/// recorded via the tracker so the per-file orphan sweep removes renamed or
/// deleted commands on the next install, and the uninstall path reaches them
/// through `managed-files.txt`.
fn sync_commands(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let source_directory = layout.root_path.join("commands");
    if !source_directory.is_dir() {
        return Ok(0);
    }
    let target_directory = commands_directory(claude_home);
    fs::create_dir_all(&target_directory)
        .map_err(|error| format!("create {}: {error}", display_path(&target_directory)))?;
    let mut synced_count = 0;
    for entry_result in fs::read_dir(&source_directory)
        .map_err(|error| format!("read {}: {error}", display_path(&source_directory)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        if source_path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = match source_path.file_name() {
            Some(name) => name.to_owned(),
            None => continue,
        };
        let target_path = target_directory.join(&file_name);
        if copy_file_if_changed(&source_path, &target_path)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

/// Deliver `output-styles/*.md` to `~/.claude/output-styles/`. Mirrors
/// `sync_commands`: the plugin path ships these via the manifest, but a native
/// install did not, so this closes that delivery gap. Each file is tracked for
/// clean uninstall.
fn sync_output_styles(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let source_directory = layout.root_path.join("output-styles");
    if !source_directory.is_dir() {
        return Ok(0);
    }
    let target_directory = claude_home.join("output-styles");
    fs::create_dir_all(&target_directory)
        .map_err(|error| format!("create {}: {error}", display_path(&target_directory)))?;
    let mut synced_count = 0;
    for entry_result in fs::read_dir(&source_directory)
        .map_err(|error| format!("read {}: {error}", display_path(&source_directory)))?
    {
        let entry = entry_result.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        if source_path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = match source_path.file_name() {
            Some(name) => name.to_owned(),
            None => continue,
        };
        let target_path = target_directory.join(&file_name);
        if copy_file_if_changed(&source_path, &target_path)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

fn write_managed_config(claude_home: &Path) -> Result<(), String> {
    let config_file = config_path(claude_home);
    let original_text = read_text_if_exists(&config_file).unwrap_or_default();
    let cleaned_text = remove_managed_block(&original_text);
    let managed_block = format!(
        "# BEGIN MANAGED BLOCK ({})\n# END MANAGED BLOCK\n",
        unix_timestamp()
    );
    let updated_text = if cleaned_text.trim().is_empty() {
        managed_block
    } else {
        format!("{}\n{}", cleaned_text.trim_end(), managed_block)
    };
    write_text(&config_file, &updated_text)?;
    Ok(())
}

fn remove_managed_block(text: &str) -> String {
    let mut lines = Vec::new();
    let mut inside_block = false;
    for line in text.lines() {
        if line.starts_with("# BEGIN MANAGED BLOCK") {
            inside_block = true;
            continue;
        }
        if line.starts_with("# END MANAGED BLOCK") {
            inside_block = false;
            continue;
        }
        if !inside_block {
            lines.push(line);
        }
    }
    lines.join("\n")
}

pub fn publish_native_executable(
    repository_root: &Path,
    claude_home: &Path,
) -> Result<bool, String> {
    let target = detect_current_target().map_err(|error| format!("detect target: {error}"))?;
    // Three source layouts must be supported, probed in this priority order:
    //
    //   1. Cargo cross-build / CI: <repo_root>/target/<triple>/release/keel.exe.
    //      Produced when `cargo build --release --target <triple>` is invoked
    //      explicitly (the release workflow does this for cross-compile).
    //      Probed first so a CI build that staged both layouts still picks
    //      the targeted artifact over a host-arch leftover.
    //
    //   2. Cargo host-default: <repo_root>/target/release/keel.exe.
    //      Produced by plain `cargo build --release` without `--target`,
    //      which is what local contributors run by default. Without this
    //      probe, `keel install` from a Cargo-direct workspace
    //      silently returns Ok(false), prints "Published executable: false",
    //      and leaves the previously-installed binary in place — the exact
    //      "stale binary" regression that surfaced as `keel memory
    //      working-brief write` returning the long-deleted "Rust native
    //      placeholder completed without Go fallback" error against a
    //      workspace where source had moved 18+ commits past the install.
    //
    //   3. Release archive bundle: <repo_root>/keel.exe. The release
    //      workflow stages the binary at the bundle root (.github/workflows/
    //      release.yml step "Stage release bundle"), and install.ps1/install.sh
    //      pass that bundle directory as --repo-root. Probed last so a
    //      Cargo-built workspace prefers its own fresh artifact over any
    //      bundle-root leftover from a previous archive install.
    //
    // Without the bundle fallback, install.ps1 ran against a fresh release
    // archive returns Ok(false) and silently leaves a stale executable on
    // disk — the regression that reproduced "Unknown hook command:
    // post-tool-use-failure" against a binary that predated PR #54. The
    // fix here keeps both legacy probes intact and adds the missing
    // host-default probe between them.
    let cargo_targeted = repository_root
        .join("target")
        .join(target.directory_name())
        .join("release")
        .join(executable_file_name());
    let cargo_host_default = repository_root
        .join("target")
        .join("release")
        .join(executable_file_name());
    let bundle_root = repository_root.join(executable_file_name());
    let source_path = if cargo_targeted.is_file() {
        cargo_targeted
    } else if cargo_host_default.is_file() {
        cargo_host_default
    } else if bundle_root.is_file() {
        bundle_root
    } else {
        return Ok(false);
    };
    let target_path = installed_executable_path(claude_home);
    if executables_are_identical(&source_path, &target_path) {
        return Ok(false);
    }
    atomic_copy_executable(&source_path, &target_path)?;
    Ok(true)
}

/// Repair-facing executable publication: restores `<claude_home>/keel[.exe]`
/// when it is missing. Unlike [`publish_native_executable`] this probes the
/// debug build layouts too (lowest priority), so a dev workspace whose last
/// build was `cargo build -p keel` can still repair a deleted binary.
/// `Ok(false)` when no artifact exists anywhere.
pub fn restore_missing_executable(
    repository_root: &Path,
    claude_home: &Path,
) -> Result<bool, String> {
    let target = detect_current_target().map_err(|error| format!("detect target: {error}"))?;
    let target_dir = repository_root.join("target");
    // Release artifacts win; debug is the last resort so developer workspaces
    // (where plain `cargo build` is the norm) can still self-repair.
    let probes = [
        target_dir
            .join(target.directory_name())
            .join("release")
            .join(executable_file_name()),
        target_dir.join("release").join(executable_file_name()),
        repository_root.join(executable_file_name()),
        target_dir
            .join(target.directory_name())
            .join("debug")
            .join(executable_file_name()),
        target_dir.join("debug").join(executable_file_name()),
    ];
    let Some(source_path) = probes.iter().find(|probe| probe.is_file()) else {
        return Ok(false);
    };
    let target_path = installed_executable_path(claude_home);
    if target_path.is_file() {
        return Ok(false);
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }
    atomic_copy_executable(source_path, &target_path)?;
    Ok(true)
}

fn executables_are_identical(source: &Path, target: &Path) -> bool {
    if !target.is_file() {
        return false;
    }
    let source_meta = match fs::metadata(source) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    let target_meta = match fs::metadata(target) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if source_meta.len() != target_meta.len() {
        return false;
    }
    match (fs::read(source), fs::read(target)) {
        (Ok(source_bytes), Ok(target_bytes)) => source_bytes == target_bytes,
        _ => false,
    }
}

fn atomic_copy_executable(source: &Path, target: &Path) -> Result<(), String> {
    let temp_path = sibling_temp_path(target);
    let _ = fs::remove_file(&temp_path);
    fs::copy(source, &temp_path).map_err(|error| {
        format!(
            "copy {} to {}: {error}",
            display_path(source),
            display_path(&temp_path)
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&temp_path)
            .map_err(|error| format!("read metadata for {}: {error}", display_path(&temp_path)))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&temp_path, permissions).map_err(|error| {
            format!("set permissions for {}: {error}", display_path(&temp_path))
        })?;
    }
    replace_executable_in_place(&temp_path, target)
}

/// Move the staged `temp_path` into `target`, replacing any running image.
///
/// On Unix, `fs::rename` swaps the inode atomically even while the old
/// executable is still running, so a single rename is correct.
///
/// On Windows, `fs::rename` maps to `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`,
/// which must *delete* the existing `target` as part of the replace. The old
/// comment here claimed that always succeeds because the loader opens the
/// image with `FILE_SHARE_DELETE` — but that only covers the loader's own
/// handle. When the install is launched *by* the running `keel.exe`
/// (exactly what the harness does when it shells out to `keel
/// install`), or when a lifecycle hook holds a handle to the binary, the
/// delete is refused with `ERROR_ACCESS_DENIED` (os error 5) and the whole
/// install aborts with a stale binary left on disk.
///
/// The Windows-safe sequence is the standard self-update dance: rename the
/// running image out of the way first — renaming a directory entry is
/// permitted while the image is mapped — then move the new binary into the
/// freed name. The displaced image is parked under the `.stale-<ts>` sibling
/// name that `find_executable_orphans` already sweeps on the next install, so
/// even if it is still mapped (and therefore undeletable right now) it is
/// cleaned up later. A short retry absorbs transient locks from
/// concurrently-firing hooks.
fn replace_executable_in_place(temp_path: &Path, target: &Path) -> Result<(), String> {
    if !target.exists() {
        return rename_with_retry(temp_path, target).inspect_err(|_| {
            let _ = fs::remove_file(temp_path);
        });
    }

    #[cfg(windows)]
    {
        let stale_path = sibling_stale_path(target);
        // Move the running image aside so the new binary can take its name.
        // If the file is genuinely unlocked the move-aside still succeeds; if
        // even the rename is refused we fall back to a direct replace so a
        // non-running target on a permissive filesystem still updates.
        if rename_with_retry(target, &stale_path).is_err() {
            return rename_with_retry(temp_path, target).inspect_err(|_| {
                let _ = fs::remove_file(temp_path);
            });
        }
        match rename_with_retry(temp_path, target) {
            Ok(()) => {
                // Best-effort cleanup; if the displaced image is still mapped
                // the orphan sweep removes it on the next install.
                let _ = fs::remove_file(&stale_path);
                Ok(())
            }
            Err(error) => {
                // Never leave the install without a binary: restore the image
                // we moved aside, drop the staged copy, and surface the error.
                let _ = fs::rename(&stale_path, target);
                let _ = fs::remove_file(temp_path);
                Err(error)
            }
        }
    }

    #[cfg(not(windows))]
    {
        rename_with_retry(temp_path, target).inspect_err(|_| {
            let _ = fs::remove_file(temp_path);
        })
    }
}

/// Rename `from` to `to`, retrying a few times to absorb transient locks.
///
/// the harness lifecycle hooks fire frequently and each one opens the
/// installed binary; a replace can land in the brief window one of them holds
/// a handle. Five attempts at 100ms spacing clears those without making a
/// genuinely stuck replace hang for long. Cleanup of `from` on permanent
/// failure is the caller's responsibility so the move-aside path can restore
/// the original image instead of deleting it.
fn rename_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..5 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 4 {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    let error = last_error.expect("retry loop records an error before failing");
    Err(format!(
        "rename {} to {}: {error}",
        display_path(from),
        display_path(to)
    ))
}

fn sibling_temp_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".new");
    target.with_file_name(name)
}

/// Sibling path used to park a running image while the new binary takes its
/// name. The `.stale-<ts>` shape matches the prefix `find_executable_orphans`
/// already detects, so a displaced image that is still mapped (and therefore
/// undeletable in this run) is swept on the next install.
///
/// Windows-only: the move-aside dance exists solely to work around the
/// Windows refusal to delete a running image. On Unix `fs::rename` swaps the
/// inode atomically, so the displaced-image path is never compiled there and
/// this helper would be dead code (`-D warnings` would reject it).
#[cfg(windows)]
fn sibling_stale_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(format!(".stale-{}", unix_timestamp()));
    target.with_file_name(name)
}

/// Discover orphaned siblings of the installed executable. Two shapes are
/// detected:
///
/// - `keel.exe.stale-*` (and the unix equivalent): legacy artifacts
///   from a pre-`33bf860` installer naming scheme that no current code path
///   creates. Found in the wild on user disks; safe to delete.
/// - `keel.exe.new` (and the unix equivalent): atomic_copy_executable
///   writes to this path before renaming. Normally removed on success or
///   rename failure, but a process crash between fs::copy and fs::rename can
///   strand it. Only flagged if it is older than the installed executable so
///   we never race with a concurrent install.
///
/// Returns an empty vec when claude_home is unreadable or missing — diagnose
/// callers treat that as "no orphans visible" rather than an error.
pub fn find_executable_orphans(claude_home: &Path) -> Vec<PathBuf> {
    let executable_name = executable_file_name();
    let stale_prefix = format!("{executable_name}.stale-");
    let new_suffix = format!("{executable_name}.new");
    let installed_executable = installed_executable_path(claude_home);
    let installed_modified = fs::metadata(&installed_executable)
        .ok()
        .and_then(|meta| meta.modified().ok());

    let entries = match fs::read_dir(claude_home) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut orphans = Vec::new();
    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let is_stale = file_name.starts_with(&stale_prefix);
        let is_abandoned_new = file_name == new_suffix
            && match (
                fs::metadata(&path)
                    .ok()
                    .and_then(|meta| meta.modified().ok()),
                installed_modified,
            ) {
                (Some(orphan_time), Some(installed_time)) => orphan_time < installed_time,
                // No installed executable means the .new is stranded with no
                // active install to race with — safe to clean up.
                (Some(_), None) => true,
                _ => false,
            };

        if is_stale || is_abandoned_new {
            orphans.push(path);
        }
    }

    orphans
}

/// Remove orphaned siblings discovered by find_executable_orphans. Best-effort:
/// a locked file (running .exe loader) or permission error must not fail the
/// install — the next install will retry.
fn remove_executable_orphans(claude_home: &Path) -> Result<usize, String> {
    let mut removed = 0usize;
    for orphan in find_executable_orphans(claude_home) {
        if fs::remove_file(&orphan).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn write_install_metadata(
    build_version: &str,
    repository_root: &Path,
    claude_home: &Path,
) -> Result<(), String> {
    let repo_version = repo_version_for_source(build_version, repository_root);
    // Avoid `-unknown` suffixes; reuse build_version when git head is missing or already embedded.
    let manager_version = {
        let short_head = git_short_head(repository_root);
        if short_head == "unknown" || short_head.is_empty() || build_version.contains(&short_head) {
            build_version.to_string()
        } else {
            format!("{build_version}-{short_head}")
        }
    };
    let metadata = format!("repo_version={repo_version}\nmanager_version={manager_version}\n");
    write_text(
        &super::verify::install_metadata_path(claude_home),
        &metadata,
    )?;
    Ok(())
}

pub fn repo_version_for_source(build_version: &str, repository_root: &Path) -> String {
    meaningful_repo_version(build_version).unwrap_or_else(|| git_short_head(repository_root))
}

pub fn repo_version_from_metadata_or_build(metadata: &str, build_version: &str) -> Option<String> {
    super::verify::metadata_value(metadata, "repo_version")
        .filter(|value| *value != "unknown")
        .map(str::to_string)
        .or_else(|| {
            super::verify::metadata_value(metadata, "manager_version")
                .and_then(repo_version_from_build_version)
        })
        .or_else(|| meaningful_repo_version(build_version))
}

fn meaningful_repo_version(build_version: &str) -> Option<String> {
    if build_version == "dev" || build_version.is_empty() {
        return None;
    }
    Some(build_version.to_string())
}

fn repo_version_from_build_version(manager_version: &str) -> Option<String> {
    let commit_hash = manager_version.split('-').next_back()?;
    if commit_hash.len() >= 7 {
        Some(commit_hash[..7].to_string())
    } else {
        None
    }
}

fn write_inventories(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &FileTracker,
) -> Result<(), String> {
    let skill_names: Vec<String> = layout.skills.iter().map(|s| s.name.clone()).collect();
    write_lines(&managed_skills_inventory_path(claude_home), &skill_names)?;
    write_lines(
        &managed_agents_inventory_path(claude_home),
        &layout.agent_names,
    )?;
    write_lines(
        &managed_shared_resources_inventory_path(claude_home),
        &layout.shared_resource_directories,
    )?;
    let file_paths: Vec<String> = tracker.files.iter().cloned().collect();
    write_lines(&managed_files_inventory_path(claude_home), &file_paths)?;
    Ok(())
}

pub fn run_update_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("update");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_update_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let current_branch =
        current_git_branch(&repository_root).unwrap_or_else(|_| "main".to_string());
    let _ = writeln!(
        standard_output,
        "Updating repository from origin/{current_branch}"
    );
    match run_command(
        "git",
        &[
            "pull".to_string(),
            "origin".to_string(),
            current_branch.clone(),
        ],
        Some(&repository_root),
    ) {
        Ok(result) if result.code != 0 => {
            let _ = writeln!(
                standard_error,
                "git pull exited with code {}:\n{}",
                result.code,
                external_failure_detail(&result)
            );
            return result.code.clamp(1, 255) as u8;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "git pull failed: {error}");
            return 1;
        }
        Ok(_) => {}
    }
    let _ = writeln!(standard_output, "Building native Rust executable");
    let build_result = run_command(
        "cargo",
        &[
            "build".to_string(),
            "--release".to_string(),
            "--bin".to_string(),
            "keel".to_string(),
        ],
        Some(&repository_root),
    );
    match build_result {
        Ok(result) if result.code != 0 => {
            let _ = writeln!(
                standard_error,
                "cargo build exited with code {}:\n{}",
                result.code,
                external_failure_detail(&result)
            );
            return result.code.clamp(1, 255) as u8;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "cargo build failed: {error}");
            return 1;
        }
        Ok(_) => {}
    }
    let _ = writeln!(standard_output, "Installing updated skill pack");
    match install_from_paths(
        build_version,
        &repository_root,
        &claude_home,
        &InstallOverrides::default(),
        install_purge_stale_enabled(false, false),
    ) {
        Ok(summary) => {
            write_install_summary(&summary, standard_output);
            let _ = writeln!(
                standard_output,
                "Feature set: standard (persistent deterministic code and memory indexes)"
            );
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "install failed: {error}");
            1
        }
    }
}

/// Bounded failure detail from a captured external command: the trailing
/// lines of stderr (stdout when stderr is empty), where the actionable
/// error message lives. Capped so a verbose build cannot flood the report.
fn external_failure_detail(result: &crate::runtime::ProcessResult) -> String {
    const MAX_LINES: usize = 20;
    let stderr = String::from_utf8_lossy(&result.stderr);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let source = if stderr.trim().is_empty() {
        &stdout
    } else {
        &stderr
    };
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() > MAX_LINES {
        let mut detail = vec![format!(
            "... ({} earlier lines omitted)",
            lines.len() - MAX_LINES
        )];
        detail.extend(
            lines[lines.len() - MAX_LINES..]
                .iter()
                .map(|line| line.to_string()),
        );
        return detail.join("\n");
    }
    source.trim().to_string()
}

fn resolve_update_repository_root(flag_value: &str) -> Result<PathBuf, String> {
    if !flag_value.trim().is_empty() {
        return resolve_repository_root(flag_value);
    }
    match std::env::current_dir() {
        Ok(path) if repository_layout_is_complete(&path) => Ok(path),
        _ => Err("Repository root not found. Use --repo-root to specify the path.".to_string()),
    }
}

fn current_git_branch(repository_root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repository_root)
        .output()
        .map_err(|error| format!("run git: {error}"))?;
    if !output.status.success() {
        return Err("git rev-parse failed".to_string());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|error| format!("parse git output: {error}"))
}

pub fn run_uninstall_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("uninstall");
    flag_set.string_flag("claude-home", "");
    // Accept-and-ignore --repo-root for parity with the documented help
    // (`uninstall [--repo-root <path>] [--claude-home <path>]`). Uninstall does
    // not need the repository — it removes managed files from the claude home —
    // but rejecting a flag the help advertises breaks documented invocations.
    flag_set.string_flag("repo-root", "");
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
    // Engagement files (root guidance, CLAUDE.md) live in the engagement home
    // even when the keel root is `~/.keel`.
    let engagement_home = crate::runtime::claude_engagement_home(&claude_home);
    let mut removed_count = 0;
    match uninstall_managed_files(&claude_home) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(standard_error, "remove managed files failed: {error}");
            return 1;
        }
    }
    for root_file_name in ["AGENTS.md", "README.md"] {
        let path = engagement_home.join(root_file_name);
        match remove_path_if_exists_counted(&path) {
            Ok(count) => removed_count += count,
            Err(error) => {
                let _ = writeln!(standard_error, "remove {root_file_name} failed: {error}");
                return 1;
            }
        }
    }
    // Strip the keel managed block from ~/.claude/CLAUDE.md, preserving
    // any user-authored content outside the sentinels. Unlike AGENTS.md (which
    // keel owns wholesale at this path), CLAUDE.md may hold the user's own
    // global memory, so we only remove our block and delete the file solely when
    // nothing else remains.
    match remove_managed_user_claude_md(&engagement_home) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "remove managed CLAUDE.md block failed: {error}"
            );
            return 1;
        }
    }
    let executable_path = installed_executable_path(&claude_home);
    match remove_path_if_exists_counted(&executable_path) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(standard_error, "remove executable failed: {error}");
            return 1;
        }
    }
    // Remove loop-generated skills and their paired subagents. Built-in
    // (repo-synced) skills are identified by the absence of the learning marker
    // and are never touched here — they were already removed by inventory above.
    match crate::runner::learning::remove_generated_artifacts(&claude_home) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(standard_error, "remove generated artifacts failed: {error}");
            return 1;
        }
    }
    // Strip the managed hook stanzas from settings.json. Without this, an
    // uninstall leaves the harness firing hooks at a now-deleted binary every
    // session. Reuse the same removal the dedicated `hook uninstall` performs so
    // unrelated user hooks are preserved.
    if let Err(error) =
        crate::runner::hook_lifecycle::remove_managed_hook_payload_for_home(&claude_home)
    {
        let _ = writeln!(standard_error, "remove managed hooks failed: {error}");
        return 1;
    }
    // Reverse the MCP registration install wrote to ~/.claude.json. Without this,
    // an uninstall leaves a dangling `mcpServers.keel` entry pointing at
    // the now-deleted binary, which the harness tries to spawn every session.
    // Preserves sibling servers and unrelated keys.
    match super::mcp_register::unregister_mcp_server(&claude_home) {
        Ok(super::mcp_register::McpUnregistration::Removed) => removed_count += 1,
        Ok(super::mcp_register::McpUnregistration::NotPresent) => {}
        Err(error) => {
            let _ = writeln!(standard_error, "remove MCP registration failed: {error}");
            return 1;
        }
    }
    if let Err(error) = remove_deprecated_config_keys(&claude_home) {
        let _ = writeln!(
            standard_error,
            "remove deprecated config keys failed: {error}"
        );
        return 1;
    }
    removed_count += remove_wired_adapters(&claude_home);
    remove_update_temp_trees(&claude_home, &engagement_home);
    removed_count += remove_legacy_keel_leftovers(&claude_home, &engagement_home);
    removed_count += remove_dropped_first_party_artifacts(&claude_home, None);
    let _ = writeln!(standard_output, "Uninstall complete");
    let _ = writeln!(standard_output, "  Removed files: {removed_count}");
    0
}

pub fn run_self_replace_command(arguments: &[String], standard_error: &mut dyn Write) -> u8 {
    let mut flag_set = FlagSet::new("__self-replace");
    flag_set.string_flag("source", "");
    flag_set.string_flag("target", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let source = PathBuf::from(flag_set.string_value("source"));
    let target = PathBuf::from(flag_set.string_value("target"));
    if !source.is_file() || target.as_os_str().is_empty() {
        let _ = writeln!(
            standard_error,
            "__self-replace requires --source and --target"
        );
        return 1;
    }
    for _ in 0..60 {
        match atomic_copy_executable(&source, &target) {
            Ok(()) => {
                // The binary was just swapped in. A swap is exactly the drift
                // vector that leaves a stale ~/.claude.json entry behind: the
                // new binary knows about `alwaysLoad`, but nothing re-ran
                // registration. Re-assert it now (idempotent — a no-op when the
                // entry already matches) so the repair does not have to wait for
                // the next SessionStart. The target is <claude_home>/keel
                // (+ extension), so its parent is the claude home. Best-effort:
                // a failure here must not fail the swap.
                if let Some(claude_home) = target.parent() {
                    let _ = super::mcp_register::self_heal_registration(claude_home);
                }
                return 0;
            }
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
    let _ = writeln!(
        standard_error,
        "unable to replace running executable at {}",
        display_path(&target)
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_opencode_mcp_preserves_existing_keys_and_adds_keel() {
        let dir = std::env::temp_dir().join(format!("ulw-mcp-merge-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let config = dir.join("opencode.json");
        fs::write(
            &config,
            r#"{"theme":"dark","mcp":{"existing":{"type":"local","command":["foo"],"enabled":true}}}"#,
        )
        .unwrap();
        let entry =
            serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
        let result = merge_opencode_mcp(&config, "keel", &entry);
        assert!(matches!(result, Ok(OpencodeMcpResult::Added)));
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["mcp"]["existing"]["command"][0], "foo");
        assert_eq!(parsed["mcp"]["keel"]["command"][0], "bin");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_opencode_mcp_tolerates_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("ulw-mcp-bom-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let config = dir.join("opencode.json");
        let with_bom = format!("\u{feff}{}", r#"{"mcp":{}}"#);
        fs::write(&config, with_bom).unwrap();
        let entry =
            serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
        let result = merge_opencode_mcp(&config, "keel", &entry);
        assert!(
            matches!(result, Ok(OpencodeMcpResult::Added)),
            "BOM-prefixed config must parse, got {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_opencode_mcp_creates_config_when_absent() {
        let dir = std::env::temp_dir().join(format!("ulw-mcp-absent-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let config = dir.join("opencode.json");
        let entry =
            serde_json::json!({"type":"local","command":["bin","mcp","serve"],"enabled":true});
        let result = merge_opencode_mcp(&config, "keel", &entry);
        assert!(matches!(result, Ok(OpencodeMcpResult::Added)));
        assert!(config.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_temp_keel_entry_detection() {
        // A dead temp fixture dir is stale and purgeable.
        let temp = std::env::temp_dir();
        let dead = temp
            .join(format!("keel-home-split-{}", std::process::id()))
            .join(".keel");
        let _ = fs::remove_dir_all(dead.parent().unwrap());
        assert!(
            is_stale_temp_keel_entry(&dead.to_string_lossy()),
            "nonexistent keel-home-split-*/.keel must be stale"
        );

        // A LIVE temp dir is NOT stale — purge must never remove a real install.
        let live = temp
            .join(format!("keel-home-split-live-{}", std::process::id()))
            .join(".keel");
        let _ = fs::create_dir_all(&live);
        assert!(
            !is_stale_temp_keel_entry(&live.to_string_lossy()),
            "existing dir must not be purged"
        );
        let _ = fs::remove_dir_all(live.parent().unwrap());

        // The real default home is never stale regardless of existence.
        assert!(
            !is_stale_temp_keel_entry("C:\\Users\\me\\.keel"),
            "default home must never match the temp pattern"
        );
        assert!(!is_stale_temp_keel_entry(""), "empty entry is not stale");
    }

    #[test]
    fn default_home_guard_excludes_temp_fixtures() {
        // The guard that stops test installs from touching the user PATH.
        let temp = std::env::temp_dir();
        let fixture = temp
            .join(format!("keel-home-split-{}", std::process::id()))
            .join(".keel");
        assert!(
            crate::runtime::is_standard_keel_home(&fixture),
            "fixture passes the basename check (why the old guard leaked)"
        );
        assert!(
            !crate::runtime::is_default_keel_home(&fixture),
            "fixture must NOT pass the default-home guard"
        );
    }

    #[test]
    fn wire_opencode_lands_under_claude_home_parent_not_env_home() {
        let base = std::env::temp_dir().join(format!("ulw-wire-herm-{}", std::process::id()));
        let claude_home = base.join("owner-home").join(".claude");
        let _ = fs::create_dir_all(&claude_home);
        let repo = create_minimal_layout("wire-opencode-herm-repo");
        let _ = fs::create_dir_all(repo.join("opencode"));
        let _ = fs::write(
            repo.join("opencode").join("keel.ts"),
            "export default async () => ({});\n",
        );

        let summary = maybe_wire_opencode(&repo, &claude_home, true);
        assert!(
            summary.is_some(),
            "standard .claude home must wire OpenCode"
        );

        let owner_config = base
            .join("owner-home")
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join("keel.ts");
        assert!(
            owner_config.is_file(),
            "plugin must land under claude_home's parent"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn resolve_install_repository_root_prefers_current_directory() {
        let root = create_minimal_layout("resolve-install-repo-root");
        let result = resolve_install_repository_root_from_candidates(&[Some(root.clone())]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), root);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_install_repository_root_falls_back_to_executable_parent() {
        let root = create_minimal_layout("resolve-install-repo-root-fallback");
        let result = resolve_install_repository_root_from_candidates(&[None, Some(root.clone())]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), root);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_install_repository_root_fails_when_no_candidate_is_complete() {
        let result = resolve_install_repository_root_from_candidates(&[None, None]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Repository root not found"));
    }

    #[test]
    fn remove_managed_block_removes_block_from_config() {
        let text =
            "key=value\n# BEGIN MANAGED BLOCK (123)\nold=data\n# END MANAGED BLOCK\nother=line\n";
        let result = remove_managed_block(text);
        assert_eq!(result, "key=value\nother=line");
    }

    #[test]
    fn remove_managed_block_preserves_text_without_block() {
        let text = "key=value\nother=line\n";
        let result = remove_managed_block(text);
        // lines().join("\n") drops the trailing newline; that is expected behavior
        assert_eq!(result, "key=value\nother=line");
    }

    #[test]
    fn repo_version_prefers_meaningful_build_version() {
        let root = create_minimal_layout("repo-version-build");
        let result = repo_version_for_source("1.2.3", &root);
        assert_eq!(result, "1.2.3");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repo_version_falls_back_to_git_short_head() {
        let root = create_minimal_layout("repo-version-git");
        let result = repo_version_for_source("dev", &root);
        assert!(!result.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn repo_version_recovers_from_installed_metadata() {
        let metadata = "repo_version=1.2.3\nmanager_version=dev-abc123\n";
        assert_eq!(
            repo_version_from_metadata_or_build(metadata, "dev").as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn repo_version_recovers_bootstrap_commit_from_installed_metadata() {
        let metadata = "repo_version=unknown\nmanager_version=bootstrap-8c0eb1cf6c20\n";
        assert_eq!(
            repo_version_from_metadata_or_build(metadata, "dev").as_deref(),
            Some("8c0eb1c")
        );
    }

    #[test]
    fn publish_native_executable_falls_back_to_bundle_root_binary() {
        // Release archives stage the binary at <bundle>/keel.exe and
        // call `keel install --repo-root <bundle>`. The cargo path
        // (<repo_root>/target/<triple>/release/<exe>) does not exist in that
        // layout. Without the fallback, publish_native_executable returns
        // Ok(false) and silently leaves the previously-installed binary in
        // place — which is the regression that surfaced as "Unknown hook
        // command: post-tool-use-failure" against a stale on-disk binary.
        let (bundle, claude_home) = unique_paths("publish-bundle-root");
        fs::create_dir_all(&bundle).unwrap();
        fs::create_dir_all(&claude_home).unwrap();

        let bundle_executable = bundle.join(executable_file_name());
        fs::write(&bundle_executable, b"new-binary-contents").unwrap();

        let installed = installed_executable_path(&claude_home);
        fs::write(&installed, b"old-binary-contents").unwrap();

        let published = publish_native_executable(&bundle, &claude_home).unwrap();
        assert!(
            published,
            "publish must report true when copying from bundle root"
        );
        assert_eq!(fs::read(&installed).unwrap(), b"new-binary-contents");

        let _ = fs::remove_dir_all(&bundle);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn publish_native_executable_prefers_cargo_built_over_bundle_root() {
        // When both layouts exist (a developer running `install` from a
        // source tree with a residual sibling exe), the cargo-built binary
        // wins — that's the freshly compiled artifact the contributor
        // intended to install.
        let (repo, claude_home) = unique_paths("publish-prefer-cargo");
        fs::create_dir_all(&claude_home).unwrap();

        let target = detect_current_target().unwrap();
        let cargo_dir = repo
            .join("target")
            .join(target.directory_name())
            .join("release");
        fs::create_dir_all(&cargo_dir).unwrap();
        fs::write(cargo_dir.join(executable_file_name()), b"cargo-built").unwrap();
        fs::write(repo.join(executable_file_name()), b"bundle-root").unwrap();

        let installed = installed_executable_path(&claude_home);
        let published = publish_native_executable(&repo, &claude_home).unwrap();
        assert!(published);
        assert_eq!(fs::read(&installed).unwrap(), b"cargo-built");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn publish_native_executable_picks_up_cargo_host_default_layout() {
        // Plain `cargo build --release` (no --target) writes to
        // <repo>/target/release/<exe>, NOT the triple-suffixed
        // <repo>/target/<os>-<arch>/release/<exe>. Local contributors and
        // anyone who runs `keel update` from a source tree get
        // this layout. Without the host-default probe, publish returned
        // Ok(false), the install summary printed "Published executable:
        // false", and the previously-installed binary stayed in place —
        // which is the regression that surfaced as `keel memory
        // working-brief write` returning the long-deleted "Rust native
        // placeholder completed without Go fallback" error against a
        // workspace 18+ commits past the install.
        let (repo, claude_home) = unique_paths("publish-host-default");
        fs::create_dir_all(&claude_home).unwrap();

        let host_default_dir = repo.join("target").join("release");
        fs::create_dir_all(&host_default_dir).unwrap();
        fs::write(
            host_default_dir.join(executable_file_name()),
            b"host-default-build",
        )
        .unwrap();

        let installed = installed_executable_path(&claude_home);
        fs::write(&installed, b"old-binary-contents").unwrap();

        let published = publish_native_executable(&repo, &claude_home).unwrap();
        assert!(
            published,
            "publish must report true when copying from target/release host-default layout"
        );
        assert_eq!(fs::read(&installed).unwrap(), b"host-default-build");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn publish_native_executable_prefers_cargo_targeted_over_host_default() {
        // CI / cross-compile runs use `cargo build --release --target <triple>`
        // and may also leave a host-default artifact behind from an earlier
        // local build. When both exist, the targeted artifact wins because
        // it is the one the operator explicitly asked to build for the
        // install host.
        let (repo, claude_home) = unique_paths("publish-prefer-targeted");
        fs::create_dir_all(&claude_home).unwrap();

        let target = detect_current_target().unwrap();
        let targeted_dir = repo
            .join("target")
            .join(target.directory_name())
            .join("release");
        let host_default_dir = repo.join("target").join("release");
        fs::create_dir_all(&targeted_dir).unwrap();
        fs::create_dir_all(&host_default_dir).unwrap();
        fs::write(targeted_dir.join(executable_file_name()), b"cargo-targeted").unwrap();
        fs::write(
            host_default_dir.join(executable_file_name()),
            b"host-default",
        )
        .unwrap();

        let installed = installed_executable_path(&claude_home);
        let published = publish_native_executable(&repo, &claude_home).unwrap();
        assert!(published);
        assert_eq!(fs::read(&installed).unwrap(), b"cargo-targeted");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn publish_native_executable_prefers_host_default_over_bundle_root() {
        // When a Cargo-direct workspace also has a leftover bundle-root
        // exe (from a previous archive install staged into the same tree),
        // the freshly built host-default artifact wins. Rationale: a
        // contributor running `keel install` after `cargo build`
        // expects their new binary to ship, not a stale archive sibling.
        let (repo, claude_home) = unique_paths("publish-host-over-bundle");
        fs::create_dir_all(&claude_home).unwrap();

        let host_default_dir = repo.join("target").join("release");
        fs::create_dir_all(&host_default_dir).unwrap();
        fs::write(
            host_default_dir.join(executable_file_name()),
            b"host-default-fresh",
        )
        .unwrap();
        fs::write(repo.join(executable_file_name()), b"bundle-leftover").unwrap();

        let installed = installed_executable_path(&claude_home);
        let published = publish_native_executable(&repo, &claude_home).unwrap();
        assert!(published);
        assert_eq!(fs::read(&installed).unwrap(), b"host-default-fresh");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&claude_home);
    }

    #[test]
    fn replace_executable_in_place_overwrites_existing_target() {
        // The core of the Windows re-install fix: replacing an existing
        // installed binary must succeed and leave the new bytes in place,
        // with no `.new` temp and no `.stale-*` sibling stranded behind on
        // the happy path. Before the fix this went through a bare
        // `fs::rename` that returned ERROR_ACCESS_DENIED (os error 5) when
        // the install was launched by the running binary; the move-aside
        // sequence avoids deleting the in-use image.
        let (dir, _) = unique_paths("replace-in-place");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join(executable_file_name());
        fs::write(&target, b"old-installed-bytes").unwrap();
        let temp = sibling_temp_path(&target);
        fs::write(&temp, b"freshly-staged-bytes").unwrap();

        replace_executable_in_place(&temp, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"freshly-staged-bytes");
        assert!(!temp.exists(), "staged .new temp must be consumed");
        // No `.stale-*` orphan should survive a successful replace.
        let orphans = find_executable_orphans(&dir);
        assert!(
            orphans.is_empty(),
            "no orphans expected after a clean replace, found {orphans:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_executable_in_place_creates_target_when_absent() {
        // First-ever install: there is no existing binary to move aside, so
        // the staged temp is renamed straight into the target name.
        let (dir, _) = unique_paths("replace-fresh");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join(executable_file_name());
        let temp = sibling_temp_path(&target);
        fs::write(&temp, b"first-install-bytes").unwrap();

        replace_executable_in_place(&temp, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"first-install-bytes");
        assert!(!temp.exists(), "staged .new temp must be consumed");

        let _ = fs::remove_dir_all(&dir);
    }

    fn create_minimal_layout(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("reviewer")).unwrap();
        fs::write(root.join("AGENTS.md"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("00-skill-routing-and-escalation.md"), "").unwrap();
        fs::write(root.join("reviewer").join("SKILL.md"), "").unwrap();
        root
    }

    fn unique_paths(name: &str) -> (PathBuf, PathBuf) {
        let suffix = format!(
            "{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            name,
        );
        let repo = std::env::temp_dir().join(format!("delta-repo-{suffix}"));
        let home = std::env::temp_dir().join(format!("delta-home-{suffix}"));
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
        (repo, home)
    }

    fn write_skill_with_reference(root: &Path, skill: &str, reference_file: &str) {
        let skill_dir = root.join(skill);
        let references_dir = skill_dir.join("references");
        fs::create_dir_all(&references_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), format!("# {skill}\n")).unwrap();
        fs::write(references_dir.join(reference_file), "reference body\n").unwrap();
    }

    fn seed_repo(root: &Path) {
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("AGENTS.md"), "agents\n").unwrap();
        fs::write(root.join("README.md"), "readme\n").unwrap();
        fs::write(root.join("00-skill-routing-and-escalation.md"), "routing\n").unwrap();
        fs::write(
            root.join("docs/runtime-guardrails-and-memory-protocols.md"),
            "guardrails\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/open-source-memory-patterns.md"),
            "patterns\n",
        )
        .unwrap();
        fs::write(root.join("docs/security-audit-status.md"), "audit\n").unwrap();
    }

    #[test]
    fn delta_installer_removes_renamed_reference_file() {
        let (repo, home) = unique_paths("rename");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-old.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let old_file = home.join("skills/reviewer/references/10-old.md");
        assert!(
            old_file.is_file(),
            "first install should have written reference"
        );

        fs::remove_file(repo.join("reviewer/references/10-old.md")).unwrap();
        fs::write(
            repo.join("reviewer/references/11-new.md"),
            "reference body\n",
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !old_file.is_file(),
            "renamed reference file must be removed from claude home"
        );
        assert!(
            home.join("skills/reviewer/references/11-new.md").is_file(),
            "new reference file must be present"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn delta_installer_removes_orphaned_skill_directory() {
        let (repo, home) = unique_paths("orphan-skill");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        write_skill_with_reference(&repo, "git-expert", "10-g.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let orphan_dir = home.join("skills/git-expert");
        assert!(orphan_dir.is_dir(), "second skill must install");

        fs::remove_dir_all(repo.join("git-expert")).unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !orphan_dir.exists(),
            "removed skill must be cleaned up entirely"
        );
        assert!(
            home.join("skills/reviewer/SKILL.md").is_file(),
            "remaining skill must stay in place"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn delta_installer_preserves_unchanged_files_across_installs() {
        let (repo, home) = unique_paths("unchanged");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        let target = home.join("skills/reviewer/references/10-r.md");
        let mtime_before = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let mtime_after = fs::metadata(&target).unwrap().modified().unwrap();

        assert_eq!(
            mtime_before, mtime_after,
            "unchanged file must not be rewritten on second install"
        );
        assert_eq!(summary.removed_stale_files, 0);
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn delta_installer_first_install_without_inventory_creates_no_false_orphans() {
        let (repo, home) = unique_paths("first-install");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert_eq!(
            summary.removed_stale_files, 0,
            "first install must not delete anything"
        );
        assert!(home.join("skills/reviewer/SKILL.md").is_file());
        assert!(
            managed_files_inventory_path(&home).is_file(),
            "per-file inventory must be written"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_into_standard_home_writes_lifecycle_hooks() {
        // Every other install test uses a home NOT named `.claude`, so
        // `maybe_install_hooks` returns None and its write branch is never
        // exercised. This test builds a home literally named `.claude` under a
        // unique parent so the standard-home guard passes and the hook write
        // path actually runs — covering the previously-untested branch where
        // settings.json is created with the managed lifecycle stanzas.
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let parent = std::env::temp_dir().join(format!("hookhome-{suffix}"));
        let repo = std::env::temp_dir().join(format!("hookrepo-{suffix}"));
        let home = parent.join(".claude");
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_dir_all(&repo);
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");

        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        // Summary reports the install, not a skip.
        let status = summary
            .hooks_installation
            .expect("standard .claude home must attempt hook install");
        assert!(
            status.starts_with("installed at"),
            "expected an install, got: {status}"
        );

        // settings.json exists and carries managed lifecycle stanzas pointing at
        // the published binary.
        let settings_path = home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        assert!(settings_path.is_file(), "settings.json must be written");
        let text = fs::read_to_string(&settings_path).unwrap();
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            document["hooks"]["SessionStart"].is_array(),
            "SessionStart hook stanza must be present"
        );
        assert!(
            document["hooks"]["UserPromptSubmit"].is_array(),
            "UserPromptSubmit hook stanza must be present"
        );
        // The budget knob folded in by build_hooks_payload lands at the new default.
        assert_eq!(
            document
                .get("skillListingBudgetFraction")
                .and_then(serde_json::Value::as_f64),
            Some(0.06),
        );

        // Re-install is idempotent and preserves an unrelated user key.
        let mut reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        reparsed["userCustomKey"] = serde_json::json!("keep-me");
        fs::write(
            &settings_path,
            serde_json::to_string_pretty(&reparsed).unwrap(),
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            after["userCustomKey"], "keep-me",
            "unrelated user keys must survive a re-install"
        );
        assert!(
            after["hooks"]["SessionStart"].is_array(),
            "managed hooks must still be present after re-install"
        );

        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn merge_managed_claude_md_into_empty_prepends_block() {
        let block = managed_claude_md_block();
        let merged = merge_managed_claude_md("", &block);
        assert!(merged.contains(MANAGED_CLAUDE_MD_BEGIN));
        assert!(merged.contains(MANAGED_CLAUDE_MD_END));
        assert!(merged.contains("Iron Law"));
    }

    #[test]
    fn merge_managed_claude_md_preserves_user_content() {
        // A user with their own global CLAUDE.md must keep it; the managed block
        // is prepended, the user's prose survives below.
        let user = "# My personal notes\n\nAlways use tabs, never spaces.\n";
        let block = managed_claude_md_block();
        let merged = merge_managed_claude_md(user, &block);
        assert!(merged.contains("My personal notes"));
        assert!(merged.contains("Always use tabs, never spaces."));
        assert!(merged.contains(MANAGED_CLAUDE_MD_BEGIN));
        // Managed block comes first so the contract is read before user prose.
        assert!(
            merged.find(MANAGED_CLAUDE_MD_BEGIN).unwrap()
                < merged.find("My personal notes").unwrap()
        );
    }

    #[test]
    fn merge_managed_claude_md_replaces_existing_block_in_place() {
        // A re-install must refresh the managed region without duplicating it or
        // disturbing user content above and below.
        let user_above = "# Top notes\n\n";
        let stale_block =
            format!("{MANAGED_CLAUDE_MD_BEGIN}\nOLD STALE CONTRACT\n{MANAGED_CLAUDE_MD_END}");
        let user_below = "\n\n# Bottom notes\n";
        let existing = format!("{user_above}{stale_block}{user_below}");
        let merged = merge_managed_claude_md(&existing, &managed_claude_md_block());

        assert!(merged.contains("Top notes"));
        assert!(merged.contains("Bottom notes"));
        assert!(
            !merged.contains("OLD STALE CONTRACT"),
            "stale managed content must be replaced"
        );
        assert!(merged.contains("Iron Law"));
        // Exactly one managed region remains.
        assert_eq!(merged.matches(MANAGED_CLAUDE_MD_BEGIN).count(), 1);
        assert_eq!(merged.matches(MANAGED_CLAUDE_MD_END).count(), 1);
    }

    #[test]
    fn merge_managed_claude_md_is_idempotent() {
        let block = managed_claude_md_block();
        let once = merge_managed_claude_md("", &block);
        let twice = merge_managed_claude_md(&once, &block);
        assert_eq!(once, twice, "re-merging an already-current file is a no-op");
    }

    #[test]
    fn strip_managed_claude_md_removes_block_keeps_user_content() {
        let user_above = "# Top notes\n";
        let block = managed_claude_md_block();
        let user_below = "# Bottom notes\n";
        let existing = format!("{user_above}\n\n{block}\n\n{user_below}");
        let stripped = strip_managed_claude_md(&existing);
        assert!(stripped.contains("Top notes"));
        assert!(stripped.contains("Bottom notes"));
        assert!(!stripped.contains(MANAGED_CLAUDE_MD_BEGIN));
        assert!(!stripped.contains("Iron Law"));
    }

    #[test]
    fn strip_managed_claude_md_all_managed_collapses_to_empty() {
        // A file that is ONLY our block must strip to empty so the caller deletes it.
        let only_block = format!("{}\n", managed_claude_md_block());
        let stripped = strip_managed_claude_md(&only_block);
        assert!(stripped.trim().is_empty());
    }

    #[test]
    fn install_into_standard_home_writes_user_claude_md() {
        // End-to-end: a real `.claude`-named home must get ~/.claude/CLAUDE.md
        // with the managed contract, and uninstall must strip it while keeping
        // any user content. This is the hook-independent channel that lands even
        // when a gateway drops the hook additionalContext.
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let parent = std::env::temp_dir().join(format!("cmdhome-{suffix}"));
        let repo = std::env::temp_dir().join(format!("cmdrepo-{suffix}"));
        let home = parent.join(".claude");
        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_dir_all(&repo);
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");

        // Pre-seed a user-authored CLAUDE.md to prove preservation.
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("CLAUDE.md"),
            "# My global prefs\n\nUse 2-space indent.\n",
        )
        .unwrap();

        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let status = summary
            .user_claude_md
            .expect("standard home must write user CLAUDE.md");
        assert!(status.starts_with("written to"), "got: {status}");

        let claude_md = home.join("CLAUDE.md");
        let text = fs::read_to_string(&claude_md).unwrap();
        assert!(
            text.contains("Iron Law"),
            "managed contract must be present"
        );
        assert!(
            text.contains("keel MCP tools"),
            "MCP imperative must be present"
        );
        assert!(
            text.contains("Use 2-space indent."),
            "user content must be preserved"
        );

        // Re-install is idempotent.
        let resummary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(
            resummary.user_claude_md.as_deref(),
            Some("already current"),
            "second install must detect the block is already current"
        );

        // Uninstall strips the managed block but keeps user content.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_uninstall_command(
            &["--claude-home".to_string(), display_path(&home)],
            &mut out,
            &mut err,
        );
        assert_eq!(
            code,
            0,
            "uninstall stderr: {}",
            String::from_utf8_lossy(&err)
        );
        let after = fs::read_to_string(&claude_md).expect("user CLAUDE.md must survive uninstall");
        assert!(
            !after.contains("Iron Law"),
            "managed block must be stripped"
        );
        assert!(
            after.contains("Use 2-space indent."),
            "user content must survive uninstall"
        );

        let _ = fs::remove_dir_all(&parent);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn install_copies_shared_resources_alongside_skills() {
        // SKILL.md files reference _shared/common-discipline.md via relative
        // paths. Without this install step, the on-disk layout under
        // ~/.claude/skills/ is missing the _shared sibling and every reference
        // resolves to a missing file. The test seeds a repo with a _shared
        // directory and asserts the installer mirrors it into the skills
        // directory tree, including nested files. It also asserts a renamed
        // shared file is cleaned up by the existing per-file orphan sweep —
        // the shared resources go through the same FileTracker as skill
        // references, so renames behave identically.
        let (repo, home) = unique_paths("shared-resources");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let shared_dir = repo.join("_shared");
        fs::create_dir_all(shared_dir.join("nested")).unwrap();
        fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();
        fs::write(shared_dir.join("nested/extra.md"), "nested body\n").unwrap();

        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        let installed_shared = home.join("skills/_shared");
        assert!(
            installed_shared.join("common-discipline.md").is_file(),
            "top-level shared file must be installed alongside skills"
        );
        assert!(
            installed_shared.join("nested/extra.md").is_file(),
            "nested shared file must be installed alongside skills"
        );

        // Rename and reinstall — the previously installed file should be
        // cleaned up exactly like a renamed skill reference.
        fs::remove_file(shared_dir.join("common-discipline.md")).unwrap();
        fs::write(
            shared_dir.join("common-discipline-v2.md"),
            "discipline body\n",
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !installed_shared.join("common-discipline.md").is_file(),
            "renamed shared file must be removed from claude home"
        );
        assert!(
            installed_shared.join("common-discipline-v2.md").is_file(),
            "new shared file must be installed"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn reinstall_is_zero_churn_when_nothing_changed() {
        // Delta-patch guarantee: a re-install with an unchanged repo must report
        // zero synced files across every category, including shared resources.
        // Regression for the prior bug where sync_shared_resources returned the
        // directory count (always 1) instead of the real change count, so the
        // install summary always claimed churn on a no-op re-install.
        let (repo, home) = unique_paths("zero-churn");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let shared_dir = repo.join("_shared");
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();

        let first =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert!(
            first.synced_shared_resources >= 1,
            "first install must actually write the shared resource"
        );

        let second =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(second.synced_skills, 0, "no skill churn on no-op reinstall");
        assert_eq!(second.synced_agents, 0, "no agent churn on no-op reinstall");
        assert_eq!(
            second.synced_subagent_definitions, 0,
            "no subagent churn on no-op reinstall"
        );
        assert_eq!(
            second.synced_commands, 0,
            "no command churn on no-op reinstall"
        );
        assert_eq!(
            second.synced_root_files, 0,
            "no root-file churn on no-op reinstall"
        );
        assert_eq!(
            second.synced_shared_resources, 0,
            "no shared-resource churn on no-op reinstall (the fixed bug)"
        );
        assert_eq!(
            second.removed_stale_files, 0,
            "nothing stale to remove on no-op reinstall"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_removes_shared_resource_directory_when_dropped_from_repo() {
        // When the entire `_shared/` directory is removed from the repo
        // upstream, the previously-installed copy under
        // `<claude_home>/skills/_shared/` must be cleaned up — both files
        // and the now-empty directory. Without the directory-level orphan
        // sweep, an empty `_shared/` directory would persist on disk.
        let (repo, home) = unique_paths("shared-dropped");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let shared_dir = repo.join("_shared");
        fs::create_dir_all(&shared_dir).unwrap();
        fs::write(shared_dir.join("common-discipline.md"), "discipline body\n").unwrap();

        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let installed_shared = home.join("skills/_shared");
        assert!(installed_shared.is_dir(), "first install seeds shared dir");

        // Drop the whole _shared directory from the repo and reinstall.
        fs::remove_dir_all(&shared_dir).unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !installed_shared.exists(),
            "removed shared directory must be cleaned up entirely (not just its files)"
        );
        assert!(
            home.join("skills/reviewer/SKILL.md").is_file(),
            "untouched skill must stay in place"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn remove_executable_orphans_deletes_legacy_stale_siblings() {
        // Pre-`33bf860` installer used a `.stale-<timestamp>` naming scheme
        // that no current code path creates. Found in the wild on user disks
        // (e.g. C:\Users\riezh\.claude\keel.exe.stale-1778857819).
        // Cleanup is safe because nothing in the current source ever produces
        // these names.
        let (_repo, home) = unique_paths("orphan-stale");
        fs::create_dir_all(&home).unwrap();
        let executable = installed_executable_path(&home);
        if let Some(parent) = executable.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&executable, b"installed").unwrap();
        let stale_a =
            executable.with_file_name(format!("{}.stale-1778857819", executable_file_name()));
        let stale_b =
            executable.with_file_name(format!("{}.stale-1234567890", executable_file_name()));
        fs::write(&stale_a, b"legacy").unwrap();
        fs::write(&stale_b, b"legacy").unwrap();

        let removed = remove_executable_orphans(&home).unwrap();

        assert_eq!(removed, 2, "both legacy stale siblings must be cleaned up");
        assert!(!stale_a.is_file());
        assert!(!stale_b.is_file());
        assert!(
            executable.is_file(),
            "installed executable must not be touched"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn remove_executable_orphans_skips_fresh_dot_new_to_avoid_racing_install() {
        // atomic_copy_executable writes to a `.new` sibling before renaming
        // over the installed executable. A concurrent install would have a
        // `.new` newer than the installed executable; deleting it would
        // race that install. Only an abandoned `.new` (older than the
        // installed binary, or with no installed binary present) is safe to
        // remove.
        let (_repo, home) = unique_paths("orphan-new-fresh");
        fs::create_dir_all(&home).unwrap();
        let executable = installed_executable_path(&home);
        if let Some(parent) = executable.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&executable, b"installed").unwrap();
        // Sleep so the .new mtime is strictly after the installed mtime.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let dot_new = executable.with_file_name(format!("{}.new", executable_file_name()));
        fs::write(&dot_new, b"in-flight").unwrap();

        let removed = remove_executable_orphans(&home).unwrap();

        assert_eq!(
            removed, 0,
            "fresh .new must not be deleted — would race a concurrent install"
        );
        assert!(dot_new.is_file());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn remove_executable_orphans_deletes_abandoned_dot_new() {
        // A `.new` older than the installed executable is a crash artifact —
        // atomic_copy_executable normally removes it on failure, but a
        // process crash between fs::copy and fs::rename can strand it.
        let (_repo, home) = unique_paths("orphan-new-stale");
        fs::create_dir_all(&home).unwrap();
        let executable = installed_executable_path(&home);
        if let Some(parent) = executable.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let dot_new = executable.with_file_name(format!("{}.new", executable_file_name()));
        // Write the orphan first, then sleep, then write the installed
        // executable so it is strictly newer.
        fs::write(&dot_new, b"crash-leftover").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&executable, b"installed").unwrap();

        let removed = remove_executable_orphans(&home).unwrap();

        assert_eq!(removed, 1, "abandoned .new must be cleaned up");
        assert!(!dot_new.is_file());
        assert!(executable.is_file());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_copies_subagent_definitions_into_user_global_agents_directory() {
        // Without this step, the project-scoped `.claude/agents/<name>.md`
        // subagent definitions only resolve when the harness spawns inside the
        // keel checkout. Host repos see no subagents at all. The
        // installer must mirror them under `<claude_home>/agents/<name>.md`
        // so the Agent tool finds them globally. Renamed definitions must be
        // cleaned up by the existing per-file orphan sweep.
        let (repo, home) = unique_paths("subagent-defs");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let agents_source = repo.join(".claude").join("agents");
        fs::create_dir_all(&agents_source).unwrap();
        fs::write(
            agents_source.join("reviewer.md"),
            "---\nname: reviewer\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            agents_source.join("git-expert.md"),
            "---\nname: git-expert\n---\nbody\n",
        )
        .unwrap();

        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(
            summary.synced_subagent_definitions, 2,
            "first install must report two newly written subagent definitions"
        );

        let installed_reviewer = home.join("agents/reviewer.md");
        let installed_git = home.join("agents/git-expert.md");
        assert!(
            installed_reviewer.is_file(),
            "reviewer subagent definition must land in user-global agents dir"
        );
        assert!(
            installed_git.is_file(),
            "git-expert subagent definition must land in user-global agents dir"
        );

        // Reinstall with no source change must report zero writes.
        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(
            summary.synced_subagent_definitions, 0,
            "no-op reinstall must not rewrite unchanged subagent definitions"
        );

        // Rename one definition and reinstall — the old file must be cleaned
        // up by the same per-file orphan sweep that handles skill references.
        fs::remove_file(agents_source.join("git-expert.md")).unwrap();
        fs::write(
            agents_source.join("git-helper.md"),
            "---\nname: git-helper\n---\nbody\n",
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !installed_git.is_file(),
            "renamed subagent definition must be removed from claude home"
        );
        assert!(
            home.join("agents/git-helper.md").is_file(),
            "new subagent definition must be installed"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_copies_slash_commands_into_user_global_commands_directory() {
        // Custom slash commands live in `<repo>/commands/*.md` and ship through
        // the plugin manifest, but the native `keel install` must also
        // mirror them under `<claude_home>/commands/<name>.md` so
        // `/keel:<name>` resolves globally for any host repo. Renamed
        // commands must be cleaned up by the existing per-file orphan sweep.
        let (repo, home) = unique_paths("slash-commands");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let commands_source = repo.join("commands");
        fs::create_dir_all(&commands_source).unwrap();
        fs::write(
            commands_source.join("workflow.md"),
            "---\ndescription: drive a workflow\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            commands_source.join("recall.md"),
            "---\ndescription: search memory\n---\nbody\n",
        )
        .unwrap();
        // A non-markdown sibling must be ignored, matching the .md-only filter.
        fs::write(commands_source.join("notes.txt"), "ignore me\n").unwrap();

        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(
            summary.synced_commands, 2,
            "first install must report two newly written command definitions"
        );

        let installed_workflow = home.join("commands/workflow.md");
        let installed_recall = home.join("commands/recall.md");
        assert!(
            installed_workflow.is_file(),
            "workflow command must land in user-global commands dir"
        );
        assert!(
            installed_recall.is_file(),
            "recall command must land in user-global commands dir"
        );
        assert!(
            !home.join("commands/notes.txt").is_file(),
            "non-markdown files must not be copied"
        );

        // Reinstall with no source change must report zero writes.
        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        assert_eq!(
            summary.synced_commands, 0,
            "no-op reinstall must not rewrite unchanged command definitions"
        );

        // Rename one command and reinstall — the old file must be cleaned up by
        // the same per-file orphan sweep that handles skill references.
        fs::remove_file(commands_source.join("recall.md")).unwrap();
        fs::write(
            commands_source.join("gain.md"),
            "---\ndescription: report savings\n---\nbody\n",
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert!(
            !installed_recall.is_file(),
            "renamed command must be removed from claude home"
        );
        assert!(
            home.join("commands/gain.md").is_file(),
            "new command must be installed"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_never_deletes_protected_user_data_even_if_inventory_lists_them() {
        // Simulate a corrupted managed-files inventory that names user data.
        // Install must refuse to delete sessions/projects/history/etc.
        let (repo, home) = unique_paths("protect-user-data");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        let sessions = home.join("sessions").join("important.jsonl");
        fs::create_dir_all(sessions.parent().unwrap()).unwrap();
        fs::write(&sessions, "user-session-data\n").unwrap();
        let projects = home.join("projects").join("proj").join("meta.json");
        fs::create_dir_all(projects.parent().unwrap()).unwrap();
        fs::write(&projects, "{\"keep\":true}\n").unwrap();
        let history = home.join("history.jsonl");
        fs::write(&history, "chat-history\n").unwrap();

        // Poison inventory with protected paths + path traversal.
        let inventory = managed_files_inventory_path(&home);
        let mut lines = super::super::verify::read_inventory_lines(&inventory);
        lines.push("sessions/important.jsonl".into());
        lines.push("projects/proj/meta.json".into());
        lines.push("history.jsonl".into());
        lines.push("../outside.txt".into());
        lines.push("skills/../../history.jsonl".into());
        crate::runtime::write_lines(&inventory, &lines).unwrap();

        // Reinstall with purge on — protected paths must survive.
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();

        assert_eq!(
            fs::read_to_string(&sessions).unwrap().trim(),
            "user-session-data",
            "sessions must never be deleted by install purge"
        );
        assert_eq!(
            fs::read_to_string(&projects).unwrap().trim(),
            "{\"keep\":true}",
            "projects must never be deleted by install purge"
        );
        assert_eq!(
            fs::read_to_string(&history).unwrap().trim(),
            "chat-history",
            "history.jsonl must never be deleted by install purge"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn install_default_purge_off_leaves_dropped_skill_directory() {
        // One-line installer default: no orphan deletes (data-safety first).
        let (repo, home) = unique_paths("no-purge-default");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        write_skill_with_reference(&repo, "git-expert", "10-g.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), true).unwrap();
        let orphan_dir = home.join("skills/git-expert");
        assert!(orphan_dir.is_dir());

        fs::remove_dir_all(repo.join("git-expert")).unwrap();
        // purge_stale = false (default one-line install)
        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
        assert_eq!(summary.removed_stale_files, 0);
        assert!(
            orphan_dir.is_dir(),
            "without purge, dropped managed skill dir must remain"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn is_allowed_managed_orphan_rejects_protected_and_traversal() {
        assert!(!is_allowed_managed_orphan_relative("sessions/x"));
        assert!(!is_allowed_managed_orphan_relative("projects/a/b"));
        assert!(!is_allowed_managed_orphan_relative("history.jsonl"));
        assert!(!is_allowed_managed_orphan_relative("memories/workspaces/x"));
        assert!(!is_allowed_managed_orphan_relative("../etc/passwd"));
        assert!(!is_allowed_managed_orphan_relative(
            "skills/../../history.jsonl"
        ));
        assert!(!is_allowed_managed_orphan_relative("CLAUDE.md"));
        assert!(!is_allowed_managed_orphan_relative("skills/learned-myproj"));
        assert!(is_allowed_managed_orphan_relative(
            "skills/reviewer/SKILL.md"
        ));
        assert!(is_allowed_managed_orphan_relative("agents/reviewer.md"));
        assert!(is_allowed_managed_orphan_relative("AGENTS.md"));
    }

    #[test]
    fn wire_pi_copies_agents_and_mcp_to_project_root() {
        let base = std::env::temp_dir().join(format!("ulw-wire-pi-{}", std::process::id()));
        let claude_home = base.join("owner-home").join(".claude");
        let _ = fs::create_dir_all(&claude_home);
        let repo = create_minimal_layout("wire-pi-repo");
        let _ = fs::create_dir_all(repo.join("pi"));
        let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
        let _ = fs::write(
            repo.join("pi").join(".mcp.json"),
            r#"{"mcpServers":{"keel":{"command":"keel","args":["mcp","serve"]}}}"#,
        );
        let _ = fs::write(repo.join("pi").join("keel-pi.ts"), "// keel pi extension\n");

        let summary = maybe_wire_pi(&repo, &claude_home, true);
        assert!(
            summary.is_some(),
            "standard .claude home must wire Pi Agent"
        );
        let status = summary.unwrap();
        assert!(
            status.contains("AGENTS.md"),
            "must report AGENTS.md wired, got: {status}"
        );
        assert!(
            status.contains("MCP"),
            "must report MCP registered, got: {status}"
        );
        assert!(
            status.contains("keel-pi.ts"),
            "must report keel-pi.ts wired, got: {status}"
        );

        let home = claude_home.parent().unwrap();
        assert!(
            home.join(".pi").join("agent").join("AGENTS.md").is_file(),
            "Pi AGENTS.md must land in ~/.pi/agent/"
        );
        assert!(
            home.join(".pi").join("agent").join("mcp.json").is_file(),
            "Pi MCP config must land in ~/.pi/agent/mcp.json (Pi's documented location, not ~/.config/mcp/)"
        );
        assert!(
            home.join(".pi")
                .join("agent")
                .join("extensions")
                .join("keel-pi.ts")
                .is_file(),
            "Pi extension must land in ~/.pi/agent/extensions/ (Pi's auto-discovery path, not ~/.pi/extensions/)"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn wire_pi_returns_none_for_non_standard_home() {
        let repo = create_minimal_layout("wire-pi-nonstd");
        let _ = fs::create_dir_all(repo.join("pi"));
        let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
        let _ = fs::write(repo.join("pi").join(".mcp.json"), r#"{"mcpServers":{}}"#);

        let claude_home =
            std::env::temp_dir().join(format!("ulw-wire-pi-nonstd-{}", std::process::id()));
        let _ = fs::create_dir_all(&claude_home);
        let result = maybe_wire_pi(&repo, &claude_home, true);
        assert!(
            result.is_none(),
            "non-standard .claude home must return None"
        );

        let _ = fs::remove_dir_all(&claude_home);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn rewrite_codex_mcp_command_wrapped_shape() {
        // The shipped .mcp.json uses the wrapped mcp_servers shape with a
        // bare PATH-dependent `keel` command; install must rewrite it to the
        // absolute binary path.
        let mut doc = serde_json::json!({
            "mcp_servers": {
                "keel": { "command": "keel", "args": ["mcp", "serve"] }
            }
        });
        let mutated = rewrite_codex_mcp_command(&mut doc, "/home/u/.claude/keel");
        assert!(mutated, "bare keel command must be rewritten");
        assert_eq!(
            doc["mcp_servers"]["keel"]["command"], "/home/u/.claude/keel",
            "command must be the absolute binary path"
        );
        // args must be preserved.
        assert_eq!(doc["mcp_servers"]["keel"]["args"][0], "mcp");
    }

    #[test]
    fn rewrite_codex_mcp_command_idempotent() {
        // A second pass over an already-absolute command must report no
        // mutation (idempotent) so re-install/update is a no-op.
        let absolute = "/home/u/.claude/keel";
        let mut doc = serde_json::json!({
            "mcp_servers": {
                "keel": { "command": absolute, "args": ["mcp", "serve"] }
            }
        });
        let mutated = rewrite_codex_mcp_command(&mut doc, absolute);
        assert!(!mutated, "already-absolute command must not be rewritten");
    }

    #[test]
    fn rewrite_codex_mcp_command_direct_shape() {
        // A direct {"keel": {...}} shape (no mcp_servers wrapper) must also be
        // handled, for robustness against alternative Codex manifests.
        let mut doc = serde_json::json!({
            "keel": { "command": "keel", "args": ["mcp", "serve"] }
        });
        let mutated = rewrite_codex_mcp_command(&mut doc, "/x/keel.exe");
        assert!(mutated, "direct-shape bare command must be rewritten");
        assert_eq!(doc["keel"]["command"], "/x/keel.exe");
    }

    #[test]
    fn rewrite_codex_mcp_command_absent_keel_is_noop() {
        // When the keel entry is absent (or the doc is not an object), the
        // helper must report no mutation rather than panic.
        let mut doc = serde_json::json!({ "mcp_servers": {} });
        assert!(!rewrite_codex_mcp_command(&mut doc, "/x/keel"));
        let mut non_object = serde_json::json!(42);
        assert!(!rewrite_codex_mcp_command(&mut non_object, "/x/keel"));
    }

    #[test]
    fn rewrite_mcp_entry_command_rewrites_bare_command() {
        // The shipped cursor/mcp.json and pi/.mcp.json template a bare
        // PATH-dependent `keel` command; the extracted entry must be rewritten
        // to the absolute binary path before merging.
        let mut entry = serde_json::json!({
            "command": "keel",
            "args": ["mcp", "serve"],
        });
        assert!(rewrite_mcp_entry_command(&mut entry, "/x/keel.exe"));
        assert_eq!(entry["command"], "/x/keel.exe");
        // Non-command fields must be preserved.
        assert_eq!(entry["args"][0], "mcp");
    }

    #[test]
    fn rewrite_mcp_entry_command_idempotent_and_robust() {
        // Already-absolute command → no mutation (re-install is a no-op).
        let mut entry = serde_json::json!({ "command": "/x/keel", "args": [] });
        assert!(!rewrite_mcp_entry_command(&mut entry, "/x/keel"));
        // Non-object entries must not panic.
        let mut not_object = serde_json::json!(null);
        assert!(!rewrite_mcp_entry_command(&mut not_object, "/x/keel"));
    }

    #[test]
    fn wire_cursor_rewrites_mcp_command_to_absolute() {
        // Install must land the absolute installed-binary path in
        // ~/.cursor/mcp.json, not the bare PATH-dependent template value.
        let base = std::env::temp_dir().join(format!("ulw-wire-cursor-abs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let claude_home = base.join(".claude");
        let _ = fs::create_dir_all(&claude_home);
        let repo = create_minimal_layout("wire-cursor-abs");
        let _ = fs::create_dir_all(repo.join("cursor"));
        let _ = fs::write(
            repo.join("cursor").join("mcp.json"),
            r#"{"mcpServers":{"keel":{"command":"keel","args":["mcp","serve"]}}}"#,
        );

        let summary = maybe_wire_cursor(&repo, &claude_home, true);
        assert!(summary.is_some());

        let home = claude_home.parent().unwrap();
        let mcp_target = home.join(".cursor").join("mcp.json");
        assert!(mcp_target.is_file(), "cursor mcp.json must be merged");
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_target).expect("read cursor mcp.json"))
                .expect("cursor mcp.json must be valid JSON");
        let command = doc["mcpServers"]["keel"]["command"]
            .as_str()
            .expect("cursor entry must have a command");
        assert_ne!(command, "keel", "must not keep the bare template command");
        assert_eq!(
            command,
            display_path(&installed_executable_path(&claude_home)),
            "cursor MCP command must be the absolute installed binary path"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn wire_pi_rewrites_mcp_command_to_absolute() {
        // Install must land the absolute installed-binary path in
        // ~/.pi/agent/mcp.json, not the bare PATH-dependent template value.
        let base = std::env::temp_dir().join(format!("ulw-wire-pi-abs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let claude_home = base.join(".claude");
        let _ = fs::create_dir_all(&claude_home);
        let repo = create_minimal_layout("wire-pi-abs");
        let _ = fs::create_dir_all(repo.join("pi"));
        let _ = fs::write(repo.join("pi").join("AGENTS.md"), "# Pi Agent\n");
        let _ = fs::write(
            repo.join("pi").join(".mcp.json"),
            r#"{"settings":{"idleTimeout":60},"mcpServers":{"keel":{"command":"keel","args":["mcp","serve"],"lifecycle":"lazy","directTools":true}}}"#,
        );

        let summary = maybe_wire_pi(&repo, &claude_home, true);
        assert!(summary.is_some());

        let home = claude_home.parent().unwrap();
        let mcp_target = home.join(".pi").join("agent").join("mcp.json");
        assert!(mcp_target.is_file(), "pi mcp.json must be merged");
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&mcp_target).expect("read pi mcp.json"))
                .expect("pi mcp.json must be valid JSON");
        let command = doc["mcpServers"]["keel"]["command"]
            .as_str()
            .expect("pi entry must have a command");
        assert_ne!(command, "keel", "must not keep the bare template command");
        assert_eq!(
            command,
            display_path(&installed_executable_path(&claude_home)),
            "pi MCP command must be the absolute installed binary path"
        );
        // Sibling fields from the shipped template must survive the rewrite.
        assert_eq!(doc["mcpServers"]["keel"]["lifecycle"], "lazy");
        assert_eq!(doc["mcpServers"]["keel"]["directTools"], true);
        assert_eq!(doc["settings"]["idleTimeout"], 60);

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&repo);
    }

    fn unique_codex_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keel-codex-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a fake user home holding `.keel` (the neutral root) and `.claude`
    /// (the engagement home) so migration tests run hermetically. Returns
    /// `(home, keel_home, claude_home)`.
    fn legacy_home_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let home =
            std::env::temp_dir().join(format!("keel-migrate-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let keel_home = home.join(".keel");
        let claude_home = home.join(".claude");
        fs::create_dir_all(&keel_home).unwrap();
        fs::create_dir_all(&claude_home).unwrap();
        (home, keel_home, claude_home)
    }

    #[test]
    fn migration_moves_keel_owned_data_to_neutral_home() {
        let (_home, keel_home, claude_home) = legacy_home_fixture("move");
        // Seed legacy data that keel owns.
        fs::create_dir_all(claude_home.join("working-briefs")).unwrap();
        fs::write(claude_home.join("working-briefs/brief.json"), "{}").unwrap();
        fs::write(claude_home.join("config.toml"), "x = 1").unwrap();
        fs::create_dir_all(claude_home.join("memories")).unwrap();

        let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(report.is_some(), "migration must report when it moves data");

        assert!(keel_home.join("working-briefs/brief.json").is_file());
        assert!(keel_home.join("config.toml").is_file());
        assert!(keel_home.join("memories").is_dir());
        // Sources are gone from the legacy home.
        assert!(!claude_home.join("working-briefs").exists());
        assert!(!claude_home.join("config.toml").exists());
        let _ = fs::remove_dir_all(keel_home.parent().unwrap());
    }

    #[test]
    fn migration_merges_legacy_dir_into_existing_destination() {
        let (_home, keel_home, claude_home) = legacy_home_fixture("nooverwrite");
        // Both homes hold working-briefs: the merge keeps the destination
        // file AND brings the legacy file over.
        fs::create_dir_all(keel_home.join("working-briefs")).unwrap();
        fs::write(keel_home.join("working-briefs/new.json"), "kept").unwrap();
        fs::create_dir_all(claude_home.join("working-briefs")).unwrap();
        fs::write(claude_home.join("working-briefs/old.json"), "legacy").unwrap();

        let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(report.is_some());
        assert_eq!(
            fs::read_to_string(keel_home.join("working-briefs/new.json")).unwrap(),
            "kept"
        );
        assert_eq!(
            fs::read_to_string(keel_home.join("working-briefs/old.json")).unwrap(),
            "legacy",
            "legacy data must land beside the existing destination content"
        );
        assert!(
            !claude_home.join("working-briefs").exists(),
            "a fully merged legacy directory must be removed"
        );
        let _ = fs::remove_dir_all(keel_home.parent().unwrap());
    }

    #[test]
    fn migration_exact_path_conflict_keeps_destination_and_source() {
        let (_home, keel_home, claude_home) = legacy_home_fixture("conflict");
        // Same relative path on both sides: destination wins, the conflicting
        // legacy copy is left in place (never deleted, never overwritten).
        fs::create_dir_all(keel_home.join("memories")).unwrap();
        fs::write(keel_home.join("memories/note.md"), "fresh").unwrap();
        fs::create_dir_all(claude_home.join("memories")).unwrap();
        fs::write(claude_home.join("memories/note.md"), "legacy").unwrap();

        let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(report.is_some());
        assert_eq!(
            fs::read_to_string(keel_home.join("memories/note.md")).unwrap(),
            "fresh",
            "destination content must win an exact-path conflict"
        );
        assert_eq!(
            fs::read_to_string(claude_home.join("memories/note.md")).unwrap(),
            "legacy",
            "the conflicting legacy copy must stay for manual reconciliation"
        );
        let _ = fs::remove_dir_all(keel_home.parent().unwrap());
    }

    #[test]
    fn migration_is_noop_when_same_root_or_non_standard() {
        // Non-standard root: engagement == root, so nothing migrates.
        let dir = unique_codex_test_dir("noop");
        let result = migrate_from_legacy_claude_home(&dir, &dir);
        assert!(result.is_none(), "non-standard roots must not migrate");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_noop_when_legacy_home_absent() {
        let home = std::env::temp_dir().join(format!("keel-migrate-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let keel_home = home.join(".keel");
        fs::create_dir_all(&keel_home).unwrap();
        let claude_home = home.join(".claude"); // does not exist
        let result = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(result.is_none(), "no legacy home means nothing to migrate");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn migration_replaces_empty_destination_dir_but_keeps_non_empty() {
        // Ordering regression: an empty scaffolded destination gets replaced
        // by legacy data; a non-empty one merges, nothing overwritten.
        let (_home, keel_home, claude_home) = legacy_home_fixture("emptydest");
        // Empty scaffolded destination + real legacy data -> moved.
        fs::create_dir_all(keel_home.join("memories")).unwrap(); // empty
        fs::create_dir_all(claude_home.join("memories")).unwrap();
        fs::write(claude_home.join("memories/note.md"), "real").unwrap();
        // Non-empty destination + legacy data with a DISTINCT path -> merged.
        fs::create_dir_all(keel_home.join("raw-output")).unwrap();
        fs::write(keel_home.join("raw-output/new.json"), "fresh").unwrap();
        fs::create_dir_all(claude_home.join("raw-output")).unwrap();
        fs::write(claude_home.join("raw-output/old.json"), "legacy").unwrap();

        let report = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(report.is_some());
        // Empty destination was replaced by the real data.
        assert_eq!(
            fs::read_to_string(keel_home.join("memories/note.md")).unwrap(),
            "real"
        );
        assert!(!claude_home.join("memories").exists());
        // Non-empty destination merged: BOTH files present, legacy dir removed.
        assert!(keel_home.join("raw-output/new.json").is_file());
        assert!(
            keel_home.join("raw-output/old.json").is_file(),
            "legacy data with a distinct path must merge into the destination"
        );
        assert!(
            !claude_home.join("raw-output").exists(),
            "a fully merged legacy directory must be removed"
        );
        let _ = fs::remove_dir_all(keel_home.parent().unwrap());
    }

    #[test]
    fn migration_idempotent_second_run_is_quiet() {
        let (_home, keel_home, claude_home) = legacy_home_fixture("idem");
        fs::write(claude_home.join("config.toml"), "x = 1").unwrap();
        let first = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(first.is_some());
        let second = migrate_from_legacy_claude_home(&keel_home, &claude_home);
        assert!(
            second.is_none(),
            "second run must be a no-op once migrated, got {second:?}"
        );
        let _ = fs::remove_dir_all(keel_home.parent().unwrap());
    }

    #[test]
    fn copy_tree_copies_nested_directories() {
        let dir = unique_codex_test_dir("copytree");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();
        fs::write(src.join("a/b/deep.txt"), "deep").unwrap();

        assert!(copy_tree(&src, &dst));
        assert_eq!(fs::read_to_string(dst.join("root.txt")).unwrap(), "root");
        assert_eq!(
            fs::read_to_string(dst.join("a/b/deep.txt")).unwrap(),
            "deep"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_temp_trees_are_deleted_and_inventories_stay() {
        let dir = unique_codex_test_dir("upd-cache");
        let keel_home = dir.join(".keel");
        let engagement = dir.join(".claude");
        fs::create_dir_all(keel_home.join("cache/update/v1")).unwrap();
        fs::write(keel_home.join("cache/update/v1/bin"), "tmp").unwrap();
        fs::create_dir_all(state_directory(&keel_home)).unwrap();
        fs::write(state_directory(&keel_home).join("managed-files.txt"), "x").unwrap();
        fs::create_dir_all(legacy_state_directory(&engagement).join("bin")).unwrap();
        fs::write(legacy_state_directory(&engagement).join("bin/old"), "stale").unwrap();
        remove_update_temp_trees(&keel_home, &engagement);
        assert!(
            !update_cache_directory(&keel_home).exists(),
            "keel-home update cache must be deleted"
        );
        assert!(
            state_directory(&keel_home)
                .join("managed-files.txt")
                .is_file(),
            "install inventories must survive cache cleanup"
        );
        assert!(
            !legacy_state_directory(&engagement).exists(),
            "leftover ~/.claude/.claude-skill-manager must be deleted"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_removes_old_claude_home_keel_leftovers() {
        let dir = unique_codex_test_dir("uninst-legacy");
        let keel_home = dir.join(".keel");
        let engagement = dir.join(".claude");
        fs::create_dir_all(&keel_home).unwrap();
        fs::create_dir_all(engagement.join("working-briefs")).unwrap();
        fs::write(engagement.join("working-briefs/old.json"), "{}").unwrap();
        fs::write(engagement.join("command-compaction-events.jsonl"), "").unwrap();
        fs::write(engagement.join("config.toml"), "x=1").unwrap();
        fs::write(engagement.join(executable_file_name()), "old-bin").unwrap();
        fs::create_dir_all(engagement.join("workflow")).unwrap();
        let removed = remove_legacy_keel_leftovers(&keel_home, &engagement);
        assert!(removed > 0);
        assert!(!engagement.join("working-briefs").exists());
        assert!(!engagement.join("command-compaction-events.jsonl").exists());
        assert!(!engagement.join("config.toml").exists());
        assert!(!engagement.join("workflow").exists());
        assert!(!engagement.join(executable_file_name()).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_without_purge_stale_still_removes_dropped_first_party_surfaces() {
        let (repo, home) = unique_paths("drop-sprint");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        fs::create_dir_all(home.join("skills/running-a-sprint")).unwrap();
        fs::write(
            home.join("skills/running-a-sprint/SKILL.md"),
            "old sprint\n",
        )
        .unwrap();
        fs::create_dir_all(home.join("commands")).unwrap();
        fs::write(home.join("commands/sprint.md"), "old sprint cmd\n").unwrap();
        fs::write(home.join("commands/user-story.md"), "old story\n").unwrap();
        fs::write(home.join("commands/workflow.md"), "old workflow\n").unwrap();

        let summary =
            install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
        assert!(
            summary.removed_stale_files >= 4,
            "dropped first-party leftovers must be counted: {}",
            summary.removed_stale_files
        );
        assert!(!home.join("skills/running-a-sprint").exists());
        assert!(!home.join("commands/sprint.md").exists());
        assert!(!home.join("commands/user-story.md").exists());
        assert!(!home.join("commands/workflow.md").exists());
        assert!(home.join("skills/reviewer/SKILL.md").is_file());
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn dropped_command_stays_when_current_pack_still_ships_it() {
        let (repo, home) = unique_paths("keep-workflow-in-pack");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        let commands_source = repo.join("commands");
        fs::create_dir_all(&commands_source).unwrap();
        fs::write(
            commands_source.join("workflow.md"),
            "---\ndescription: still in pack\n---\n",
        )
        .unwrap();
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
        assert!(
            home.join("commands/workflow.md").is_file(),
            "a command still in the source pack must not be treated as dropped"
        );
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_removes_dropped_first_party_surfaces_missing_from_inventory() {
        let (repo, home) = unique_paths("uninst-drop");
        seed_repo(&repo);
        write_skill_with_reference(&repo, "reviewer", "10-r.md");
        install_from_paths("dev", &repo, &home, &InstallOverrides::default(), false).unwrap();
        fs::create_dir_all(home.join("skills/writing-user-stories")).unwrap();
        fs::write(
            home.join("skills/writing-user-stories/SKILL.md"),
            "old stories\n",
        )
        .unwrap();
        fs::create_dir_all(home.join("commands")).unwrap();
        fs::write(home.join("commands/sprint.md"), "old\n").unwrap();

        let code = run_uninstall_command(
            &["--claude-home".to_string(), home.display().to_string()],
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(code, 0);
        assert!(!home.join("skills/writing-user-stories").exists());
        assert!(!home.join("commands/sprint.md").exists());
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn move_path_preserving_moves_file_and_dir() {
        let dir = unique_codex_test_dir("movepath");
        let src_file = dir.join("a.txt");
        let dst_file = dir.join("b.txt");
        fs::write(&src_file, "hello").unwrap();
        assert!(move_path_preserving(&src_file, &dst_file));
        assert!(!src_file.exists());
        assert_eq!(fs::read_to_string(&dst_file).unwrap(), "hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_codex_marketplace_creates_catalog_when_absent() {
        let dir = unique_codex_test_dir("market-create");
        let path = dir.join(".agents/plugins/marketplace.json");
        let result = merge_codex_marketplace(&path).unwrap();
        assert!(
            matches!(result, CodexMarketplaceResult::Added),
            "absent manifest must report Added, got {result:?}"
        );
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["name"], CODEX_PERSONAL_MARKETPLACE_NAME);
        let plugins = doc["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0]["name"], "keel");
        assert_eq!(plugins[0]["source"]["path"], "~/.codex/plugins/keel");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_codex_marketplace_preserves_siblings_and_is_idempotent() {
        let dir = unique_codex_test_dir("market-idem");
        let path = dir.join("marketplace.json");
        fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"other-plugin","source":{"source":"local","path":"~/p/other"}}]}"#,
        )
        .unwrap();
        let first = merge_codex_marketplace(&path).unwrap();
        assert!(matches!(first, CodexMarketplaceResult::Added));
        let second = merge_codex_marketplace(&path).unwrap();
        assert!(
            matches!(second, CodexMarketplaceResult::AlreadyCurrent),
            "second merge must be a no-op, got {second:?}"
        );
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["name"], "user-catalog", "user metadata preserved");
        let names: Vec<&str> = doc["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"other-plugin"), "sibling entry preserved");
        assert!(names.contains(&"keel"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_codex_marketplace_updates_stale_keel_entry() {
        let dir = unique_codex_test_dir("market-stale");
        let path = dir.join("marketplace.json");
        fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"keel","source":{"source":"local","path":"/old/path"}}]}"#,
        )
        .unwrap();
        let result = merge_codex_marketplace(&path).unwrap();
        assert!(
            matches!(result, CodexMarketplaceResult::Updated),
            "stale keel entry must report Updated, got {result:?}"
        );
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["plugins"][0]["source"]["path"], "~/.codex/plugins/keel");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_appends_section_when_absent() {
        let dir = unique_codex_test_dir("enable-absent");
        let path = dir.join("config.toml");
        fs::write(&path, "model = \"some-model\"\n").unwrap();
        let result = ensure_codex_plugin_enabled(&path).unwrap();
        assert!(matches!(result, CodexEnableResult::Added));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(CODEX_PLUGIN_CONFIG_SECTION));
        assert!(text.contains("enabled = true"));
        assert!(
            text.contains("model = \"some-model\""),
            "existing keys must survive the append"
        );
        // The result must still be valid TOML with the enabled flag set.
        let doc: toml::Value = text.parse().unwrap();
        assert_eq!(
            doc["plugins"]["keel@personal-keel"]["enabled"].as_bool(),
            Some(true)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_creates_missing_file() {
        let dir = unique_codex_test_dir("enable-newfile");
        let path = dir.join("config.toml");
        let result = ensure_codex_plugin_enabled(&path).unwrap();
        assert!(matches!(result, CodexEnableResult::Added));
        assert!(path.is_file());
        let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            doc["plugins"]["keel@personal-keel"]["enabled"].as_bool(),
            Some(true)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_is_idempotent() {
        let dir = unique_codex_test_dir("enable-idem");
        let path = dir.join("config.toml");
        assert!(matches!(
            ensure_codex_plugin_enabled(&path).unwrap(),
            CodexEnableResult::Added
        ));
        assert!(matches!(
            ensure_codex_plugin_enabled(&path).unwrap(),
            CodexEnableResult::AlreadyEnabled
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_respects_user_disable() {
        let dir = unique_codex_test_dir("enable-disabled");
        let path = dir.join("config.toml");
        let body = format!("{CODEX_PLUGIN_CONFIG_SECTION}\nenabled = false\n");
        fs::write(&path, &body).unwrap();
        let result = ensure_codex_plugin_enabled(&path).unwrap();
        assert!(
            matches!(result, CodexEnableResult::UnchangedDisabled),
            "an explicit user disable must win, got {result:?}"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            body,
            "the file must be untouched when the user disabled the plugin"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_inserts_under_existing_header() {
        let dir = unique_codex_test_dir("enable-header");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            format!("model = \"x\"\n{CODEX_PLUGIN_CONFIG_SECTION}\nother_key = 1\n"),
        )
        .unwrap();
        let result = ensure_codex_plugin_enabled(&path).unwrap();
        assert!(matches!(result, CodexEnableResult::Added));
        let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        let entry = &doc["plugins"]["keel@personal-keel"];
        assert_eq!(entry["enabled"].as_bool(), Some(true));
        assert_eq!(entry["other_key"].as_integer(), Some(1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_plugin_enabled_refuses_unparseable_toml() {
        let dir = unique_codex_test_dir("enable-badtoml");
        let path = dir.join("config.toml");
        let broken = "model = \"unterminated\n";
        fs::write(&path, broken).unwrap();
        assert!(
            ensure_codex_plugin_enabled(&path).is_err(),
            "unparseable config.toml must be refused, never mutated"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_codex_marketplace_entry_removes_keel_and_keeps_siblings() {
        let dir = unique_codex_test_dir("market-remove");
        let path = dir.join("marketplace.json");
        fs::write(
            &path,
            r#"{"name":"user-catalog","plugins":[{"name":"keel","source":{}},{"name":"keep-me","source":{}}]}"#,
        )
        .unwrap();
        assert_eq!(remove_codex_marketplace_entry(&path), 1);
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let names: Vec<&str> = doc["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["keep-me"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_codex_marketplace_entry_deletes_keel_only_catalog() {
        let dir = unique_codex_test_dir("market-remove-only");
        let path = dir.join("marketplace.json");
        fs::write(
            &path,
            r#"{"name":"personal-keel","plugins":[{"name":"keel"}]}"#,
        )
        .unwrap();
        assert!(remove_codex_marketplace_entry(&path) >= 1);
        assert!(
            !path.exists(),
            "a catalog that held only keel must be removed wholesale"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_native_mcp_appends_section_when_absent() {
        let dir = unique_codex_test_dir("natmcp-absent");
        let path = dir.join("config.toml");
        fs::write(&path, "model = \"some-model\"\n").unwrap();
        let binary = dir.join("keel.exe");
        let result = ensure_codex_native_mcp(&path, &binary).unwrap();
        assert!(matches!(result, CodexNativeMcpResult::Added));
        let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        let entry = &doc["mcp_servers"]["keel"];
        assert_eq!(
            entry["command"].as_str().unwrap(),
            display_path(&binary),
            "command must be the absolute binary path"
        );
        let args: Vec<&str> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(args, vec!["mcp", "serve"]);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("model = \"some-model\""),
            "existing keys must survive the append"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_native_mcp_is_idempotent() {
        let dir = unique_codex_test_dir("natmcp-idem");
        let path = dir.join("config.toml");
        let binary = dir.join("keel.exe");
        assert!(matches!(
            ensure_codex_native_mcp(&path, &binary).unwrap(),
            CodexNativeMcpResult::Added
        ));
        assert!(matches!(
            ensure_codex_native_mcp(&path, &binary).unwrap(),
            CodexNativeMcpResult::AlreadyCurrent
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_native_mcp_updates_stale_command_preserving_siblings() {
        let dir = unique_codex_test_dir("natmcp-stale");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.other]\ncommand = \"other-mcp\"\n\n[mcp_servers.keel]\ncommand = \"old\"\nargs = [\"mcp\", \"serve\"]\n",
        )
        .unwrap();
        let binary = dir.join("keel.exe");
        let result = ensure_codex_native_mcp(&path, &binary).unwrap();
        assert!(matches!(result, CodexNativeMcpResult::Updated));
        let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["keel"]["command"].as_str().unwrap(),
            display_path(&binary)
        );
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
            "other-mcp",
            "a sibling MCP server must survive untouched"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ensure_codex_native_mcp_escapes_windows_backslashes() {
        let dir = unique_codex_test_dir("natmcp-escape");
        let path = dir.join("config.toml");
        let binary = dir.join("sub dir").join("keel.exe");
        let result = ensure_codex_native_mcp(&path, &binary).unwrap();
        assert!(matches!(result, CodexNativeMcpResult::Added));
        // The written TOML must parse back to the exact path (escaping valid).
        let doc: toml::Value = fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["keel"]["command"].as_str().unwrap(),
            display_path(&binary)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sync_codex_agents_md_writes_contract_and_preserves_user_content() {
        let dir = unique_codex_test_dir("agents-md");
        let path = dir.join("AGENTS.md");
        fs::write(&path, "# My codex notes\n").unwrap();
        let status = sync_codex_agents_md(&path).unwrap();
        assert!(status.starts_with("AGENTS.md written"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("Iron Law"),
            "the contract must carry the Iron Law"
        );
        assert!(text.contains("My codex notes"), "user content must survive");
        assert!(text.contains(MANAGED_CODEX_AGENTS_BEGIN));
        // Second run is a no-op (already current) and never duplicates.
        let again = sync_codex_agents_md(&path).unwrap();
        assert_eq!(again, "AGENTS.md already current");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn strip_managed_region_removes_block_keeps_user_content() {
        let user_above = "# Top notes\n";
        let block = format!("{MANAGED_CODEX_AGENTS_BEGIN}\ncontract\n{MANAGED_CODEX_AGENTS_END}");
        let user_below = "# Bottom notes\n";
        let existing = format!("{user_above}\n{block}\n\n{user_below}");
        let stripped = strip_managed_region(
            &existing,
            MANAGED_CODEX_AGENTS_BEGIN,
            MANAGED_CODEX_AGENTS_END,
        )
        .unwrap();
        assert!(stripped.contains("Top notes"));
        assert!(stripped.contains("Bottom notes"));
        assert!(!stripped.contains("contract"));
        // A block-only file collapses to empty so the caller can delete it.
        let only_block =
            strip_managed_region(&block, MANAGED_CODEX_AGENTS_BEGIN, MANAGED_CODEX_AGENTS_END)
                .unwrap();
        assert!(only_block.trim().is_empty());
        // No block present -> None.
        assert!(strip_managed_region(
            "# just user\n",
            MANAGED_CODEX_AGENTS_BEGIN,
            MANAGED_CODEX_AGENTS_END
        )
        .is_none());
    }

    #[test]
    fn remove_codex_native_mcp_section_removes_only_keel() {
        let dir = unique_codex_test_dir("natmcp-remove");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "[mcp_servers.keel]\ncommand = \"x\"\nargs = []\n\n[mcp_servers.keep]\ncommand = \"y\"\n",
        )
        .unwrap();
        assert_eq!(remove_codex_native_mcp_section(&path), 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("mcp_servers.keel"));
        assert!(text.contains("mcp_servers.keep"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_codex_managed_agents_md_deletes_keel_only_file() {
        let dir = unique_codex_test_dir("agents-md-remove");
        let path = dir.join("AGENTS.md");
        let block = format!("{MANAGED_CODEX_AGENTS_BEGIN}\ncontract\n{MANAGED_CODEX_AGENTS_END}");
        fs::write(&path, &block).unwrap();
        assert!(remove_codex_managed_agents_md(&path) >= 1);
        assert!(!path.exists(), "a keel-only AGENTS.md must be removed");
        // With user content, the block is stripped but the file stays.
        fs::write(&path, format!("user stuff\n\n{block}\n")).unwrap();
        assert_eq!(remove_codex_managed_agents_md(&path), 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("user stuff"));
        assert!(!text.contains(MANAGED_CODEX_AGENTS_BEGIN));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_codex_plugin_section_removes_only_keel_section() {
        let dir = unique_codex_test_dir("section-remove");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            format!(
                "# user comment\nmodel = \"x\"\n\n{CODEX_PLUGIN_CONFIG_SECTION}\nenabled = true\n\n[plugins.\"other@market\"]\nenabled = false\n"
            ),
        )
        .unwrap();
        assert_eq!(remove_codex_plugin_section(&path), 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("keel@personal-keel"));
        assert!(text.contains("[plugins.\"other@market\"]"));
        assert!(text.contains("# user comment"), "comments must survive");
        let doc: toml::Value = text.parse().unwrap();
        assert_eq!(
            doc["plugins"]["other@market"]["enabled"].as_bool(),
            Some(false),
            "sibling plugin sections must be untouched"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn grok_stop_hook_is_silent_not_post_tool_batch() {
        let root = std::env::temp_dir().join(format!(
            "keel-grok-stop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let keel_home = root.join(".keel");
        let grok_dir = root.join(".grok");
        fs::create_dir_all(&keel_home).unwrap();
        fs::create_dir_all(&grok_dir).unwrap();
        let status = maybe_wire_grok(&keel_home).expect("wire when .grok exists");
        assert!(
            !status.contains("skipped"),
            "standard .keel + detected .grok must write hooks: {status}"
        );
        let text = fs::read_to_string(grok_dir.join("hooks").join("keel.json")).unwrap();
        assert!(
            text.contains("hook stop"),
            "Grok Stop must call silent hook stop: {text}"
        );
        assert!(
            !text.contains("post-tool-batch"),
            "Grok Stop must not inject post-tool-batch closeout: {text}"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
