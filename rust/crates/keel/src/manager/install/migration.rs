// Installer migration helpers.
use super::super::agent_config::unix_timestamp;
use super::*;
use crate::runtime::{
    commands_directory, display_path, installed_executable_path, is_standard_keel_home,
    legacy_claude_executable_path, legacy_state_directory, remove_path_if_exists, skills_directory,
    state_directory, update_cache_directory, RepositoryLayout,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
/// Top-level names under a legacy `~/.claude` home that keel owns (creates
/// and reads) and that the claude harness never reads. These are copied to
/// the host-neutral root during migration; the legacy source remains intact.
pub(crate) const MIGRATION_DATA_NAMES: &[&str] = &[
    "working-briefs",
    "memories",
    "memory",
    "sprint",
    "state",
    // NOTE: `agent-profiles` is NOT migrated. Install re-syncs it into the
    // engagement home every run, so copying it would only churn.
    ".claude-skill-manager",
    "workflow",
    "anvil",
    "raw-output",
    "config.toml",
    "command-compaction-events.jsonl",
    "recall-index.sqlite3",
];

/// Copy keel-owned data from a legacy `~/.claude` install into the
/// host-neutral root. Runs on every install/update while retaining the legacy
/// data and binary as recovery copies until the install completes.
///
/// Destination content wins conflicts. Any copy failure is reported and never
/// converted into source deletion.
pub(crate) fn migrate_from_legacy_claude_home(
    keel_home: &Path,
    engagement_home: &Path,
) -> Option<String> {
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
            // Copy into the neutral root while retaining the legacy source as
            // a recovery copy. Install must never make migration destructive.
            let (copied, conflicts) = copy_tree_preserving(&source, &destination);
            moved += copied;
            if conflicts > 0 {
                skipped += 1;
            }
            continue;
        }
        if destination.exists() {
            // Type mismatch or existing destination: preserve both copies and
            // report the conflict for explicit operator reconciliation.
            if source.is_file()
                && destination.is_file()
                && files_are_identical(&source, &destination)
            {
                continue;
            }
            skipped += 1;
            continue;
        }
        if copy_path_preserving(&source, &destination) {
            moved += 1;
        } else {
            skipped += 1;
        }
    }
    // SQLite WAL sidecars are copied with the database. The legacy sidecars
    // remain available if the install fails or the new index is incomplete.
    for suffix in ["-wal", "-shm"] {
        let source = legacy.join(format!("recall-index.sqlite3{suffix}"));
        if source.exists() {
            let destination = keel_home.join(format!("recall-index.sqlite3{suffix}"));
            if !destination.exists() && copy_path_preserving(&source, &destination) {
                moved += 1;
            }
        }
    }
    if moved == 0 && skipped == 0 {
        return None;
    }
    let mut report = format!("copied {moved} item(s) from {}", display_path(legacy));
    if skipped > 0 {
        report.push_str(&format!(
            ", skipped {skipped} (destination exists or copy failed; legacy data retained)"
        ));
    }
    Some(report)
}

/// Copy a legacy directory tree into an existing destination without deleting
/// either side. Returns `(copied, conflicts)`.
pub(crate) fn copy_tree_preserving(source: &Path, destination: &Path) -> (usize, usize) {
    let mut copied = 0usize;
    let mut conflicts = 0usize;
    let Ok(entries) = fs::read_dir(source) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let child_source = entry.path();
        let child_destination = destination.join(entry.file_name());
        if child_source.is_dir() {
            if child_destination.is_dir() {
                let (child_copied, child_conflicts) =
                    copy_tree_preserving(&child_source, &child_destination);
                copied += child_copied;
                conflicts += child_conflicts;
            } else if child_destination.exists() {
                conflicts += 1;
            } else if copy_tree(&child_source, &child_destination) {
                copied += 1;
            } else {
                conflicts += 1;
            }
        } else if child_destination.is_file() {
            if !files_are_identical(&child_source, &child_destination) {
                conflicts += 1;
            }
        } else if child_destination.exists() {
            conflicts += 1;
        } else if copy_tree(&child_source, &child_destination) {
            copied += 1;
        } else {
            conflicts += 1;
        }
    }
    (copied, conflicts)
}

