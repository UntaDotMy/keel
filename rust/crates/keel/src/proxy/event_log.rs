//! Purpose: Append token-saving measurement events for the gain/discover surfaces.
//! Caller: proxy::run after raw output is saved and compact output is rendered.
//! Dependencies: RunMeta, CompactResult, Claude home resolution, and JSONL file storage.
//! Main Functions: record_compaction_event, rotate_event_log_if_needed.
//! Side Effects: Appends one JSON object per proxied command to the command compaction event log, rotates when size exceeds 5MB.

use crate::proxy::adapter::CompactResult;
use crate::proxy::injection_guard::InjectionFinding;
use crate::proxy::raw_store::RunMeta;
use crate::runtime::{display_path, resolve_claude_home, COMMAND_COMPACTION_EVENTS_FILE_NAME};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;

/// Maximum event log size in bytes before rotation trims old entries (5 MB).
const MAX_EVENT_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Number of most-recent lines to keep after rotation.
const EVENT_LOG_KEEP_LINES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEvent {
    pub timestamp: String,
    #[serde(flatten)]
    pub meta: RunMeta,
}

/// Rotate the event log when it exceeds MAX_EVENT_LOG_BYTES by keeping only
/// the most recent EVENT_LOG_KEEP_LINES lines. Silently skips on any I/O error
/// so a rotation failure never blocks event recording.
fn rotate_event_log_if_needed(event_path: &std::path::Path) {
    let size = match fs::metadata(event_path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return,
    };
    if size <= MAX_EVENT_LOG_BYTES {
        return;
    }
    let content = match fs::read_to_string(event_path) {
        Ok(text) => text,
        Err(_) => return,
    };
    let kept_lines: Vec<&str> = content
        .lines()
        .rev()
        .take(EVENT_LOG_KEEP_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let trimmed = kept_lines.join("\n") + "\n";
    let _ = fs::write(event_path, trimmed);
}

pub fn record_compaction_event(
    meta: &RunMeta,
    compact: &CompactResult,
    findings: &[InjectionFinding],
) {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    if fs::create_dir_all(&claude_home).is_err() {
        return;
    }
    let event_path = claude_home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME);
    rotate_event_log_if_needed(&event_path);
    let injection_patterns: Vec<&str> = findings.iter().map(|f| f.pattern).collect();
    let payload = serde_json::json!({
        "timestamp": meta.started_at.to_string(),
        "command": &meta.command,
        "exit_code": meta.exit_code,
        "exitCode": meta.exit_code,
        "compacted": meta.compacted,
        "adapter_name": &meta.adapter_name,
        "adapterName": &meta.adapter_name,
        "tokens_before": meta.estimated_tokens_before,
        "reducer": &compact.adapter_name,
        "tokens_after": meta.estimated_tokens_after,
        "commandFamily": &compact.adapter_name,
        "tokens_saved": meta.estimated_tokens_saved.max(0) as usize,
        "exact_tokens_before": meta.estimated_tokens_before,
        "exact_tokens_after": meta.estimated_tokens_after,
        "exact_tokens_saved": meta.estimated_tokens_saved.max(0) as usize,
        "tokenizer": "o200k_base",
        "token_counting": "exact",
        "summary": &compact.summary,
        "savings_pct": meta.savings_pct,
        "stdoutBytes": meta.stdout_bytes,
        "raw_path": display_path(&meta.raw_path),
        "stderrBytes": meta.stderr_bytes,
        "rawBytes": meta.stdout_bytes + meta.stderr_bytes,
        "compact_path": display_path(&meta.compact_path),
        "renderedBytes": meta.compact_stdout_bytes + meta.compact_stderr_bytes,
        "savedBytes": (meta.stdout_bytes + meta.stderr_bytes)
            .saturating_sub(meta.compact_stdout_bytes + meta.compact_stderr_bytes),
        "tokensBefore": meta.estimated_tokens_before,
        "tokensAfter": meta.estimated_tokens_after,
        "tokensSaved": meta.estimated_tokens_saved.max(0) as usize,
        "savingsPct": meta.savings_pct,
        "rawPath": display_path(&meta.raw_path),
        "rawOutputPath": display_path(&meta.raw_path),
        "compactPath": display_path(&meta.compact_path),
        "agent": &meta.agent,
        "workspace": display_path(&meta.workspace),
        "injection_neutralized": !findings.is_empty(),
        "injection_findings": findings.len(),
        "injection_patterns": injection_patterns,
    });
    let Ok(rendered) = serde_json::to_string(&payload) else {
        return;
    };
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(event_path)
    {
        let _ = writeln!(file, "{rendered}");
    }
}
