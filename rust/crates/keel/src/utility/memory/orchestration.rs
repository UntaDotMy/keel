//! Purpose: Orchestration command handlers (runtime-preflight, resume-status, task lifecycle, checkpoint)
//! Caller: mod.rs re-exports run_orchestration_command
//! Dependencies: std::fs, std::io::Write, crate::args::FlagSet, crate::json, crate::runtime, crate::utility::record_store, crate::utility::workflow_ledger, crate::utility::working_brief
//! Main Functions: run_orchestration_command
//! Side Effects: Creates task/checkpoint ledger files

use std::fs;
use std::io::Write;

use crate::args::FlagSet;
use crate::json::Value;
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::record_store::{
    allocate_unique_record_id, field, record_to_value, Record, RecordStore,
};
use crate::utility::workflow_ledger::{
    current_timestamp_millis, entry_to_value, format_timestamp_iso8601, list_entries, Entry,
    STATUS_OPEN,
};
use crate::utility::working_brief::list_briefs;

use super::shared::{is_help_argument, probe_marker, probe_value, render_workflow_json};

pub(super) fn run_orchestration_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel orchestration [runtime-preflight|resume-status] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "runtime-preflight" => {
            run_orchestration_runtime_preflight(&arguments[1..], standard_output, standard_error)
        }
        "resume-status" => {
            run_orchestration_resume_status(&arguments[1..], standard_output, standard_error)
        }
        "task" => run_orchestration_task(&arguments[1..], standard_output, standard_error),
        "checkpoint" => {
            run_orchestration_checkpoint(&arguments[1..], standard_output, standard_error)
        }
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown orchestration command: {other} (expected runtime-preflight|resume-status|task|checkpoint)"
            );
            1
        }
    }
}

fn run_orchestration_runtime_preflight(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("orchestration runtime-preflight");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home_probe = resolve_claude_home(flag_set.string_value("claude-home"));
    let ledger_probe = claude_home_probe
        .as_ref()
        .map_err(|error| error.clone())
        .and_then(|claude_home| {
            let directory = claude_home.join("workflow");
            fs::create_dir_all(&directory)
                .map_err(|error| format!("create {}: {error}", display_path(&directory)))?;
            Ok(directory)
        });
    let git_probe = match crate::runtime::run_command("git", &["--version".to_string()], None) {
        Ok(result) if result.code == 0 => {
            Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
        }
        Ok(result) => Err(format!("git --version exited with code {}", result.code)),
        Err(error) => Err(error),
    };
    let claude_home_status = claude_home_probe
        .as_ref()
        .map(|path| display_path(path))
        .unwrap_or_else(|error| error.clone());
    let ledger_status = ledger_probe
        .as_ref()
        .map(|path| display_path(path))
        .unwrap_or_else(|error| error.clone());
    let git_status = match &git_probe {
        Ok(version) => version.clone(),
        Err(error) => error.clone(),
    };
    let all_ok = claude_home_probe.is_ok() && ledger_probe.is_ok() && git_probe.is_ok();
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("ok".into(), Value::Bool(all_ok)),
            (
                "claudeHome".into(),
                probe_value(&claude_home_probe, &claude_home_status),
            ),
            (
                "ledgerDirectory".into(),
                probe_value(&ledger_probe, &ledger_status),
            ),
            ("git".into(), probe_value(&git_probe, &git_status)),
        ]);
        let exit = render_workflow_json(standard_output, standard_error, &payload);
        return if all_ok { exit } else { 1 };
    }
    let _ = writeln!(
        standard_output,
        "orchestration runtime-preflight: {}",
        if all_ok { "ok" } else { "fail" }
    );
    let _ = writeln!(
        standard_output,
        "  claude_home: {} {claude_home_status}",
        probe_marker(&claude_home_probe)
    );
    let _ = writeln!(
        standard_output,
        "  ledger:      {} {ledger_status}",
        probe_marker(&ledger_probe)
    );
    let _ = writeln!(
        standard_output,
        "  git:         {} {git_status}",
        probe_marker(&git_probe)
    );
    if all_ok {
        0
    } else {
        1
    }
}

fn run_orchestration_resume_status(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("orchestration resume-status");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration resume-status: {error}");
            return 1;
        }
    };
    let entries = match list_entries(&claude_home) {
        Ok(entries) => entries,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration resume-status: {error}");
            return 1;
        }
    };
    let open_entries: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.status == STATUS_OPEN)
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
                "open".into(),
                Value::Array(
                    open_entries
                        .iter()
                        .map(|entry| entry_to_value(entry))
                        .collect(),
                ),
            ),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "orchestration resume-status: open={} ledger={}",
        open_entries.len(),
        display_path(&claude_home.join("workflow"))
    );
    if open_entries.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no open workflow entries (start one with: keel workflow start --request \"...\")"
        );
        return 0;
    }
    for entry in &open_entries {
        let _ = writeln!(
            standard_output,
            "  {} [{}] {} (started {})",
            entry.id, entry.preset, entry.request, entry.started_at
        );
    }
    0
}

