//! Purpose: Install, sync, update, and uninstall logic for claude-skills manager.
//! Caller: commands.rs via run_install_command, run_update_command, run_uninstall_command.
//! Dependencies: std::fs, std::io, std::path, std::process, std::thread, std::time, claude_skills_platform, crate::args, crate::runtime.
//! Main Functions: install_from_flags, install_from_paths, sync_root_files, sync_skills, sync_shared_resources, sync_agents, sync_subagent_definitions, sync_commands, publish_native_executable, run_update_command, run_uninstall_command.
//! Side Effects: Copies managed skill-pack files, writes Claude home config/state, publishes the Rust binary, runs git commands, and removes managed files during uninstall.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use claude_skills_platform::detect_current_target;

use crate::args::FlagSet;
use crate::runtime::{
    agent_profiles_directory, agents_directory, commands_directory, config_path,
    discover_repository_layout, display_path, executable_file_name, git_short_head,
    installed_executable_path, read_text_if_exists, remove_path_if_exists,
    repository_layout_is_complete, resolve_claude_home, resolve_repository_root, run_command,
    skills_directory, state_directory, write_lines, write_text, RepositoryLayout,
    SKILL_SYNC_DIRECTORIES,
};

use super::agent_config::{parse_agent_config, render_agent_toml, unix_timestamp};

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
    install_from_paths(build_version, &repository_root, &claude_home)
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
) -> Result<InstallSummary, String> {
    let layout = discover_repository_layout(repository_root)?;
    ensure_claude_home_directories(claude_home)?;
    remove_deprecated_config_keys(claude_home)?;

    let previous_files = read_inventory_set(&managed_files_inventory_path(claude_home));
    let previous_skills = read_inventory_set(&managed_skills_inventory_path(claude_home));
    let previous_shared_resources =
        read_inventory_set(&managed_shared_resources_inventory_path(claude_home));
    let mut tracker = FileTracker::new(claude_home);

    let synced_root_files = sync_root_files(&layout, claude_home, &mut tracker)?;
    let synced_skills = sync_skills(&layout, claude_home, &mut tracker)?;
    let synced_shared_resources = sync_shared_resources(&layout, claude_home, &mut tracker)?;
    let synced_agents = sync_agents(&layout, claude_home, &mut tracker)?;
    let synced_subagent_definitions =
        sync_subagent_definitions(&layout, claude_home, &mut tracker)?;
    let synced_commands = sync_commands(&layout, claude_home, &mut tracker)?;

    let removed_stale_files = remove_orphans(
        claude_home,
        &previous_files,
        &previous_skills,
        &previous_shared_resources,
        &layout,
        &tracker,
    )?;

    write_managed_config(claude_home)?;
    let published_executable = publish_native_executable(repository_root, claude_home)?;
    let removed_executable_orphans = remove_executable_orphans(claude_home)?;
    write_install_metadata(build_version, repository_root, claude_home)?;
    write_inventories(&layout, claude_home, &tracker)?;
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
    })
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

