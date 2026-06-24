//! Purpose: Workflow command handlers (route, start, cockpit, finish, resume) and display helpers
//! Caller: mod.rs re-exports run_workflow_command
//! Dependencies: std::io::Write, crate::args::FlagSet, crate::json, crate::runtime, crate::utility::workflow_ledger
//! Main Functions: run_workflow_command
//! Side Effects: Creates/writes workflow ledger files

use std::io::Write;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::workflow_ledger::{
    allocate_unique_entry_id, close_entry, create_entry, current_timestamp_millis, entry_to_value,
    format_timestamp_iso8601, list_entries, read_entry, write_entry, Entry, STATUS_CLOSED,
    STATUS_OPEN,
};

use super::routing::{first_matching_keyword, match_routing_rule};
use super::shared::is_help_argument;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    On,
    Off,
}

fn colorize(text: &str, color_prefix: &str, mode: ColorMode) -> String {
    match mode {
        ColorMode::On => format!("{color_prefix}{text}\x1b[0m"),
        ColorMode::Off => text.to_string(),
    }
}

fn status_color_prefix(status: &str, mode: ColorMode) -> &'static str {
    if mode == ColorMode::Off {
        return "";
    }
    match status {
        "done" | "closed" => "\x1b[32m",
        "in-progress" | "open" => "\x1b[33m",
        "blocked" => "\x1b[31m",
        _ => "",
    }
}

pub(super) fn run_workflow_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        render_workflow_help(standard_output);
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "route" => run_workflow_route(&arguments[1..], standard_output, standard_error),
        "start" => run_workflow_start(&arguments[1..], standard_output, standard_error),
        "cockpit" | "status" | "dashboard" | "watch" => {
            run_workflow_cockpit(&arguments[1..], standard_output, standard_error)
        }
        "finish" => run_workflow_finish(&arguments[1..], standard_output, standard_error),
        "resume" => run_workflow_resume(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown workflow command: {other} (expected start|resume|finish|status|cockpit|dashboard|watch|route)"
            );
            1
        }
    }
}

fn run_workflow_start(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("workflow start");
    flag_set.string_flag("request", "");
    flag_set.string_flag("preset", "feature");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let mut request = flag_set.string_value("request").to_string();
    if request.trim().is_empty() && !flag_set.positional.is_empty() {
        request = flag_set.positional.join(" ");
    }
    if request.trim().is_empty() {
        let _ = writeln!(
            standard_error,
            "workflow start: --request is required (e.g. --request \"ship pagination\")"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow start: {error}");
            return 1;
        }
    };
    let now_millis = current_timestamp_millis();
    let entry_id = match allocate_unique_entry_id(&claude_home, now_millis) {
        Ok(id) => id,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow start: {error}");
            return 1;
        }
    };
    let entry = create_entry(
        entry_id,
        request.trim().to_string(),
        flag_set.string_value("preset").trim().to_string(),
        format_timestamp_iso8601(now_millis),
    );
    let path = match write_entry(&claude_home, &entry) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow start: {error}");
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("created".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&path))),
            ("entry".into(), entry_to_value(&entry)),
        ]);
        return super::shared::render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "workflow start: id={}", entry.id);
    let _ = writeln!(standard_output, "  request: {}", entry.request);
    let _ = writeln!(standard_output, "  preset: {}", entry.preset);
    let _ = writeln!(standard_output, "  started_at: {}", entry.started_at);
    let _ = writeln!(standard_output, "  ledger: {}", display_path(&path));
    0
}

