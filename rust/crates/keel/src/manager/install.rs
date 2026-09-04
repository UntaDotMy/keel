//! Purpose: Install, sync, update, and uninstall logic for keel manager.
//! Caller: commands.rs via run_install_command, run_update_command, run_uninstall_command.
//! Dependencies: std::fs, std::io, std::path, std::process, std::thread, std::time, keel_platform, crate::args, crate::runtime.
//! Main Functions: install_from_flags, install_from_paths, sync_root_files, sync_skills, sync_shared_resources, sync_agents, sync_subagent_definitions, sync_commands, publish_native_executable, run_update_command, run_uninstall_command.
//! Side Effects: Copies managed skill-pack files, writes harness home config/state, publishes the Rust binary, runs git commands, and removes managed files during uninstall.

mod codex;
mod commands;
mod executable;
mod flags;
mod hosts;
mod managed;
mod mcp;
mod migration;
mod path;
mod sync;
#[cfg(test)]
mod tests;

pub use commands::{
    run_self_replace_command, run_uninstall_command, run_update_command, write_install_summary,
};
pub use executable::{
    find_executable_orphans, publish_native_executable, repo_version_for_source,
    repo_version_from_metadata_or_build, restore_missing_executable,
};
pub use flags::InstallOverrides;
pub(crate) use flags::PlatformName;
#[cfg(all(test, windows))]
pub(crate) use hosts::grok_hooks_are_current;
#[cfg(test)]
pub(crate) use hosts::{antigravity_hooks_payload, grok_hooks_payload};
pub(crate) use hosts::{
    grok_config_home, grok_hooks_are_effective, maybe_wire_agents_gateway, maybe_wire_antigravity,
    maybe_wire_codex, maybe_wire_commandcode, maybe_wire_cowork, maybe_wire_cursor,
    maybe_wire_grok, maybe_wire_omp, maybe_wire_opencode, maybe_wire_pi, maybe_wire_zcode,
};
pub use path::ensure_keel_home_on_path;
pub(crate) use sync::backup_file_before_managed_overwrite;

pub(crate) use codex::{
    codex_plugin_installation, ensure_codex_native_mcp, ensure_codex_plugin_enabled,
    install_codex_plugin, merge_codex_marketplace, remove_codex_managed_agents_md,
    remove_codex_marketplace_entry, remove_codex_native_mcp_section, remove_codex_plugin_cache,
    remove_codex_plugin_section, sync_codex_agents_md, CodexEnableResult, CodexMarketplaceResult,
    CodexNativeMcpResult, CodexPluginInstallation,
};
#[cfg(test)]
pub(crate) use codex::{
    strip_managed_region, CODEX_PERSONAL_MARKETPLACE_NAME, CODEX_PLUGIN_CONFIG_SECTION,
    MANAGED_CODEX_AGENTS_BEGIN, MANAGED_CODEX_AGENTS_END,
};
pub(crate) use commands::{ensure_claude_home_directories, remove_deprecated_config_keys};
pub(crate) use executable::{
    atomic_copy_executable, remove_executable_orphans, write_install_metadata, write_inventories,
};
#[cfg(test)]
pub(crate) use executable::{replace_executable_in_place, sibling_temp_path};
pub(crate) use flags::{apply_overrides, host_user_home, is_standard_home, parse_overrides};
pub(crate) use hosts::{
    claude_desktop_config_path, maybe_install_hooks, maybe_register_mcp_server,
};
#[cfg(test)]
pub(crate) use managed::is_allowed_managed_orphan_relative;
pub(crate) use managed::{
    install_purge_stale_enabled, managed_agents_inventory_path, managed_files_inventory_path,
    managed_shared_resources_inventory_path, managed_skills_inventory_path,
    maybe_sync_user_claude_md, remove_managed_user_claude_md, remove_orphans,
};
#[cfg(test)]
pub(crate) use managed::{
    managed_claude_md_block, merge_managed_claude_md, strip_managed_claude_md,
    MANAGED_CLAUDE_MD_BEGIN, MANAGED_CLAUDE_MD_END,
};
pub(crate) use mcp::{
    merge_json_mcp, remove_json_mcp_entry, rewrite_codex_mcp_command, rewrite_mcp_entry_command,
    JsonMcpMergeResult,
};
pub(crate) use migration::{
    cleanup_identical_legacy_data, migrate_from_legacy_claude_home, migrate_legacy_state_directory,
    remove_dropped_first_party_artifacts, remove_legacy_binary, remove_legacy_keel_leftovers,
    remove_update_temp_trees,
};
#[cfg(test)]
pub(crate) use migration::{copy_path_preserving, copy_tree};
#[cfg(test)]
pub(crate) use path::is_stale_temp_keel_entry;
pub(crate) use path::purge_stale_temp_keel_path_entries;
#[cfg(test)]
pub(crate) use sync::copy_file_if_changed;
pub(crate) use sync::{
    remove_managed_block, remove_path_if_exists_counted, sync_agents, sync_commands,
    sync_output_styles, sync_root_files, sync_shared_resources, sync_skills,
    sync_subagent_definitions, write_managed_config,
};

