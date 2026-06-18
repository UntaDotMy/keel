//! Purpose: Compact build and package-manager output with errors first and repeated noise removed.
//! Caller: AdapterRegistry for cargo build/check, tsc, dotnet build, and npm/pnpm/yarn install-style commands.
//! Dependencies: CommandAst classification, RunMeta, and shared adapter helpers.
//! Main Functions: BuildAdapter::compact.
//! Side Effects: None; proxy::run owns raw persistence.

use crate::adapters::common::{
    compact_edges, make_result, merge_streams, normalized_command, signal_lines, strip_ansi_escape,
};
use crate::proxy::adapter::{CommandAdapter, CompactResult};
use crate::proxy::command_ast::{CommandAst, CommandKind};
use crate::proxy::raw_store::RunMeta;

pub struct BuildAdapter;

impl CommandAdapter for BuildAdapter {
    fn name(&self) -> &'static str {
        "build"
    }

    fn matches(&self, ast: &CommandAst) -> bool {
        matches!(
            ast.detected_kind,
            CommandKind::Build | CommandKind::PackageManager
        )
    }

    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult {
        let merged = merge_streams(stdout, stderr);
        // Strip ANSI colors before signal analysis (build tools often use colored output)
        let clean = strip_ansi_escape(&merged);
        let signals = signal_lines(&clean, 80);
        let command = normalized_command(&meta.program, &meta.args);
        let mut rendered = if exit_code == 0 {
            format!("build ok: {command}")
        } else {
            format!("build failed: {command}")
        };
        if !signals.is_empty() {
            rendered.push_str("\n\nerrors and warnings:");
            for line in signals {
                rendered.push_str(&format!("\n- {line}"));
            }
        } else {
            rendered.push_str("\n\n");
            rendered.push_str(&compact_edges(&clean, "build output", 80));
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
