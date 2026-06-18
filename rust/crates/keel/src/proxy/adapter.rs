//! Purpose: Define the command-adapter contract used by the token-saving proxy.
//! Caller: proxy::run selects adapters through AdapterRegistry before rendering compact output.
//! Dependencies: CommandAst, RawRun, and RunMeta from the proxy layer.
//! Main Functions: CommandAdapter, CompactResult.
//! Side Effects: None; adapters return data for the proxy to persist and render.

use crate::proxy::command_ast::CommandAst;
use crate::proxy::raw_store::RunMeta;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CompactResult {
    pub adapter_name: String,
    pub compacted: bool,
    pub summary: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub raw_id: String,
    pub raw_path: PathBuf,
    pub original_stdout_bytes: usize,
    pub original_stderr_bytes: usize,
    pub compact_stdout_bytes: usize,
    pub compact_stderr_bytes: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub estimated_tokens_saved: isize,
    pub savings_pct: f64,
    pub warnings: Vec<String>,
}

pub trait CommandAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, ast: &CommandAst) -> bool;
    fn compact(
        &self,
        stdout: &[u8],
        stderr: &[u8],
        exit_code: i32,
        meta: &RunMeta,
    ) -> CompactResult;
    fn rewrite_args(&self, _ast: &CommandAst) -> Option<CommandAst> {
        None
    }
}
