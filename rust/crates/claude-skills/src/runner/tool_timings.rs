//! Purpose: Append-only per-day JSONL log of tool execution durations.
//! Caller: runner::hook_lifecycle for PostToolUse and PostToolUseFailure.
//! Dependencies: serde_json, runtime::resolve_claude_home, std::fs.
//! Main Functions: record_tool_timing.
//! Side Effects: Creates `<claude_home>/state/tool-timings/<YYYY-MM-DD>.jsonl` and appends one line per call.
//!
//! Design note: Claude Code 2.1.119 added `duration_ms` to PostToolUse and
//! PostToolUseFailure hook input (tool execution time, excluding permission
//! prompts and PreToolUse hooks). We persist the field verbatim — no unit
//! math, no aggregation — so a future analyzer can decide its own buckets.
//! The file is append-only JSONL with daily rotation by filename so a long
//! session does not produce a single unbounded file. Errors are logged to
//! stderr and swallowed: a telemetry write must never fail the hook.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as JsonDocument};

use crate::runtime::resolve_claude_home;

/// JSON key Claude Code uses on PostToolUse / PostToolUseFailure input for
/// the tool execution duration. Documented in the v2.1.119 changelog.
pub const TIMING_INPUT_FIELD: &str = "duration_ms";

/// Append one JSONL line capturing the tool timing for `event` from `input`.
///
/// `input` is the raw stdin JSON Claude Code delivered to the hook, already
/// parsed by the caller. Returns `Ok(false)` when the field is absent (older
/// CC builds, or events without timing) so the caller can decide whether to
/// log a debug note. Returns `Ok(true)` when a line was appended.
///
/// `event` is the canonical PascalCase Claude Code event name, e.g.
/// `"PostToolUse"` or `"PostToolUseFailure"` — recorded verbatim in the JSONL
/// row so a single analyzer can split success from failure.
pub fn record_tool_timing(
    event: &str,
    input: &JsonDocument,
) -> std::io::Result<bool> {
    // Accept either an integer or a float so we still record if Claude Code
    // ever emits the field as a JSON number from a JS source (e.g. `1234.0`).
    // `as_u64()` alone would return None on a float and silently drop the
    // sample. The changelog describes the value as "tool execution time in
    // ms" so a fractional component carries no meaning we'd lose by truncating.
    let Some(duration_ms) = input
        .get(TIMING_INPUT_FIELD)
        .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
    else {
        return Ok(false);
    };

    let Some(path) = timings_path_for_today() else {
        return Ok(false);
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let line = json!({
        "recorded_at_ms": now_ms(),
        "event": event,
        "tool_name": input.get("tool_name").and_then(JsonDocument::as_str).unwrap_or_default(),
        "duration_ms": duration_ms,
        "session_id": input.get("session_id").and_then(JsonDocument::as_str).unwrap_or_default(),
        "cwd": input.get("cwd").and_then(JsonDocument::as_str).unwrap_or_default(),
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    writeln!(file, "{line}")?;

    Ok(true)
}

/// Resolve `<claude_home>/state/tool-timings/<YYYY-MM-DD>.jsonl`. Returns
/// None when claude_home cannot be resolved — the caller treats that as
/// "telemetry disabled" and exits silently rather than failing the hook.
fn timings_path_for_today() -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    Some(
        claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{date}.jsonl")),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// `CLAUDE_TARGET_OVERRIDE` is process-global. Cargo runs tests in
    /// threads, so two tests that set the override concurrently would each
    /// see the other's value and read from the wrong path. Serialize with a
    /// Mutex so each test gets exclusive ownership of the env for its run.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Use a unique `CLAUDE_TARGET_OVERRIDE` per test so concurrent runs in
    /// `cargo test` do not stomp on each other's state directory.
    fn with_isolated_claude_home<F: FnOnce(&PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "claude-skills-tool-timings-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test claude home");

        let previous = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &root);

        let result = run(&root);

        match previous {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }

        let _ = fs::remove_dir_all(&root);
        result
    }

    #[test]
    fn record_tool_timing_writes_jsonl_line_when_duration_present() {
        with_isolated_claude_home("happy", |root| {
            let input = json!({
                "tool_name": "Bash",
                "duration_ms": 1234u64,
                "session_id": "session-abc",
                "cwd": "/tmp/example",
            });

            let recorded = record_tool_timing("PostToolUse", &input).expect("record");
            assert!(recorded);

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let log_path = root
                .join("state")
                .join("tool-timings")
                .join(format!("{date}.jsonl"));

            let body = fs::read_to_string(&log_path).expect("read log");
            let line = body.lines().next().expect("at least one line");
            let parsed: JsonDocument = serde_json::from_str(line).expect("valid jsonl");

            assert_eq!(parsed.get("event").and_then(JsonDocument::as_str), Some("PostToolUse"));
            assert_eq!(parsed.get("tool_name").and_then(JsonDocument::as_str), Some("Bash"));
            assert_eq!(parsed.get("duration_ms").and_then(JsonDocument::as_u64), Some(1234));
            assert_eq!(parsed.get("session_id").and_then(JsonDocument::as_str), Some("session-abc"));
            assert_eq!(parsed.get("cwd").and_then(JsonDocument::as_str), Some("/tmp/example"));
            assert!(parsed.get("recorded_at_ms").and_then(JsonDocument::as_u64).is_some());
        });
    }

    #[test]
    fn record_tool_timing_skips_when_duration_absent() {
        with_isolated_claude_home("missing", |root| {
            let input = json!({
                "tool_name": "Bash",
            });

            let recorded = record_tool_timing("PostToolUse", &input).expect("record");
            assert!(!recorded);

            // No file should have been created when the duration field is absent.
            assert!(!root.join("state").join("tool-timings").exists());
        });
    }

    #[test]
    fn record_tool_timing_appends_subsequent_calls() {
        with_isolated_claude_home("append", |root| {
            for index in 0..3u64 {
                let input = json!({
                    "tool_name": "Edit",
                    "duration_ms": index * 100,
                });
                record_tool_timing("PostToolUse", &input).expect("record");
            }

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let log_path = root
                .join("state")
                .join("tool-timings")
                .join(format!("{date}.jsonl"));

            let body = fs::read_to_string(&log_path).expect("read log");
            assert_eq!(body.lines().count(), 3, "each call should append one line");
        });
    }

    #[test]
    fn record_tool_timing_records_failure_event_distinctly() {
        with_isolated_claude_home("failure", |root| {
            let input = json!({
                "tool_name": "Bash",
                "duration_ms": 42u64,
            });

            record_tool_timing("PostToolUseFailure", &input).expect("record");

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let log_path = root
                .join("state")
                .join("tool-timings")
                .join(format!("{date}.jsonl"));

            let body = fs::read_to_string(&log_path).expect("read log");
            let line = body.lines().next().expect("line");
            let parsed: JsonDocument = serde_json::from_str(line).expect("valid jsonl");
            assert_eq!(
                parsed.get("event").and_then(JsonDocument::as_str),
                Some("PostToolUseFailure"),
            );
        });
    }

    #[test]
    fn record_tool_timing_accepts_float_duration() {
        // Claude Code's hook input is JSON; if a value originates from a JS
        // `number` the serializer can emit `1234.0` instead of `1234`. Make
        // sure we still record the sample (truncated to integer ms) instead
        // of silently dropping it.
        with_isolated_claude_home("float", |root| {
            let input = json!({
                "tool_name": "Read",
                "duration_ms": 1234.5,
            });

            let recorded = record_tool_timing("PostToolUse", &input).expect("record");
            assert!(recorded);

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let log_path = root
                .join("state")
                .join("tool-timings")
                .join(format!("{date}.jsonl"));

            let body = fs::read_to_string(&log_path).expect("read log");
            let line = body.lines().next().expect("line");
            let parsed: JsonDocument = serde_json::from_str(line).expect("valid jsonl");
            assert_eq!(
                parsed.get("duration_ms").and_then(JsonDocument::as_u64),
                Some(1234),
            );
        });
    }
}