fn remove_orphans(
    claude_home: &Path,
    previous_files: &BTreeSet<String>,
    previous_skills: &BTreeSet<String>,
    previous_shared_resources: &BTreeSet<String>,
    layout: &RepositoryLayout,
    tracker: &FileTracker,
) -> Result<usize, String> {
    let mut removed = 0;
    for relative in previous_files.difference(&tracker.files) {
        let absolute = claude_home.join(relative);
        if absolute.is_file() {
            removed += remove_path_if_exists_counted(&absolute)?;
        }
    }
    let current_skills: BTreeSet<String> = layout.skills.iter().map(|s| s.name.clone()).collect();
    for orphan_skill in previous_skills.difference(&current_skills) {
        let skill_directory = skills_directory(claude_home).join(orphan_skill);
        removed += remove_path_if_exists_counted(&skill_directory)?;
    }
    let current_shared_resources: BTreeSet<String> =
        layout.shared_resource_directories.iter().cloned().collect();
    for orphan_shared in previous_shared_resources.difference(&current_shared_resources) {
        let shared_directory = skills_directory(claude_home).join(orphan_shared);
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
        "  Removed stale files: {}",
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
    let file_inventory = read_inventory_set(&managed_files_inventory_path(claude_home));
    for relative in &file_inventory {
        let absolute = claude_home.join(relative);
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
) -> Result<(), String> {
    if !source_directory.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(target_directory)
        .map_err(|error| format!("create {}: {error}", display_path(target_directory)))?;
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
            sync_directory_delta(&source_path, &target_path, tracker)?;
        } else if file_type.is_file() {
            copy_file_if_changed(&source_path, &target_path)?;
            tracker.record(&target_path);
        }
    }
    Ok(())
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
        if copy_file_if_changed(&source_path, &target_path)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
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
            sync_directory_delta(&source_directory, &target_directory, tracker)?;
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
/// of shared-resource directories staged this run (zero when the repo has
/// none).
fn sync_shared_resources(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    for shared_directory_name in &layout.shared_resource_directories {
        let source_directory = layout.root_path.join(shared_directory_name);
        let target_directory = skills_directory(claude_home).join(shared_directory_name);
        sync_directory_delta(&source_directory, &target_directory, tracker)?;
    }
    Ok(layout.shared_resource_directories.len())
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

/// Copy Claude Code subagent definitions from `<repo>/.claude/agents/*.md`
/// into `<claude_home>/agents/<name>.md` so they load globally for any host
/// repo. Without this step the subagent `.md` files only resolve when Claude
/// Code spawns inside the claude_core checkout itself, because Claude Code
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
/// `<claude_home>/commands/<name>.md` so `/claude-core:<name>` commands resolve
/// globally for any host repo. Claude Code reads project-scoped
/// `.claude/commands/` only from the active project root, so without this step
/// the commands ship only through the plugin install path, never the native
/// `claude-skills install`.
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
    //   1. Cargo cross-build / CI: <repo_root>/target/<triple>/release/claude-skills.exe.
    //      Produced when `cargo build --release --target <triple>` is invoked
    //      explicitly (the release workflow does this for cross-compile).
    //      Probed first so a CI build that staged both layouts still picks
    //      the targeted artifact over a host-arch leftover.
    //
    //   2. Cargo host-default: <repo_root>/target/release/claude-skills.exe.
    //      Produced by plain `cargo build --release` without `--target`,
    //      which is what local contributors run by default. Without this
    //      probe, `claude-skills install` from a Cargo-direct workspace
    //      silently returns Ok(false), prints "Published executable: false",
    //      and leaves the previously-installed binary in place — the exact
    //      "stale binary" regression that surfaced as `claude-skills memory
    //      working-brief write` returning the long-deleted "Rust native
    //      placeholder completed without Go fallback" error against a
    //      workspace where source had moved 18+ commits past the install.
    //
    //   3. Release archive bundle: <repo_root>/claude-skills.exe. The release
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
/// handle. When the install is launched *by* the running `claude-skills.exe`
/// (exactly what Claude Code does when it shells out to `claude-skills
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
/// Claude Code lifecycle hooks fire frequently and each one opens the
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
/// - `claude-skills.exe.stale-*` (and the unix equivalent): legacy artifacts
///   from a pre-`33bf860` installer naming scheme that no current code path
///   creates. Found in the wild on user disks; safe to delete.
/// - `claude-skills.exe.new` (and the unix equivalent): atomic_copy_executable
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
    let manager_version = format!("{}-{}", build_version, git_short_head(repository_root));
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
    if let Err(error) = run_command(
        "git",
        &[
            "pull".to_string(),
            "origin".to_string(),
            current_branch.clone(),
        ],
        Some(&repository_root),
    ) {
        let _ = writeln!(standard_error, "git pull failed: {error}");
        return 1;
    }
    let _ = writeln!(standard_output, "Building native Rust executable");
    let build_result = run_command(
        "cargo",
        &[
            "build".to_string(),
            "--release".to_string(),
            "--bin".to_string(),
            "claude-skills".to_string(),
        ],
        Some(&repository_root),
    );
    if let Err(error) = build_result {
        let _ = writeln!(standard_error, "cargo build failed: {error}");
        return 1;
    }
    let _ = writeln!(standard_output, "Installing updated skill pack");
    match install_from_paths(build_version, &repository_root, &claude_home) {
        Ok(summary) => {
            write_install_summary(&summary, standard_output);
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "install failed: {error}");
            1
        }
    }
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
    let mut removed_count = 0;
    match uninstall_managed_files(&claude_home) {
        Ok(count) => removed_count += count,
        Err(error) => {
            let _ = writeln!(standard_error, "remove managed files failed: {error}");
            return 1;
        }
    }
    for root_file_name in ["AGENTS.md", "README.md"] {
        let path = claude_home.join(root_file_name);
        match remove_path_if_exists_counted(&path) {
            Ok(count) => removed_count += count,
            Err(error) => {
                let _ = writeln!(standard_error, "remove {root_file_name} failed: {error}");
                return 1;
            }
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
    // uninstall leaves Claude Code firing hooks at a now-deleted binary every
    // session. Reuse the same removal the dedicated `hook uninstall` performs so
    // unrelated user hooks are preserved.
    if let Err(error) =
        crate::runner::hook_lifecycle::remove_managed_hook_payload_for_home(&claude_home)
    {
        let _ = writeln!(standard_error, "remove managed hooks failed: {error}");
        return 1;
    }
    if let Err(error) = remove_deprecated_config_keys(&claude_home) {
        let _ = writeln!(
            standard_error,
            "remove deprecated config keys failed: {error}"
        );
        return 1;
    }
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
            Ok(()) => return 0,
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
        // Release archives stage the binary at <bundle>/claude-skills.exe and
        // call `claude-skills install --repo-root <bundle>`. The cargo path
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
        // anyone who runs `claude-skills update` from a source tree get
        // this layout. Without the host-default probe, publish returned
        // Ok(false), the install summary printed "Published executable:
        // false", and the previously-installed binary stayed in place —
        // which is the regression that surfaced as `claude-skills memory
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
        // contributor running `claude-skills install` after `cargo build`
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
        install_from_paths("dev", &repo, &home).unwrap();
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
        install_from_paths("dev", &repo, &home).unwrap();

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
        install_from_paths("dev", &repo, &home).unwrap();
        let orphan_dir = home.join("skills/git-expert");
        assert!(orphan_dir.is_dir(), "second skill must install");

        fs::remove_dir_all(repo.join("git-expert")).unwrap();
        install_from_paths("dev", &repo, &home).unwrap();

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
        install_from_paths("dev", &repo, &home).unwrap();

        let target = home.join("skills/reviewer/references/10-r.md");
        let mtime_before = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let summary = install_from_paths("dev", &repo, &home).unwrap();
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
        let summary = install_from_paths("dev", &repo, &home).unwrap();

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

        install_from_paths("dev", &repo, &home).unwrap();

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
        install_from_paths("dev", &repo, &home).unwrap();

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

        install_from_paths("dev", &repo, &home).unwrap();
        let installed_shared = home.join("skills/_shared");
        assert!(installed_shared.is_dir(), "first install seeds shared dir");

        // Drop the whole _shared directory from the repo and reinstall.
        fs::remove_dir_all(&shared_dir).unwrap();
        install_from_paths("dev", &repo, &home).unwrap();

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
        // (e.g. C:\Users\riezh\.claude\claude-skills.exe.stale-1778857819).
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
        // subagent definitions only resolve when Claude Code spawns inside the
        // claude_core checkout. Host repos see no subagents at all. The
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

        let summary = install_from_paths("dev", &repo, &home).unwrap();
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
        let summary = install_from_paths("dev", &repo, &home).unwrap();
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
        install_from_paths("dev", &repo, &home).unwrap();

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
        // the plugin manifest, but the native `claude-skills install` must also
        // mirror them under `<claude_home>/commands/<name>.md` so
        // `/claude-core:<name>` resolves globally for any host repo. Renamed
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

        let summary = install_from_paths("dev", &repo, &home).unwrap();
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
        let summary = install_from_paths("dev", &repo, &home).unwrap();
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
        install_from_paths("dev", &repo, &home).unwrap();

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
}
