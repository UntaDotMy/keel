//! Purpose: Read-only summary view over the tool-timings JSONL store.
//! Caller: commands.rs `telemetry summary` arm via `Application::run_telemetry_command`.
//! Dependencies: serde_json, runner::tool_timings::iter_day_files, args::FlagSet, json::Value.
//! Main Functions: run_telemetry_command (CLI surface), aggregate_rows (pure aggregator), read_rows (line streamer).
//! Side Effects: Writes a table or JSON payload to the supplied writer; does not modify the JSONL store.
//!
//! Design note: the JSONL row schema is owned by `tool_timings.rs` —
//! `record_tool_timing` writes `{recorded_at_ms, event, tool_name, duration_ms,
//! session_id, cwd, effort_level}`. This module is the *reader* and must
//! tolerate three failure modes silently (the writer is fail-open by
//! contract): missing directory, missing day file, malformed JSONL line.
//! A `--session` filter or `--top` truncation never propagates a non-zero
//! exit code to the caller.

use std::fs;
use std::io::Write;

use serde_json::Value as JsonDocument;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runner::tool_timings::iter_day_files;

/// One parsed JSONL row. Only the fields the aggregator needs are kept; the
/// rest of the row is discarded so a future schema addition does not break
/// the reader. `session_id` is preserved on the row even though the
/// aggregator does not key on it: callers in `read_rows` need the value
/// to apply the optional `--session` filter, and downstream tooling (a
/// future `tail` subcommand, debugging) benefits from carrying it through.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TimingRow {
    pub tool_name: String,
    pub duration_ms: u64,
    pub session_id: String,
}

/// One row in the aggregated summary table — keyed by tool_name with
/// pre-computed count/sum/mean/max in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSummary {
    pub tool_name: String,
    pub count: u64,
    pub sum_ms: u64,
    pub mean_ms: u64,
    pub max_ms: u64,
}

/// CLI dispatcher for `keel telemetry`. Mirrors the `flow` shape
/// (one parent command, one or more subcommands). Today only `summary` is
/// supported; the structure leaves room for `tail` / `export` later.
pub fn run_telemetry_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() {
        render_telemetry_help(standard_output);
        return 0;
    }
    let subcommand_name = arguments[0].trim();
    let subcommand_arguments = &arguments[1..];
    match subcommand_name {
        "summary" => run_telemetry_summary(subcommand_arguments, standard_output, standard_error),
        "help" | "--help" | "-h" => {
            render_telemetry_help(standard_output);
            0
        }
        _ => {
            let _ = writeln!(
                standard_error,
                "Unknown telemetry subcommand: {subcommand_name}"
            );
            render_telemetry_help(standard_output);
            1
        }
    }
}

fn run_telemetry_summary(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("telemetry summary");
    flag_set.string_flag("days", "1");
    flag_set.string_flag("session", "");
    flag_set.string_flag("top", "10");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    // FlagSet has no native int_flag, so the documented pattern (see
    // `runner::run_raw_prune`) is `string_flag` + manual parse with a
    // sane default. We treat a malformed value as "use the default"
    // rather than erroring out — telemetry is a read-only view and a
    // typo on the command line should not block the report.
    let days = flag_set
        .string_value("days")
        .trim()
        .parse::<u64>()
        .unwrap_or(1);
    let top = flag_set
        .string_value("top")
        .trim()
        .parse::<usize>()
        .unwrap_or(10);
    let session_filter = flag_set.string_value("session").trim().to_string();
    let emit_json = flag_set.bool_value("json");

    let day_files = match iter_day_files(days) {
        Ok(files) => files,
        Err(io_error) => {
            // Even an IO error reading the directory should not crash the
            // whole subcommand — surface to stderr and emit an empty
            // summary, which is what every fail-open caller expects.
            let _ = writeln!(
                standard_error,
                "telemetry summary: unable to read tool-timings directory: {io_error}"
            );
            Vec::new()
        }
    };

    let session_filter_opt = if session_filter.is_empty() {
        None
    } else {
        Some(session_filter.as_str())
    };
    let rows = read_rows(
        day_files.iter().map(|(_, path)| path.as_path()),
        session_filter_opt,
    );
    let summaries = aggregate_rows(rows, top);

    if emit_json {
        let payload = render_summary_json(days, &summaries);
        if let Err(write_error) = write_indented(standard_output, &payload) {
            let _ = writeln!(
                standard_error,
                "telemetry summary: unable to render JSON: {write_error}"
            );
            return 1;
        }
        return 0;
    }
    render_summary_table(standard_output, days, &summaries);
    0
}

