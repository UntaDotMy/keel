//! Purpose: Per-skill match-usage telemetry. Each time the matcher selects a
//!   skill, a monotonic counter under `<claude_home>/state/skill-usage/<name>.count`
//!   is incremented. `skill_catalog` reads these counts so `skill_list` can show
//!   which skills the matcher picks often and which never fire.
//! Caller: `utility::skill_match::match_skill_for_prompt` (record on match),
//!   `utility::skill_match::skill_catalog` (read to populate `use_count`).
//! Dependencies: std::fs, std::path.
//! Main Functions: record_skill_match, skill_use_count.
//! Side Effects: Reads/writes counter files under `<claude_home>/state/skill-usage/`.

use std::fs;
use std::path::{Path, PathBuf};

/// The directory holding per-skill `.count` files.
fn usage_directory(claude_home: &Path) -> PathBuf {
    claude_home.join("state").join("skill-usage")
}

/// The counter file for a single skill. The skill name is used verbatim as the
/// file stem (skill names are already lowercase-hyphenated safe identifiers).
fn usage_file(claude_home: &Path, skill_name: &str) -> PathBuf {
    usage_directory(claude_home).join(format!("{skill_name}.count"))
}

/// Increment the match counter for `skill_name` by one. Called when the matcher
/// selects the skill. Fail-open: a write error is swallowed (telemetry must
/// never break the match path), returning the best-effort new value.
pub fn record_skill_match(claude_home: &Path, skill_name: &str) -> u64 {
    let path = usage_file(claude_home, skill_name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let current = fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let next = current.saturating_add(1);
    let _ = fs::write(&path, next.to_string());
    next
}

/// Read the match counter for `skill_name`. Returns 0 when the file is absent
/// or unreadable (a skill never matched has no counter file).
pub fn skill_use_count(claude_home: &Path, skill_name: &str) -> u64 {
    fs::read_to_string(usage_file(claude_home, skill_name))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str) -> crate::test_support::TestTempDir {
        crate::test_support::unique_temp_dir(&format!("keel-skill-usage-{label}"))
    }

    #[test]
    fn record_increments_and_read_returns_it() {
        let home = temp_home("incr");
        assert_eq!(skill_use_count(&home, "reviewer"), 0);
        assert_eq!(record_skill_match(&home, "reviewer"), 1);
        assert_eq!(record_skill_match(&home, "reviewer"), 2);
        assert_eq!(skill_use_count(&home, "reviewer"), 2);
    }

    #[test]
    fn unrecorded_skill_reads_zero() {
        let home = temp_home("zero");
        assert_eq!(skill_use_count(&home, "never-matched"), 0);
    }

    #[test]
    fn counters_are_independent_per_skill() {
        let home = temp_home("indep");
        record_skill_match(&home, "reviewer");
        record_skill_match(&home, "reviewer");
        record_skill_match(&home, "git-expert");
        assert_eq!(skill_use_count(&home, "reviewer"), 2);
        assert_eq!(skill_use_count(&home, "git-expert"), 1);
    }
}