fn run_orchestration_task(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel orchestration task [begin|progress|complete|list] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "begin" => run_orchestration_task_begin(&arguments[1..], standard_output, standard_error),
        "progress" => {
            run_orchestration_task_progress(&arguments[1..], standard_output, standard_error)
        }
        "complete" => {
            run_orchestration_task_complete(&arguments[1..], standard_output, standard_error)
        }
        "list" => run_orchestration_task_list(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown orchestration task action: {other} (expected begin|progress|complete|list)"
            );
            1
        }
    }
}

/// Tasks live one-JSON-per-record under `<claude_home>/orchestration/tasks/`.
/// Status moves open -> in-progress -> done. The record is a flat string map so
/// it round-trips through the shared key=string reader like every other ledger.
fn task_store(claude_home: &std::path::Path) -> RecordStore {
    RecordStore::new(claude_home, "orchestration/tasks")
}

const TASK_STATUS_IN_PROGRESS: &str = "in-progress";
const TASK_STATUS_DONE: &str = "done";

fn run_orchestration_task_begin(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("orchestration task begin");
    flag_set.string_flag("task", "");
    flag_set.string_flag("phase", "");
    flag_set.string_flag("next-step", "");
    flag_set.string_flag("skills", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let mut task = flag_set.string_value("task").trim().to_string();
    if task.is_empty() && !flag_set.positional.is_empty() {
        task = flag_set.positional.join(" ");
    }
    if task.trim().is_empty() {
        let _ = writeln!(
            standard_error,
            "orchestration task begin: --task is required (the active task description)"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration task begin: {error}");
            return 1;
        }
    };
    let now_millis = current_timestamp_millis();
    let store = task_store(&claude_home);
    let id = allocate_unique_record_id(&store, &format!("task-{now_millis:x}"));
    let started_at = format_timestamp_iso8601(now_millis);
    let record: Record = vec![
        ("id".into(), id.clone()),
        ("task".into(), task.trim().to_string()),
        (
            "phase".into(),
            flag_set.string_value("phase").trim().to_string(),
        ),
        ("status".into(), TASK_STATUS_IN_PROGRESS.to_string()),
        (
            "nextStep".into(),
            flag_set.string_value("next-step").trim().to_string(),
        ),
        (
            "skills".into(),
            flag_set.string_value("skills").trim().to_string(),
        ),
        ("startedAt".into(), started_at),
        ("updatedAt".into(), String::new()),
        ("completedAt".into(), String::new()),
        ("note".into(), String::new()),
    ];
    let path = match store.write_record(&id, &record) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration task begin: {error}");
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("created".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&path))),
            ("task".into(), record_to_value(&record)),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "orchestration task begin: id={id}");
    let _ = writeln!(standard_output, "  task: {}", task.trim());
    let _ = writeln!(standard_output, "  status: {TASK_STATUS_IN_PROGRESS}");
    let _ = writeln!(standard_output, "  ledger: {}", display_path(&path));
    0
}

fn run_orchestration_task_progress(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    update_existing_task(
        "orchestration task progress",
        arguments,
        standard_output,
        standard_error,
        TASK_STATUS_IN_PROGRESS,
    )
}

fn run_orchestration_task_complete(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    update_existing_task(
        "orchestration task complete",
        arguments,
        standard_output,
        standard_error,
        TASK_STATUS_DONE,
    )
}