/// Read every line of every day file, drop malformed rows silently, and
/// optionally restrict to a single session. Returns a `Vec` (not an
/// iterator) because each call site immediately collects into the
/// aggregator and the row volume is bounded by the user's daily tool use.
pub fn read_rows<'a>(
    day_files: impl IntoIterator<Item = &'a std::path::Path>,
    session_filter: Option<&str>,
) -> Vec<TimingRow> {
    let mut out: Vec<TimingRow> = Vec::new();
    for path in day_files {
        let Ok(body) = fs::read_to_string(path) else {
            // Day file vanished between iter_day_files and now (e.g. a
            // concurrent prune). Skip silently — reader contract.
            continue;
        };
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(parsed): Option<JsonDocument> = serde_json::from_str(trimmed).ok() else {
                // Malformed line — record_tool_timing is fail-open on the
                // way in too, so a partial write or hand-edit could leave
                // a row that does not parse. Skip silently.
                continue;
            };
            let tool_name = parsed
                .get("tool_name")
                .and_then(JsonDocument::as_str)
                .unwrap_or("")
                .to_string();
            let duration_ms = parsed
                .get("duration_ms")
                .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
                .unwrap_or(0);
            let session_id = parsed
                .get("session_id")
                .and_then(JsonDocument::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(filter) = session_filter {
                if session_id != filter {
                    continue;
                }
            }
            out.push(TimingRow {
                tool_name,
                duration_ms,
                session_id,
            });
        }
    }
    out
}

/// Group rows by `tool_name`, compute count/sum/mean/max in ms, sort by
/// sum_ms descending, and truncate to `top` entries. `top == 0` returns
/// the full list (no truncation) so a caller passing `--top 0` can dump
/// every tool. Empty input yields an empty Vec — never an error.
///
/// Pure function: no filesystem, no clock. The whole point of separating
/// it from `read_rows` is that we can test ordering and truncation
/// without touching the JSONL store.
pub fn aggregate_rows(rows: impl IntoIterator<Item = TimingRow>, top: usize) -> Vec<ToolSummary> {
    use std::collections::HashMap;

    #[derive(Default)]
    struct Accumulator {
        count: u64,
        sum_ms: u64,
        max_ms: u64,
    }
    let mut buckets: HashMap<String, Accumulator> = HashMap::new();
    for row in rows {
        let entry = buckets.entry(row.tool_name).or_default();
        entry.count += 1;
        entry.sum_ms = entry.sum_ms.saturating_add(row.duration_ms);
        if row.duration_ms > entry.max_ms {
            entry.max_ms = row.duration_ms;
        }
    }
    let mut summaries: Vec<ToolSummary> = buckets
        .into_iter()
        .map(|(tool_name, acc)| {
            let mean_ms = acc.sum_ms.checked_div(acc.count).unwrap_or(0);
            ToolSummary {
                tool_name,
                count: acc.count,
                sum_ms: acc.sum_ms,
                mean_ms,
                max_ms: acc.max_ms,
            }
        })
        .collect();
    // Sort by sum_ms desc; tie-break by tool_name asc so the table is
    // deterministic for the snapshot test below.
    summaries.sort_by(|left, right| {
        right
            .sum_ms
            .cmp(&left.sum_ms)
            .then_with(|| left.tool_name.cmp(&right.tool_name))
    });
    if top > 0 && summaries.len() > top {
        summaries.truncate(top);
    }
    summaries
}

