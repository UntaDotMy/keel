//! Purpose: `keel stats`, a single dashboard over the axes that today live in
//!   separate commands: token savings (`gain`), tool timings (`telemetry`),
//!   gate/enforcement activity, recall/memory health, and sprint/work progress.
//!   It answers "what has keel done, what did it save, what did it catch" in one
//!   compact read instead of four invocations.
//! Caller: commands.rs `stats` dispatch, MCP `stats` tool.
//! Dependencies: the gain, telemetry, recall, sprint, and hook-lifecycle readers.
//!   Every datum is pulled from a function that already backs another command, so
//!   `stats` cannot drift from the surfaces it aggregates; it reuses readers, it
//!   never re-parses.
//! Side Effects: read-only. `recall_status_snapshot` lazily syncs the recall
//!   index; everything else reads files. No writes, no network.

use std::fs;
use std::io::Write;
use std::path::Path;

use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runner::hook_lifecycle::gate_status_rows;
use crate::runner::telemetry::{aggregate_rows, read_rows};
use crate::runtime::{display_path, resolve_claude_home, COMMAND_COMPACTION_EVENTS_FILE_NAME};
use crate::utility::gain::parse_gain_summary;
use crate::utility::recall::recall_status_snapshot;

/// Default `--days` window for the savings/timing axes. Matches the telemetry
/// default so the two surfaces agree on a window when neither is given one.
const DEFAULT_DAYS: u64 = 7;

pub fn run_stats_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("stats");
    flag_set.bool_flag("json", false);
    flag_set.string_flag("days", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("top", "5");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let days = flag_set
        .string_value("days")
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_DAYS);
    let top_count: usize = flag_set
        .string_value("top")
        .trim()
        .parse()
        .unwrap_or(5)
        .min(20);

    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(home) => home,
        Err(error) => {
            let _ = writeln!(standard_error, "stats: {error}");
            return 1;
        }
    };
    let workspace_root = {
        let flag = flag_set.string_value("workspace-root").trim().to_string();
        if flag.is_empty() {
            std::env::current_dir()
                .map(|path| display_path(&path))
                .unwrap_or_default()
        } else {
            flag
        }
    };

    let snapshot = collect_snapshot(&claude_home, &workspace_root, days, top_count);

    if flag_set.bool_value("json") {
        let payload = snapshot.to_json(days);
        if let Err(write_error) = write_indented(standard_output, &payload) {
            let _ = writeln!(
                standard_error,
                "stats: unable to render JSON: {write_error}"
            );
            return 1;
        }
        return 0;
    }
    snapshot.render_text(standard_output, days);
    0
}

/// One aggregated read over every axis. Fields are already-computed values from
/// the owning readers; `stats` adds no parsing of its own on top of them.
struct StatsSnapshot {
    tokens_saved: u64,
    tokens_before: u64,
    savings_percent: f64,
    commands_observed: u64,
    commands_compacted: u64,
    top_commands: Vec<(String, u64)>,
    top_tools: Vec<(String, u64, u64)>,
    gates: Vec<(String, u64)>,
    recall_documents: Option<u64>,
    recall_last_indexed_ms: u128,
    anvil: Option<Vec<(String, String)>>,
}

/// Read the compaction event log under `claude_home` and reuse the `gain`
/// parser. Home-driven (not env-driven) so `stats --claude-home` reports the
/// home it resolved and so tests stay hermetic. Missing/unreadable log yields
/// the parser's empty summary, matching `gain` on a fresh home.
fn gain_summary_from_home(claude_home: &Path, days: u64) -> crate::utility::gain::GainSummary {
    let path = claude_home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME);
    let text = fs::read_to_string(path).unwrap_or_default();
    parse_gain_summary(&text, Some(days_cutoff(days)), None)
}

