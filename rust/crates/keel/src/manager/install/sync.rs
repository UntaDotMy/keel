// Installer synchronization helpers.
use super::super::agent_config::{parse_agent_config, render_agent_toml, unix_timestamp};
use super::*;
use crate::runtime::{
    agent_profiles_directory, agents_directory, commands_directory, config_path, display_path,
    read_text_if_exists, remove_path_if_exists, skills_directory, write_text, RepositoryLayout,
    SKILL_SYNC_DIRECTORIES,
};
use std::fs;
use std::path::Path;
/// Deliver `output-styles/*.md` to `~/.claude/output-styles/`. Mirrors
/// `sync_commands`: the plugin path ships these via the manifest, but a native
/// install did not, so this closes that delivery gap. Each file is tracked for
/// clean uninstall.
pub(crate) fn sync_output_styles(
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
        if copy_file_if_changed(&source_path, &target_path, tracker)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

pub(crate) fn write_managed_config(claude_home: &Path) -> Result<(), String> {
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

pub(crate) fn remove_managed_block(text: &str) -> String {
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
pub(crate) fn copy_file_if_changed(
    source: &Path,
    target: &Path,
    tracker: &FileTracker<'_>,
) -> Result<bool, String> {
    if target.is_file() {
        let source_bytes =
            fs::read(source).map_err(|error| format!("read {}: {error}", display_path(source)))?;
        let target_bytes =
            fs::read(target).map_err(|error| format!("read {}: {error}", display_path(target)))?;
        if source_bytes == target_bytes {
            return Ok(false);
        }
        backup_target_before_overwrite(tracker, target)?;
    } else if target.exists() {
        return Err(format!(
            "managed target is not a file: {}",
            display_path(target)
        ));
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

pub(crate) fn write_text_if_changed(
    path: &Path,
    content: &str,
    tracker: &FileTracker<'_>,
) -> Result<bool, String> {
    if path.is_file() {
        let existing = read_text_if_exists(path).unwrap_or_default();
        if existing == content {
            return Ok(false);
        }
        backup_target_before_overwrite(tracker, path)?;
    } else if path.exists() {
        return Err(format!(
            "managed target is not a file: {}",
            display_path(path)
        ));
    }
    write_text(path, content)?;
    Ok(true)
}

pub(crate) fn backup_target_before_overwrite(
    tracker: &FileTracker<'_>,
    target_path: &Path,
) -> Result<(), String> {
    let relative_name = target_path
        .strip_prefix(tracker.claude_home)
        .map_err(|_| {
            format!(
                "managed target is outside home: {}",
                display_path(target_path)
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    backup_file_before_managed_overwrite(tracker.claude_home, target_path, &relative_name)
}

pub(crate) fn sync_directory_delta(
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
            if copy_file_if_changed(&source_path, &target_path, tracker)? {
                changed += 1;
            }
            tracker.record(&target_path);
        }
    }
    Ok(changed)
}

pub(crate) fn remove_path_if_exists_counted(path: &Path) -> Result<usize, String> {
    if !path.exists() {
        return Ok(0);
    }
    remove_path_if_exists(path)?;
    Ok(1)
}

pub(crate) fn sync_root_files(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut synced_count = 0;
    for root_file_name in &layout.root_files {
        let source_path = layout.root_path.join(root_file_name);
        let target_path = claude_home.join(root_file_name);
        if copy_file_if_changed(&source_path, &target_path, tracker)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}

/// Mandatory snapshot of an existing file before managed overwrite.
/// Each install gets a unique backup directory; backup failure aborts install.
pub(crate) fn backup_file_before_managed_overwrite(
    claude_home: &Path,
    target_path: &Path,
    relative_name: &str,
) -> Result<(), String> {
    let existing = match fs::read(target_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "read existing managed file {}: {error}",
                display_path(target_path)
            ))
        }
    };
    let backups_root = claude_home.join("backups");
    fs::create_dir_all(&backups_root).map_err(|error| {
        format!(
            "create backup root {}: {error}",
            display_path(&backups_root)
        )
    })?;
    let stamp = unix_timestamp();
    let mut attempt = 0usize;
    let backup_root = loop {
        let candidate =
            backups_root.join(format!("install-{stamp}-{}-{attempt}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt = attempt.saturating_add(1);
            }
            Err(error) => {
                return Err(format!(
                    "create backup directory {}: {error}",
                    display_path(&candidate)
                ))
            }
        }
    };
    let backup_path = backup_root.join(relative_name.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create backup dir {}: {e}", display_path(parent)))?;
    }
    fs::write(&backup_path, existing)
        .map_err(|e| format!("backup {}: {e}", display_path(&backup_path)))?;
    Ok(())
}

pub(crate) fn sync_skills(
    layout: &RepositoryLayout,
    claude_home: &Path,
    tracker: &mut FileTracker,
) -> Result<usize, String> {
    let mut synced_count = 0;
    for skill in &layout.skills {
        let target_skill_directory = skills_directory(claude_home).join(&skill.name);
        let target_skill_file = target_skill_directory.join("SKILL.md");
        if copy_file_if_changed(
            &skill.skill_path.join("SKILL.md"),
            &target_skill_file,
            tracker,
        )? {
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
pub(crate) fn sync_shared_resources(
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

pub(crate) fn sync_agents(
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
            if write_text_if_changed(&target_path, &toml_content, tracker)? {
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
pub(crate) fn sync_subagent_definitions(
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
        if copy_file_if_changed(&source_path, &target_path, tracker)? {
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
pub(crate) fn sync_commands(
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
        if copy_file_if_changed(&source_path, &target_path, tracker)? {
            synced_count += 1;
        }
        tracker.record(&target_path);
    }
    Ok(synced_count)
}
