//! Persistent indexed code search and completeness scanning.
//!
//! The workspace index owns source discovery. This module only parses command
//! flags, formats ranked evidence, and keeps the existing sibling completeness
//! contract. Retrieval errors are returned to the caller; there is no live-scan
//! fallback.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::runtime::resolve_repository_root;
use crate::utility::workspace_index::{self, IndexStatus, SearchHit};

pub fn run_code_search_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel code-search search|siblings [flags]\n\
             search     --query <text> [--workspace-root <path>] [--claude-home <path>] [--path <filter>] [--limit N] [--json]\n\
             siblings   [--query <text>] [--workspace-root <path>] [--claude-home <path>] [--json]"
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

pub fn run_code_index_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel code-index <refresh|status|map> [--workspace-root <path>] [--claude-home <path>] [--force] [--json]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    let action = arguments[0].as_str();
    let mut flags = FlagSet::new(format!("code-index {action}"));
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("force", false);
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let root = match resolve_root(
        flags.string_value("workspace-root"),
        "code-index",
        standard_error,
    ) {
        Some(root) => root,
        None => return 1,
    };
    let home = flags.string_value("claude-home");
    match action {
        "refresh" => match workspace_index::refresh(&root, home, flags.bool_value("force")) {
            Ok(report) => {
                if flags.bool_value("json") {
                    let payload = serde_json::json!({
                        "filesIndexed": report.files_indexed,
                        "filesAdded": report.files_added,
                        "filesUpdated": report.files_updated,
                        "filesRemoved": report.files_removed,
                        "symbolsIndexed": report.symbols_indexed,
                        "chunksIndexed": report.chunks_indexed,
                        "edgesIndexed": report.edges_indexed,
                        "generation": report.generation,
                        "indexedCommit": report.indexed_commit,
                    });
                    let _ = writeln!(standard_output, "{payload}");
                } else {
                    let _ = writeln!(
                        standard_output,
                        "code-index refresh: files={} added={} updated={} removed={} symbols={} chunks={} edges={} generation={} commit={}",
                        report.files_indexed,
                        report.files_added,
                        report.files_updated,
                        report.files_removed,
                        report.symbols_indexed,
                        report.chunks_indexed,
                        report.edges_indexed,
                        report.generation,
                        report.indexed_commit
                    );
                }
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "code-index refresh: {error}");
                1
            }
        },
        "status" => match workspace_index::status(&root, home) {
            Ok(status) => {
                render_status(&status, flags.bool_value("json"), standard_output);
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "code-index status: {error}");
                1
            }
        },
        "map" => match workspace_index::render_map(&root, home) {
            Ok(map) => {
                let _ = writeln!(standard_output, "{map}");
                0
            }
            Err(error) => {
                let _ = writeln!(standard_error, "code-index map: {error}");
                1
            }
        },
        other => {
            let _ = writeln!(standard_error, "code-index: unknown action {other}");
            1
        }
    }
}

