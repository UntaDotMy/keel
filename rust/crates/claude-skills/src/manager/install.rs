//! Purpose: Install, sync, update, and uninstall logic for claude-skills manager.
//! Caller: commands.rs via run_install_command, run_update_command, run_uninstall_command.
//! Dependencies: std::fs, std::io, std::path, std::process, std::thread, std::time, claude_skills_platform, crate::args, crate::runtime.
//! Main Functions: install_from_flags, install_from_paths, sync_root_files, sync_skills, sync_agents, publish_native_executable, run_update_command, run_uninstall_command.
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
    agent_profiles_directory, agents_directory, config_path, discover_repository_layout,
    display_path, executable_file_name, git_short_head, installed_executable_path,
    read_text_if_exists, remove_path_if_exists, repository_layout_is_complete, resolve_claude_home,
    resolve_repository_root, run_command, skills_directory, state_directory, write_lines,
    write_text, RepositoryLayout, SKILL_SYNC_DIRECTORIES,
};

use super::agent_config::{parse_agent_config, render_agent_toml, unix_timestamp};

#[derive(Default)]
pub struct InstallSummary {
    pub synced_skills: usize,
    pub synced_agents: usize,
    pub synced_root_files: usize,
    pub removed_stale_files: usize,
    pub removed_executable_orphans: usize,
    pub published_executable: bool,
}

#[derive(Default)]
struct SyncStats {
    created: usize,
    updated: usize,
    unchanged: usize,
    #[allow(dead_code)]
    removed: usize,
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
    let mut tracker = FileTracker::new(claude_home);

    let synced_root_files = sync_root_files(&layout, claude_home, &mut tracker)?;
    let synced_skills = sync_skills(&layout, claude_home, &mut tracker)?;
    let synced_agents = sync_agents(&layout, claude_home, &mut tracker)?;

    let removed_stale_files = remove_orphans(
        claude_home,
        &previous_files,
        &previous_skills,
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
        synced_root_files,
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

fn remove_orphans(
    claude_home: &Path,
    previous_files: &BTreeSet<String>,
    previous_skills: &BTreeSet<String>,
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
    Ok(removed)
}

pub fn write_install_summary(summary: &InstallSummary, output: &mut dyn Write) {
    let _ = writeln!(output, "Native Rust install complete");
    let _ = writeln!(output);
    let _ = writeln!(output, "Summary:");
    let _ = writeln!(output, "  Synced skills: {}", summary.synced_skills);
    let _ = writeln!(output, "  Synced agents: {}", summary.synced_agents);
    let _ = writeln!(output, "  Synced root files: {}", summary.synced_root_files);
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
    stats: &mut SyncStats,
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
            sync_directory_delta(&source_path, &target_path, stats, tracker)?;
        } else if file_type.is_file() {
            if target_path.is_file() {
                if copy_file_if_changed(&source_path, &target_path)? {
                    stats.updated += 1;
                } else {
                    stats.unchanged += 1;
                }
            } else {
                copy_file_if_changed(&source_path, &target_path)?;
                stats.created += 1;
            }
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
            let mut stats = SyncStats::default();
            sync_directory_delta(&source_directory, &target_directory, &mut stats, tracker)?;
        }
    }
    Ok(synced_count)
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
    // Two source layouts must be supported:
    //   1. Cargo-built source tree: <repo_root>/target/<triple>/release/claude-skills.exe.
    //      This is what `cargo build --release` produces and what local
    //      contributors have on disk.
    //   2. Release archive bundle: <repo_root>/claude-skills.exe. The release
    //      workflow stages the binary at the bundle root (.github/workflows/release.yml
    //      step "Stage release bundle"), and install.ps1/install.sh pass that
    //      bundle directory as --repo-root.
    // Without the fallback, install.ps1 ran against a fresh release archive
    // returns Ok(false) and silently leaves a stale executable on disk —
    // which is the regression that reproduced "Unknown hook command:
    // post-tool-use-failure" against a binary that predated PR #54.
    let cargo_built = repository_root
        .join("target")
        .join(target.directory_name())
        .join("release")
        .join(executable_file_name());
    let bundle_root = repository_root.join(executable_file_name());
    let source_path = if cargo_built.is_file() {
        cargo_built
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
    // On Windows, std::fs::rename calls MoveFileExW with MOVEFILE_REPLACE_EXISTING,
    // which atomically replaces a running .exe (loader opens it FILE_SHARE_DELETE,
    // so the directory entry is replaced while the running process keeps its handle).
    fs::rename(&temp_path, target).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "rename {} to {}: {error}",
            display_path(&temp_path),
            display_path(target)
        )
    })?;
    Ok(())
}

fn sibling_temp_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".new");
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
}
