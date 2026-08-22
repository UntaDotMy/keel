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

/// Canonical slug core behind [`sanitize_key`]: same normalization, bounded to
/// `max_len` characters so callers with directory-length constraints (code-graph
/// and design-intelligence workspace lanes) reuse one implementation instead of
/// carrying private copies.
pub fn bounded_slug(value: &str, max_len: usize) -> String {
    let slug = sanitize_key(value);
    if slug.chars().count() <= max_len {
        slug
    } else {
        slug.chars().take(max_len).collect()
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
}
