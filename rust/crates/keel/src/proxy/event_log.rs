//! Purpose: Append token-saving measurement events for the gain/discover surfaces.
//! Caller: proxy::run after raw output is saved and compact output is rendered.
//! Dependencies: RunMeta, CompactResult, harness home resolution, and JSONL file storage.
//! Main Functions: record_compaction_event, rotate_event_log_if_needed.
//! Side Effects: Appends one JSON object per proxied command to the command compaction event log, rotates when size exceeds 5MB.

use crate::proxy::adapter::CompactResult;
use crate::proxy::injection_guard::InjectionFinding;
use crate::proxy::raw_store::RunMeta;
use crate::runtime::{
    display_path, resolve_claude_home, write_text, COMMAND_COMPACTION_EVENTS_FILE_NAME,
};
use std::fs;
use std::io::Write;

/// Maximum event log size in bytes before rotation trims old entries (5 MB).
const MAX_EVENT_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Number of most-recent lines to keep after rotation.
const EVENT_LOG_KEEP_LINES: usize = 10_000;

const SECRET_VALUE_FLAGS: &[&str] = &[
    "--access-token",
    "--api-key",
    "--api_key",
    "--apikey",
    "--auth-token",
    "--authorization",
    "--bearer-token",
    "--client-secret",
    "--connection-string",
    "--credential",
    "--credentials",
    "--database-url",
    "--passphrase",
    "--passwd",
    "--password",
    "--private-key",
    "--redis-url",
    "--secret",
    "--secret-access-key",
    "--secret-key",
    "--token",
];