/// Day-file paths under `claude_home/state/tool-timings/<date>.jsonl` for the
/// trailing `days` window, oldest-first, existing files only. This mirrors
/// `iter_day_files` but is rooted at the resolved home so it never reads the
/// process env home. Parsing stays in `read_rows`/`aggregate_rows`.
fn telemetry_day_files(claude_home: &Path, days: u64) -> Vec<PathBuf> {
    let directory = claude_home.join("state").join("tool-timings");
    let today = chrono::Local::now().date_naive();
    let mut paths = Vec::new();
    for offset in 0..days {
        let Some(date) = today.checked_sub_days(chrono::Days::new(offset)) else {
            break;
        };
        let path = directory.join(format!("{}.jsonl", date.format("%Y-%m-%d")));
        if path.exists() {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn collect_snapshot(
    claude_home: &Path,
    _workspace_root: &str,
    days: u64,
    top_count: usize,
) -> StatsSnapshot {
    let gain = gain_summary_from_home(claude_home, days);
    let top_commands = gain
        .top_commands
        .iter()
        .take(top_count)
        .map(|item| (item.command.clone(), item.tokens_saved))
        .collect::<Vec<_>>();

    let rows = read_rows(
        telemetry_day_files(claude_home, days)
            .iter()
            .map(PathBuf::as_path),
        None,
    );
    let top_tools = aggregate_rows(rows, top_count)
        .into_iter()
        .map(|summary| (summary.tool_name, summary.count, summary.sum_ms))
        .collect::<Vec<_>>();

    let gates = gate_activity(claude_home);

    let (recall_documents, recall_last_indexed_ms) = match recall_status_snapshot(claude_home) {
        Ok(status) => (Some(status.document_count), status.last_indexed_at_millis),
        Err(_) => (None, 0),
    };

    let anvil: Option<Vec<(String, String)>> = None;

    StatsSnapshot {
        tokens_saved: gain.tokens_saved,
        tokens_before: gain.tokens_before,
        savings_percent: gain.savings_percent(),
        commands_observed: gain.commands_observed,
        commands_compacted: gain.commands_compacted,
        top_commands,
        top_tools,
        gates,
        recall_documents,
        recall_last_indexed_ms,
        anvil,
    }
}

/// Sum every per-session counter file under each gate's state directory. Gates
/// persist one counter file per session key, so cross-session activity is the
/// directory total. The dir/label pairs come from the single source of truth
/// (`gate_status_rows`) so `stats` reports the same gates the hook path fires.
fn gate_activity(claude_home: &Path) -> Vec<(String, u64)> {
    gate_status_rows()
        .into_iter()
        .map(|row| {
            let dir = claude_home.join("state").join(row.dir);
            let total = fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .filter_map(std::result::Result::ok)
                        .map(|entry| {
                            fs::read_to_string(entry.path())
                                .ok()
                                .and_then(|text| text.trim().parse::<u64>().ok())
                                .unwrap_or(0)
                        })
                        .sum::<u64>()
                })
                .unwrap_or(0);
            (row.label.to_string(), total)
        })
        .collect()
}

fn days_cutoff(days: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    now.saturating_sub(days.saturating_mul(24 * 3600))
}

impl StatsSnapshot {
    fn render_text(&self, standard_output: &mut dyn Write, days: u64) {
        let _ = writeln!(
            standard_output,
            "keel stats (last {}d): {} tokens saved ({:.1}% of {})",
            days, self.tokens_saved, self.savings_percent, self.tokens_before
        );
        let _ = writeln!(
            standard_output,
            "  commands: {} observed, {} compacted",
            self.commands_observed, self.commands_compacted
        );
        if !self.top_commands.is_empty() {
            let _ = writeln!(standard_output, "  top savers:");
            for (command, saved) in &self.top_commands {
                let _ = writeln!(standard_output, "    {saved:>8}  {command}");
            }
        }
        if !self.top_tools.is_empty() {
            let _ = writeln!(standard_output, "  top tools (by time):");
            for (tool, count, sum_ms) in &self.top_tools {
                let _ = writeln!(standard_output, "    {sum_ms:>8}ms  {tool} (x{count})");
            }
        }
        let fired: Vec<(&String, u64)> = self
            .gates
            .iter()
            .map(|(label, count)| (label, *count))
            .filter(|(_, count)| *count > 0)
            .collect();
        if fired.is_empty() {
            let _ = writeln!(standard_output, "  gates: none fired");
        } else {
            let _ = writeln!(standard_output, "  gates fired:");
            for (label, count) in fired {
                let _ = writeln!(standard_output, "    {label}: {count}");
            }
        }
        match self.recall_documents {
            Some(count) => {
                let _ = writeln!(standard_output, "  memory: {count} documents indexed");
            }
            None => {
                let _ = writeln!(standard_output, "  memory: index unavailable");
            }
        }
        match &self.anvil {
            None => {
                let _ = writeln!(standard_output, "  anvil: none active");
            }
            Some(open) if open.is_empty() => {
                let _ = writeln!(standard_output, "  anvil: complete");
            }
            Some(open) => {
                let _ = writeln!(standard_output, "  anvil: {} open", open.len());
                for (id, state) in open.iter().take(5) {
                    let _ = writeln!(standard_output, "    {id} [{state}]");
                }
            }
        }
    }

    fn to_json(&self, days: u64) -> Value {
        let top_commands = self
            .top_commands
            .iter()
            .map(|(command, saved)| {
                Value::Object(vec![
                    ("command".into(), Value::String(command.clone())),
                    ("tokensSaved".into(), Value::Number(saved.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let top_tools = self
            .top_tools
            .iter()
            .map(|(tool, count, sum_ms)| {
                Value::Object(vec![
                    ("tool".into(), Value::String(tool.clone())),
                    ("count".into(), Value::Number(count.to_string())),
                    ("sumMs".into(), Value::Number(sum_ms.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let gates = self
            .gates
            .iter()
            .map(|(label, count)| {
                Value::Object(vec![
                    ("gate".into(), Value::String(label.clone())),
                    ("count".into(), Value::Number(count.to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let open_stories = self
            .anvil
            .as_ref()
            .map(|stories| {
                stories
                    .iter()
                    .map(|(id, state)| {
                        Value::Object(vec![
                            ("id".into(), Value::String(id.clone())),
                            ("state".into(), Value::String(state.clone())),
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Value::Object(vec![
            ("days".into(), Value::Number(days.to_string())),
            (
                "tokensSaved".into(),
                Value::Number(self.tokens_saved.to_string()),
            ),
            (
                "tokensBefore".into(),
                Value::Number(self.tokens_before.to_string()),
            ),
            (
                "savingsPercent".into(),
                Value::Number(format!("{:.2}", self.savings_percent)),
            ),
            (
                "commandsObserved".into(),
                Value::Number(self.commands_observed.to_string()),
            ),
            (
                "commandsCompacted".into(),
                Value::Number(self.commands_compacted.to_string()),
            ),
            ("topCommands".into(), Value::Array(top_commands)),
            ("topTools".into(), Value::Array(top_tools)),
            ("gates".into(), Value::Array(gates)),
            (
                "recallDocuments".into(),
                match self.recall_documents {
                    Some(count) => Value::Number(count.to_string()),
                    None => Value::String("unavailable".into()),
                },
            ),
            (
                "recallLastIndexedMs".into(),
                Value::Number(self.recall_last_indexed_ms.to_string()),
            ),
            ("openStories".into(), Value::Array(open_stories)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn with_isolated_home<F: FnOnce(&std::path::PathBuf) -> R, R>(suffix: &str, run: F) -> R {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "keel-stats-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test claude home");
        let result = run(&root);
        let _ = fs::remove_dir_all(&root);
        result
    }

    #[test]
    fn days_cutoff_rolls_back_window() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cutoff = days_cutoff(7);
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(cutoff <= before.saturating_sub(7 * 24 * 3600));
        assert!(cutoff >= after.saturating_sub(8 * 24 * 3600));
    }

    #[test]
    fn gate_activity_empty_when_no_state() {
        with_isolated_home("gates-empty", |home| {
            let activity = gate_activity(home);
            assert_eq!(activity.len(), 6);
            assert!(activity.iter().all(|(_, count)| *count == 0));
        });
    }

    #[test]
    fn gate_activity_sums_session_counters() {
        with_isolated_home("gates-sum", |home| {
            let dir = home.join("state").join("review-gate-blocks");
            fs::create_dir_all(&dir).expect("mkdir gate dir");
            fs::write(dir.join("sess-a"), "3").expect("write counter a");
            fs::write(dir.join("sess-b"), "2").expect("write counter b");
            let activity = gate_activity(home);
            let review = activity
                .iter()
                .find(|(label, _)| label == "review")
                .expect("review gate present");
            assert_eq!(review.1, 5);
        });
    }

    #[test]
    fn snapshot_renders_headline_and_axes() {
        with_isolated_home("render", |home| {
            let snapshot = collect_snapshot(home, "", 7, 5);
            let mut out: Vec<u8> = Vec::new();
            snapshot.render_text(&mut out, 7);
            let rendered = String::from_utf8_lossy(&out);
            assert!(rendered.contains("tokens saved"), "rendered: {rendered}");
            assert!(rendered.contains("commands:"), "rendered: {rendered}");
            assert!(rendered.contains("gates:"), "rendered: {rendered}");
            assert!(rendered.contains("memory:"), "rendered: {rendered}");
            assert!(rendered.contains("anvil:"), "rendered: {rendered}");
        });
    }

    #[test]
    fn json_payload_carries_all_axes() {
        with_isolated_home("json", |home| {
            let snapshot = collect_snapshot(home, "", 7, 5);
            let payload = snapshot.to_json(7);
            let Value::Object(map) = &payload else {
                panic!("expected object payload");
            };
            let keys: Vec<&str> = map.iter().map(|(key, _)| key.as_str()).collect();
            for expected in [
                "tokensSaved",
                "commandsObserved",
                "topCommands",
                "topTools",
                "gates",
                "recallDocuments",
                "openStories",
            ] {
                assert!(keys.contains(&expected), "missing {expected}: {keys:?}");
            }
        });
    }
}