fn run_code_search_search(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("code-search search");
    flags.string_flag("query", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.string_flag("path", "");
    flags.string_flag("limit", "20");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let query = flags.string_value("query").trim();
    if query.is_empty() {
        let _ = writeln!(standard_error, "code-search search: --query required");
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("workspace-root"),
        "code-search",
        standard_error,
    ) else {
        return 1;
    };
    let limit = match parse_limit(flags.string_value("limit")) {
        Ok(limit) => limit,
        Err(error) => {
            let _ = writeln!(standard_error, "code-search search: {error}");
            return 1;
        }
    };
    let path_filter = normalize_path_filter(flags.string_value("path"));
    let hits = match workspace_index::search_filtered(
        &root,
        flags.string_value("claude-home"),
        query,
        limit,
        (!path_filter.is_empty()).then_some(path_filter.as_str()),
    ) {
        Ok(hits) => hits,
        Err(error) => {
            let _ = writeln!(standard_error, "code-search search: {error}");
            return 1;
        }
    };
    let hits: Vec<SearchHit> = hits.into_iter().collect();
    if flags.bool_value("json") {
        let _ = writeln!(
            standard_output,
            "{}",
            serde_json::to_string(&hits_to_json(query, &hits)).unwrap_or_else(|_| "{}".to_string())
        );
        return 0;
    }
    if hits.is_empty() {
        let _ = writeln!(
            standard_output,
            "No indexed matches found for query: {query}"
        );
    } else {
        for hit in &hits {
            let symbol = if hit.symbol.is_empty() {
                String::new()
            } else {
                format!(" [{}]", hit.symbol)
            };
            let snippet = if hit.snippet.is_empty() {
                String::new()
            } else {
                format!(" {}", hit.snippet)
            };
            let _ = writeln!(
                standard_output,
                "{}:{}-{}{} ({}) score={:.5}{}",
                hit.path, hit.start_line, hit.end_line, symbol, hit.reason, hit.score, snippet
            );
        }
        let _ = writeln!(standard_output, "\nFound {} indexed match(es)", hits.len());
    }
    0
}

fn run_code_search_siblings(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("code-search siblings");
    flags.string_flag("query", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(root) = resolve_root(
        flags.string_value("workspace-root"),
        "code-search siblings",
        standard_error,
    ) else {
        return 1;
    };
    let explicit = flags.string_value("query").trim().to_string();
    let queries = if explicit.is_empty() {
        queries_from_git_diff(&root)
    } else {
        vec![explicit]
    };
    if queries.is_empty() {
        let _ = writeln!(
            standard_error,
            "code-search siblings: no query or git-diff tokens"
        );
        return 1;
    }
    let changed = changed_paths_from_git(&root);
    let mut sibling_hits = Vec::new();
    for query in &queries {
        let hits =
            match workspace_index::search(&root, flags.string_value("claude-home"), query, 80) {
                Ok(hits) => hits,
                Err(error) => {
                    let _ = writeln!(standard_error, "code-search siblings: {error}");
                    return 1;
                }
            };
        for hit in hits {
            if changed.iter().any(|path| path == &hit.path) {
                continue;
            }
            sibling_hits.push(format!(
                "[{query}] {}:{}-{} ({}) {}",
                hit.path, hit.start_line, hit.end_line, hit.reason, hit.snippet
            ));
            if sibling_hits.len() >= 80 {
                break;
            }
        }
        if sibling_hits.len() >= 80 {
            break;
        }
    }
    crate::runner::hook_lifecycle::record_completeness_gate_clear_for(&root);
    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "queries": queries,
            "changed": changed,
            "siblingCount": sibling_hits.len(),
            "siblings": sibling_hits,
        });
        let _ = writeln!(standard_output, "{payload}");
        return 0;
    }
    let _ = writeln!(standard_output, "code-search siblings");
    let _ = writeln!(standard_output, "queries: {}", queries.join(", "));
    let changed_display = if changed.is_empty() {
        "(none from git diff)".to_string()
    } else {
        changed.join(", ")
    };
    let _ = writeln!(standard_output, "changed: {changed_display}");
    if sibling_hits.is_empty() {
        let _ = writeln!(standard_output, "siblings: none outside the changed set.");
    } else {
        let _ = writeln!(
            standard_output,
            "siblings: {} indexed hit(s) outside the changed set:",
            sibling_hits.len()
        );
        for hit in &sibling_hits {
            let _ = writeln!(standard_output, "  {hit}");
        }
    }
    0
}