fn render_summary_table(standard_output: &mut dyn Write, days: u64, summaries: &[ToolSummary]) {
    let _ = writeln!(
        standard_output,
        "telemetry summary: window={days}d, tools={}",
        summaries.len()
    );
    if summaries.is_empty() {
        let _ = writeln!(standard_output, "  (no rows recorded in window)");
        return;
    }
    let _ = writeln!(
        standard_output,
        "  {:<24} {:>6} {:>9} {:>8} {:>8}",
        "tool_name", "count", "sum_ms", "mean_ms", "max_ms"
    );
    for summary in summaries {
        let _ = writeln!(
            standard_output,
            "  {:<24} {:>6} {:>9} {:>8} {:>8}",
            truncate_for_table(&summary.tool_name, 24),
            summary.count,
            summary.sum_ms,
            summary.mean_ms,
            summary.max_ms,
        );
    }
}

fn truncate_for_table(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    // Reserve one char for the ellipsis marker so the column still aligns.
    let take = max.saturating_sub(1);
    let mut clipped: String = value.chars().take(take).collect();
    clipped.push('…');
    clipped
}

fn render_summary_json(days: u64, summaries: &[ToolSummary]) -> Value {
    let rows = summaries
        .iter()
        .map(|summary| {
            Value::Object(vec![
                ("toolName".into(), Value::String(summary.tool_name.clone())),
                ("count".into(), Value::Number(summary.count.to_string())),
                ("sumMs".into(), Value::Number(summary.sum_ms.to_string())),
                ("meanMs".into(), Value::Number(summary.mean_ms.to_string())),
                ("maxMs".into(), Value::Number(summary.max_ms.to_string())),
            ])
        })
        .collect();
    Value::Object(vec![
        ("days".into(), Value::Number(days.to_string())),
        ("rows".into(), Value::Array(rows)),
    ])
}

