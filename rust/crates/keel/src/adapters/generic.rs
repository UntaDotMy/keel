//! Purpose: Provide fallback head/tail compaction when no semantic adapter matches.
//! Caller: AdapterRegistry as the final command adapter.
//! Dependencies: proxy adapter contracts and token metering.
//! Main Functions: GenericAdapter::compact.
//! Side Effects: None; raw recovery is owned by proxy::run.

use crate::adapters::common::{
    compact_json_structure, dedup_lines, make_result, normalized_command, strip_ansi_escape,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::CommandAst;
use crate::proxy::raw_store::RunMeta;

pub struct GenericAdapter;

const EDGE_LINES: usize = 20;
const LINE_LIMIT: usize = 80;
const SIGNAL_LIMIT: usize = 40;

impl CommandAdapter for GenericAdapter {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn matches(&self, _ast: &CommandAst) -> bool {
        true // Fallback adapter
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let stdout = String::from_utf8_lossy(stdout);
        let stderr = String::from_utf8_lossy(stderr);

        // Pre-process: strip ANSI, dedup, try JSON structure compaction
        let clean_stdout = strip_ansi_escape(&stdout);
        let clean_stderr = strip_ansi_escape(&stderr);
        let deduped_stdout = dedup_lines(&clean_stdout);
        let deduped_stderr = dedup_lines(&clean_stderr);

        // Try JSON structure compaction for JSON-heavy output
        let json_compacted = compact_json_structure(&deduped_stdout);
        let input_for_compact = if json_compacted != deduped_stdout {
            &json_compacted
        } else {
            &deduped_stdout
        };

        let compact_stdout = compact_stream(input_for_compact, "stdout");
        let compact_stderr = compact_stream(&deduped_stderr, "stderr");
        let compacted = compact_stdout != stdout || compact_stderr != stderr;
        make_result(
            self.name(),
            format!(
                "[keel] compacted command output\ncommand: {}\nreducer: generic-high-signal; command_family: generic\nstdout: {} lines, {} bytes; stderr: {} lines, {} bytes",
                normalized_command(&meta.program, &meta.args),
                stdout.lines().count(),
                stdout.len(),
                stderr.lines().count(),
                stderr.len()
            ),
            compact_stdout,
            compact_stderr,
            exit_code,
            meta,
            compacted,
        )
    }
}

fn compact_stream(text: &str, label: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let line_count = text.lines().count();
    let signals = signal_lines(text, SIGNAL_LIMIT);
    if signals.is_empty() && line_count <= LINE_LIMIT {
        return text.to_string();
    }

    let mut rendered = String::new();
    if !signals.is_empty() {
        rendered.push_str("[semantic reducer]\n");
        for (_, line) in &signals {
            rendered.push_str(&format!("- {line}\n"));
        }
        rendered.push('\n');
        rendered.push_str(&format!("[{label} high-signal lines]\n"));
        for (line_number, line) in &signals {
            rendered.push_str(&format!("L{line_number}: {line}\n"));
        }
        rendered.push('\n');
    }

    rendered.push_str(&format!("[{label}]\n"));
    rendered.push_str(&compact_edges(text, label, LINE_LIMIT));
    rendered
}

fn compact_edges(text: &str, label: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }

    let edge = EDGE_LINES.min(max_lines / 2).max(5);
    let omitted = lines.len().saturating_sub(edge * 2);
    format!(
        "{}\n... omitted {omitted} {label} lines; raw output saved for recovery ...\n{}",
        lines[..edge].join("\n"),
        lines[lines.len() - edge..].join("\n")
    )
}

fn signal_lines(text: &str, max_lines: usize) -> Vec<(usize, String)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut selected = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        let is_signal = [
            "error",
            "failed",
            "failure",
            "fatal",
            "panic",
            "exception",
            "traceback",
            "assert",
            "warning",
            "denied",
            "not found",
            "cannot",
            "undefined",
            "mismatched",
            "expected",
            "actual",
            "timeout",
            "timed out",
        ]
        .iter()
        .any(|needle| normalized.contains(needle));
        if is_signal && seen.insert(trimmed.to_string()) {
            selected.push((index + 1, trimmed.to_string()));
        }
        if selected.len() >= max_lines {
            break;
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::GenericAdapter;
    use crate::proxy::adapter::CommandAdapter;
    use crate::proxy::raw_store::RunMeta;
    use std::path::PathBuf;

    #[test]
    fn noisy_output_compacts_under_two_hundred_lines_and_keeps_failure() {
        let mut stdout = String::new();
        for index in 0..10_000 {
            if index == 5_000 {
                stdout.push_str("ERROR tests/api/test_users.py:88 expected 201 got 500\n");
            } else {
                stdout.push_str(&format!("noise line {index}\n"));
            }
        }
        let meta = meta(stdout.len());
        let result = GenericAdapter.compact(stdout.as_bytes(), b"", 1, &meta);
        let rendered = crate::proxy::render::render_compact_result(&result);
        assert!(result.compacted);
        assert!(rendered.lines().count() < 200);
        assert!(rendered.contains("expected 201 got 500"));
        assert!(rendered.contains("raw: keel raw test-raw"));
    }

    fn meta(stdout_bytes: usize) -> RunMeta {
        RunMeta {
            raw_id: "test-raw".to_string(),
            command: "unknown noisy".to_string(),
            program: "unknown".to_string(),
            args: vec!["noisy".to_string()],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 1,
            adapter_name: "generic".to_string(),
            raw_path: PathBuf::from("/tmp/test-raw"),
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
