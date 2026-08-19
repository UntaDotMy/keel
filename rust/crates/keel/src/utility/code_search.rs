//! Purpose: Code search command handler
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::io, std::fs, std::path, crate::args, crate::runtime
//! Main Functions: run_code_search_command
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
            "Usage: keel code-search search|siblings [flags]\n\
             search     --query <text> [--workspace-root <path>] [--path <filter>]\n\
             siblings   [--query <text>] [--workspace-root <path>]\n\
                        Scan the class, not one hit: search the query (or tokens\n\
                        from the current git diff) and list every other in-repo copy."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "search" => run_code_search_search(&arguments[1..], standard_output, standard_error),
        "siblings" => run_code_search_siblings(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "Unknown code-search command: {other}");
            1
        }
    }
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
        let _ = writeln!(standard_error, "{}", error.message);
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

fn run_code_search_siblings(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("code-search siblings");
    flag_set.string_flag("query", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.bool_flag("json", false);
    if let Err(error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let root = if flag_set.string_value("workspace-root").is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "code-search siblings: no repository root found"
                );
                return 1;
            }
        }
    } else {
        PathBuf::from(flag_set.string_value("workspace-root"))
    };
    if !root.is_dir() {
        let _ = writeln!(
            standard_error,
            "code-search siblings: workspace-root not a directory: {}",
            display_path(&root)
        );
        return 1;
    }

    let explicit = flag_set.string_value("query").trim().to_string();
    let queries = if !explicit.is_empty() {
        vec![explicit]
    } else {
        queries_from_git_diff(&root)
    };
    if queries.is_empty() {
        let _ = writeln!(
            standard_error,
            "code-search siblings: no --query and no git diff tokens. Pass --query with the bug shape (identifier, string, or call)."
        );
        return 1;
    }

    let changed = changed_paths_from_git(&root);
    let mut sibling_hits = Vec::new();
    for query in &queries {
        let mut hits = Vec::new();
        search_files_for_query(&root, query, "", &mut hits);
        for hit in hits {
            let file = hit.split(':').next().unwrap_or("");
            let file_norm = file.replace('\\', "/");
            let in_changed = changed.iter().any(|path| {
                let norm = path.replace('\\', "/");
                file_norm == norm || file_norm.ends_with(&norm) || norm.ends_with(&file_norm)
            });
            if !in_changed {
                sibling_hits.push(format!("[{query}] {hit}"));
            }
            if sibling_hits.len() >= 80 {
                break;
            }
        }
        if sibling_hits.len() >= 80 {
            break;
        }
    }

    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(&root);

    if flag_set.bool_value("json") {
        let payload = serde_json::json!({
            "queries": queries,
            "changed": changed,
            "siblingCount": sibling_hits.len(),
            "siblings": sibling_hits,
        });
        let _ = writeln!(
            standard_output,
            "{}",
            serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
        );
        return 0;
    }

    let _ = writeln!(standard_output, "code-search siblings");
    let _ = writeln!(standard_output, "queries: {}", queries.join(", "));
    if changed.is_empty() {
        let _ = writeln!(standard_output, "changed: (none from git diff)");
    } else {
        let _ = writeln!(standard_output, "changed: {}", changed.join(", "));
    }
    if sibling_hits.is_empty() {
        let _ = writeln!(
            standard_output,
            "siblings: none outside the changed set. Still confirm edge cases (install/update/uninstall, other hosts, tests)."
        );
    } else {
        let _ = writeln!(
            standard_output,
            "siblings: {} hit(s) outside the changed set — fix each or mark out of scope:",
            sibling_hits.len()
        );
        for hit in &sibling_hits {
            let _ = writeln!(standard_output, "  {hit}");
        }
    }
    let _ = writeln!(
        standard_output,
        "account: a one-site fix is unfinished. Same shape on other hosts, CLIs, tests, and install/uninstall paths must be handled in this turn."
    );
    0
}

fn queries_from_git_diff(root: &Path) -> Vec<String> {
    distinctive_tokens(&git_added_text(root))
}

fn git_added_text(root: &Path) -> String {
    let mut text = String::new();
    for extra in [&[][..], &["--cached"][..]] {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(root).arg("diff").arg("-U0").args(extra);
        if let Ok(output) = cmd.output() {
            text.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }
    text
}

fn changed_paths_from_git(root: &Path) -> Vec<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["diff", "--name-only", "HEAD"]);
    let Ok(output) = cmd.output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/"))
        .collect()
}

fn distinctive_tokens(diff: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "return", "string", "format", "error", "value", "result", "assert", "import", "export",
        "const", "static", "struct", "class", "function", "false", "true", "null", "none", "self",
        "this", "that", "with", "from", "into", "where", "match", "while", "break", "continue",
        "pub", "let", "mut", "fn", "use", "mod", "impl",
    ];
    let mut counts: Vec<(String, usize)> = Vec::new();
    for line in diff.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('+') || trimmed.starts_with("+++") {
            continue;
        }
        let body = &trimmed[1..];
        let mut current = String::new();
        for ch in body.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                current.push(ch);
            } else if !current.is_empty() {
                push_token(&mut counts, &current, STOP);
                current.clear();
            }
        }
        if !current.is_empty() {
            push_token(&mut counts, &current, STOP);
        }
    }
    counts.sort_by(|left, right| right.0.len().cmp(&left.0.len()).then(left.0.cmp(&right.0)));
    counts.into_iter().take(6).map(|(token, _)| token).collect()
}

