//! Purpose: Compact search command output by grouping matches by file and capping examples.
//! Caller: AdapterRegistry for rg and grep commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: SearchAdapter::compact.
//! Side Effects: None; proxy::run saves raw and compact output.

use crate::adapters::common::{compact_edges, make_result, normalized_command};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct SearchAdapter;

impl CommandAdapter for SearchAdapter {
    fn name(&self) -> &'static str {
        "search"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Search
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let stdout_text = String::from_utf8_lossy(stdout);
        let stderr_text = String::from_utf8_lossy(stderr);
        let grouped = compact_search_output(&stdout_text);
        let compacted = grouped != stdout_text || stdout_text.lines().count() > 40;
        make_result(
            self.name(),
            normalized_command(&meta.program, &meta.args),
            grouped,
            compact_edges(&stderr_text, "search diagnostics", 40),
            exit_code,
            meta,
            compacted,
        )
    }
}

fn compact_search_output(stdout: &str) -> String {
    let mut files = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut total = 0usize;
    for line in stdout.lines() {
        if let Some((file, rest)) = split_match_line(line) {
            total += 1;
            let examples = files.entry(file.to_string()).or_default();
            if examples.len() < 3 {
                examples.push(rest.to_string());
            }
        }
    }
    if files.is_empty() {
        return compact_edges(stdout, "search output", 60);
    }
    let mut rendered = format!("{total} matches in {} files", files.len());
    for (file, examples) in files.iter().take(12) {
        rendered.push_str(&format!("\n\n{file}"));
        for example in examples {
            rendered.push_str(&format!("\n  {example}"));
        }
    }
    let shown: usize = files.values().take(12).map(Vec::len).sum();
    if total > shown {
        rendered.push_str(&format!("\n\nomitted: {} matches", total - shown));
    }
    rendered
}

fn split_match_line(line: &str) -> Option<(&str, &str)> {
    let search_start = if line.as_bytes().get(1) == Some(&b':') {
        2
    } else {
        0
    };
    let first = line[search_start..].find(':')? + search_start;
    let file = &line[..first];
    let rest = &line[first + 1..];
    if file.is_empty() || rest.is_empty() {
        return None;
    }
    Some((file, rest))
}

#[cfg(test)]
mod tests {
    use super::SearchAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

    #[test]
    fn search_groups_many_matches_by_file() {
        let mut stdout = String::new();
        for index in 0..20 {
            stdout.push_str(&format!("src/lib.rs:{}:CompactResult hit\n", index + 1));
        }
        for index in 0..20 {
            stdout.push_str(&format!("src/run.rs:{}:CompactResult hit\n", index + 1));
        }
        let result = SearchAdapter.compact(stdout.as_bytes(), b"", 0, &meta(stdout.len()));
        assert!(result.stdout.contains("40 matches in 2 files"));
        assert!(result.stdout.contains("src/lib.rs"));
        assert!(result.stdout.contains("omitted:"));
    }

    fn meta(stdout_bytes: usize) -> RunMeta {
        RunMeta {
            raw_id: "raw".to_string(),
            command: "rg CompactResult .".to_string(),
            program: "rg".to_string(),
            args: vec!["CompactResult".to_string(), ".".to_string()],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "search".to_string(),
            raw_path: PathBuf::from("/tmp/raw"),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from("."),
            stdout_bytes,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: stdout_bytes / 4,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }
}