fn run_workflow_cockpit(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("workflow cockpit");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("closed-tail", "5");
    flag_set.bool_flag("json", false);
    flag_set.bool_flag("color", false);
    flag_set.bool_flag("no-color", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let is_json = flag_set.bool_value("json");
    let color_mode = if is_json || flag_set.bool_value("no-color") {
        ColorMode::Off
    } else if flag_set.bool_value("color") {
        ColorMode::On
    } else {
        ColorMode::Off
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow cockpit: {error}");
            return 1;
        }
    };
    let entries = match list_entries(&claude_home) {
        Ok(entries) => entries,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow cockpit: {error}");
            return 1;
        }
    };
    let closed_tail: usize = flag_set
        .string_value("closed-tail")
        .parse()
        .unwrap_or(5usize);
    let open_entries: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.status == STATUS_OPEN)
        .collect();
    let closed_entries: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.status == STATUS_CLOSED)
        .rev()
        .take(closed_tail)
        .collect();
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            (
                "ledgerDirectory".into(),
                Value::String(display_path(&claude_home.join("workflow"))),
            ),
            (
                "openCount".into(),
                Value::Number(open_entries.len().to_string()),
            ),
            (
                "totalCount".into(),
                Value::Number(entries.len().to_string()),
            ),
            (
                "open".into(),
                Value::Array(
                    open_entries
                        .iter()
                        .map(|entry| entry_to_value(entry))
                        .collect(),
                ),
            ),
            (
                "recentlyClosed".into(),
                Value::Array(
                    closed_entries
                        .iter()
                        .map(|entry| entry_to_value(entry))
                        .collect(),
                ),
            ),
        ]);
        return super::shared::render_workflow_json(standard_output, standard_error, &payload);
    }

    let _ = writeln!(standard_output, "\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    let _ = writeln!(
        standard_output,
        "\u{2551}  {}{}",
        colorize("KEEL COCKPIT", "\x1b[1;36m", color_mode),
        pad_to_width("", 37)
    );
    let _ = writeln!(standard_output, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");

    if let Some(entry) = open_entries.first() {
        let _ = writeln!(
            standard_output,
            "\u{2551}  Request: {}{}",
            truncate(&entry.request, 38),
            pad_to_width(&truncate(&entry.request, 38), 38)
        );
        let _ = writeln!(
            standard_output,
            "\u{2551}  Preset:  {}{}",
            entry.preset,
            pad_to_width(&entry.preset, 38)
        );
        let status_text = "\u{25cf} in-progress".to_string();
        let colored_status = colorize(&status_text, "\x1b[33m", color_mode);
        let _ = writeln!(
            standard_output,
            "\u{2551}  Status:  {colored_status}{}",
            pad_to_width(&status_text, 38)
        );
    } else {
        let _ = writeln!(standard_output, "\u{2551}  No open workflow entries");
        let _ = writeln!(
            standard_output,
            "\u{2551}  Use: keel workflow start --request \"...\""
        );
    }

    let _ = writeln!(standard_output, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");

    let _ = writeln!(
        standard_output,
        "\u{2551}  {}{}",
        colorize("PROOF BOARD", "\x1b[1;33m", color_mode),
        pad_to_width("", 37)
    );
    if open_entries.is_empty() && closed_entries.is_empty() {
        let _ = writeln!(standard_output, "\u{2551}  (no workflow entries yet)");
    } else if let Some(entry) = open_entries.first() {
        let brief_done = !entry.proof.is_empty();
        let tests_done = entry.proof.contains("test") || entry.proof.contains("green");
        let review_done = entry.proof.contains("review");
        let pr_done = entry.proof.contains("pr") || entry.proof.contains("merge");
        render_proof_item(
            standard_output,
            "Working brief written",
            brief_done || !entry.request.is_empty(),
            color_mode,
        );
        render_proof_item(standard_output, "Tests passing", tests_done, color_mode);
        render_proof_item(standard_output, "Review pending", review_done, color_mode);
        render_proof_item(standard_output, "PR not yet created", !pr_done, color_mode);
    } else {
        render_proof_item(standard_output, "Working brief written", true, color_mode);
        render_proof_item(standard_output, "Tests passing", true, color_mode);
        render_proof_item(standard_output, "Review completed", true, color_mode);
        render_proof_item(standard_output, "PR merged", true, color_mode);
    }

    render_team_lanes(standard_output, &claude_home, color_mode);
    render_compaction_loss(standard_output, color_mode);

    let _ = writeln!(standard_output, "\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}");

    let next_cmd = if open_entries.is_empty() {
        "keel workflow start --request \"...\""
    } else {
        "keel workflow finish --id <entry-id> --proof \"...\""
    };
    let next_line = format!("  NEXT: {next_cmd}");
    let _ = writeln!(standard_output, "{}", colorize(&next_line, "\x1b[1;32m", color_mode));
    0
}

fn pad_to_width(text: &str, width: usize) -> String {
    let visible_len = text.chars().count();
    if visible_len >= width {
        String::new()
    } else {
        " ".repeat(width - visible_len)
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        text.to_string()
    } else {
        chars[..max_chars].iter().collect::<String>() + "..."
    }
}

fn render_proof_item(output: &mut dyn Write, label: &str, done: bool, mode: ColorMode) {
    let (icon, color) = if done {
        ("\u{2713}", "\x1b[32m")
    } else {
        ("\u{25cb}", "\x1b[33m")
    };
    let colored_icon = colorize(icon, color, mode);
    let _ = writeln!(output, "\u{2551}  {colored_icon} {label}");
}

fn render_compaction_loss(output: &mut dyn Write, mode: ColorMode) {
    use crate::utility::gain::load_compaction_loss_today;

    let loss = load_compaction_loss_today();

    let _ = writeln!(output, "\u{2551}  {}{}",
        colorize("COMPACTION LOSS", "\x1b[1;36m", mode),
        pad_to_width("", 36)
    );

    if loss.commands_observed == 0 {
        let _ = writeln!(output, "\u{2551}  (no compaction events today)");
        return;
    }

    let pct = loss.savings_percent();
    let line = format!(
        "{} cmds observed, {} compacted | {} -> {} tokens (saved {}, {:.1}%)",
        loss.commands_observed,
        loss.commands_compacted,
        loss.tokens_before,
        loss.tokens_after,
        loss.tokens_saved,
        pct,
    );
    let _ = writeln!(output, "\u{2551}  {}", line);
}

fn render_team_lanes(output: &mut dyn Write, _claude_home: &std::path::Path, mode: ColorMode) {
    use crate::runtime::run_command;

    let _ = writeln!(output, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(
        output,
        "\u{2551}  {}{}",
        colorize("TEAM LANES", "\x1b[1;35m", mode),
        pad_to_width("", 38)
    );

    let prefix = "keel-team-";
    let sessions = match run_command("tmux", &["list-sessions".to_string()], None) {
        Ok(result) if result.code == 0 => {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let mut names = Vec::new();
            for line in stdout.lines() {
                if let Some(name) = line.split(':').next() {
                    let trimmed = name.trim().to_string();
                    if trimmed.starts_with(prefix) {
                        names.push(trimmed);
                    }
                }
            }
            names
        }
        _ => Vec::new(),
    };

    if sessions.is_empty() {
        let _ = writeln!(output, "\u{2551}  (no active team panes)");
    } else {
        for session in &sessions {
            let name = session.strip_prefix(prefix).unwrap_or(session);
            let colored_name = colorize(name, "\x1b[32m", mode);
            let _ = writeln!(output, "\u{2551}  {colored_name}: running");
        }
    }
}

fn run_workflow_finish(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("workflow finish");
    flag_set.string_flag("id", "");
    flag_set.string_flag("proof", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let entry_id = flag_set.string_value("id").trim().to_string();
    if entry_id.is_empty() {
        let _ = writeln!(
            standard_error,
            "workflow finish: --id is required (e.g. --id wf-1971c61bb00)"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow finish: {error}");
            return 1;
        }
    };
    let existing = match read_entry(&claude_home, &entry_id) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            let _ = writeln!(
                standard_error,
                "workflow finish: no ledger entry with id {entry_id}"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "workflow finish: {error}");
            return 1;
        }
    };
    if existing.status == STATUS_CLOSED {
        let _ = writeln!(
            standard_error,
            "workflow finish: entry {entry_id} is already closed (finished {})",
            existing.finished_at
        );
        return 1;
    }
    let now_millis = current_timestamp_millis();
    let closed = close_entry(
        existing,
        format_timestamp_iso8601(now_millis),
        flag_set.string_value("proof").trim().to_string(),
    );
    let path = match write_entry(&claude_home, &closed) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow finish: {error}");
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("closed".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&path))),
            ("entry".into(), entry_to_value(&closed)),
        ]);
        return super::shared::render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "workflow finish: id={}", closed.id);
    let _ = writeln!(standard_output, "  finished_at: {}", closed.finished_at);
    if !closed.proof.is_empty() {
        let _ = writeln!(standard_output, "  proof: {}", closed.proof);
    }
    let _ = writeln!(standard_output, "  ledger: {}", display_path(&path));
    0
}

