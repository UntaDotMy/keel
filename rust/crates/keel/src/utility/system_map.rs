//! Purpose: System map rendering and workspace structure detection
//! Caller: memory.rs scope/system-map commands
//! Dependencies: std::fs, std::path, crate::runtime
//! Main Functions: render_system_map, sanitize_key
//! Side Effects: Reads workspace directory structure

use std::fs;
use std::path::{Path, PathBuf};

use crate::runtime::display_path;

pub fn render_system_map(workspace_root: &Path) -> String {
    let top_level_entries = workspace_entries(workspace_root);
    let applications = detect_applications(workspace_root);
    let entrypoints = detect_entrypoints(workspace_root);
    let ownership_hints = detect_ownership_hints(workspace_root);
    let mut lines = vec![
        "# SYSTEM_MAP".to_string(),
        String::new(),
        format!("- workspace_root: {}", display_path(workspace_root)),
        "- storage: the harness-global per-workspace reference lane".to_string(),
        "- runtime: rust".to_string(),
        "- go_fallback: false".to_string(),
        String::new(),
        "## Top-Level Entries".to_string(),
    ];
    if top_level_entries.is_empty() {
        lines.push("- Not found".to_string());
    } else {
        for entry in &top_level_entries {
            lines.push(format!("- {} ({})", entry.name, entry.kind));
        }
    }
    lines.push(String::new());
    lines.push("## Direct Child Structure".to_string());
    append_direct_child_structure(workspace_root, &top_level_entries, &mut lines);
    lines.push(String::new());
    lines.push("## Applications".to_string());
    append_list_or_not_found(&mut lines, applications);
    lines.push(String::new());
    lines.push("## Entrypoints".to_string());
    append_list_or_not_found(&mut lines, entrypoints);
    lines.push(String::new());
    lines.push("## Ownership Hints".to_string());
    append_list_or_not_found(&mut lines, ownership_hints);
    lines.push(String::new());
    lines.push("## Maintenance".to_string());
    lines.push(
        "- Refresh this map after creating, deleting, moving, or renaming files or folders."
            .to_string(),
    );
    lines.push("- Command: `keel memory system-map refresh`".to_string());
    lines.push(String::new());
    lines.join("\n")
}

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

struct WorkspaceEntry {
    name: String,
    kind: &'static str,
    path: PathBuf,
}

