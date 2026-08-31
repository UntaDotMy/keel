// Installer command orchestration.
use super::*;
use crate::runtime::{
    agent_profiles_directory, agents_directory, config_path, display_path,
    installed_executable_path, read_text_if_exists, resolve_claude_home, run_command,
    skills_directory, state_directory, update_cache_directory, write_text,
};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
fn remove_owned_file_if_marked(path: &Path, marker: &str) -> usize {
    let Ok(content) = read_text_if_exists(path) else {
        return 0;
    };
    if !content.contains(marker) {
        return 0;
    }
    remove_path_if_exists_counted(path).unwrap_or(0)
}
fn remove_empty_directory(path: &Path) -> usize {
    if !path.is_dir() {
        return 0;
    }
    let Ok(mut entries) = fs::read_dir(path) else {
        return 0;
    };
    if entries.next().is_some() {
        return 0;
    }
    fs::remove_dir(path).map(|_| 1).unwrap_or(0)
}

fn remove_managed_cursor_hooks(path: &Path) -> usize {
    let Ok(text) = read_text_if_exists(path) else {
        return 0;
    };
    let Ok(mut document) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    let (changed, hooks_empty) = {
        let Some(hooks) = document
            .get_mut("hooks")
            .and_then(serde_json::Value::as_object_mut)
        else {
            return 0;
        };
        let mut changed = false;
        for entries in hooks.values_mut() {
            let Some(entries) = entries.as_array_mut() else {
                continue;
            };
            let original_len = entries.len();
            entries.retain(|entry| {
                !entry
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|command| command.contains("keel-cursor.sh"))
            });
            changed |= entries.len() != original_len;
        }
        let hooks_empty = hooks.values().all(|entries| {
            entries
                .as_array()
                .map(|array| array.is_empty())
                .unwrap_or(false)
        });
        (changed, hooks_empty)
    };
    if !changed {
        return 0;
    }
    let only_cursor_metadata = hooks_empty
        && document
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "version" | "hooks"))
            })
            .unwrap_or(false);
    if only_cursor_metadata {
        return remove_path_if_exists_counted(path).unwrap_or(0);
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

pub(crate) fn ensure_claude_home_directories(claude_home: &Path) -> Result<(), String> {
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

pub(crate) fn uninstall_managed_files(claude_home: &Path) -> Result<usize, String> {
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
        crate::manager::verify::install_metadata_path(claude_home),
    ] {
        let _ = remove_path_if_exists_counted(&inventory)?;
    }
    // Packaged installs need this owned cache after OS temp cleanup.
    // Uninstall removes the durable copy at the lifecycle boundary.
    removed_count += remove_path_if_exists_counted(&update_cache_directory(claude_home))?;
    Ok(removed_count)
}

pub(crate) fn remove_deprecated_config_keys(claude_home: &Path) -> Result<(), String> {
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

pub(crate) fn remove_wired_adapters(claude_home: &Path) -> usize {
    if !is_standard_home(claude_home) {
        return 0;
    }
    let mut removed = 0;
    let home = match host_user_home(claude_home) {
        Some(path) => path,
        None => return 0,
    };

    let plugin_file = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    removed += remove_owned_file_if_marked(&plugin_file, "bridge-core");

    let opencode_config = home.join(".config").join("opencode").join("opencode.json");
    removed += remove_json_mcp_entry(&opencode_config, "mcp");

    let codex_dir = home.join(".codex").join("plugins").join("keel");
    for (relative, marker) in [
        ("hooks/hooks.json", "keel-codex.js"),
        ("hooks/hooks.json", "keel-codex.ts"),
        ("keel-codex.ts", "keel Codex CLI Plugin"),
        ("keel-codex.js", "resolveBinary"),
        (
            ".codex-plugin/plugin.json",
            "\"mcpServers\": \"./.mcp.json\"",
        ),
        (".mcp.json", "keel MCP server registration"),
    ] {
        removed += remove_owned_file_if_marked(&codex_dir.join(relative), marker);
    }
    removed += remove_empty_directory(&codex_dir.join("hooks"));
    removed += remove_empty_directory(&codex_dir.join(".codex-plugin"));
    removed += remove_empty_directory(&codex_dir);

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

    // Cowork's Desktop config uses the standard mcpServers container.
    let desktop_config = claude_desktop_config_path(&home);
    removed += remove_json_mcp_entry(&desktop_config, "mcpServers");
    // The legacy Cowork plugin path was never owned by the current installer;
    // leave it untouched rather than deleting a user-created directory.

    // Cursor hooks: remove only Keel's hook entries and script.
    removed += remove_managed_cursor_hooks(&home.join(".cursor").join("hooks.json"));
    removed += remove_owned_file_if_marked(
        &home.join(".cursor").join("hooks").join("keel-cursor.sh"),
        "keel Cursor",
    );

    let cursor_mcp = home.join(".cursor").join("mcp.json");
    removed += remove_json_mcp_entry(&cursor_mcp, "mcpServers");

    let agents_md = home.join(".pi").join("agent").join("AGENTS.md");
    if agents_md.is_file() {
        if let Ok(content) = std::fs::read_to_string(&agents_md) {
            if content.starts_with("# keel Iron Law for Pi Agent") {
                removed += remove_path_if_exists_counted(&agents_md).unwrap_or(0);
            }
        }
    }

    for mcp_json in [
        home.join(".pi").join("agent").join("mcp.json"),
        home.join(".config").join("mcp").join("mcp.json"),
    ] {
        removed += remove_json_mcp_entry(&mcp_json, "mcpServers");
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
        removed += remove_owned_file_if_marked(&ext, "keel Pi Agent Extension");
    }
    // Command Code (cmdc): remove only the shipped mod; preserve custom files.
    let cmdc_mod = home.join(".commandcode").join("mods").join("keel-cmdc.ts");
    removed += remove_owned_file_if_marked(&cmdc_mod, "keel Command Code (cmdc) Mod");

    let cmdc_mcp = home.join(".commandcode").join("mcp.json");
    removed += remove_json_mcp_entry(&cmdc_mcp, "mcpServers");

    let grok_home = grok_config_home(&home);
    removed += remove_owned_file_if_marked(
        &grok_home.join("hooks").join("keel.json"),
        "hook session-start",
    );
    removed += remove_codex_native_mcp_section(&grok_home.join("config.toml"));

    removed
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
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let repository_root =
        match resolve_update_repository_root(flag_set.string_value("repo-root"), &claude_home) {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        };
    if !repository_root.join(".git").is_dir() {
        return run_packaged_release_update(&claude_home, standard_output, standard_error);
    }
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
            "--ff-only".to_string(),
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
pub(crate) fn external_failure_detail(result: &crate::runtime::ProcessResult) -> String {
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

pub(crate) fn resolve_update_repository_root(
    flag_value: &str,
    keel_home: &Path,
) -> Result<PathBuf, String> {
    super::super::verify::resolve_manager_repository_root(flag_value, keel_home)
}

fn run_packaged_release_update(
    keel_home: &Path,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let metadata = read_text_if_exists(&super::super::verify::install_metadata_path(keel_home))
        .unwrap_or_default();
    let repository = super::super::verify::metadata_value(&metadata, "repository_slug")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("UntaDotMy/keel");
    if !valid_repository_slug(repository) {
        let _ = writeln!(
            standard_error,
            "invalid repository slug in install metadata"
        );
        return 1;
    }
    let _ = writeln!(standard_output, "Updating from the latest packaged release");
    let mut command = if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                r#"$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $d=Join-Path ([IO.Path]::GetTempPath()) ('keel-update-'+[Guid]::NewGuid().ToString('N')); try { New-Item -ItemType Directory -Path $d | Out-Null; $s=Join-Path $d 'install.ps1'; $c=Join-Path $d 'install.ps1.sha256'; Invoke-WebRequest -TimeoutSec 60 -Uri ($args[0]+'/install.ps1') -OutFile $s; Invoke-WebRequest -TimeoutSec 60 -Uri ($args[0]+'/install.ps1.sha256') -OutFile $c; $expected=((Get-Content -Raw $c).Trim() -split '\s+')[0].ToUpperInvariant(); if($expected -notmatch '^[0-9A-F]{64}$'){throw 'Invalid installer checksum'}; $actual=(Get-FileHash -Algorithm SHA256 $s).Hash.ToUpperInvariant(); if($actual -ne $expected){throw 'Installer checksum mismatch'}; & $s; exit $LASTEXITCODE } finally { if(Test-Path -LiteralPath $d){Remove-Item -LiteralPath $d -Recurse -Force} }"#,
                &format!(
                    "https://github.com/{repository}/releases/latest/download"
                ),
            ]);
        command
    } else {
        let mut command = Command::new("bash");
        command.args([
                "-c",
                "set -euo pipefail; d=$(mktemp -d \"${TMPDIR:-/tmp}/keel-update.XXXXXX\"); trap 'rm -rf \"$d\"' EXIT; curl -fsSL --connect-timeout 15 --max-time 60 -o \"$d/install.sh\" \"$1/install.sh\"; curl -fsSL --connect-timeout 15 --max-time 60 -o \"$d/install.sh.sha256\" \"$1/install.sh.sha256\"; expected=$(awk 'NF {print tolower($1); exit}' \"$d/install.sh.sha256\"); case \"$expected\" in (*[!0-9a-f]*|'') echo 'Invalid installer checksum' >&2; exit 1;; esac; if [ ${#expected} -ne 64 ]; then echo 'Invalid installer checksum' >&2; exit 1; fi; if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum \"$d/install.sh\" | awk '{print tolower($1)}'); else actual=$(shasum -a 256 \"$d/install.sh\" | awk '{print tolower($1)}'); fi; [ \"$actual\" = \"$expected\" ] || { echo 'Installer checksum mismatch' >&2; exit 1; }; bash \"$d/install.sh\"",
                "keel-update",
                &format!(
                    "https://github.com/{repository}/releases/latest/download"
                ),
            ]);
        command
    };
    command
        .env("KEEL_HOME", keel_home)
        .env("CLAUDE_SKILLS_REPOSITORY", repository)
        .env("CLAUDE_SKILLS_VERSION", "latest");
    let result = crate::runtime::run_prepared_command_with_timeout(
        command,
        "packaged release update",
        std::time::Duration::from_secs(300),
    );
    match result {
        Ok(process_result) => {
            crate::runtime::forward_process_result(
                &process_result,
                standard_output,
                standard_error,
            );
            process_result.code.clamp(0, 255) as u8
        }
        Err(error) => {
            let _ = writeln!(standard_error, "release update failed: {error}");
            1
        }
    }
}

fn valid_repository_slug(repository: &str) -> bool {
    repository.split_once('/').is_some_and(|(owner, name)| {
        !owner.is_empty()
            && !name.is_empty()
            && owner
                .bytes()
                .chain(name.bytes())
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    })
}

pub(crate) fn current_git_branch(repository_root: &Path) -> Result<String, String> {
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
    // Remove loop-generated skills and their paired subagents.
    match crate::runner::learning::remove_generated_artifacts(&claude_home) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(standard_error, "remove generated artifacts failed: {error}");
            return 1;
        }
    }
    // Strip the managed hook stanzas from settings.json. Without this, an
    if let Err(error) =
        crate::runner::hook_lifecycle::remove_managed_hook_payload_for_home(&claude_home)
    {
        let _ = writeln!(standard_error, "remove managed hooks failed: {error}");
        return 1;
    }
    // Reverse the MCP registration install wrote to ~/.claude.json. Without this,
    match super::super::mcp_register::unregister_mcp_server(&claude_home) {
        Ok(super::super::mcp_register::McpUnregistration::Removed) => removed_count += 1,
        Ok(super::super::mcp_register::McpUnregistration::NotPresent) => {}
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
                if let Some(claude_home) = target.parent() {
                    let _ = super::super::mcp_register::self_heal_registration(claude_home);
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
mod update_tests {
    use super::*;

    #[test]
    fn release_repository_slug_rejects_shell_metacharacters() {
        assert!(valid_repository_slug("UntaDotMy/keel"));
        assert!(!valid_repository_slug("UntaDotMy/keel;whoami"));
        assert!(!valid_repository_slug("UntaDotMy"));
        assert!(!valid_repository_slug("/keel"));
    }
}
