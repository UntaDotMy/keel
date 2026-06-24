//! Purpose: Working brief command handlers (write, show, list)
//! Caller: mod.rs run_memory_command
//! Dependencies: std::io, crate::args::FlagSet, crate::json, crate::runtime, crate::utility::working_brief, crate::utility::recall
//! Main Functions: run_working_brief_command
//! Side Effects: Writes working brief files, triggers recall reindex

use std::io::Write;

use crate::args::FlagSet;
use crate::json::Value;
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::working_brief::{
    brief_directory, brief_to_value, create_brief, list_briefs, read_brief, write_brief, Brief,
};

use super::shared::{is_help_argument, render_workflow_json};

pub(super) fn run_working_brief_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} working-brief [write|show|list] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "write" => run_working_brief_write(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "show" => run_working_brief_show(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "list" => run_working_brief_list(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} working-brief action: {other} (expected write|show|list)"
            );
            1
        }
    }
}

fn run_working_brief_write(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("working-brief write");
    flag_set.string_flag("id", "");
    flag_set.string_flag("request", "");
    flag_set.string_flag("constraints", "");
    flag_set.string_flag("acceptance-criteria", "");
    flag_set.string_flag("assumptions", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let request = flag_set.string_value("request").trim().to_string();
    if request.is_empty() {
        let _ = writeln!(
            standard_error,
            "{command_group} working-brief write: --request is required"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief write: {error}"
            );
            return 1;
        }
    };
    let now_millis = crate::utility::workflow_ledger::current_timestamp_millis();
    let entry_id = flag_set.string_value("id").trim().to_string();
    let entry_id = if entry_id.is_empty() {
        format!("wb-{now_millis:x}")
    } else {
        entry_id
    };
    // Capture the workspace this brief belongs to so the default-on working-brief
    // gate can scope "did this workspace get a brief this session" instead of
    // counting a brief written for an unrelated project. Empty when the cwd
    // cannot be resolved — consumers treat empty as "applies anywhere", so a
    // resolution failure fails open rather than misattributing the brief.
    let workspace = std::env::current_dir()
        .map(|cwd| display_path(&cwd))
        .unwrap_or_default();
    let brief = create_brief(
        entry_id,
        request,
        split_csv_lines(flag_set.string_value("constraints")),
        split_csv_lines(flag_set.string_value("acceptance-criteria")),
        split_csv_lines(flag_set.string_value("assumptions")),
        workspace,
        crate::utility::workflow_ledger::format_timestamp_iso8601(now_millis),
    );
    let path = match write_brief(&claude_home, &brief) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief write: {error}"
            );
            return 1;
        }
    };
    // Sync the recall FTS index now so the brief is searchable on the very next
    // `recall` without a separate trigger. Best-effort: the brief on disk is
    // durable regardless, and a failed sync is reconciled by the next read-path
    // sync.
    if let Err(error) = crate::utility::recall::reindex_after_write(&claude_home) {
        let _ = writeln!(
            standard_error,
            "{command_group} working-brief write: recall index sync skipped ({error})"
        );
    }
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("written".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&path))),
            ("brief".into(), brief_to_value(&brief)),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{command_group} working-brief write: id={}",
        brief.id
    );
    let _ = writeln!(standard_output, "  request: {}", brief.request);
    let _ = writeln!(
        standard_output,
        "  constraints: {} entries",
        brief.constraints.len()
    );
    let _ = writeln!(
        standard_output,
        "  acceptance_criteria: {} entries",
        brief.acceptance_criteria.len()
    );
    let _ = writeln!(
        standard_output,
        "  assumptions: {} entries",
        brief.assumptions.len()
    );
    let _ = writeln!(standard_output, "  path: {}", display_path(&path));
    0
}

fn run_working_brief_show(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("working-brief show");
    flag_set.string_flag("id", "");
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
            "{command_group} working-brief show: --id is required"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief show: {error}"
            );
            return 1;
        }
    };
    let brief = match read_brief(&claude_home, &entry_id) {
        Ok(Some(brief)) => brief,
        Ok(None) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief show: no brief with id {entry_id}"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief show: {error}"
            );
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        return render_workflow_json(standard_output, standard_error, &brief_to_value(&brief));
    }
    render_brief_text(standard_output, &brief);
    0
}

fn run_working_brief_list(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("working-brief list");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief list: {error}"
            );
            return 1;
        }
    };
    let briefs = match list_briefs(&claude_home) {
        Ok(briefs) => briefs,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} working-brief list: {error}"
            );
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            (
                "directory".into(),
                Value::String(display_path(&brief_directory(&claude_home))),
            ),
            ("count".into(), Value::Number(briefs.len().to_string())),
            (
                "briefs".into(),
                Value::Array(briefs.iter().map(brief_to_value).collect()),
            ),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{command_group} working-brief list: directory={} count={}",
        display_path(&brief_directory(&claude_home)),
        briefs.len()
    );
    if briefs.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no briefs (write one with: keel {command_group} working-brief write --request \"...\")"
        );
        return 0;
    }
    for brief in &briefs {
        let _ = writeln!(
            standard_output,
            "  {} {} (created {})",
            brief.id, brief.request, brief.created_at
        );
    }
    0
}

fn render_brief_text(standard_output: &mut dyn Write, brief: &Brief) {
    let _ = writeln!(standard_output, "id: {}", brief.id);
    let _ = writeln!(standard_output, "request: {}", brief.request);
    let _ = writeln!(standard_output, "created_at: {}", brief.created_at);
    let _ = writeln!(standard_output, "constraints:");
    for line in &brief.constraints {
        let _ = writeln!(standard_output, "  - {line}");
    }
    let _ = writeln!(standard_output, "acceptance_criteria:");
    for line in &brief.acceptance_criteria {
        let _ = writeln!(standard_output, "  - {line}");
    }
    let _ = writeln!(standard_output, "assumptions:");
    for line in &brief.assumptions {
        let _ = writeln!(standard_output, "  - {line}");
    }
}

fn split_csv_lines(joined: &str) -> Vec<String> {
    joined
        .split(['|', '\n'])
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}