/// Shared update path for `progress` and `complete`: load the record by id,
/// apply the new status plus any supplied fields, and stamp `updatedAt`
/// (and `completedAt` when transitioning to done).
fn update_existing_task(
    command_label: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    new_status: &str,
) -> u8 {
    let mut flag_set = FlagSet::new(command_label.to_string());
    flag_set.string_flag("id", "");
    flag_set.string_flag("phase", "");
    flag_set.string_flag("next-step", "");
    flag_set.string_flag("skills", "");
    flag_set.string_flag("note", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let id = flag_set.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(
            standard_error,
            "{command_label}: --id is required (the task id from `orchestration task begin`)"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{command_label}: {error}");
            return 1;
        }
    };
    let store = task_store(&claude_home);
    let mut record = match store.read_record(&id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "{command_label}: no task with id {id}");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{command_label}: {error}");
            return 1;
        }
    };
    let now_millis = current_timestamp_millis();
    let timestamp = format_timestamp_iso8601(now_millis);
    set_field(&mut record, "status", new_status.to_string());
    set_field(&mut record, "updatedAt", timestamp.clone());
    if new_status == TASK_STATUS_DONE {
        set_field(&mut record, "completedAt", timestamp);
    }
    for (flag, key) in [
        ("phase", "phase"),
        ("next-step", "nextStep"),
        ("skills", "skills"),
        ("note", "note"),
    ] {
        let value = flag_set.string_value(flag).trim().to_string();
        if !value.is_empty() {
            set_field(&mut record, key, value);
        }
    }
    let path = match store.write_record(&id, &record) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{command_label}: {error}");
            return 1;
        }
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("updated".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&path))),
            ("task".into(), record_to_value(&record)),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "{command_label}: id={id}");
    let _ = writeln!(
        standard_output,
        "  status: {}",
        field(&record, "status").unwrap_or(new_status)
    );
    let _ = writeln!(standard_output, "  ledger: {}", display_path(&path));
    0
}

fn run_orchestration_task_list(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("orchestration task list");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("open-only", false);
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration task list: {error}");
            return 1;
        }
    };
    let store = task_store(&claude_home);
    let records = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration task list: {error}");
            return 1;
        }
    };
    let open_only = flag_set.bool_value("open-only");
    let selected: Vec<&Record> = records
        .iter()
        .map(|(_, record)| record)
        .filter(|record| {
            if !open_only {
                return true;
            }
            !matches!(field(record, "status"), Some(TASK_STATUS_DONE))
        })
        .collect();
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("count".into(), Value::Number(selected.len().to_string())),
            (
                "tasks".into(),
                Value::Array(
                    selected
                        .iter()
                        .map(|record| record_to_value(record))
                        .collect(),
                ),
            ),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "orchestration task list: {} task(s)",
        selected.len()
    );
    for record in &selected {
        let _ = writeln!(
            standard_output,
            "  {} [{}] {}",
            field(record, "id").unwrap_or("?"),
            field(record, "status").unwrap_or("?"),
            field(record, "task").unwrap_or("")
        );
    }
    0
}

fn set_field(record: &mut Record, key: &str, value: String) {
    if let Some(slot) = record.iter_mut().find(|(field_key, _)| field_key == key) {
        slot.1 = value;
    } else {
        record.push((key.to_string(), value));
    }
}

/// Checkpoint refreshes the durable artifacts that survive compaction, then
/// reports their current state.
fn run_orchestration_checkpoint(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("orchestration checkpoint");
    flag_set.string_flag("note", "");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration checkpoint: {error}");
            return 1;
        }
    };
    let open_tasks = task_store(&claude_home)
        .list_records()
        .map(|records| {
            records
                .into_iter()
                .filter(|(_, record)| !matches!(field(record, "status"), Some(TASK_STATUS_DONE)))
                .count()
        })
        .unwrap_or(0);
    let open_workflows = list_entries(&claude_home)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry.status == STATUS_OPEN)
                .count()
        })
        .unwrap_or(0);
    let brief_count = list_briefs(&claude_home)
        .map(|briefs| briefs.len())
        .unwrap_or(0);
    let note = flag_set.string_value("note").trim().to_string();

    let now_millis = current_timestamp_millis();
    let checkpoint_store = RecordStore::new(&claude_home, "orchestration/checkpoints");
    let checkpoint_id =
        allocate_unique_record_id(&checkpoint_store, &format!("checkpoint-{now_millis:x}"));
    let checkpoint_record: Record = vec![
        ("id".into(), checkpoint_id.clone()),
        ("at".into(), format_timestamp_iso8601(now_millis)),
        ("note".into(), note.clone()),
        ("openTasks".into(), open_tasks.to_string()),
        ("openWorkflows".into(), open_workflows.to_string()),
        ("workingBriefs".into(), brief_count.to_string()),
    ];
    let checkpoint_path = match checkpoint_store.write_record(&checkpoint_id, &checkpoint_record) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "orchestration checkpoint: {error}");
            return 1;
        }
    };

    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("checkpointed".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(&checkpoint_path))),
            ("snapshot".into(), record_to_value(&checkpoint_record)),
        ]);
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "orchestration checkpoint: {checkpoint_id}");
    if !note.is_empty() {
        let _ = writeln!(standard_output, "  note: {note}");
    }
    let _ = writeln!(standard_output, "  open tasks: {open_tasks}");
    let _ = writeln!(standard_output, "  open workflows: {open_workflows}");
    let _ = writeln!(standard_output, "  working briefs: {brief_count}");
    let _ = writeln!(
        standard_output,
        "  saved: {}",
        display_path(&checkpoint_path)
    );
    0
}
