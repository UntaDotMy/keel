//! Purpose: Token savings analytics from native command compaction events
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::fs, std::io, std::path, crate::args, crate::json, crate::runtime
//! Main Functions: run_gain_command, load_gain_summary, run_gain_reset
//! Side Effects: Reads compaction event log, writes analytics to stdout

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home, COMMAND_COMPACTION_EVENTS_FILE_NAME};

pub fn run_gain_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.first().map(String::as_str) == Some("reset") {
        return run_gain_reset(standard_output, standard_error);
    }
    let mut flag_set = FlagSet::new("gain");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("since", "today");
    flag_set.string_flag("adapter", "");
    flag_set.string_flag("top", "10");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let since_timestamp = gain_since_timestamp_v2(&flag_set);
    let adapter_filter = flag_set.string_value("adapter").trim();
    let summary = load_gain_summary(
        Some(since_timestamp),
        if adapter_filter.is_empty() {
            None
        } else {
            Some(adapter_filter)
        },
    );
    let top_count: usize = flag_set.string_value("top").parse().unwrap_or(10).min(100);
    if flag_set.bool_value("json") {
        let top_commands: Vec<Value> = summary
            .top_commands
            .iter()
            .take(top_count)
            .map(|item| {
                Value::Object(vec![
                    ("command".into(), Value::String(item.command.clone())),
                    (
                        "tokensSaved".into(),
                        Value::Number(item.tokens_saved.to_string()),
                    ),
                    ("count".into(), Value::Number(item.count.to_string())),
                ])
            })
            .collect();
        let top_reducers: Vec<Value> = summary
            .top_reducers
            .iter()
            .take(top_count)
            .map(|item| {
                Value::Object(vec![
                    ("reducer".into(), Value::String(item.name.clone())),
                    (
                        "tokensSaved".into(),
                        Value::Number(item.tokens_saved.to_string()),
                    ),
                    ("count".into(), Value::Number(item.count.to_string())),
                ])
            })
            .collect();
        let top_families: Vec<Value> = summary
            .top_families
            .iter()
            .take(top_count)
            .map(|item| {
                Value::Object(vec![
                    ("family".into(), Value::String(item.name.clone())),
                    (
                        "tokensSaved".into(),
                        Value::Number(item.tokens_saved.to_string()),
                    ),
                    ("count".into(), Value::Number(item.count.to_string())),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            (
                "commandsObserved".into(),
                Value::Number(summary.commands_observed.to_string()),
            ),
            (
                "commandsCompacted".into(),
                Value::Number(summary.commands_compacted.to_string()),
            ),
            (
                "tokensBefore".into(),
                Value::Number(summary.tokens_before.to_string()),
            ),
            (
                "tokensAfter".into(),
                Value::Number(summary.tokens_after.to_string()),
            ),
            (
                "tokensSaved".into(),
                Value::Number(summary.tokens_saved.to_string()),
            ),
            (
                "savingsPercent".into(),
                Value::Number(format!("{:.2}", summary.savings_percent())),
            ),
            ("topCommands".into(), Value::Array(top_commands)),
            ("topReducers".into(), Value::Array(top_reducers)),
            ("topFamilies".into(), Value::Array(top_families)),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }
    let _ = writeln!(standard_output, "Token Savings Analytics");
    let _ = writeln!(
        standard_output,
        "Commands observed: {}",
        summary.commands_observed
    );
    let _ = writeln!(
        standard_output,
        "Commands compacted: {}",
        summary.commands_compacted
    );
    let _ = writeln!(standard_output, "Tokens before: {}", summary.tokens_before);
    let _ = writeln!(standard_output, "Tokens after: {}", summary.tokens_after);
    let _ = writeln!(standard_output, "Tokens saved: {}", summary.tokens_saved);
    let _ = writeln!(
        standard_output,
        "Savings: {:.2}%",
        summary.savings_percent()
    );
    if !summary.top_commands.is_empty() {
        let _ = writeln!(standard_output, "\nTop Commands by Savings:");
        for (index, item) in summary.top_commands.iter().take(top_count).enumerate() {
            let _ = writeln!(
                standard_output,
                "  {}. {} - {} tokens saved ({} runs)",
                index + 1,
                item.command,
                item.tokens_saved,
                item.count
            );
        }
    }
    if !summary.top_reducers.is_empty() {
        let _ = writeln!(standard_output, "\nTop Reducers by Savings:");
        for (index, item) in summary.top_reducers.iter().take(top_count).enumerate() {
            let _ = writeln!(
                standard_output,
                "  {}. {} - {} tokens saved ({} runs)",
                index + 1,
                item.name,
                item.tokens_saved,
                item.count
            );
        }
    }
    if !summary.top_families.is_empty() {
        let _ = writeln!(standard_output, "\nTop Families by Savings:");
        for (index, item) in summary.top_families.iter().take(top_count).enumerate() {
            let _ = writeln!(
                standard_output,
                "  {}. {} - {} tokens saved ({} runs)",
                index + 1,
                item.name,
                item.tokens_saved,
                item.count
            );
        }
    }
    0
}

fn run_gain_reset(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let Some(path) = gain_events_path() else {
        let _ = writeln!(
            standard_error,
            "Unable to resolve Claude home for gain reset"
        );
        return 1;
    };
    match fs::remove_file(&path) {
        Ok(()) => {
            let _ = writeln!(
                standard_output,
                "gain reset: removed {}",
                display_path(&path)
            );
            0
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(
                standard_output,
                "gain reset: no native compaction events at {}",
                display_path(&path)
            );
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "remove {}: {error}", display_path(&path));
            1
        }
    }
}

fn load_gain_summary(since_timestamp: Option<u64>, adapter_filter: Option<&str>) -> GainSummary {
    let Some(path) = gain_events_path() else {
        return GainSummary::default();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return GainSummary::default(),
    };
    let mut commands_observed: u64 = 0;
    let mut commands_compacted: u64 = 0;
    let mut tokens_before: u64 = 0;
    let mut tokens_after: u64 = 0;
    let mut tokens_saved: u64 = 0;
    let mut command_map: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    let mut reducer_map: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    let mut family_map: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    for line in text.lines() {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let timestamp = event
            .get("timestamp")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if let Some(cutoff) = since_timestamp {
            if timestamp < cutoff {
                continue;
            }
        }
        if let Some(filter) = adapter_filter {
            let adapter = event
                .get("adapter")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if adapter != filter {
                continue;
            }
        }
        let before = event
            .get("tokens_before")
            .or_else(|| event.get("tokensBefore"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let after = event
            .get("tokens_after")
            .or_else(|| event.get("tokensAfter"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let saved = before.saturating_sub(after);
        let compacted = event
            .get("compacted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        commands_observed += 1;
        if compacted {
            commands_compacted += 1;
        }
        tokens_before += before;
        tokens_after += after;
        tokens_saved += saved;
        if let Some(command) = event.get("command").and_then(serde_json::Value::as_str) {
            let entry = command_map.entry(command.to_string()).or_insert((0, 0));
            entry.0 += saved;
            entry.1 += 1;
        }
        if let Some(reducer) = event.get("reducer").and_then(serde_json::Value::as_str) {
            let entry = reducer_map.entry(reducer.to_string()).or_insert((0, 0));
            entry.0 += saved;
            entry.1 += 1;
        }
        if let Some(family) = event.get("family").and_then(serde_json::Value::as_str) {
            let entry = family_map.entry(family.to_string()).or_insert((0, 0));
            entry.0 += saved;
            entry.1 += 1;
        }
    }
    let mut top_commands: Vec<GainCommandSummary> = command_map
        .into_iter()
        .map(|(command, (tokens_saved, count))| GainCommandSummary {
            command,
            tokens_saved,
            count,
        })
        .collect();
    top_commands.sort_by_key(|item| std::cmp::Reverse(item.tokens_saved));
    let mut top_reducers: Vec<GainDimensionSummary> = reducer_map
        .into_iter()
        .map(|(name, (tokens_saved, count))| GainDimensionSummary {
            name,
            tokens_saved,
            count,
        })
        .collect();
    top_reducers.sort_by_key(|item| std::cmp::Reverse(item.tokens_saved));
    let mut top_families: Vec<GainDimensionSummary> = family_map
        .into_iter()
        .map(|(name, (tokens_saved, count))| GainDimensionSummary {
            name,
            tokens_saved,
            count,
        })
        .collect();
    top_families.sort_by_key(|item| std::cmp::Reverse(item.tokens_saved));
    GainSummary {
        commands_observed,
        commands_compacted,
        tokens_before,
        tokens_after,
        tokens_saved,
        top_commands,
        top_reducers,
        top_families,
    }
}

fn gain_events_path() -> Option<PathBuf> {
    resolve_claude_home("")
        .ok()
        .map(|home| home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME))
}

fn gain_since_timestamp_v2(flag_set: &FlagSet) -> u64 {
    let since_value = flag_set.string_value("since");
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

#[derive(Default)]
struct GainSummary {
    commands_observed: u64,
    commands_compacted: u64,
    tokens_before: u64,
    tokens_after: u64,
    tokens_saved: u64,
    top_commands: Vec<GainCommandSummary>,
    top_reducers: Vec<GainDimensionSummary>,
    top_families: Vec<GainDimensionSummary>,
}

impl GainSummary {
    fn savings_percent(&self) -> f64 {
        if self.tokens_before == 0 {
            0.0
        } else {
            (self.tokens_saved as f64 / self.tokens_before as f64) * 100.0
        }
    }
}

#[derive(Clone)]
struct GainCommandSummary {
    command: String,
    tokens_saved: u64,
    count: u64,
}

#[derive(Clone)]
struct GainDimensionSummary {
    name: String,
    tokens_saved: u64,
    count: u64,
}