fn workspace_entries(workspace_root: &Path) -> Vec<WorkspaceEntry> {
    let mut entries = Vec::new();
    if let Ok(read_directory) = fs::read_dir(workspace_root) {
        for entry_result in read_directory.flatten() {
            let name = entry_result.file_name().to_string_lossy().to_string();
            let path = entry_result.path();
            if should_skip_workspace_entry(&name, &path) {
                continue;
            }
            let kind = if path.is_dir() { "dir" } else { "file" };
            entries.push(WorkspaceEntry { name, kind, path });
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries.into_iter().take(200).collect()
}

fn append_direct_child_structure(
    workspace_root: &Path,
    entries: &[WorkspaceEntry],
    lines: &mut Vec<String>,
) {
    let mut rendered_any = false;
    for entry in entries.iter().filter(|entry| entry.path.is_dir()).take(60) {
        let mut child_dirs = Vec::new();
        let mut child_files = Vec::new();
        if let Ok(children) = fs::read_dir(&entry.path) {
            for child_result in children.flatten() {
                let child_name = child_result.file_name().to_string_lossy().to_string();
                let child_path = child_result.path();
                if should_skip_workspace_entry(&child_name, &child_path) {
                    continue;
                }
                if child_path.is_dir() {
                    child_dirs.push(format!("`{child_name}/`"));
                } else if child_path.is_file() && !is_probably_binary(&child_path) {
                    child_files.push(format!("`{child_name}`"));
                }
            }
        }
        child_dirs.sort();
        child_files.sort();
        let relative_path = entry
            .path
            .strip_prefix(workspace_root)
            .unwrap_or(&entry.path);
        lines.push(format!(
            "- `{}/` -> dirs: {}; files: {}.",
            markdown_path(relative_path),
            summarize_names(&child_dirs),
            summarize_names(&child_files)
        ));
        rendered_any = true;
    }
    if !rendered_any {
        lines.push("- Not found".to_string());
    }
}

fn detect_applications(workspace_root: &Path) -> Vec<String> {
    let mut applications = Vec::new();
    for marker in APPLICATION_MARKERS {
        for relative_path in collect_matching_relative_paths(workspace_root, marker.0, 4, 40) {
            applications.push(format!("- `{relative_path}` - {}", marker.1));
        }
    }
    applications.sort();
    applications.dedup();
    applications
}

/// Manifest filenames that mark a project's primary surface, with the human label
/// rendered next to each match. Recursive lookup runs against this list so a
/// workspace whose project lives one or two directories deep (a single
/// `Kiro-Go/go.mod`, for example) still gets recognized instead of rendering
/// "Not found".
const APPLICATION_MARKERS: &[(&str, &str)] = &[
    ("Cargo.toml", "Rust workspace/package"),
    ("package.json", "JavaScript package"),
    ("go.mod", "Go module"),
    ("pyproject.toml", "Python project"),
    ("requirements.txt", "Python requirements"),
    ("setup.py", "Python setup"),
    ("pom.xml", "Maven project"),
    ("build.gradle", "Gradle project"),
    ("build.gradle.kts", "Gradle Kotlin project"),
    ("Gemfile", "Ruby bundle"),
    ("composer.json", "PHP composer package"),
    ("Dockerfile", "Container image definition"),
    ("docker-compose.yml", "Docker Compose stack"),
    ("docker-compose.yaml", "Docker Compose stack"),
    ("terraform.tf", "Terraform root"),
    ("main.tf", "Terraform module"),
];

fn detect_entrypoints(workspace_root: &Path) -> Vec<String> {
    let mut entrypoints = Vec::new();
    for entrypoint_name in ENTRYPOINT_FILENAMES {
        for relative_path in collect_matching_relative_paths(workspace_root, entrypoint_name, 4, 40)
        {
            entrypoints.push(format!("- `{relative_path}`"));
        }
    }
    entrypoints.sort();
    entrypoints.dedup();
    entrypoints
}

/// Filenames that typically host a program's entry point. Each is searched
/// recursively (depth-bounded) so projects rooted in a subdirectory or
/// organized as a multi-binary workspace surface every entry instead of just
/// the ones at canonical paths.
const ENTRYPOINT_FILENAMES: &[&str] = &[
    "main.rs",
    "main.go",
    "main.py",
    "__main__.py",
    "manage.py",
    "app.py",
    "server.js",
    "server.ts",
    "index.js",
    "index.ts",
    "Main.java",
    "Application.java",
    "Program.cs",
];

fn detect_ownership_hints(workspace_root: &Path) -> Vec<String> {
    let mut hints = Vec::new();
    for hint in OWNERSHIP_HINT_FILENAMES {
        for relative_path in collect_matching_relative_paths(workspace_root, hint.0, 4, 40) {
            hints.push(format!("- `{relative_path}` - {}", hint.1));
        }
    }
    hints.sort();
    hints.dedup();
    hints
}

/// Documentation surfaces that signal who owns or how to operate the project.
/// Recursive depth-bound lookup catches the same files when the project lives
/// in a subdirectory (a `Kiro-Go/README.md`, for example) instead of only
/// recognizing them at the workspace root.
const OWNERSHIP_HINT_FILENAMES: &[(&str, &str)] = &[
    ("AGENTS.md", "managed agent routing and repository policy"),
    ("README.md", "user-facing product and install surface"),
    ("CLAUDE.md", "the harness project guide"),
    ("CONTRIBUTING.md", "contributor onboarding and workflow"),
    ("SKILL.md", "specialist skill surface"),
    ("CODEOWNERS", "code-ownership routing"),
];

fn collect_matching_relative_paths(
    workspace_root: &Path,
    file_name: &str,
    max_depth: usize,
    max_results: usize,
) -> Vec<String> {
    let mut matches = Vec::new();
    collect_matching_relative_paths_inner(
        workspace_root,
        workspace_root,
        file_name,
        0,
        max_depth,
        max_results,
        &mut matches,
    );
    matches.sort();
    matches
}

fn collect_matching_relative_paths_inner(
    workspace_root: &Path,
    directory: &Path,
    file_name: &str,
    depth: usize,
    max_depth: usize,
    max_results: usize,
    matches: &mut Vec<String>,
) {
    if depth > max_depth || matches.len() >= max_results {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry_result in entries.flatten() {
        let child_name = entry_result.file_name().to_string_lossy().to_string();
        let child_path = entry_result.path();
        if should_skip_workspace_entry(&child_name, &child_path)
            || should_skip_deep_scan(&child_name)
        {
            continue;
        }
        // file_type() does NOT follow symlinks; skip any symlink so a link
        // pointing outside the workspace cannot be descended (which would leak
        // external absolute paths into the map via the strip_prefix fallback).
        let Ok(file_type) = entry_result.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_matching_relative_paths_inner(
                workspace_root,
                &child_path,
                file_name,
                depth + 1,
                max_depth,
                max_results,
                matches,
            );
        } else if file_type.is_file() && child_name == file_name {
            let relative_path = child_path
                .strip_prefix(workspace_root)
                .unwrap_or(&child_path);
            matches.push(markdown_path(relative_path));
            if matches.len() >= max_results {
                return;
            }
        }
    }
}

fn append_list_or_not_found(lines: &mut Vec<String>, values: Vec<String>) {
    if values.is_empty() {
        lines.push("- Not found".to_string());
    } else {
        lines.extend(values);
    }
}

fn summarize_names(names: &[String]) -> String {
    if names.is_empty() {
        return "Not found".to_string();
    }
    let mut rendered = names
        .iter()
        .take(12)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > 12 {
        rendered.push_str(&format!(", ... {} more", names.len() - 12));
    }
    rendered
}

fn markdown_path(path: &Path) -> String {
    display_path(path).replace('\\', "/")
}

fn should_skip_workspace_entry(name: &str, path: &Path) -> bool {
    matches!(
        name,
        ".git" | ".claude" | "target" | "node_modules" | "vendor"
    ) || (name.starts_with('.') && path.is_dir() && !matches!(name, ".github" | ".gitlab"))
        || (path.is_file() && is_probably_binary(path))
}

/// Trees that stay visible as a top-level name but must not be recursively
/// scanned. Walking `hermes-agent` (or similar clones) on every marker
/// search is what made Grok's `system_map` tool sit for tens of seconds.
fn should_skip_deep_scan(name: &str) -> bool {
    matches!(
        name,
        "hermes-agent"
            | "karpathy-skills-cmp"
            | "dist"
            | "build"
            | "out"
            | "__pycache__"
            | ".venv"
            | "venv"
            | "coverage"
            | "site-packages"
            | ".pytest-cache"
            | "agent-tools"
            | "terminals"
            | "mcps"
    )
}

fn is_probably_binary(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or(""),
        "exe" | "dll" | "png" | "jpg" | "jpeg" | "gif" | "zip" | "gz" | "tar" | "lock"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir_under(label: &str) -> PathBuf {
        let unique_suffix: u128 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent for touch");
        }
        fs::write(path, b"").expect("touch test file");
    }

    #[test]
    fn detect_applications_finds_nested_go_module_when_root_has_no_manifest() {
        // The workspace root has no manifest of its own; the project lives one
        // directory deep. Before recursive lookup, this rendered "Not found"
        // for Applications. The test ensures the deeper go.mod is surfaced.
        let workspace = tempdir_under("system-map-nested-go-module");
        touch(&workspace.join("Kiro-Go").join("go.mod"));
        touch(&workspace.join("Kiro-Go").join("README.md"));

        let map = render_system_map(&workspace);
        assert!(
            map.contains("`Kiro-Go/go.mod` - Go module"),
            "expected nested go.mod to be listed under Applications:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn detect_entrypoints_finds_nested_main_go() {
        let workspace = tempdir_under("system-map-nested-main-go");
        touch(&workspace.join("Kiro-Go").join("main.go"));

        let map = render_system_map(&workspace);
        assert!(
            map.contains("`Kiro-Go/main.go`"),
            "expected nested main.go to appear under Entrypoints:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn detect_ownership_hints_finds_nested_readme() {
        let workspace = tempdir_under("system-map-nested-readme");
        touch(&workspace.join("Kiro-Go").join("README.md"));

        let map = render_system_map(&workspace);
        assert!(
            map.contains("`Kiro-Go/README.md` - user-facing product and install surface"),
            "expected nested README.md to appear under Ownership Hints:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn detect_applications_renders_not_found_when_no_manifest_present() {
        let workspace = tempdir_under("system-map-empty-applications");
        touch(&workspace.join("notes.txt"));

        let map = render_system_map(&workspace);
        let applications_section = section_after_heading(&map, "## Applications");
        assert_eq!(
            applications_section.trim(),
            "- Not found",
            "Applications section should still render Not found when no manifest exists:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn detect_applications_skips_target_and_node_modules() {
        let workspace = tempdir_under("system-map-skip-pruned-trees");
        touch(&workspace.join("Cargo.toml"));
        touch(&workspace.join("target").join("nested").join("Cargo.toml"));
        touch(
            &workspace
                .join("node_modules")
                .join("dep")
                .join("package.json"),
        );

        let map = render_system_map(&workspace);
        assert!(
            map.contains("`Cargo.toml` - Rust workspace/package"),
            "root Cargo.toml should still be listed:\n{map}"
        );
        assert!(
            !map.contains("target/"),
            "Cargo.toml inside target/ must not leak through:\n{map}"
        );
        assert!(
            !map.contains("node_modules/"),
            "package.json inside node_modules/ must not leak through:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn detect_applications_skips_vendored_clone_trees() {
        let workspace = tempdir_under("system-map-skip-hermes");
        touch(&workspace.join("Cargo.toml"));
        touch(
            &workspace
                .join("hermes-agent")
                .join("web")
                .join("package.json"),
        );
        touch(&workspace.join("hermes-agent").join("README.md"));

        let map = render_system_map(&workspace);
        assert!(
            map.contains("- hermes-agent (dir)"),
            "top-level clone name stays visible:\n{map}"
        );
        assert!(
            !map.contains("hermes-agent/web"),
            "must not recursively scan hermes-agent:\n{map}"
        );
        assert!(
            !map.contains("hermes-agent/README"),
            "ownership walk must skip hermes-agent:\n{map}"
        );

        let _ = fs::remove_dir_all(workspace);
    }

    fn section_after_heading<'a>(map: &'a str, heading: &str) -> &'a str {
        let after_heading = map.split_once(heading).map(|(_, rest)| rest).unwrap_or("");
        let trimmed = after_heading.trim_start_matches('\n');
        match trimmed.find("\n## ") {
            Some(index) => &trimmed[..index],
            None => trimmed,
        }
    }
}