fn resolve_root(
    raw: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<std::path::PathBuf> {
    let root = match resolve_repository_root(raw) {
        Ok(root) => root,
        Err(_) => {
            let _ = writeln!(standard_error, "{label}: no repository root found");
            return None;
        }
    };
    if !root.is_dir() {
        let _ = writeln!(
            standard_error,
            "{label}: workspace root is not a directory: {}",
            root.display()
        );
        None
    } else {
        Some(root)
    }
}

fn render_status(status: &IndexStatus, json: bool, output: &mut dyn Write) {
    if json {
        let payload = serde_json::json!({
            "databasePath": status.database_path,
            "workspaceRoot": status.workspace_root,
            "indexedCommit": status.indexed_commit,
            "generation": status.generation,
            "files": status.file_count,
            "symbols": status.symbol_count,
            "chunks": status.chunk_count,
            "edges": status.edge_count,
            "stale": status.stale,
        });
        let _ = writeln!(output, "{payload}");
    } else {
        let _ = writeln!(
            output,
            "code-index status: files={} symbols={} chunks={} edges={} generation={} stale={} commit={} db={}",
            status.file_count,
            status.symbol_count,
            status.chunk_count,
            status.edge_count,
            status.generation,
            status.stale,
            status.indexed_commit,
            status.database_path.display()
        );
    }
}

fn hits_to_json(query: &str, hits: &[SearchHit]) -> serde_json::Value {
    serde_json::json!({
        "query": query,
        "count": hits.len(),
        "matches": hits,
    })
}

/// Deliberate divergence vs `recall::parse_limit`: search silently caps the
/// display limit at 50, while recall is error-facing with no cap. Do not unify.
fn parse_limit(raw: &str) -> Result<usize, String> {
    let limit = raw
        .trim()
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if limit == 0 {
        return Err("limit must be a positive integer".to_string());
    }
    Ok(limit.min(50))
}

fn normalize_path_filter(path_filter: &str) -> String {
    path_filter.replace('\\', "/").to_ascii_lowercase()
}

fn queries_from_git_diff(root: &Path) -> Vec<String> {
    distinctive_tokens(&git_added_text(root))
}

fn git_added_text(root: &Path) -> String {
    let mut text = String::new();
    for extra in [&[][..], &["--cached"][..]] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("diff")
            .arg("-U0")
            .args(extra)
            .output();
        if let Ok(output) = output {
            text.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }
    for relative in untracked_paths_from_git(root) {
        if text.len() >= 1_048_576 {
            break;
        }
        let path = root.join(&relative);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.len() > 262_144 || bytes.contains(&0) {
            continue;
        }
        if let Ok(contents) = String::from_utf8(bytes) {
            for line in contents.lines() {
                text.push_str("+ ");
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    text
}

fn changed_paths_from_git(root: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", "HEAD"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let tracked = String::from_utf8_lossy(&output.stdout);
    merge_changed_paths(&tracked, &untracked_paths_from_git(root).join("\n"))
}

fn untracked_paths_from_git(root: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--exclude-standard"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/"))
        .collect()
}

fn merge_changed_paths(tracked: &str, untracked: &str) -> Vec<String> {
    let mut paths: Vec<String> = tracked
        .lines()
        .chain(untracked.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/"))
        .collect();
    paths.sort();
    paths.dedup();
    paths
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
        let mut current = String::new();
        for ch in trimmed[1..].chars() {
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
    if raw.len() < 5 || raw.bytes().all(|byte| byte.is_ascii_digit()) {
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

fn is_help_argument(argument: &str) -> bool {
    crate::utility::memory::shared::is_help_argument(argument)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_filter_normalizes_windows_separators() {
        assert_eq!(
            normalize_path_filter(r"rust\crates\keel"),
            "rust/crates/keel"
        );
    }

    #[test]
    fn distinctive_tokens_ignore_common_syntax() {
        let tokens =
            distinctive_tokens("+ pub fn resolve_workspace_index() {}\n+ let value = 1;\n");
        assert_eq!(tokens, vec!["resolve_workspace_index".to_string()]);
    }

    #[test]
    fn parse_limit_is_bounded() {
        assert_eq!(parse_limit("100").expect("limit"), 50);
        assert!(parse_limit("0").is_err());
    }

    #[test]
    fn changed_paths_include_untracked_files_without_duplicates() {
        assert_eq!(
            merge_changed_paths("src/lib.rs\nsrc/main.rs\n", "src/main.rs\nnew/file.rs\n"),
            ["new/file.rs", "src/lib.rs", "src/main.rs"]
        );
    }
}
