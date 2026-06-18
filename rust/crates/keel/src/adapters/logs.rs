//! Purpose: Compact logs and infrastructure command output by deduplicating repeated resource messages.
//! Caller: AdapterRegistry for docker, kubectl, terraform, and aws logs commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: LogsAdapter::compact.
//! Side Effects: None; raw recovery is handled by proxy::run.

use crate::adapters::common::{
    compact_edges, dedup_lines, make_result, merge_streams, normalized_command, signal_lines,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct LogsAdapter;

impl CommandAdapter for LogsAdapter {
    fn name(&self) -> &'static str {
        "logs"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Logs
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        // Deduplicate consecutive identical log lines (common in docker/kubectl polling)
        let deduped = dedup_lines(&merged);
        let signals = signal_lines(&deduped, 100);
        let command = normalized_command(&meta.program, &meta.args);
        let mut rendered = format!(
            "{}: {}",
            if exit_code == 0 {
                "infra ok"
            } else {
                "infra failed"
            },
            command
        );
        if !signals.is_empty() {
            rendered.push_str("\n\nsignal:");
            for line in signals {
                rendered.push_str(&format!("\n- {line}"));
            }
        } else {
            rendered.push_str("\n\n");
            rendered.push_str(&compact_edges(&deduped, "log output", 100));
        }
        make_result(
            self.name(),
            normalized_command(&meta.program, &meta.args),
            rendered,
            String::new(),
            exit_code,
            meta,
            true,
        )
    }
}
