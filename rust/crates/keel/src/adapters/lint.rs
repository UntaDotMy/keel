//! Purpose: Compact lint/type-check output by grouping actionable diagnostics first.
//! Caller: AdapterRegistry for eslint, ruff, mypy, biome, and cargo clippy.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: LintAdapter::compact.
//! Side Effects: None; proxy::run writes raw and compact artifacts.

use crate::adapters::common::{
    make_result, merge_streams, normalized_command, signal_lines, strip_ansi_escape,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct LintAdapter;

impl CommandAdapter for LintAdapter {
    fn name(&self) -> &'static str {
        "lint"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        ast.detected_kind == CommandKind::Lint
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        // Strip ANSI colors (linters typically output colored diagnostics)
        let clean = strip_ansi_escape(&merged);
        let signals = signal_lines(&clean, 120);
        let command = normalized_command(&meta.program, &meta.args);
        let mut rendered = if exit_code == 0 {
            format!("lint ok: {command}")
        } else {
            format!("lint failed: {command}")
        };
        if !signals.is_empty() {
            rendered.push_str("\n\ndiagnostics:");
            for line in signals {
                rendered.push_str(&format!("\n- {line}"));
            }
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
