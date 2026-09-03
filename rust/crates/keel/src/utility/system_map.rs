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
    let hash = crate::utility::hashing::fnv1a64_hex(value);
    let short_hash = &hash[..8.min(hash.len())];
    let slug = sanitize_key(value);
    let max_prefix = 100;
    let prefix: String = slug.chars().take(max_prefix).collect();
    let prefix = prefix.trim_matches('-');
    if prefix.is_empty() {
        format!("ws-{short_hash}")
    } else {
        format!("{prefix}-{short_hash}")
    }
}
/// `max_len` characters so callers with directory-length constraints (code-graph
/// and design-intelligence workspace lanes) reuse one implementation instead of
/// carrying private copies.
pub fn bounded_slug(value: &str, max_len: usize) -> String {
    let slug = sanitize_key(value);
    if slug.chars().count() <= max_len {
        slug
    } else if max_len == 0 {
        String::new()
    } else {
        let hash = crate::utility::hashing::fnv1a64_hex(value);
        if max_len <= hash.len() {
            hash.chars().take(max_len).collect()
        } else {
            let prefix_len = max_len - hash.len() - 1;
            let prefix: String = slug.chars().take(prefix_len).collect();
            format!("{prefix}-{hash}")
        }
    }
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
    fn bounded_slugs_keep_distinct_long_paths_distinct() {
        let shared = "C:/work/a-very-long-parent-segment-that-would-consume-the-entire-old-directory-key-before-the-project-name/";
        let first = bounded_slug(&format!("{shared}alpha"), 64);
        let second = bounded_slug(&format!("{shared}beta"), 64);
        assert_ne!(first, second);
        assert!(first.len() <= 64);
        assert!(second.len() <= 64);
    }
}
