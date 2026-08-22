// Installer executable publication helpers.
use super::super::agent_config::unix_timestamp;
use super::*;
use crate::runtime::{
    display_path, executable_file_name, git_short_head, installed_executable_path, write_lines,
    write_text, RepositoryLayout,
};
use keel_platform::detect_current_target;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
pub fn publish_native_executable(
    repository_root: &Path,
    claude_home: &Path,
) -> Result<bool, String> {
    let target = detect_current_target().map_err(|error| format!("detect target: {error}"))?;
    // Three source layouts must be supported, probed in this priority order:
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

pub(crate) fn executables_are_identical(source: &Path, target: &Path) -> bool {
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

pub(crate) fn atomic_copy_executable(source: &Path, target: &Path) -> Result<(), String> {
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
pub(crate) fn replace_executable_in_place(temp_path: &Path, target: &Path) -> Result<(), String> {
    if !target.exists() {
        return rename_with_retry(temp_path, target).inspect_err(|_| {
            let _ = fs::remove_file(temp_path);
        });
    }

    #[cfg(windows)]
    {
        let stale_path = sibling_stale_path(target);
        // Move the running image aside so the new binary can take its name.
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
                // Never leave the install without a binary; restore the image,
                // remove the staged copy, and surface the error.
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
pub(crate) fn rename_with_retry(from: &Path, to: &Path) -> Result<(), String> {
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

pub(crate) fn sibling_temp_path(target: &Path) -> PathBuf {
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
pub(crate) fn sibling_stale_path(target: &Path) -> PathBuf {
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
                // active install to race with ; safe to clean up.
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
pub(crate) fn remove_executable_orphans(claude_home: &Path) -> Result<usize, String> {
    let mut removed = 0usize;
    for orphan in find_executable_orphans(claude_home) {
        if fs::remove_file(&orphan).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

pub(crate) fn write_install_metadata(
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
        &super::super::verify::install_metadata_path(claude_home),
        &metadata,
    )?;
    Ok(())
}

pub fn repo_version_for_source(build_version: &str, repository_root: &Path) -> String {
    meaningful_repo_version(build_version).unwrap_or_else(|| git_short_head(repository_root))
}

pub fn repo_version_from_metadata_or_build(metadata: &str, build_version: &str) -> Option<String> {
    super::super::verify::metadata_value(metadata, "repo_version")
        .filter(|value| *value != "unknown")
        .map(str::to_string)
        .or_else(|| {
            super::super::verify::metadata_value(metadata, "manager_version")
                .and_then(repo_version_from_build_version)
        })
        .or_else(|| meaningful_repo_version(build_version))
}

pub(crate) fn meaningful_repo_version(build_version: &str) -> Option<String> {
    if build_version == "dev" || build_version.is_empty() {
        return None;
    }
    Some(build_version.to_string())
}

pub(crate) fn repo_version_from_build_version(manager_version: &str) -> Option<String> {
    let commit_hash = manager_version.split('-').next_back()?;
    if commit_hash.len() >= 7 {
        Some(commit_hash[..7].to_string())
    } else {
        None
    }
}

pub(crate) fn write_inventories(
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