fn run_workflow_resume(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("workflow resume");
    flag_set.string_flag("id", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow resume: {error}");
            return 1;
        }
    };
    let requested_id = flag_set.string_value("id").trim().to_string();
    let json_output = flag_set.bool_value("json");
    if !requested_id.is_empty() {
        let entry = match read_entry(&claude_home, &requested_id) {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                let _ = writeln!(
                    standard_error,
                    "workflow resume: no ledger entry with id {requested_id}"
                );
                return 1;
            }
            Err(error) => {
                let _ = writeln!(standard_error, "workflow resume: {error}");
                return 1;
            }
        };
        if entry.status == STATUS_CLOSED {
            let _ = writeln!(
                standard_error,
                "workflow resume: entry {requested_id} is already closed (finished {})",
                entry.finished_at
            );
            return 1;
        }
        if json_output {
            let payload = Value::Object(vec![
                (
                    "ledgerDirectory".into(),
                    Value::String(display_path(&claude_home.join("workflow"))),
                ),
                ("entry".into(), entry_to_value(&entry)),
                (
                    "nextCommand".into(),
                    Value::String(format!(
                        "keel workflow finish --id {} --proof <evidence>",
                        entry.id
                    )),
                ),
            ]);
            return super::shared::render_workflow_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(standard_output, "workflow resume: id={}", entry.id);
        let _ = writeln!(standard_output, "  request: {}", entry.request);
        let _ = writeln!(standard_output, "  preset: {}", entry.preset);
        let _ = writeln!(standard_output, "  started_at: {}", entry.started_at);
        let _ = writeln!(
            standard_output,
            "  next: keel workflow finish --id {} --proof <evidence>",
            entry.id
        );
        return 0;
    }
    let entries = match list_entries(&claude_home) {
        Ok(entries) => entries,
        Err(error) => {
            let _ = writeln!(standard_error, "workflow resume: {error}");
            return 1;
        }
    };
    let open_entries: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.status == STATUS_OPEN)
        .collect();
    if json_output {
        let payload = Value::Object(vec![
            (
                "ledgerDirectory".into(),
                Value::String(display_path(&claude_home.join("workflow"))),
            ),
            (
                "openCount".into(),
                Value::Number(open_entries.len().to_string()),
            ),
            (
                "open".into(),
                Value::Array(
                    open_entries
                        .iter()
                        .map(|entry| entry_to_value(entry))
                        .collect(),
                ),
            ),
        ]);
        return super::shared::render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "workflow resume: ledger={}",
        display_path(&claude_home.join("workflow"))
    );
    if open_entries.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no open workflow entries (start one with: keel workflow start --request \"...\")"
        );
        return 0;
    }
    let _ = writeln!(standard_output, "  open entries: {}", open_entries.len());
    for entry in &open_entries {
        let _ = writeln!(
            standard_output,
            "    {} [{}] {} (started {})",
            entry.id, entry.preset, entry.request, entry.started_at
        );
        let _ = writeln!(
            standard_output,
            "      next: keel workflow resume --id {}",
            entry.id
        );
    }
    0
}

