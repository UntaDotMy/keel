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
    if arguments.first().map(String::as_str) == Some("discover") {
        return run_gain_discover(&arguments[1..], standard_output, standard_error);
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
            "Unable to resolve harness home for gain reset"
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

/// `discover` surfaces missed-savings opportunities: commands that ran through
/// the proxy but were NOT compacted (passthrough), grouped by command with the
/// estimated tokens that entered context uncompacted. This is the RTK-style
/// "you left savings on the table" probe — `gain` reports what we saved,
/// `discover` reports what we did not.
fn run_gain_discover(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("gain discover");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("since", "today");
    flag_set.string_flag("top", "10");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let since_timestamp = gain_since_timestamp_v2(&flag_set);
    let top_count: usize = flag_set.string_value("top").parse().unwrap_or(10).min(100);
    let missed = load_missed_opportunities(Some(since_timestamp));

    if flag_set.bool_value("json") {
        let opportunities: Vec<Value> = missed
            .commands
            .iter()
            .take(top_count)
            .map(|item| {
                Value::Object(vec![
                    ("command".into(), Value::String(item.command.clone())),
                    (
                        "uncompactedTokens".into(),
                        Value::Number(item.uncompacted_tokens.to_string()),
                    ),
                    ("runs".into(), Value::Number(item.count.to_string())),
                ])
            })
            .collect();
        let payload = Value::Object(vec![
            (
                "passthroughCommands".into(),
                Value::Number(missed.passthrough_commands.to_string()),
            ),
            (
                "uncompactedTokens".into(),
                Value::Number(missed.uncompacted_tokens.to_string()),
            ),
            ("opportunities".into(), Value::Array(opportunities)),
        ]);
        return render_gain_json(standard_output, standard_error, &payload);
    }

    let _ = writeln!(standard_output, "Missed Savings Opportunities");
    let _ = writeln!(
        standard_output,
        "Passthrough commands (ran without compaction): {}",
        missed.passthrough_commands
    );
    let _ = writeln!(
        standard_output,
        "Estimated uncompacted tokens that entered context: {}",
        missed.uncompacted_tokens
    );
    if missed.commands.is_empty() {
        let _ = writeln!(
            standard_output,
            "\nNo passthrough commands recorded — every observed command was compacted, or none ran in this window."
        );
        return 0;
    }
    let _ = writeln!(standard_output, "\nTop uncompacted commands:");
    for (index, item) in missed.commands.iter().take(top_count).enumerate() {
        let _ = writeln!(
            standard_output,
            "  {}. {} - ~{} uncompacted tokens ({} runs)",
            index + 1,
            item.command,
            item.uncompacted_tokens,
            item.count
        );
    }
    let _ = writeln!(
        standard_output,
        "\nRoute these through `keel run -- <command>` to capture the savings."
    );
    0
}

fn render_gain_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(error) = write_indented(standard_output, value) {
        let _ = writeln!(standard_error, "Unable to render gain JSON output: {error}");
        return 1;
    }
    0
}

/// Read passthrough (non-compacted) events from the same event log `gain` uses
/// and group them by command. `uncompacted_tokens` is the `tokens_before` of
/// each passthrough run — the volume that entered context without compaction.
fn load_missed_opportunities(since_timestamp: Option<u64>) -> MissedOpportunities {
    let Some(path) = gain_events_path() else {
        return MissedOpportunities::default();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return MissedOpportunities::default(),
    };
    parse_missed_opportunities(&text, since_timestamp)
}

/// Read `timestamp` whether serialized as a JSON number or string.
/// event_log.rs writes it as a string; older/test events use a number.
fn event_timestamp(event: &serde_json::Value) -> u64 {
    event
        .get("timestamp")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(0)
}

