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
//! Claude Code 2.1.133 added an `effort.level` JSON input field and the
//! `$CLAUDE_EFFORT` environment variable. We attach the value to the same
//! row so an analyzer can correlate slow tools with the active effort.
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

/// Environment variable Claude Code 2.1.133+ exports with the active effort
/// level. Used as the fallback when the `effort.level` JSON field is absent
/// (older CC builds, or hook inputs that omit it).
pub const EFFORT_ENV_VAR: &str = "CLAUDE_EFFORT";

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
pub fn record_tool_timing(event: &str, input: &JsonDocument) -> std::io::Result<bool> {
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
        "effort_level": effort_level_from(input),
    });

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

    writeln!(file, "{line}")?;

    Ok(true)
}

/// Resolve the active effort level for the row.
///
/// Per the Claude Code 2.1.133 changelog, hooks receive the value via the
/// `effort.level` JSON input field AND the `$CLAUDE_EFFORT` environment
/// variable. We prefer the JSON input because it is per-call and authoritative
/// at the moment the hook fires; the env var is the documented fallback.
/// Returns an empty string when neither source is present so the row keeps
/// the same shape every time and a downstream analyzer can detect "missing"
/// without a special-cased absent key.
fn effort_level_from(input: &JsonDocument) -> String {
    input
        .get("effort")
        .and_then(|effort| effort.get("level"))
        .and_then(JsonDocument::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| std::env::var(EFFORT_ENV_VAR).unwrap_or_default())
}