fn sensitive_assignment_key(key: &str) -> bool {
    let key = key
        .trim_matches(|character: char| character == '$' || character == '\'' || character == '"')
        .to_ascii_lowercase()
        .replace('-', "_");
    [
        "access_key",
        "api_key",
        "auth_token",
        "client_secret",
        "credential",
        "database_url",
        "password",
        "passwd",
        "private_key",
        "redis_url",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn argument_has_secret_marker(argument: &str) -> bool {
    let trimmed = argument.trim_matches(|character: char| character == '\'' || character == '"');
    let lower = trimmed.to_ascii_lowercase();
    if SECRET_VALUE_FLAGS.iter().any(|flag| {
        lower == *flag
            || lower
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('='))
    }) {
        return true;
    }
    if let Some((key, value)) = trimmed.split_once('=') {
        if !value.is_empty() && sensitive_assignment_key(key) {
            return true;
        }
    }
    crate::adapters::common::redact_possible_secret(trimmed) != trimmed
}

fn redacted_event_command(meta: &RunMeta) -> String {
    let command_was_redacted =
        crate::adapters::common::redact_possible_secret(&meta.command) != meta.command;
    let argv_has_secret = meta
        .args
        .iter()
        .any(|argument| argument_has_secret_marker(argument));
    let embedded_shell_secret = meta
        .command
        .split_whitespace()
        .any(argument_has_secret_marker);
    if !command_was_redacted && !argv_has_secret && !embedded_shell_secret {
        return meta.command.clone();
    }

    let program = meta.program.trim();
    let safe_program = if program.is_empty()
        || crate::adapters::common::redact_possible_secret(program) != program
    {
        "command"
    } else {
        program
    };
    format!("{safe_program} [redacted arguments]")
}

/// Rotate the event log when it exceeds MAX_EVENT_LOG_BYTES by keeping only
/// the most recent EVENT_LOG_KEEP_LINES lines. Silently skips on any I/O error
/// so a rotation failure never blocks event recording.
///
/// Concurrency: the rotate-then-append sequence in `record_compaction_event`
/// has no cross-process lock, so two concurrent `keel run` processes could both
/// rotate. To avoid a half-written log under that race, rotation writes to a
/// sibling temp file and renames it into place (atomic on the same filesystem),
/// mirroring the `write_text` pattern used elsewhere. A concurrent appender
/// opening the old inode during the rename window at worst appends to a stale
/// file that the next rotation reclaims — no corruption, at most a lost line.
fn rotate_event_log_if_needed(event_path: &std::path::Path) {
    rotate_event_log(event_path, MAX_EVENT_LOG_BYTES, EVENT_LOG_KEEP_LINES);
}

fn rotate_event_log(event_path: &std::path::Path, max_bytes: u64, keep_lines: usize) {
    // Versions before the unique atomic writer used this fixed sibling name.
    // It is Keel-owned and safe to reclaim after an interrupted rotation.
    let legacy_temp = event_path.with_extension("log.rotate-tmp");
    let _ = fs::remove_file(legacy_temp);

    let size = match fs::metadata(event_path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return,
    };
    if size <= max_bytes {
        return;
    }
    let content = match fs::read_to_string(event_path) {
        Ok(text) => text,
        Err(_) => return,
    };
    let kept_lines: Vec<&str> = content
        .lines()
        .rev()
        .take(keep_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let trimmed = kept_lines.join("\n") + "\n";
    // The shared writer uses collision-free sibling temps, reclaims temps
    // whose owner died, and handles replace semantics on Windows.
    let _ = write_text(event_path, &trimmed);
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
    let redacted_command = redacted_event_command(meta);
    let (tokens_after, tokens_saved, savings_pct) = if meta.compacted {
        (
            meta.estimated_tokens_after,
            meta.estimated_tokens_saved.max(0) as usize,
            meta.savings_pct,
        )
    } else {
        (meta.estimated_tokens_before, 0, 0.0)
    };
    let payload = serde_json::json!({
        "timestamp": meta.started_at.to_string(),
        "command": &redacted_command,
        "exit_code": meta.exit_code,
        "exitCode": meta.exit_code,
        "compacted": meta.compacted,
        "adapter_name": &meta.adapter_name,
        "adapterName": &meta.adapter_name,
        "tokens_before": meta.estimated_tokens_before,
        "reducer": &compact.adapter_name,
        "tokens_after": tokens_after,
        "commandFamily": &compact.adapter_name,
        "tokens_saved": tokens_saved,
        "exact_tokens_before": meta.estimated_tokens_before,
        "exact_tokens_after": tokens_after,
        "exact_tokens_saved": tokens_saved,
        "tokenizer": "o200k_base",
        "token_counting": "exact",
        "summary": &compact.summary,
        "savings_pct": savings_pct,
        "stdoutBytes": meta.stdout_bytes,
        "raw_path": display_path(&meta.raw_path),
        "stderrBytes": meta.stderr_bytes,
        "rawBytes": meta.stdout_bytes + meta.stderr_bytes,
        "compact_path": display_path(&meta.compact_path),
        "renderedBytes": meta.compact_stdout_bytes + meta.compact_stderr_bytes,
        "savedBytes": (meta.stdout_bytes + meta.stderr_bytes)
            .saturating_sub(meta.compact_stdout_bytes + meta.compact_stderr_bytes),
        "tokensBefore": meta.estimated_tokens_before,
        "tokensAfter": tokens_after,
        "tokensSaved": tokens_saved,
        "savingsPct": savings_pct,
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
        .open(&event_path)
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = file.metadata() {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                // why: best-effort file permission restriction on Unix
                let _ = fs::set_permissions(&event_path, perms);
            }
        }
        let _ = writeln!(file, "{rendered}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(command: &str, program: &str, args: &[&str]) -> RunMeta {
        RunMeta {
            raw_id: "20260911-000000-deadbeef".to_string(),
            command: command.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: std::path::PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "generic".to_string(),
            raw_path: std::path::PathBuf::from("raw"),
            compact_path: std::path::PathBuf::from("compact"),
            agent: "test".to_string(),
            workspace: std::path::PathBuf::from("."),
            stdout_bytes: 0,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 0,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        }
    }

    #[test]
    fn rotation_replaces_existing_log_and_keeps_newest_lines() {
        let root = crate::test_support::unique_temp_dir("keel-event-log-rotate");
        let event_path = root.join("events.jsonl");
        fs::write(&event_path, "one\ntwo\nthree\n").unwrap();

        rotate_event_log(&event_path, 8, 2);

        assert_eq!(fs::read_to_string(event_path).unwrap(), "two\nthree\n");
    }

    #[test]
    fn rotation_reclaims_legacy_temp_even_when_log_is_small() {
        let root = crate::test_support::unique_temp_dir("keel-event-log-stale");
        let event_path = root.join("events.jsonl");
        let legacy_temp = event_path.with_extension("log.rotate-tmp");
        fs::write(&event_path, "one\n").unwrap();
        fs::write(&legacy_temp, "stale").unwrap();

        rotate_event_log(&event_path, 1024, 2);

        assert!(!legacy_temp.exists());
        assert_eq!(fs::read_to_string(event_path).unwrap(), "one\n");
    }

    #[test]
    fn event_command_redacts_separate_secret_flag_values() {
        let meta = sample_meta(
            // The display command can omit or normalize argv, so the decision
            // must inspect the authoritative argument vector as well.
            "curl https://example.invalid",
            "curl",
            &["--password", "hunter2", "https://example.invalid"],
        );

        let redacted = redacted_event_command(&meta);
        assert_eq!(redacted, "curl [redacted arguments]");
        assert!(!redacted.contains("hunter2"));
    }

    #[test]
    fn event_command_redacts_secret_assignments_in_argv() {
        for argument in [
            "--api-key=sk-example",
            "DATABASE_URL=postgres://user:secret@example.invalid/db",
            "$AUTH_TOKEN=opaque-value",
        ] {
            let meta = sample_meta("deploy", "deploy", &[argument]);
            assert_eq!(
                redacted_event_command(&meta),
                "deploy [redacted arguments]",
                "expected argv assignment to be treated as sensitive: {argument}"
            );
        }
    }

    #[test]
    fn event_command_preserves_non_secret_command_shape() {
        let meta = sample_meta(
            "cargo test --workspace --tokenize",
            "cargo",
            &["test", "--workspace", "--tokenize"],
        );
        assert_eq!(
            redacted_event_command(&meta),
            "cargo test --workspace --tokenize"
        );
    }
}
