//! Purpose: Code search and design intelligence command handlers
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::io, std::fs, std::path, crate::args, crate::runtime
//! Main Functions: run_code_search_command, run_design_intelligence_command
//! Side Effects: Reads repository files, writes search results to stdout

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_repository_root};

pub fn run_code_search_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills code-search search [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "search" => run_code_search_search(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "Unknown code-search command: {other}");
            1
        }
    }
}

pub fn run_design_intelligence_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills design-intelligence recommend [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    if arguments[0] != "recommend" {
        let _ = writeln!(
            standard_error,
            "Unknown design-intelligence command: {}",
            arguments[0]
        );
        return 1;
    }
    let _ = writeln!(standard_output, "Design Intelligence Recommendation");
    let _ = writeln!(
        standard_output,
        "- preserve existing visual language before changing components"
    );
    let _ = writeln!(
        standard_output,
        "- validate responsive behavior on desktop and mobile"
    );
    let _ = writeln!(
        standard_output,
        "- avoid generic AI-looking layouts unless matching an existing system"
    );
    0
}

fn run_code_search_search(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("code-search search");
    flag_set.string_flag("query", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("path", "");
    if let Err(error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "code-search search: {}", error.message);
        return 1;
    }
    let query = flag_set.string_value("query");
    let workspace_root = flag_set.string_value("workspace-root");
    let path_filter = flag_set.string_value("path");
    if query.is_empty() {
        let _ = writeln!(
            standard_error,
            "code-search search: --query required (example: --query \"RunReview owner path\")"
        );
        return 1;
    }
    let root = if workspace_root.is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "code-search search: no repository root found"
                );
                return 1;
            }
        }
    } else {
        PathBuf::from(workspace_root)
    };
    if !root.is_dir() {
        let _ = writeln!(
            standard_error,
            "code-search search: workspace-root not a directory: {}",
            display_path(&root)
        );
        return 1;
    }
    let mut matches = Vec::new();
    search_files_for_query(&root, query, path_filter, &mut matches);
    if matches.is_empty() {
        let _ = writeln!(standard_output, "No matches found for query: {query}");
    } else {
        for line in &matches {
            let _ = writeln!(standard_output, "{line}");
        }
        let _ = writeln!(
            standard_output,
            "\nFound {} match{}",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" }
        );
    }
    0
}

fn search_files_for_query(root: &Path, query: &str, path_filter: &str, matches: &mut Vec<String>) {
    let mut candidates = Vec::new();
    collect_search_candidates(root, &mut candidates);
    for path in candidates {
        if !path_filter.is_empty() && !path.to_string_lossy().contains(path_filter) {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(query) {
                let relative_path = path.strip_prefix(root).unwrap_or(&path);
                matches.push(format!(
                    "{}:{}:{}",
                    display_path(relative_path),
                    line_index + 1,
                    line.trim()
                ));
                if matches.len() >= 1000 {
                    return;
                }
            }
        }
    }
}

fn collect_search_candidates(root: &Path, candidates: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if candidates.len() >= 10000 {
            return;
        }
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry_result in entries.flatten() {
            let path = entry_result.path();
            let name = entry_result.file_name().to_string_lossy().to_string();
            if should_skip_search_entry(&name, &path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && !is_probably_binary(&path) {
                candidates.push(path);
            }
        }
    }
}

fn should_skip_search_entry(name: &str, path: &Path) -> bool {
    if name.starts_with('.') && name != ".github" {
        return true;
    }
    if path.is_dir() {
        matches!(
            name,
            "node_modules"
                | ".venv"
                | "venv"
                | "env"
                | "vendor"
                | "target"
                | ".gradle"
                | "bin"
                | "obj"
                | "pkg"
                | ".git"
                | ".vscode"
                | ".idea"
                | "__pycache__"
                | "dist"
                | "build"
                | "tmp"
                | "coverage"
                | ".next"
                | ".nuxt"
                | ".cache"
        )
    } else {
        name.ends_with(".log")
            || name.ends_with(".lock")
            || name.ends_with(".min.js")
            || name.ends_with(".min.css")
            || name.ends_with(".map")
    }
}

fn is_probably_binary(path: &Path) -> bool {
    if let Some(extension) = path.extension() {
        let extension_str = extension.to_string_lossy();
        matches!(
            extension_str.as_ref(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "ico"
                | "svg"
                | "webp"
                | "mp4"
                | "webm"
                | "mp3"
                | "wav"
                | "ogg"
                | "pdf"
                | "zip"
                | "tar"
                | "gz"
                | "7z"
                | "rar"
                | "exe"
                | "dll"
                | "so"
                | "dylib"
                | "wasm"
                | "ttf"
                | "woff"
                | "woff2"
                | "eot"
                | "otf"
        )
    } else {
        false
    }
}

fn is_help_argument(argument: &str) -> bool {
    argument == "--help" || argument == "-h" || argument == "help"
}