fn run_workflow_route(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("workflow route");
    flag_set.string_flag("request", "");
    flag_set.string_flag("format", "text");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let mut request = flag_set.string_value("request").to_string();
    if request.is_empty() && !flag_set.positional.is_empty() {
        request = flag_set.positional.join(" ");
    }
    if request.trim().is_empty() {
        let _ = writeln!(
            standard_error,
            "workflow route: --request is required (e.g. --request \"audit the release pipeline\")"
        );
        return 1;
    }
    let matched_rule = match_routing_rule(&request);
    let format = flag_set.string_value("format");
    if format == "json" {
        let payload = Value::Object(vec![
            ("request".into(), Value::String(request.clone())),
            (
                "specialist".into(),
                Value::String(matched_rule.specialist.into()),
            ),
            ("reason".into(), Value::String(matched_rule.reason.into())),
            (
                "matchedKeyword".into(),
                Value::String(first_matching_keyword(&request, matched_rule).into()),
            ),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }
    let _ = writeln!(standard_output, "specialist: {}", matched_rule.specialist);
    let _ = writeln!(standard_output, "reason: {}", matched_rule.reason);
    let matched_keyword = first_matching_keyword(&request, matched_rule);
    if !matched_keyword.is_empty() {
        let _ = writeln!(standard_output, "matched_keyword: {matched_keyword}");
    }
    0
}

fn render_workflow_help(standard_output: &mut dyn Write) {
    let _ = writeln!(standard_output, "Usage: keel workflow [command]");
    let _ = writeln!(standard_output, "Commands:");
    let _ = writeln!(
        standard_output,
        "  route                       Route a request to a specialist agent"
    );
    let _ = writeln!(
        standard_output,
        "  start                       Start new workflow"
    );
    let _ = writeln!(
        standard_output,
        "  cockpit|status|dashboard|watch  Show workflow board"
    );
    let _ = writeln!(
        standard_output,
        "  finish                      Finish a workflow"
    );
    let _ = writeln!(
        standard_output,
        "  resume                      Resume an open workflow"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utility::workflow_ledger::{
        close_entry, create_entry, format_timestamp_iso8601, write_entry,
    };
    use std::fs;

    fn tempdir_under(label: &str) -> std::path::PathBuf {
        let unique_suffix: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
    }

    fn seeded_open_entry(claude_home: &std::path::Path, id: &str, request: &str) -> Entry {
        let entry = create_entry(
            id.to_string(),
            request.to_string(),
            "feature".to_string(),
            format_timestamp_iso8601(0),
        );
        write_entry(claude_home, &entry).expect("seed open entry");
        entry
    }

    fn route(request: &str) -> (u8, String, String) {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "route".to_string(),
                "--request".to_string(),
                request.to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        (
            exit_code,
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
        )
    }

    #[test]
    fn route_audit_request_targets_reviewer() {
        let (exit_code, stdout, stderr) =
            route("audit the release pipeline for production readiness");
        assert_eq!(exit_code, 0, "stderr: {stderr}");
        assert!(stdout.contains("specialist: reviewer"), "stdout: {stdout}");
    }

    #[test]
    fn route_brownfield_edit_targets_preserve_existing_flow() {
        let (exit_code, stdout, _) = route("trace the existing flow before editing");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: preserve-existing-flow"));
    }

    #[test]
    fn route_pr_workflow_targets_git_expert() {
        let (exit_code, stdout, _) = route("open a pull request and rebase the branch");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: git-expert"));
    }

    #[test]
    fn route_threat_model_targets_security_auditor() {
        let (exit_code, stdout, _) = route("threat model the new authentication endpoint");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: security-and-compliance-auditor"));
    }

    #[test]
    fn route_test_strategy_targets_qa() {
        let (exit_code, stdout, _) = route("design a playwright e2e test strategy");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: qa-and-automation-engineer"));
    }

    #[test]
    fn route_kubernetes_targets_devops() {
        let (exit_code, stdout, _) = route("update the kubernetes deployment and rollout plan");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: cloud-and-devops-expert"));
    }

    #[test]
    fn route_database_schema_targets_backend() {
        let (exit_code, stdout, _) = route("design a postgres schema for the new microservice");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: backend-and-data-architecture"));
    }

    #[test]
    fn route_ios_targets_mobile() {
        let (exit_code, stdout, _) = route("fix the swift crash on ios startup");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: mobile-development-life-cycle"));
    }

    #[test]
    fn route_react_targets_web() {
        let (exit_code, stdout, _) = route("refactor the react component on the dashboard webpage");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: web-development-life-cycle"));
    }

    #[test]
    fn route_journey_friction_targets_ux() {
        let (exit_code, stdout, _) =
            route("investigate the signup funnel drop-off with user research");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: ux-research-and-experience-strategy"));
    }

    #[test]
    fn route_design_system_targets_ui() {
        let (exit_code, stdout, _) =
            route("align the design system tokens for the responsive layout");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: ui-design-systems-and-responsive-interfaces"));
    }

    #[test]
    fn route_memory_health_targets_memory_status_reporter() {
        let (exit_code, stdout, _) = route("show memory health and what did you learn today");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: memory-status-reporter"));
    }

    #[test]
    fn route_unknown_request_falls_back_to_sdlc_default() {
        let (exit_code, stdout, _) = route("plan the next quarter roadmap");
        assert_eq!(exit_code, 0);
        assert!(stdout.contains("specialist: software-development-life-cycle"));
        assert!(stdout.contains("default lane"));
    }

    #[test]
    fn route_stripe_payment_targets_stripe_skill() {
        let (exit_code, stdout, _) =
            route("verify our stripe webhook signature handling on the payment intent");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: stripe-integration"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_websocket_request_targets_realtime_skill() {
        let (exit_code, stdout, _) = route("design a websocket reconnection protocol");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: websocket-realtime-design"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_postgres_migration_targets_migration_skill() {
        let (exit_code, stdout, _) =
            route("plan a postgres migration to add a not null column on a 50M row table");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: postgres-migration-safety"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_react_perf_targets_perf_audit_skill() {
        let (exit_code, stdout, _) =
            route("the react profiler shows a render storm on the dashboard");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: react-performance-audit"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_api_contract_targets_contract_skill() {
        let (exit_code, stdout, _) =
            route("plan the openapi diff for breaking changes before the release");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: api-contract-design"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_observability_targets_observability_skill() {
        let (exit_code, stdout, _) = route("define SLOs and burn-rate alerting with opentelemetry");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: observability-and-incident-response"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_identity_build_targets_auth_skill() {
        let (exit_code, stdout, _) =
            route("implement the oauth2 oidc login flow with refresh tokens");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: authentication-and-identity"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_threat_model_still_beats_auth_build() {
        let (exit_code, stdout, _) = route("threat model the new authentication endpoint");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: security-and-compliance-auditor"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_etl_pipeline_targets_data_ml_skill() {
        let (exit_code, stdout, _) = route("build an ETL pipeline feeding a dbt data warehouse");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: data-and-ml-engineering"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_finops_targets_cost_skill() {
        let (exit_code, stdout, _) =
            route("cloud cost rightsizing and savings plan with infracost");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: cloud-cost-and-finops"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_i18n_targets_localization_skill() {
        let (exit_code, stdout, _) =
            route("add i18n message catalogs with ICU MessageFormat pluralization");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: internationalization-and-localization"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_dependency_upgrade_targets_supply_chain_skill() {
        let (exit_code, stdout, _) =
            route("upgrade transitive dependencies and generate an sbom with provenance");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: dependency-and-supply-chain"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_plain_deploy_still_targets_devops() {
        let (exit_code, stdout, _) =
            route("deploy the service to kubernetes with a terraform rollout");
        assert_eq!(exit_code, 0);
        assert!(
            stdout.contains("specialist: cloud-and-devops-expert"),
            "stdout: {stdout}"
        );
    }

    #[test]
    fn route_single_token_uses_word_boundary_matching() {
        let (exit_code, stdout, _) = route("redesign the kiosk display");
        assert_eq!(exit_code, 0);
        assert!(
            !stdout.contains("specialist: ui-design-systems-and-responsive-interfaces"),
            "ui keyword should not match inside 'kiosk': {stdout}"
        );
    }

    #[test]
    fn route_json_format_emits_structured_payload() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "route".to_string(),
                "--request".to_string(),
                "audit production readiness".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("\"specialist\": \"reviewer\""));
        assert!(output.contains("\"matchedKeyword\""));
        assert!(output.contains("\"reason\""));
    }

    #[test]
    fn route_missing_request_returns_error() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(&["route".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit_code, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("--request is required"));
    }

    #[test]
    fn route_accepts_positional_request() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "route".to_string(),
                "audit".to_string(),
                "the".to_string(),
                "release".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        assert!(String::from_utf8_lossy(&stdout).contains("specialist: reviewer"));
    }

    #[test]
    fn workflow_resume_lists_open_entries_when_no_id_supplied() {
        let temporary_directory = tempdir_under("keel-workflow-resume-list");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-aaaa", "ship pagination");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("workflow resume: ledger="),
            "stdout: {output}"
        );
        assert!(output.contains("wf-aaaa"), "stdout: {output}");
        assert!(output.contains("ship pagination"), "stdout: {output}");
        assert!(
            output.contains("keel workflow resume --id wf-aaaa"),
            "expected resume hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_with_no_open_entries_emits_action_hint() {
        let temporary_directory = tempdir_under("keel-workflow-resume-empty");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(claude_home.join("workflow")).expect("create ledger dir");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("no open workflow entries"),
            "stdout: {output}"
        );
        assert!(
            output.contains("keel workflow start"),
            "expected start hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_focuses_single_entry_when_id_supplied() {
        let temporary_directory = tempdir_under("keel-workflow-resume-id");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-bbbb", "investigate signup funnel");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--id".to_string(),
                "wf-bbbb".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("workflow resume: id=wf-bbbb"));
        assert!(output.contains("investigate signup funnel"));
        assert!(
            output.contains("keel workflow finish --id wf-bbbb --proof"),
            "expected finish hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_unknown_id_returns_error() {
        let temporary_directory = tempdir_under("keel-workflow-resume-missing");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(claude_home.join("workflow")).expect("create ledger dir");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--id".to_string(),
                "wf-missing".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("no ledger entry with id wf-missing"),
            "stderr: {stderr_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_rejects_already_closed_entry() {
        let temporary_directory = tempdir_under("keel-workflow-resume-closed");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let open = seeded_open_entry(&claude_home, "wf-cccc", "rotate auth secrets");
        let closed = close_entry(
            open,
            format_timestamp_iso8601(1),
            "ladder green".to_string(),
        );
        write_entry(&claude_home, &closed).expect("seed closed entry");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--id".to_string(),
                "wf-cccc".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("entry wf-cccc is already closed"),
            "stderr: {stderr_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_json_emits_structured_payload() {
        let temporary_directory = tempdir_under("keel-workflow-resume-json");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-dddd", "audit production readiness");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "resume".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("\"openCount\": 1"), "stdout: {output}");
        assert!(output.contains("\"id\": \"wf-dddd\""), "stdout: {output}");
        assert!(output.contains("\"ledgerDirectory\""), "stdout: {output}");

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn colorize_on_wraps_text_with_ansi_codes() {
        let result = colorize("hello", "\x1b[32m", ColorMode::On);
        assert_eq!(result, "\x1b[32mhello\x1b[0m");
    }

    #[test]
    fn colorize_off_returns_plain_text() {
        let result = colorize("hello", "\x1b[32m", ColorMode::Off);
        assert_eq!(result, "hello");
    }

    #[test]
    fn status_color_prefix_green_for_done() {
        let result = status_color_prefix("done", ColorMode::On);
        assert_eq!(result, "\x1b[32m");
    }

    #[test]
    fn status_color_prefix_yellow_for_in_progress() {
        let result = status_color_prefix("in-progress", ColorMode::On);
        assert_eq!(result, "\x1b[33m");
    }

    #[test]
    fn status_color_prefix_red_for_blocked() {
        let result = status_color_prefix("blocked", ColorMode::On);
        assert_eq!(result, "\x1b[31m");
    }

    #[test]
    fn status_color_prefix_empty_when_off() {
        let result = status_color_prefix("done", ColorMode::Off);
        assert_eq!(result, "");
    }

    #[test]
    fn cockpit_with_no_color_flag_has_no_ansi_codes() {
        let temporary_directory = tempdir_under("keel-workflow-cockpit-nocolor");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-nc", "ship feature");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "cockpit".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--no-color".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            !output.contains('\x1b'),
            "cockpit with --no-color should not contain escape codes: {output}"
        );
        assert!(output.contains("KEEL COCKPIT"), "stdout: {output}");
        assert!(output.contains("NEXT:"), "stdout: {output}");

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn cockpit_with_color_flag_has_ansi_codes() {
        let temporary_directory = tempdir_under("keel-workflow-cockpit-color");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-c", "ship feature");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "cockpit".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--color".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains('\x1b'),
            "cockpit with --color should contain escape codes: {output}"
        );
        assert!(output.contains("\x1b[1;36m"), "expected cyan header: {output}");
        assert!(output.contains("\x1b[33m"), "expected yellow in-progress: {output}");
        assert!(output.contains("\x1b[1;32m"), "expected green NEXT: {output}");

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn cockpit_json_has_no_color_codes() {
        let temporary_directory = tempdir_under("keel-workflow-cockpit-json");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-json", "audit stuff");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "cockpit".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--json".to_string(),
                "--color".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            !output.contains('\x1b'),
            "JSON output should never contain escape codes even with --color: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn cockpit_shows_compaction_loss_section() {
        let temporary_directory = tempdir_under("keel-workflow-cockpit-compaction");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-cl", "ship feature");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "cockpit".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--no-color".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("COMPACTION LOSS"),
            "cockpit should contain COMPACTION LOSS section: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn cockpit_compaction_loss_no_color_has_no_ansi() {
        let temporary_directory = tempdir_under("keel-workflow-compaction-nocolor");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-cln", "ship feature");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_workflow_command(
            &[
                "cockpit".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--no-color".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("COMPACTION LOSS"));
        assert!(
            !output.contains('\x1b'),
            "compaction loss section with --no-color should not contain escape codes"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }
}