/// True when both paths are files with identical bytes. Any read error
/// conservatively answers `false` so a never-read file is never discarded.
pub(crate) fn files_are_identical(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Copy a file or directory tree without deleting the source.
pub(crate) fn copy_path_preserving(source: &Path, destination: &Path) -> bool {
    copy_tree(source, destination) && destination.exists()
}

/// Remove only exact copies left in the legacy keel-owned lane after a
/// successful install. Mismatches remain for recovery and manual review.
pub(crate) fn cleanup_identical_legacy_data(keel_home: &Path, engagement_home: &Path) -> usize {
    if keel_home == engagement_home {
        return 0;
    }
    let mut removed = 0usize;
    for name in MIGRATION_DATA_NAMES {
        removed += remove_identical_legacy_tree(&engagement_home.join(name), &keel_home.join(name));
    }
    removed += remove_identical_legacy_tree(
        &engagement_home.join(".claude-skill-manager"),
        &state_directory(keel_home),
    );
    removed
}

pub(crate) fn remove_identical_legacy_tree(source: &Path, destination: &Path) -> usize {
    if source.is_file() && destination.is_file() {
        if !files_are_identical(source, destination) {
            return 0;
        }
        return usize::from(fs::remove_file(source).is_ok());
    }
    if !source.is_dir() || !destination.is_dir() {
        return 0;
    }
    let mut removed = 0usize;
    let Ok(entries) = fs::read_dir(source) else {
        return 0;
    };
    for entry in entries.flatten() {
        let child_source = entry.path();
        let child_destination = destination.join(entry.file_name());
        removed += remove_identical_legacy_tree(&child_source, &child_destination);
    }
    let empty = fs::read_dir(source)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false);
    if empty {
        removed += usize::from(fs::remove_dir(source).is_ok());
    }
    removed
}

/// Recursive copy for files and directories (best-effort: per-entry failures
/// propagate as a false result rather than partial-success lies).
pub(crate) fn copy_tree(source: &Path, destination: &Path) -> bool {
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
pub(crate) fn remove_legacy_binary(keel_home: &Path) -> String {
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
pub(crate) fn migrate_legacy_state_directory(home: &Path) {
    let current = state_directory(home);
    let legacy = legacy_state_directory(home);
    if current.exists() || !legacy.exists() {
        return;
    }
    let _ = fs::rename(&legacy, &current);
}

/// Delete transient update extract trees while retaining legacy state files.
pub(crate) fn remove_update_temp_trees(keel_home: &Path, engagement_home: &Path) {
    let _ = remove_path_if_exists(&update_cache_directory(keel_home));
    let _ = remove_path_if_exists(&legacy_state_directory(keel_home).join("bin"));
    if engagement_home != keel_home {
        // The neutral home owns update cache; retain generic engagement cache.
        // Only transient extraction directories are disposable.
        let _ = remove_path_if_exists(&legacy_state_directory(engagement_home).join("bin"));
        let _ = remove_path_if_exists(&state_directory(engagement_home).join("bin"));
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
pub(crate) const DROPPED_FIRST_PARTY_SKILLS: &[&str] =
    &["running-a-sprint", "writing-user-stories"];

/// First-party slash-command files deleted from `commands/`. Same rule as
/// [`DROPPED_FIRST_PARTY_SKILLS`].
pub(crate) const DROPPED_FIRST_PARTY_COMMANDS: &[&str] =
    &["sprint.md", "user-story.md", "workflow.md"];

/// Remove deleted first-party skills/commands from the engagement home.
///
/// `--purge-stale` only deletes names that were in a prior inventory. An old
/// install that copied `sprint.md` before inventories existed (or after a
/// failed inventory write) keeps teaching the deleted loop. This list is the
/// product-cutover owner: always run, skip a name only when the current pack
/// still contains it.
pub(crate) fn remove_dropped_first_party_artifacts(
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

pub(crate) fn current_pack_command_names(layout: Option<&RepositoryLayout>) -> BTreeSet<String> {
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
pub(crate) fn remove_legacy_keel_leftovers(keel_home: &Path, engagement_home: &Path) -> usize {
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
