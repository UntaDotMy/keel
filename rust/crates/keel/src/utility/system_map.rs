//! Workspace map key helpers.
//!
//! The persisted workspace index owns map generation. This module only keeps the
//! canonical workspace-key normalization used by the global memory lane.

pub fn sanitize_key(value: &str) -> String {
    let raw_key = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    collapse_separator_runs(&raw_key)
}

/// Canonical workspace key used by all lanes (SYSTEM_MAP, code-graph, design-intelligence, recall):
/// bounded slug plus deterministic 8-hex hash suffix. Guarantees uniqueness for all paths
/// (including non-ASCII/unicode and long paths) without length overflow.
pub fn workspace_key(value: &str) -> String {
    keel_flow::workspace_key(std::path::Path::new(value))
}

pub fn workspace_key_aliases(value: &str) -> Vec<String> {
    keel_flow::workspace_key_aliases(std::path::Path::new(value))
}
fn collapse_separator_runs(value: &str) -> String {
    let mut collapsed = String::new();
    let mut previous_was_separator = false;
    for character in value.chars() {
        if character == '-' {
            if !previous_was_separator {
                collapsed.push(character);
            }
            previous_was_separator = true;
        } else {
            collapsed.push(character);
            previous_was_separator = false;
        }
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_windows_paths_deterministically() {
        assert_eq!(
            sanitize_key(r"D:\Nasri\Project\keel"),
            "d-nasri-project-keel"
        );
        assert_eq!(sanitize_key("workspace///name"), "workspace-name");
    }

    #[test]
    fn workspace_keys_keep_distinct_long_paths_distinct() {
        let shared = "C:/work/a-very-long-parent-segment-that-would-consume-the-entire-old-directory-key-before-the-project-name/";
        let first = workspace_key(&format!("{shared}alpha"));
        let second = workspace_key(&format!("{shared}beta"));
        assert_ne!(first, second);
        assert!(first.len() <= 64);
        assert!(second.len() <= 64);
    }
}