use crate::args::FlagSet;
use crate::runtime::{
    discover_repository_layout, display_path, is_default_keel_home, repository_layout_is_complete,
    resolve_claude_home, resolve_repository_root,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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
    /// Reuses Grok's Claude-compatible hooks by default, with a native fallback
    /// when Claude hook compatibility is disabled.
    pub grok_wiring: Option<String>,
    /// Publishes the small host-neutral gateway skill at ~/.agents/skills.
    pub agents_gateway_wiring: Option<String>,
    /// Oh My Pi native extension, MCP, instructions, and gateway skill.
    pub omp_wiring: Option<String>,
    /// ZCode native MCP, hooks, instructions, and gateway skill.
    pub zcode_wiring: Option<String>,
    /// Google Antigravity global plugin and always-on instructions.
    pub antigravity_wiring: Option<String>,
    /// Human-readable outcome of migrating keel-owned data out of a legacy
    /// `~/.claude` install into the host-neutral root, or `None` when there
    /// was nothing to migrate. Data-preserving by construction.
    pub migration_report: Option<String>,
    /// Human-readable outcome of putting the keel home on PATH for bash, zsh,
    /// sh/dash, fish, and Windows User PATH. None when skipped (non-default home).
    pub path_wiring: Option<String>,
}

pub(crate) struct FileTracker<'a> {
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
    // Copy keel-owned data out of a legacy ~/.claude install into the neutral
    // root; data-preserving, keel-owned names only, never overwrites.
    let mut migration_report = migrate_from_legacy_claude_home(claude_home, &engagement_home);
    migrate_legacy_state_directory(claude_home);
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
    if published_executable {
        // Remove only after the replacement binary is safely published.
        let binary_outcome = remove_legacy_binary(claude_home);
        if !binary_outcome.is_empty() {
            let report = migration_report.get_or_insert_with(String::new);
            if !report.is_empty() {
                report.push_str("; ");
            }
            report.push_str(&binary_outcome);
        }
    }
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
    let agents_gateway_wiring = maybe_wire_agents_gateway(repository_root, claude_home);
    let opencode_wiring = maybe_wire_opencode(repository_root, claude_home, detected.opencode);
    let cursor_wiring = maybe_wire_cursor(repository_root, claude_home, detected.cursor);
    let codex_wiring = maybe_wire_codex(repository_root, claude_home, detected.codex);
    let pi_wiring = maybe_wire_pi(repository_root, claude_home, detected.pi);
    let cowork_wiring = maybe_wire_cowork(repository_root, claude_home, detected.cowork);
    let commandcode_wiring =
        maybe_wire_commandcode(repository_root, claude_home, detected.commandcode);
    let grok_wiring = maybe_wire_grok(claude_home, detected.grok);
    let omp_wiring = maybe_wire_omp(repository_root, claude_home, detected.omp);
    let zcode_wiring = maybe_wire_zcode(repository_root, claude_home, detected.zcode);
    let antigravity_wiring =
        maybe_wire_antigravity(repository_root, claude_home, detected.antigravity);
    let removed_legacy_duplicates = cleanup_identical_legacy_data(claude_home, &engagement_home);
    if removed_legacy_duplicates > 0 {
        let report = migration_report.get_or_insert_with(String::new);
        if !report.is_empty() {
            report.push_str("; ");
        }
        report.push_str(&format!(
            "removed {removed_legacy_duplicates} verified legacy duplicate(s)"
        ));
    }
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
        agents_gateway_wiring,
        omp_wiring,
        zcode_wiring,
        antigravity_wiring,
        migration_report,
        path_wiring,
    })
}
