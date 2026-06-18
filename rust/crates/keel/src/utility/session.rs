//! Purpose: Session grouping and analytics over compaction event history
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::fs, std::io, crate::args, crate::json, crate::runtime
//! Main Functions: run_session_command
//! Side Effects: Reads compaction event log, writes session table to stdout

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{resolve_claude_home, COMMAND_COMPACTION_EVENTS_FILE_NAME};

pub fn run_session_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("session");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("since", "today");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let since_value = flag_set.string_value("since");
    let all_sessions = since_value == "all";
    let since_timestamp = compute_since_timestamp(since_value);

    let Some(path) = gain_events_path() else {
        let _ = writeln!(standard_error, "No compaction events found");
        return 1;
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            let _ = writeln!(standard_error, "No compaction events found");
            return 1;
        }
    };

    let mut events: Vec<SessionEvent> = Vec::new();
    for line in text.lines() {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let timestamp = event
            .get("timestamp")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if !all_sessions && timestamp < since_timestamp {
            continue;
        }
        let tokens_before = event
            .get("tokens_before")
            .or_else(|| event.get("tokensBefore"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let tokens_after = event
            .get("tokens_after")
            .or_else(|| event.get("tokensAfter"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(tokens_before);
        let tokens_saved = event
            .get("tokens_saved")
            .or_else(|| event.get("tokensSaved"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_else(|| tokens_before.saturating_sub(tokens_after));
        let compacted = event
            .get("compacted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let exit_code = event
            .get("exit_code")
            .or_else(|| event.get("exitCode"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0) as i32;
        events.push(SessionEvent {
            timestamp,
            tokens_before,
            tokens_after,
            tokens_saved,
            compacted,
            exit_code,
        });
    }

    events.sort_by_key(|event| event.timestamp);

    let session_gap_secs: u64 = 30 * 60;
    let mut sessions: Vec<SessionSummary> = Vec::new();
    let mut current: Option<SessionSummary> = None;

    for event in &events {
        if let Some(ref mut session) = current {
            if event.timestamp >= session.last_timestamp
                && event.timestamp - session.last_timestamp > session_gap_secs
            {
                let finished = std::mem::replace(
                    session,
                    SessionSummary {
                        start_timestamp: event.timestamp,
                        last_timestamp: event.timestamp,
                        commands: 1,
                        compacted: if event.compacted { 1 } else { 0 },
                        tokens_before: event.tokens_before,
                        tokens_after: event.tokens_after,
                        tokens_saved: event.tokens_saved,
                        failed: if event.exit_code != 0 { 1 } else { 0 },
                    },
                );
                sessions.push(finished);
                continue;
            }
            session.last_timestamp = event.timestamp;
            session.commands += 1;
            if event.compacted {
                session.compacted += 1;
            }
            session.tokens_before += event.tokens_before;
            session.tokens_after += event.tokens_after;
            session.tokens_saved += event.tokens_saved;
            if event.exit_code != 0 {
                session.failed += 1;
            }
        } else {
            current = Some(SessionSummary {
                start_timestamp: event.timestamp,
                last_timestamp: event.timestamp,
                commands: 1,
                compacted: if event.compacted { 1 } else { 0 },
                tokens_before: event.tokens_before,
                tokens_after: event.tokens_after,
                tokens_saved: event.tokens_saved,
                failed: if event.exit_code != 0 { 1 } else { 0 },
            });
        }
    }
    if let Some(session) = current.take() {
        sessions.push(session);
    }

    let total_commands: u64 = sessions.iter().map(|session| session.commands).sum();
    let total_compacted: u64 = sessions.iter().map(|session| session.compacted).sum();
    let total_before: u64 = sessions.iter().map(|session| session.tokens_before).sum();
    let total_after: u64 = sessions.iter().map(|session| session.tokens_after).sum();
    let total_saved: u64 = sessions.iter().map(|session| session.tokens_saved).sum();
    let total_failed: u64 = sessions.iter().map(|session| session.failed).sum();

    if flag_set.bool_value("json") {
        let session_list: Vec<Value> = sessions
            .iter()
            .map(|session| {
                Value::Object(vec![
                    (
                        "sessionStart".into(),
                        Value::Number(session.start_timestamp.to_string()),
                    ),
                    (
                        "sessionEnd".into(),
                        Value::Number(session.last_timestamp.to_string()),
                    ),
                    (
                        "commands".into(),
                        Value::Number(session.commands.to_string()),
                    ),
                    (
                        "compacted".into(),
                        Value::Number(session.compacted.to_string()),
                    ),
                    ("failed".into(), Value::Number(session.failed.to_string())),
                    (
                        "tokensBefore".into(),
                        Value::Number(session.tokens_before.to_string()),
                    ),
                    (
                        "tokensAfter".into(),
                        Value::Number(session.tokens_after.to_string()),
                    ),
                    (
                        "tokensSaved".into(),
                        Value::Number(session.tokens_saved.to_string()),
                    ),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            ("sessions".into(), Value::Array(session_list)),
            (
                "totalSessions".into(),
                Value::Number(sessions.len().to_string()),
            ),
            (
                "totalCommands".into(),
                Value::Number(total_commands.to_string()),
            ),
            (
                "totalCompacted".into(),
                Value::Number(total_compacted.to_string()),
            ),
            (
                "totalFailed".into(),
                Value::Number(total_failed.to_string()),
            ),
            (
                "totalTokensBefore".into(),
                Value::Number(total_before.to_string()),
            ),
            (
                "totalTokensAfter".into(),
                Value::Number(total_after.to_string()),
            ),
            (
                "totalTokensSaved".into(),
                Value::Number(total_saved.to_string()),
            ),
            (
                "savingsPercent".into(),
                Value::Number(format!(
                    "{:.1}",
                    if total_before == 0 {
                        0.0
                    } else {
                        total_saved as f64 / total_before as f64 * 100.0
                    }
                )),
            ),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }

    let _ = writeln!(
        standard_output,
        " {:<11} {:>8} {:>8} {:>8} {:>8} {:>7} {:>9}",
        "Session", "Cmds", "Saved", "Before", "After", "Savings", "Compacted"
    );
    let _ = writeln!(
        standard_output,
        " {}",
        format_args!(
            "{:-<11} {:-<8} {:-<8} {:-<8} {:-<8} {:-<7} {:-<9}",
            "", "", "", "", "", "", ""
        )
    );
    for session in &sessions {
        let start_time = format_timestamp_time(session.start_timestamp);
        let end_time = format_timestamp_time(session.last_timestamp);
        let savings = if session.tokens_before == 0 {
            0.0
        } else {
            session.tokens_saved as f64 / session.tokens_before as f64 * 100.0
        };
        let _ = writeln!(
            standard_output,
            " {:<11} {:>8} {:>8} {:>8} {:>8} {:>6.1}% {:>9}",
            format!("{}-{}", start_time, end_time),
            session.commands,
            format_count(session.tokens_saved),
            format_count(session.tokens_before),
            format_count(session.tokens_after),
            savings,
            session.compacted,
        );
    }
    let _ = writeln!(
        standard_output,
        " {}",
        format_args!(
            "{:-<11} {:-<8} {:-<8} {:-<8} {:-<8} {:-<7} {:-<9}",
            "", "", "", "", "", "", ""
        )
    );
    let total_savings = if total_before == 0 {
        0.0
    } else {
        total_saved as f64 / total_before as f64 * 100.0
    };
    let _ = writeln!(
        standard_output,
        " {:<11} {:>8} {:>8} {:>8} {:>8} {:>6.1}% {:>9}",
        if all_sessions { "Overall" } else { "Period" },
        total_commands,
        format_count(total_saved),
        format_count(total_before),
        format_count(total_after),
        total_savings,
        total_compacted,
    );
    0
}

fn gain_events_path() -> Option<PathBuf> {
    resolve_claude_home("")
        .ok()
        .map(|home| home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME))
}

fn compute_since_timestamp(since_value: &str) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    match since_value {
        "today" => now.saturating_sub(24 * 3600),
        "week" => now.saturating_sub(7 * 24 * 3600),
        "month" => now.saturating_sub(30 * 24 * 3600),
        _ => 0,
    }
}

fn format_timestamp_time(timestamp: u64) -> String {
    let seconds = timestamp as i64;
    let hours = (seconds / 3600) % 24;
    let minutes = (seconds / 60) % 60;
    format!("{:02}:{:02}", hours, minutes)
}

fn format_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

struct SessionEvent {
    timestamp: u64,
    tokens_before: u64,
    tokens_after: u64,
    tokens_saved: u64,
    compacted: bool,
    exit_code: i32,
}

struct SessionSummary {
    start_timestamp: u64,
    last_timestamp: u64,
    commands: u64,
    compacted: u64,
    tokens_before: u64,
    tokens_after: u64,
    tokens_saved: u64,
    failed: u64,
}