fn push_token(counts: &mut Vec<(String, usize)>, raw: &str, stop: &[&str]) {
    if raw.len() < 5 {
        return;
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        return;
    }
    let lower = raw.to_ascii_lowercase();
    if stop.contains(&lower.as_str()) {
        return;
    }
    if let Some(entry) = counts.iter_mut().find(|(token, _)| token == raw) {
        entry.1 = entry.1.saturating_add(1);
    } else {
        counts.push((raw.to_string(), 1));
    }
}

fn search_files_for_query(root: &Path, query: &str, path_filter: &str, matches: &mut Vec<String>) {
    let mut candidates = Vec::new();
    collect_search_candidates(root, &mut candidates);
    let normalized_filter = normalize_path_filter(path_filter);
    for path in candidates {
        if !path_matches_filter(&path, root, &normalized_filter) {
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

/// Normalize path filters so `rust/crates/keel` matches Windows `rust\crates\keel`.
fn normalize_path_filter(path_filter: &str) -> String {
    path_filter.replace('\\', "/")
}

/// Cross-platform path filter: compare with `/` separators against relative and absolute forms.
fn path_matches_filter(path: &Path, root: &Path, normalized_filter: &str) -> bool {
    if normalized_filter.is_empty() {
        return true;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative_norm = relative.to_string_lossy().replace('\\', "/");
    let absolute_norm = path.to_string_lossy().replace('\\', "/");
    relative_norm.contains(normalized_filter) || absolute_norm.contains(normalized_filter)
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
            // Use the dir entry's file type, which does NOT follow symlinks, and
            // skip any symlink. is_dir()/is_file() follow links, so a symlink
            // pointing outside the workspace would otherwise let the search
            // recurse into and read the contents of external directories.
            let Ok(file_type) = entry_result.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && !is_probably_binary(&path) {
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
                | "target-test"
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
                // Local research / comparison clones (gitignored, not product source).
                | "hermes-agent"
                | "karpathy-skills-cmp"
                | "agent-tools"
                | "terminals"
                | "mcps"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_filter_matches_forward_and_backslash_forms() {
        let root = PathBuf::from("workspace");
        let path = root
            .join("rust")
            .join("crates")
            .join("keel")
            .join("src")
            .join("main.rs");
        assert!(path_matches_filter(
            &path,
            &root,
            &normalize_path_filter("rust/crates/keel")
        ));
        assert!(path_matches_filter(
            &path,
            &root,
            &normalize_path_filter(r"rust\crates\keel")
        ));
        assert!(!path_matches_filter(
            &path,
            &root,
            &normalize_path_filter("docs/only")
        ));
    }

    #[test]
    fn normalize_path_filter_unifies_separators() {
        assert_eq!(
            normalize_path_filter(r"rust\crates\keel"),
            "rust/crates/keel"
        );
        assert_eq!(
            normalize_path_filter("rust/crates/keel"),
            "rust/crates/keel"
        );
    }

    #[test]
    fn skip_list_covers_research_clones() {
        let root =
            std::env::temp_dir().join(format!("keel-code-search-skip-{}", std::process::id()));
        // Best-effort cleanup of a prior crashed run; ignore missing dir.
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(root.join("hermes-agent")).expect("create hermes-agent");
        fs::create_dir_all(root.join("karpathy-skills-cmp")).expect("create karpathy");
        fs::create_dir_all(root.join("target-test")).expect("create target-test");
        assert!(should_skip_search_entry(
            "hermes-agent",
            &root.join("hermes-agent")
        ));
        assert!(should_skip_search_entry(
            "karpathy-skills-cmp",
            &root.join("karpathy-skills-cmp")
        ));
        assert!(should_skip_search_entry(
            "target-test",
            &root.join("target-test")
        ));
        for extra in ["agent-tools", "terminals", "mcps"] {
            fs::create_dir_all(root.join(extra)).expect("create extra skip dir");
            assert!(
                should_skip_search_entry(extra, &root.join(extra)),
                "must skip {extra}"
            );
        }
        fs::remove_dir_all(&root).expect("cleanup temp skip-list fixture");
    }

    #[test]
    fn distinctive_tokens_keeps_bug_shape_not_stopwords() {
        let diff = "\
+    emit_pretool_deny(reason);\n\
+    return search_replace;\n\
+    let value = true;\n";
        let tokens = distinctive_tokens(diff);
        assert!(
            tokens
                .iter()
                .any(|token| token == "emit_pretool_deny" || token == "search_replace"),
            "tokens={tokens:?}"
        );
        assert!(!tokens
            .iter()
            .any(|token| token == "return" || token == "true"));
    }

    #[test]
    fn siblings_lists_the_other_copy() {
        let root = std::env::temp_dir().join(format!(
            "keel-siblings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(root.join("src/one.rs"), "fn unique_sibling_token() {}\n").unwrap();
        fs::write(root.join("src/two.rs"), "fn unique_sibling_token() {}\n").unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_code_search_siblings(
            &[
                format!("--workspace-root={}", root.display()),
                "--query=unique_sibling_token".into(),
            ],
            &mut stdout,
            &mut stderr,
        );
        let text = String::from_utf8_lossy(&stdout);
        assert_eq!(code, 0, "stderr={}", String::from_utf8_lossy(&stderr));
        assert!(text.contains("unique_sibling_token"), "{text}");
        assert!(text.contains("siblings:"), "{text}");
        fs::remove_dir_all(&root).ok();
    }
}