/// Resolve `<claude_home>/state/tool-timings/<YYYY-MM-DD>.jsonl`. Returns
/// None when claude_home cannot be resolved — the caller treats that as
/// "telemetry disabled" and exits silently rather than failing the hook.
fn timings_path_for_today() -> Option<PathBuf> {
    let directory = timings_directory()?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    Some(directory.join(format!("{date}.jsonl")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve `<claude_home>/state/tool-timings/`. Returns None when claude_home
/// cannot be resolved — the caller treats that as "telemetry disabled" and
/// exits silently rather than failing.
fn timings_directory() -> Option<PathBuf> {
    let claude_home = resolve_claude_home("").ok()?;
    Some(claude_home.join("state").join("tool-timings"))
}

/// Delete per-day JSONL files whose filename date is older than `days` ago.
///
/// The date is encoded in the filename (`YYYY-MM-DD.jsonl`), written by
/// `timings_path_for_today`. Filename parsing is preferred over file mtime
/// here because the date is the structural truth — an unrelated `touch`
/// would not move a row's logical age. Files that do not match the
/// `YYYY-MM-DD.jsonl` shape are left untouched: the directory is owned
/// by `record_tool_timing`, but a future writer might drop sidecar files
/// (a README, a manifest) and we do not delete what we did not write.
///
/// Returns the number of files removed. `Ok(0)` when the directory does
/// not exist yet (no rows have ever been recorded). Errors propagate so
/// the SessionEnd caller can log them; the caller already swallows the
/// `Err` to keep telemetry pruning non-fatal.
pub fn prune_older_than(days: u64) -> std::io::Result<usize> {
    let Some(directory) = timings_directory() else {
        return Ok(0);
    };
    if !directory.exists() {
        return Ok(0);
    }

    let today = chrono::Local::now().date_naive();
    let cutoff = match today.checked_sub_days(chrono::Days::new(days)) {
        Some(date) => date,
        // `days` so large it underflowed the calendar — nothing to prune.
        None => return Ok(0),
    };

    let mut removed = 0usize;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(file_date) = chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d") else {
            continue;
        };
        if file_date < cutoff {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// `CLAUDE_TARGET_OVERRIDE` and `CLAUDE_EFFORT` are process-global. Cargo
    /// runs tests in threads, so two tests that set them concurrently would
    /// each see the other's value and read from the wrong path / record the
    /// wrong effort. Serialize with a Mutex so each test gets exclusive
    /// ownership of the env for its run.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Use a unique `CLAUDE_TARGET_OVERRIDE` per test so concurrent runs in
    /// `cargo test` do not stomp on each other's state directory. Also clears
    /// `CLAUDE_EFFORT` for the duration so the surrounding shell does not
    /// leak an effort level into the test row.
    fn with_isolated_claude_home<F: FnOnce(&PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

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

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let previous_effort = std::env::var(EFFORT_ENV_VAR).ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &root);
        std::env::remove_var(EFFORT_ENV_VAR);

        let result = run(&root);

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        match previous_effort {
            Some(value) => std::env::set_var(EFFORT_ENV_VAR, value),
            None => std::env::remove_var(EFFORT_ENV_VAR),
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

            assert_eq!(
                parsed.get("event").and_then(JsonDocument::as_str),
                Some("PostToolUse")
            );
            assert_eq!(
                parsed.get("tool_name").and_then(JsonDocument::as_str),
                Some("Bash")
            );
            assert_eq!(
                parsed.get("duration_ms").and_then(JsonDocument::as_u64),
                Some(1234)
            );
            assert_eq!(
                parsed.get("session_id").and_then(JsonDocument::as_str),
                Some("session-abc")
            );
            assert_eq!(
                parsed.get("cwd").and_then(JsonDocument::as_str),
                Some("/tmp/example")
            );
            assert_eq!(
                parsed.get("effort_level").and_then(JsonDocument::as_str),
                Some(""),
                "effort_level defaults to empty string when neither JSON nor env supply it"
            );
            assert!(parsed
                .get("recorded_at_ms")
                .and_then(JsonDocument::as_u64)
                .is_some());
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

    #[test]
    fn effort_level_prefers_json_input_over_env() {
        // Per the 2.1.133 changelog, hooks receive the level via both the
        // `effort.level` JSON field and the `$CLAUDE_EFFORT` env var. When
        // both are set the JSON wins because it is per-call; the env var is
        // a fallback for older hook inputs.
        with_isolated_claude_home("effort-json-wins", |root| {
            std::env::set_var(EFFORT_ENV_VAR, "low");

            let input = json!({
                "tool_name": "Bash",
                "duration_ms": 10u64,
                "effort": { "level": "high" },
            });

            record_tool_timing("PostToolUse", &input).expect("record");

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let body = fs::read_to_string(
                root.join("state")
                    .join("tool-timings")
                    .join(format!("{date}.jsonl")),
            )
            .expect("read log");
            let parsed: JsonDocument =
                serde_json::from_str(body.lines().next().unwrap()).expect("valid jsonl");
            assert_eq!(
                parsed.get("effort_level").and_then(JsonDocument::as_str),
                Some("high"),
            );
        });
    }

    #[test]
    fn effort_level_falls_back_to_env_var() {
        with_isolated_claude_home("effort-env-fallback", |root| {
            std::env::set_var(EFFORT_ENV_VAR, "medium");

            let input = json!({
                "tool_name": "Bash",
                "duration_ms": 10u64,
            });

            record_tool_timing("PostToolUse", &input).expect("record");

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let body = fs::read_to_string(
                root.join("state")
                    .join("tool-timings")
                    .join(format!("{date}.jsonl")),
            )
            .expect("read log");
            let parsed: JsonDocument =
                serde_json::from_str(body.lines().next().unwrap()).expect("valid jsonl");
            assert_eq!(
                parsed.get("effort_level").and_then(JsonDocument::as_str),
                Some("medium"),
            );
        });
    }

    #[test]
    fn effort_level_is_empty_when_neither_source_provides_it() {
        with_isolated_claude_home("effort-empty", |root| {
            // Helper has already cleared CLAUDE_EFFORT; the input has no
            // effort field. Row should still record with an empty string so
            // the schema stays uniform.
            let input = json!({
                "tool_name": "Bash",
                "duration_ms": 10u64,
            });

            record_tool_timing("PostToolUse", &input).expect("record");

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let body = fs::read_to_string(
                root.join("state")
                    .join("tool-timings")
                    .join(format!("{date}.jsonl")),
            )
            .expect("read log");
            let parsed: JsonDocument =
                serde_json::from_str(body.lines().next().unwrap()).expect("valid jsonl");
            assert_eq!(
                parsed.get("effort_level").and_then(JsonDocument::as_str),
                Some(""),
            );
        });
    }

    #[test]
    fn prune_older_than_removes_only_files_older_than_cutoff() {
        // Three fixture files: today, 5 days ago, 30 days ago. With a 10-day
        // retention only the 30-day file is older than the cutoff and gets
        // removed; today and 5-days-ago survive. Asserting the survivors as
        // well as the removed count guards against an "off by one" cutoff
        // that would also delete the 5-day file.
        with_isolated_claude_home("prune-selective", |root| {
            let timings = root.join("state").join("tool-timings");
            fs::create_dir_all(&timings).expect("create timings dir");

            let today = chrono::Local::now().date_naive();
            let fresh = today;
            let middle = today
                .checked_sub_days(chrono::Days::new(5))
                .expect("sub 5 days");
            let stale = today
                .checked_sub_days(chrono::Days::new(30))
                .expect("sub 30 days");

            for date in [fresh, middle, stale] {
                let path = timings.join(format!("{}.jsonl", date.format("%Y-%m-%d")));
                fs::write(&path, "{}\n").expect("write fixture row");
            }

            let removed = prune_older_than(10).expect("prune");
            assert_eq!(removed, 1, "only the 30-day file should be removed");

            assert!(
                timings
                    .join(format!("{}.jsonl", fresh.format("%Y-%m-%d")))
                    .exists(),
                "today's file must survive"
            );
            assert!(
                timings
                    .join(format!("{}.jsonl", middle.format("%Y-%m-%d")))
                    .exists(),
                "5-day-old file must survive a 10-day retention"
            );
            assert!(
                !timings
                    .join(format!("{}.jsonl", stale.format("%Y-%m-%d")))
                    .exists(),
                "30-day-old file must be removed by a 10-day retention"
            );
        });
    }

    #[test]
    fn prune_older_than_returns_zero_when_directory_missing() {
        // No rows have ever been recorded — the directory does not exist.
        // Prune must succeed with zero removed rather than erroring out.
        with_isolated_claude_home("prune-missing", |root| {
            assert!(!root.join("state").join("tool-timings").exists());
            let removed = prune_older_than(30).expect("prune missing dir");
            assert_eq!(removed, 0);
        });
    }

    #[test]
    fn prune_older_than_skips_files_that_do_not_match_date_shape() {
        // The store directory could contain a future sidecar file (README,
        // manifest) that the prune helper did not write. Filename parsing
        // protects those: anything that does not strip cleanly into a
        // `YYYY-MM-DD` stem is left in place even if it happens to be
        // older than the cutoff.
        with_isolated_claude_home("prune-skip-foreign", |root| {
            let timings = root.join("state").join("tool-timings");
            fs::create_dir_all(&timings).expect("create timings dir");

            // A genuine ancient row that *should* be pruned.
            let stale_date = chrono::Local::now()
                .date_naive()
                .checked_sub_days(chrono::Days::new(100))
                .expect("sub 100 days");
            let stale_row = timings.join(format!("{}.jsonl", stale_date.format("%Y-%m-%d")));
            fs::write(&stale_row, "{}\n").expect("write stale row");

            // Foreign files: wrong extension, wrong stem shape, plain README.
            let foreign_paths = [
                timings.join("README.md"),
                timings.join("not-a-date.jsonl"),
                timings.join("2026-13-01.jsonl"), // invalid month → won't parse
            ];
            for path in &foreign_paths {
                fs::write(path, b"sidecar").expect("write foreign file");
            }

            let removed = prune_older_than(30).expect("prune");
            assert_eq!(removed, 1, "only the matching JSONL must be removed");
            assert!(!stale_row.exists(), "stale row must be removed");
            for path in &foreign_paths {
                assert!(
                    path.exists(),
                    "foreign file must survive: {}",
                    path.display()
                );
            }
        });
    }

    /// Sentinel env var name for the SessionEnd timings prune retention.
    /// Hard-coded here so a rename in `hook_lifecycle.rs` would break this
    /// test instead of silently disabling the env override at the wiring
    /// layer.
    const TIMINGS_RETENTION_ENV_VAR: &str = "CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS";

    /// Set `TIMINGS_RETENTION_ENV_VAR`, run the closure, restore prior
    /// state. Pairs with `with_isolated_claude_home` which already holds
    /// the process-wide `ENV_LOCK` for the duration of the test, so this
    /// helper does not need its own lock — it just guarantees the env
    /// var leaves the process in the same state it was found.
    fn with_timings_retention_env<F: FnOnce() -> R, R>(value: Option<&str>, run: F) -> R {
        let previous = std::env::var(TIMINGS_RETENTION_ENV_VAR).ok();
        match value {
            Some(v) => std::env::set_var(TIMINGS_RETENTION_ENV_VAR, v),
            None => std::env::remove_var(TIMINGS_RETENTION_ENV_VAR),
        }
        let result = run();
        match previous {
            Some(v) => std::env::set_var(TIMINGS_RETENTION_ENV_VAR, v),
            None => std::env::remove_var(TIMINGS_RETENTION_ENV_VAR),
        }
        result
    }

    #[test]
    fn prune_tool_timings_store_honors_env_var_override() {
        // Wiring-layer test: with `CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS=2`
        // a 5-day-old file gets pruned even though the compiled-in default
        // (30 days) would have kept it. Confirms the env var name in
        // `hook_lifecycle.rs` matches the contract documented for
        // operators and that the parse + override path is reachable.
        with_isolated_claude_home("prune-env-override", |root| {
            with_timings_retention_env(Some("2"), || {
                let timings = root.join("state").join("tool-timings");
                fs::create_dir_all(&timings).expect("create timings dir");

                let stale_date = chrono::Local::now()
                    .date_naive()
                    .checked_sub_days(chrono::Days::new(5))
                    .expect("sub 5 days");
                let stale_row = timings.join(format!("{}.jsonl", stale_date.format("%Y-%m-%d")));
                fs::write(&stale_row, "{}\n").expect("write stale row");

                let mut stderr = Vec::new();
                crate::runner::hook_lifecycle::prune_tool_timings_store(&mut stderr);

                assert!(
                    !stale_row.exists(),
                    "5-day-old file must be removed when env var sets retention to 2 days"
                );
                assert!(
                    stderr.is_empty(),
                    "successful prune must not write to stderr: {}",
                    String::from_utf8_lossy(&stderr)
                );
            });
        });
    }

    #[test]
    fn prune_tool_timings_store_disabled_when_retention_is_zero() {
        // Wiring-layer test: `CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS=0` is
        // the documented escape hatch for operators who want to retain
        // every row indefinitely. The function must early-return without
        // touching the directory, even when an obviously-ancient file
        // exists.
        with_isolated_claude_home("prune-env-disabled", |root| {
            with_timings_retention_env(Some("0"), || {
                let timings = root.join("state").join("tool-timings");
                fs::create_dir_all(&timings).expect("create timings dir");

                let ancient_date = chrono::Local::now()
                    .date_naive()
                    .checked_sub_days(chrono::Days::new(365))
                    .expect("sub 365 days");
                let ancient_row =
                    timings.join(format!("{}.jsonl", ancient_date.format("%Y-%m-%d")));
                fs::write(&ancient_row, "{}\n").expect("write ancient row");

                let mut stderr = Vec::new();
                crate::runner::hook_lifecycle::prune_tool_timings_store(&mut stderr);

                assert!(
                    ancient_row.exists(),
                    "retention=0 must disable the prune even for a 365-day-old file"
                );
            });
        });
    }
}