fn render_telemetry_help(standard_output: &mut dyn Write) {
    let _ = writeln!(standard_output, "Usage: keel telemetry [summary] [flags]");
    let _ = writeln!(
        standard_output,
        "  telemetry summary [--days N] [--session ID] [--top N] [--json]"
    );
    let _ = writeln!(
        standard_output,
        "  Reads <claude_home>/state/tool-timings/<YYYY-MM-DD>.jsonl"
    );
    let _ = writeln!(
        standard_output,
        "  Defaults: --days 1, --top 10, --session (all sessions)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::tool_timings::EFFORT_ENV_VAR;
    use crate::test_support::ENV_LOCK;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Mirror of the helper in `tool_timings::tests` — duplicated rather
    /// than promoted to a shared module because the brief explicitly
    /// recommends keeping the helper local to each test module so a
    /// future caller cannot accidentally couple to it. Like its sibling,
    /// this also clears `CLAUDE_EFFORT` for the duration of the closure
    /// so the surrounding shell does not leak an effort level into the
    /// process and shift any code path that reads it.
    fn with_isolated_claude_home<F: FnOnce(&PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "keel-telemetry-{}-{nanos}-{suffix}",
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

    fn write_jsonl(path: &Path, lines: &[&str]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        let body = lines.join("\n") + "\n";
        fs::write(path, body).expect("write jsonl fixture");
    }

    fn row(tool: &str, duration: u64, session: &str) -> TimingRow {
        TimingRow {
            tool_name: tool.to_string(),
            duration_ms: duration,
            session_id: session.to_string(),
        }
    }

    #[test]
    fn aggregate_rows_returns_empty_for_empty_input() {
        let result = aggregate_rows(std::iter::empty(), 10);
        assert!(result.is_empty());
    }

    #[test]
    fn aggregate_rows_sums_per_tool_and_orders_by_sum_desc() {
        // Bash dominates by total time; Edit has higher count but smaller
        // sum. Confirms the ordering criterion is sum_ms, not count.
        let rows = vec![
            row("Bash", 1000, "s1"),
            row("Bash", 2000, "s1"),
            row("Edit", 50, "s1"),
            row("Edit", 50, "s1"),
            row("Edit", 50, "s1"),
            row("Read", 100, "s1"),
        ];
        let summaries = aggregate_rows(rows, 10);
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].tool_name, "Bash");
        assert_eq!(summaries[0].count, 2);
        assert_eq!(summaries[0].sum_ms, 3000);
        assert_eq!(summaries[0].mean_ms, 1500);
        assert_eq!(summaries[0].max_ms, 2000);
        // Edit: 3 calls × 50ms = 150ms total. Read: 1 × 100ms = 100ms
        // total. Edit ranks ahead of Read on sum_ms even though Read's
        // single sample is larger — this is the doc'd ordering criterion.
        assert_eq!(summaries[1].tool_name, "Edit");
        assert_eq!(summaries[1].count, 3);
        assert_eq!(summaries[1].sum_ms, 150);
        assert_eq!(summaries[1].mean_ms, 50);
        assert_eq!(summaries[1].max_ms, 50);
        assert_eq!(summaries[2].tool_name, "Read");
        assert_eq!(summaries[2].sum_ms, 100);
    }

    #[test]
    fn aggregate_rows_truncates_to_top_n() {
        let rows = vec![
            row("A", 500, "s"),
            row("B", 400, "s"),
            row("C", 300, "s"),
            row("D", 200, "s"),
            row("E", 100, "s"),
        ];
        let summaries = aggregate_rows(rows, 2);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].tool_name, "A");
        assert_eq!(summaries[1].tool_name, "B");
    }

    #[test]
    fn aggregate_rows_top_zero_keeps_everything() {
        // top == 0 is the documented "no truncation" knob. Verify it
        // does not silently collapse the list to empty.
        let rows = vec![row("A", 1, "s"), row("B", 1, "s"), row("C", 1, "s")];
        let summaries = aggregate_rows(rows, 0);
        assert_eq!(summaries.len(), 3);
    }

    #[test]
    fn aggregate_rows_top_larger_than_result_returns_full_list() {
        // Pins the "top > len" guard. A `>=` typo would still pass the
        // tie-break + truncate tests above but would slice at len-1
        // here. This locks the inequality.
        let rows = vec![row("A", 3, "s"), row("B", 2, "s"), row("C", 1, "s")];
        let summaries = aggregate_rows(rows, 99);
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].tool_name, "A");
        assert_eq!(summaries[2].tool_name, "C");
    }

    #[test]
    fn aggregate_rows_buckets_missing_tool_name_under_empty_string() {
        // Rows without a `tool_name` (or with an explicit empty string)
        // are kept and grouped under the "" bucket. The reader contract
        // is fail-open: we don't drop them, we just label them honestly
        // so a caller can spot the gap. Two empty-name rows must merge
        // into a single bucket with their durations summed.
        let rows = vec![row("", 40, "s"), row("Bash", 100, "s"), row("", 60, "s")];
        let summaries = aggregate_rows(rows, 10);
        assert_eq!(summaries.len(), 2);
        // Bash sum 100 < empty bucket sum 100? Bash 100, empty 100 — tie
        // breaks lexicographically: "" < "Bash", so empty wins position 0.
        assert_eq!(summaries[0].tool_name, "");
        assert_eq!(summaries[0].count, 2);
        assert_eq!(summaries[0].sum_ms, 100);
        assert_eq!(summaries[1].tool_name, "Bash");
        assert_eq!(summaries[1].count, 1);
    }

    #[test]
    fn aggregate_rows_breaks_sum_tie_by_name_ascending() {
        // Two tools with identical sum_ms must produce a deterministic
        // ordering. Lexicographic on tool_name is the doc'd tie-break.
        let rows = vec![row("Zeta", 100, "s"), row("Alpha", 100, "s")];
        let summaries = aggregate_rows(rows, 10);
        assert_eq!(summaries[0].tool_name, "Alpha");
        assert_eq!(summaries[1].tool_name, "Zeta");
    }

    #[test]
    fn read_rows_drops_malformed_lines_silently() {
        with_isolated_claude_home("read-malformed", |root| {
            let timings = root.join("state").join("tool-timings");
            let day = chrono::Local::now().date_naive();
            let path = timings.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
            write_jsonl(
                &path,
                &[
                    r#"{"tool_name":"Bash","duration_ms":100,"session_id":"s1"}"#,
                    "this is not json",
                    r#"{"tool_name":"Edit","duration_ms":50,"session_id":"s1"}"#,
                    "",
                    r#"{"tool_name":"Read","duration_ms":25,"session_id":"s1"}"#,
                ],
            );

            let rows = read_rows([path.as_path()], None);
            assert_eq!(
                rows.len(),
                3,
                "malformed line and blank line must be skipped"
            );
            let names: Vec<&str> = rows.iter().map(|r| r.tool_name.as_str()).collect();
            assert_eq!(names, vec!["Bash", "Edit", "Read"]);
        });
    }

    #[test]
    fn read_rows_filters_by_session_when_requested() {
        with_isolated_claude_home("read-session-filter", |root| {
            let timings = root.join("state").join("tool-timings");
            let day = chrono::Local::now().date_naive();
            let path = timings.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
            write_jsonl(
                &path,
                &[
                    r#"{"tool_name":"Bash","duration_ms":10,"session_id":"alpha"}"#,
                    r#"{"tool_name":"Bash","duration_ms":20,"session_id":"beta"}"#,
                    r#"{"tool_name":"Edit","duration_ms":30,"session_id":"alpha"}"#,
                ],
            );

            let rows = read_rows([path.as_path()], Some("alpha"));
            assert_eq!(rows.len(), 2);
            for row in &rows {
                assert_eq!(row.session_id, "alpha");
            }
        });
    }

    #[test]
    fn run_telemetry_summary_renders_table_for_today_window() {
        with_isolated_claude_home("summary-table", |root| {
            let timings = root.join("state").join("tool-timings");
            let day = chrono::Local::now().date_naive();
            let path = timings.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
            write_jsonl(
                &path,
                &[
                    r#"{"tool_name":"Bash","duration_ms":1000,"session_id":"s1"}"#,
                    r#"{"tool_name":"Bash","duration_ms":2000,"session_id":"s1"}"#,
                    r#"{"tool_name":"Edit","duration_ms":250,"session_id":"s1"}"#,
                ],
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit = run_telemetry_command(
                &["summary".to_string(), "--days".to_string(), "1".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit, 0);
            let body = String::from_utf8_lossy(&stdout);
            assert!(body.contains("window=1d, tools=2"), "header: {body}");
            assert!(body.contains("Bash"), "Bash row missing: {body}");
            assert!(body.contains("3000"), "sum_ms missing: {body}");
            assert!(body.contains("Edit"), "Edit row missing: {body}");
            assert!(
                stderr.is_empty(),
                "no stderr expected: {}",
                String::from_utf8_lossy(&stderr)
            );
        });
    }

    #[test]
    fn run_telemetry_summary_renders_json_with_camel_case_keys() {
        with_isolated_claude_home("summary-json", |root| {
            let timings = root.join("state").join("tool-timings");
            let day = chrono::Local::now().date_naive();
            let path = timings.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
            write_jsonl(
                &path,
                &[
                    r#"{"tool_name":"Bash","duration_ms":100,"session_id":"s1"}"#,
                    r#"{"tool_name":"Bash","duration_ms":200,"session_id":"s1"}"#,
                ],
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit = run_telemetry_command(
                &[
                    "summary".to_string(),
                    "--json".to_string(),
                    "--days".to_string(),
                    "1".to_string(),
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit, 0);
            let body = String::from_utf8_lossy(&stdout).to_string();
            // Parse to confirm valid JSON and the key shape.
            let parsed: JsonDocument = serde_json::from_str(&body).expect("valid json");
            assert_eq!(parsed.get("days").and_then(JsonDocument::as_u64), Some(1));
            let rows = parsed
                .get("rows")
                .and_then(JsonDocument::as_array)
                .expect("rows");
            assert_eq!(rows.len(), 1);
            let first = &rows[0];
            assert_eq!(
                first.get("toolName").and_then(JsonDocument::as_str),
                Some("Bash")
            );
            assert_eq!(first.get("count").and_then(JsonDocument::as_u64), Some(2));
            assert_eq!(first.get("sumMs").and_then(JsonDocument::as_u64), Some(300));
            assert_eq!(
                first.get("meanMs").and_then(JsonDocument::as_u64),
                Some(150)
            );
            assert_eq!(first.get("maxMs").and_then(JsonDocument::as_u64), Some(200));
        });
    }

    #[test]
    fn run_telemetry_summary_session_filter_restricts_rows() {
        with_isolated_claude_home("summary-session", |root| {
            let timings = root.join("state").join("tool-timings");
            let day = chrono::Local::now().date_naive();
            let path = timings.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
            write_jsonl(
                &path,
                &[
                    r#"{"tool_name":"Bash","duration_ms":100,"session_id":"keep"}"#,
                    r#"{"tool_name":"Bash","duration_ms":9999,"session_id":"drop"}"#,
                    r#"{"tool_name":"Edit","duration_ms":50,"session_id":"keep"}"#,
                ],
            );

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit = run_telemetry_command(
                &[
                    "summary".to_string(),
                    "--json".to_string(),
                    "--session".to_string(),
                    "keep".to_string(),
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit, 0);
            let parsed: JsonDocument =
                serde_json::from_str(&String::from_utf8_lossy(&stdout)).expect("valid json");
            let rows = parsed
                .get("rows")
                .and_then(JsonDocument::as_array)
                .expect("rows");
            // 2 distinct tools (Bash, Edit) inside session "keep".
            assert_eq!(rows.len(), 2);
            // The big 9999ms Bash entry was filtered out — total Bash sum
            // should reflect only the kept session's row.
            for r in rows {
                if r.get("toolName").and_then(JsonDocument::as_str) == Some("Bash") {
                    assert_eq!(r.get("sumMs").and_then(JsonDocument::as_u64), Some(100));
                }
            }
        });
    }

    #[test]
    fn run_telemetry_summary_emits_empty_summary_when_no_rows() {
        with_isolated_claude_home("summary-empty", |_root| {
            // Cold path — the directory does not exist. Subcommand must
            // exit 0 with the "no rows recorded" placeholder, not a crash.
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit = run_telemetry_command(
                &["summary".to_string(), "--days".to_string(), "7".to_string()],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit, 0);
            let body = String::from_utf8_lossy(&stdout);
            assert!(body.contains("window=7d, tools=0"));
            assert!(body.contains("no rows recorded in window"));
            assert!(stderr.is_empty());
        });
    }

    #[test]
    fn run_telemetry_command_renders_help_when_no_subcommand() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_telemetry_command(&[], &mut stdout, &mut stderr);
        assert_eq!(exit, 0);
        assert!(String::from_utf8_lossy(&stdout).contains("Usage: keel telemetry"));
        assert!(stderr.is_empty());
    }

    #[test]
    fn run_telemetry_command_rejects_unknown_subcommand() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_telemetry_command(&["mystery".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("Unknown telemetry subcommand"));
        // Help still emitted on stdout so the user sees the valid options.
        assert!(String::from_utf8_lossy(&stdout).contains("Usage: keel telemetry"));
    }

    #[test]
    fn run_telemetry_summary_invalid_days_falls_back_to_default() {
        // A typo on --days must not block the report. The default (1) is
        // applied silently — this matches the "telemetry is read-only,
        // never fail loudly" contract.
        with_isolated_claude_home("summary-bad-days", |_root| {
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit = run_telemetry_command(
                &[
                    "summary".to_string(),
                    "--days".to_string(),
                    "not-a-number".to_string(),
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit, 0);
            assert!(String::from_utf8_lossy(&stdout).contains("window=1d"));
        });
    }
}