/// Pure parser over the JSONL event text, split out from `load_missed_opportunities`
/// so it is testable without touching the filesystem or harness home.
fn parse_missed_opportunities(text: &str, since_timestamp: Option<u64>) -> MissedOpportunities {
    let mut passthrough_commands: u64 = 0;
    let mut uncompacted_tokens: u64 = 0;
    let mut command_map: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    for line in text.lines() {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let timestamp = event_timestamp(&event);
        if let Some(cutoff) = since_timestamp {
            if timestamp < cutoff {
                continue;
            }
        }
        let compacted = event
            .get("compacted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if compacted {
            continue;
        }
        let before = event
            .get("tokens_before")
            .or_else(|| event.get("tokensBefore"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        passthrough_commands += 1;
        uncompacted_tokens += before;
        if let Some(command) = event.get("command").and_then(serde_json::Value::as_str) {
            let entry = command_map.entry(command.to_string()).or_insert((0, 0));
            entry.0 += before;
            entry.1 += 1;
        }
    }
    let mut commands: Vec<MissedCommand> = command_map
        .into_iter()
        .map(|(command, (uncompacted_tokens, count))| MissedCommand {
            command,
            uncompacted_tokens,
            count,
        })
        .collect();
    commands.sort_by_key(|item| std::cmp::Reverse(item.uncompacted_tokens));
    MissedOpportunities {
        passthrough_commands,
        uncompacted_tokens,
        commands,
    }
}

#[derive(Default)]
struct MissedOpportunities {
    passthrough_commands: u64,
    uncompacted_tokens: u64,
    commands: Vec<MissedCommand>,
}

struct MissedCommand {
    command: String,
    uncompacted_tokens: u64,
    count: u64,
}

fn load_gain_summary(since_timestamp: Option<u64>, adapter_filter: Option<&str>) -> GainSummary {
    let Some(path) = gain_events_path() else {
        return GainSummary::default();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return GainSummary::default(),
    };
    parse_gain_summary(&text, since_timestamp, adapter_filter)
}

pub(crate) fn parse_gain_summary(
    text: &str,
    since_timestamp: Option<u64>,
    adapter_filter: Option<&str>,
) -> GainSummary {
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
        let timestamp = event_timestamp(&event);
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
        let compacted = event
            .get("compacted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let before = event
            .get("tokens_before")
            .or_else(|| event.get("tokensBefore"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let after = if compacted {
            event
                .get("tokens_after")
                .or_else(|| event.get("tokensAfter"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        } else {
            event
                .get("tokens_after")
                .or_else(|| event.get("tokensAfter"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(before)
        };
        let saved = if compacted {
            before.saturating_sub(after)
        } else {
            0
        };
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
pub(crate) struct GainSummary {
    pub commands_observed: u64,
    pub commands_compacted: u64,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub tokens_saved: u64,
    pub top_commands: Vec<GainCommandSummary>,
    top_reducers: Vec<GainDimensionSummary>,
    top_families: Vec<GainDimensionSummary>,
}

impl GainSummary {
    pub fn savings_percent(&self) -> f64 {
        if self.tokens_before == 0 {
            0.0
        } else {
            (self.tokens_saved as f64 / self.tokens_before as f64) * 100.0
        }
    }
}

#[derive(Clone)]
pub(crate) struct GainCommandSummary {
    pub command: String,
    pub tokens_saved: u64,
    count: u64,
}

#[derive(Clone)]
struct GainDimensionSummary {
    name: String,
    tokens_saved: u64,
    count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: &str = concat!(
        r#"{"timestamp":1000,"command":"aws s3 ls","compacted":false,"tokens_before":500}"#,
        "\n",
        r#"{"timestamp":1100,"command":"aws s3 ls","compacted":false,"tokens_before":700}"#,
        "\n",
        r#"{"timestamp":1200,"command":"cargo test","compacted":true,"tokens_before":900,"tokens_after":100}"#,
        "\n",
        r#"{"timestamp":1300,"command":"terraform plan","compacted":false,"tokens_before":300}"#,
        "\n",
        "not json - must be skipped\n",
    );

    #[test]
    fn discover_groups_passthrough_commands_and_excludes_compacted() {
        let missed = parse_missed_opportunities(EVENTS, None);
        // 3 passthrough events (two aws, one terraform); the compacted cargo test excluded.
        assert_eq!(missed.passthrough_commands, 3);
        assert_eq!(missed.uncompacted_tokens, 500 + 700 + 300);
        // Highest uncompacted command first: aws s3 ls (1200 across 2 runs).
        assert_eq!(missed.commands[0].command, "aws s3 ls");
        assert_eq!(missed.commands[0].uncompacted_tokens, 1200);
        assert_eq!(missed.commands[0].count, 2);
        // The compacted command must not appear as an opportunity.
        assert!(!missed.commands.iter().any(|c| c.command == "cargo test"));
    }

    #[test]
    fn discover_since_cutoff_filters_old_events() {
        // Cutoff at 1150 keeps only the 1200 terraform passthrough.
        let missed = parse_missed_opportunities(EVENTS, Some(1150));
        assert_eq!(missed.passthrough_commands, 1);
        assert_eq!(missed.commands[0].command, "terraform plan");
    }

    #[test]
    fn discover_empty_log_yields_no_opportunities() {
        let missed = parse_missed_opportunities("", None);
        assert_eq!(missed.passthrough_commands, 0);
        assert!(missed.commands.is_empty());
    }

    // event_log.rs serializes `timestamp` as a STRING (`"timestamp":"1700"`),
    // not a number. The since-cutoff parser must coerce it, or every real
    // event is read as timestamp 0 and filtered out by any cutoff > 0.
    const STRING_TS_EVENTS: &str = concat!(
        r#"{"timestamp":"1000","command":"aws s3 ls","compacted":false,"tokens_before":500}"#,
        "\n",
        r#"{"timestamp":"1300","command":"terraform plan","compacted":false,"tokens_before":300}"#,
        "\n",
    );

    #[test]
    fn discover_accepts_string_typed_timestamps_under_cutoff() {
        let missed = parse_missed_opportunities(STRING_TS_EVENTS, Some(1150));
        assert_eq!(missed.passthrough_commands, 1);
        assert_eq!(missed.commands[0].command, "terraform plan");
    }

    #[test]
    fn summary_counts_string_typed_timestamps_under_cutoff() {
        let summary = parse_gain_summary(STRING_TS_EVENTS, Some(1150), None);
        assert_eq!(summary.commands_observed, 1);
        assert_eq!(summary.tokens_before, 300);
    }

    #[test]
    fn gain_summary_savings_percent_zero_when_no_tokens() {
        let summary = GainSummary::default();
        assert_eq!(summary.savings_percent(), 0.0);
    }

    #[test]
    fn gain_summary_savings_percent_calculates_correctly() {
        let summary = GainSummary {
            tokens_before: 1000,
            tokens_saved: 800,
            ..GainSummary::default()
        };
        assert_eq!(summary.savings_percent(), 80.0);
    }

    #[test]
    fn gain_summary_from_real_events() {
        let summary = parse_gain_summary(EVENTS, None, None);
        assert_eq!(summary.commands_observed, 4);
        assert_eq!(summary.commands_compacted, 1);
        assert_eq!(summary.tokens_before, 500 + 700 + 900 + 300);
        assert_eq!(summary.tokens_after, 500 + 700 + 100 + 300);
        assert_eq!(summary.tokens_saved, 900 - 100);
    }
}
