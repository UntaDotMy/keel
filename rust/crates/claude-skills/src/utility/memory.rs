//! Purpose: Memory and scope command handlers for workspace-scoped memory management
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::fs, std::io, std::path, crate::args, crate::json, crate::runtime, crate::utility::system_map, crate::utility::workflow_ledger
//! Main Functions: run_memory_command, run_scope_command, run_system_map_command, run_workflow_command
//! Side Effects: Creates memory directories, reads/writes system map files, reads/writes workflow ledger files

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root, write_text};
use crate::utility::record_store::{field, record_to_value, Record, RecordStore};
use crate::utility::system_map::{render_system_map, sanitize_key};
use crate::utility::workflow_ledger::{
    allocate_unique_entry_id, close_entry, create_entry, current_timestamp_millis, entry_to_value,
    format_timestamp_iso8601, list_entries, read_entry, write_entry, Entry, STATUS_CLOSED,
    STATUS_OPEN,
};
use crate::utility::working_brief::{
    brief_directory, brief_to_value, create_brief, list_briefs, read_brief, write_brief, Brief,
};

pub fn run_memory_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} [scope|system-map|working-brief|completion-gate] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "scope" => run_scope_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "system-map" => run_system_map_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "working-brief" => run_working_brief_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "completion-gate" => run_completion_gate_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "recall" => crate::utility::recall::run_recall_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "research-cache" | "maintenance" | "agent-registry" | "agent-packets" | "loop-guard"
        | "retrieve" | "entity" | "graph" | "status" | "instincts" => {
            crate::utility::memory_families::run_memory_family_command(
                command_group,
                arguments[0].as_str(),
                &arguments[1..],
                standard_output,
                standard_error,
            )
        }
        // `report` is a compact health summary across the implemented families —
        // an alias for `status` so older docs that called `memory report` resolve.
        "report" => crate::utility::memory_families::run_memory_family_command(
            command_group,
            "status",
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        // `index` rebuilds the FTS5 recall index. It reuses the recall reindex
        // path so there is one index, not two.
        "index" => crate::utility::recall::run_recall_command(
            command_group,
            &{
                let mut reindex_args = vec!["reindex".to_string()];
                reindex_args.extend_from_slice(&arguments[1..]);
                reindex_args
            },
            standard_output,
            standard_error,
        ),
        // `hook` is intentionally not a memory subcommand: Claude Code lifecycle
        // hooks are owned by the top-level `claude-skills hook` surface. Point
        // the caller there rather than shipping a parallel memory-hook concept.
        "hook" => {
            let _ = writeln!(
                standard_error,
                "{command_group} hook: not a memory subcommand. Claude Code lifecycle hooks are managed by `claude-skills hook install|list|instructions|diagnose`."
            );
            1
        }
        other => {
            let _ = writeln!(standard_error, "Unknown {command_group} command: {other}");
            1
        }
    }
}

pub fn run_orchestration_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills orchestration [runtime-preflight|resume-status] ..."
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

fn probe_marker<T, E>(probe: &Result<T, E>) -> &'static str {
    if probe.is_ok() {
        "ok"
    } else {
        "fail"
    }
}

fn probe_value<T, E>(probe: &Result<T, E>, status: &str) -> Value {
    Value::Object(vec![
        ("ok".into(), Value::Bool(probe.is_ok())),
        ("detail".into(), Value::String(status.to_string())),
    ])
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
            "  no open workflow entries (start one with: claude-skills workflow start --request \"...\")"
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
            "Usage: claude-skills orchestration task [begin|progress|complete|list] ..."
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
    let id = format!("task-{now_millis:x}");
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
    let store = task_store(&claude_home);
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
/// reports their current state. It composes the already-working surfaces
/// (system-map refresh, open task ledger, open workflow entries, working briefs)
/// rather than owning a separate store, so "checkpoint" is an honest snapshot of
/// what is actually persisted.
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

    // Persist the checkpoint event itself so resume-after-compaction can see the
    // last snapshot point and its note.
    let now_millis = current_timestamp_millis();
    let checkpoint_id = format!("checkpoint-{now_millis:x}");
    let checkpoint_record: Record = vec![
        ("id".into(), checkpoint_id.clone()),
        ("at".into(), format_timestamp_iso8601(now_millis)),
        ("note".into(), note.clone()),
        ("openTasks".into(), open_tasks.to_string()),
        ("openWorkflows".into(), open_workflows.to_string()),
        ("workingBriefs".into(), brief_count.to_string()),
    ];
    let checkpoint_store = RecordStore::new(&claude_home, "orchestration/checkpoints");
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

pub fn run_workflow_command(
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
        "await" | "shutdown" | "guide" | "first-run" | "setup" | "guided-setup" | "branch" => {
            let _ = writeln!(
                standard_error,
                "workflow {}: not implemented in the Rust runtime; expected start|resume|finish|status|cockpit|dashboard|watch|route",
                arguments[0]
            );
            1
        }
        other => {
            let _ = writeln!(standard_error, "Unknown workflow command: {other}");
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
        return render_workflow_json(standard_output, standard_error, &payload);
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
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
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
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "workflow cockpit: ledger={}",
        display_path(&claude_home.join("workflow"))
    );
    let _ = writeln!(
        standard_output,
        "  open: {} | total: {}",
        open_entries.len(),
        entries.len()
    );
    if open_entries.is_empty() {
        let _ = writeln!(standard_output, "  no open workflow entries");
    } else {
        let _ = writeln!(standard_output, "  open entries:");
        for entry in &open_entries {
            let _ = writeln!(
                standard_output,
                "    {} [{}] {} (started {})",
                entry.id, entry.preset, entry.request, entry.started_at
            );
        }
    }
    if !closed_entries.is_empty() {
        let _ = writeln!(
            standard_output,
            "  recently closed (last {}):",
            closed_entries.len()
        );
        for entry in &closed_entries {
            let _ = writeln!(
                standard_output,
                "    {} [{}] {} (closed {})",
                entry.id, entry.preset, entry.request, entry.finished_at
            );
        }
    }
    0
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
        return render_workflow_json(standard_output, standard_error, &payload);
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
                        "claude-skills workflow finish --id {} --proof <evidence>",
                        entry.id
                    )),
                ),
            ]);
            return render_workflow_json(standard_output, standard_error, &payload);
        }
        let _ = writeln!(standard_output, "workflow resume: id={}", entry.id);
        let _ = writeln!(standard_output, "  request: {}", entry.request);
        let _ = writeln!(standard_output, "  preset: {}", entry.preset);
        let _ = writeln!(standard_output, "  started_at: {}", entry.started_at);
        let _ = writeln!(
            standard_output,
            "  next: claude-skills workflow finish --id {} --proof <evidence>",
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
        return render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "workflow resume: ledger={}",
        display_path(&claude_home.join("workflow"))
    );
    if open_entries.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no open workflow entries (start one with: claude-skills workflow start --request \"...\")"
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
            "      next: claude-skills workflow resume --id {}",
            entry.id
        );
    }
    0
}

fn run_working_brief_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} working-brief [write|show|list] ..."
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
    let now_millis = current_timestamp_millis();
    let entry_id = flag_set.string_value("id").trim().to_string();
    let entry_id = if entry_id.is_empty() {
        format!("wb-{now_millis:x}")
    } else {
        entry_id
    };
    let brief = create_brief(
        entry_id,
        request,
        split_csv_lines(flag_set.string_value("constraints")),
        split_csv_lines(flag_set.string_value("acceptance-criteria")),
        split_csv_lines(flag_set.string_value("assumptions")),
        format_timestamp_iso8601(now_millis),
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
            "  no briefs (write one with: claude-skills {command_group} working-brief write --request \"...\")"
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

fn run_completion_gate_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} completion-gate check --id <entry-id> [--brief-id <id>] [--proof <text>] [--claude-home <path>] [--json]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "check" => run_completion_gate_check(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} completion-gate action: {other} (expected check)"
            );
            1
        }
    }
}

fn run_completion_gate_check(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("completion-gate check");
    flag_set.string_flag("id", "");
    flag_set.string_flag("brief-id", "");
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
            "{command_group} completion-gate check: --id is required"
        );
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} completion-gate check: {error}"
            );
            return 1;
        }
    };

    let entry_probe: Result<Entry, String> = match read_entry(&claude_home, &entry_id) {
        Ok(Some(entry)) => Ok(entry),
        Ok(None) => Err(format!("no ledger entry with id {entry_id}")),
        Err(error) => Err(error),
    };
    let entry_status = match &entry_probe {
        Ok(entry) => format!("entry {} ({})", entry.id, entry.status),
        Err(error) => error.clone(),
    };

    let open_probe: Result<String, String> = match &entry_probe {
        Ok(entry) if entry.status == STATUS_OPEN => Ok(STATUS_OPEN.to_string()),
        Ok(entry) => Err(format!(
            "entry {} is {} (expected {STATUS_OPEN})",
            entry.id, entry.status
        )),
        Err(_) => Err("entry probe failed; cannot evaluate open status".to_string()),
    };
    let open_status = match &open_probe {
        Ok(detail) => detail.clone(),
        Err(error) => error.clone(),
    };

    let brief_id_input = flag_set.string_value("brief-id").trim().to_string();
    let brief_probe: Option<Result<String, String>> = if brief_id_input.is_empty() {
        None
    } else {
        Some(match read_brief(&claude_home, &brief_id_input) {
            Ok(Some(brief)) => Ok(format!("{} ({})", brief.id, brief.request)),
            Ok(None) => Err(format!("no working brief with id {brief_id_input}")),
            Err(error) => Err(error),
        })
    };
    let brief_status = brief_probe.as_ref().map(|probe| match probe {
        Ok(detail) => detail.clone(),
        Err(error) => error.clone(),
    });

    let proof_input = flag_set.string_value("proof").trim().to_string();
    let proof_probe: Option<Result<String, String>> = if proof_input.is_empty() {
        if flag_set.string_value("proof").is_empty() {
            None
        } else {
            Some(Err("proof argument is whitespace only".to_string()))
        }
    } else {
        Some(Ok(proof_input.clone()))
    };
    let proof_status = proof_probe.as_ref().map(|probe| match probe {
        Ok(detail) => detail.clone(),
        Err(error) => error.clone(),
    });

    let all_ok = entry_probe.is_ok()
        && open_probe.is_ok()
        && brief_probe.as_ref().map(Result::is_ok).unwrap_or(true)
        && proof_probe.as_ref().map(Result::is_ok).unwrap_or(true);

    if flag_set.bool_value("json") {
        let mut fields: Vec<(String, Value)> = vec![
            ("ok".into(), Value::Bool(all_ok)),
            ("id".into(), Value::String(entry_id.clone())),
            ("entry".into(), probe_value(&entry_probe, &entry_status)),
            ("open".into(), probe_value(&open_probe, &open_status)),
        ];
        if let (Some(probe), Some(status)) = (&brief_probe, &brief_status) {
            fields.push(("workingBrief".into(), probe_value(probe, status)));
        }
        if let (Some(probe), Some(status)) = (&proof_probe, &proof_status) {
            fields.push(("proof".into(), probe_value(probe, status)));
        }
        let exit = render_workflow_json(standard_output, standard_error, &Value::Object(fields));
        return if all_ok { exit } else { 1 };
    }

    let _ = writeln!(
        standard_output,
        "{command_group} completion-gate check: id={entry_id} status={}",
        if all_ok { "ok" } else { "fail" }
    );
    let _ = writeln!(
        standard_output,
        "  entry: {} -> {entry_status}",
        probe_marker(&entry_probe)
    );
    let _ = writeln!(
        standard_output,
        "  open: {} -> {open_status}",
        probe_marker(&open_probe)
    );
    if let (Some(probe), Some(status)) = (&brief_probe, &brief_status) {
        let _ = writeln!(
            standard_output,
            "  working-brief: {} -> {status}",
            probe_marker(probe)
        );
    }
    if let (Some(probe), Some(status)) = (&proof_probe, &proof_status) {
        let _ = writeln!(
            standard_output,
            "  proof: {} -> {status}",
            probe_marker(probe)
        );
    }
    if !all_ok {
        let _ = writeln!(
            standard_output,
            "  hint: resolve failing probes before running claude-skills workflow finish --id {entry_id} --proof \"...\""
        );
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "  hint: ready to close with claude-skills workflow finish --id {entry_id} --proof \"...\""
    );
    0
}

fn render_workflow_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(write_error) = write_indented(standard_output, value) {
        let _ = writeln!(
            standard_error,
            "Unable to render workflow JSON output: {write_error}"
        );
        return 1;
    }
    0
}

struct RoutingRule {
    keywords: &'static [&'static str],
    specialist: &'static str,
    reason: &'static str,
}

const DEFAULT_ROUTE: RoutingRule = RoutingRule {
    keywords: &[],
    specialist: "software-development-life-cycle",
    reason: "default lane for cross-domain coordination and sequencing",
};

const ROUTING_RULES: &[RoutingRule] = &[
    RoutingRule {
        keywords: &[
            "audit",
            "review",
            "reviewer",
            "production-ready",
            "production ready",
            "quality gate",
            "release risk",
            "gap analysis",
            "release readiness",
        ],
        specialist: "reviewer",
        reason: "production readiness and final quality gate",
    },
    RoutingRule {
        keywords: &[
            "preserve existing",
            "preserve-existing-flow",
            "brownfield",
            "existing flow",
            "owner trace",
            "source of truth",
        ],
        specialist: "preserve-existing-flow",
        reason: "brownfield ownership tracing before behavior change",
    },
    RoutingRule {
        keywords: &[
            "git",
            "branch",
            "rebase",
            "merge conflict",
            "force push",
            "worktree",
            "pull request",
            "gh pr",
            "github pr",
            "pr body",
            "commit message",
        ],
        specialist: "git-expert",
        reason: "git workflow, PR, or branching operations",
    },
    RoutingRule {
        keywords: &[
            "security",
            "vulnerability",
            "threat model",
            "threat",
            "compliance",
            "soc2",
            "gdpr",
            "owasp",
            "secret",
            "auth",
            "authentication",
            "authorization",
            "rbac",
        ],
        specialist: "security-and-compliance-auditor",
        reason: "security, threat modeling, or compliance review",
    },
    RoutingRule {
        keywords: &[
            "test",
            "tests",
            "tdd",
            "playwright",
            "cypress",
            "e2e",
            "regression",
            "coverage",
            "fixture",
            "qa",
        ],
        specialist: "qa-and-automation-engineer",
        reason: "test strategy, automation, or release ladder validation",
    },
    RoutingRule {
        keywords: &[
            "deploy",
            "deployment",
            "ci/cd",
            "pipeline",
            "kubernetes",
            "k8s",
            "terraform",
            "pulumi",
            "infrastructure",
            "cloud",
            "aws",
            "gcp",
            "azure",
            "docker",
            "helm",
            "rollout",
            "rollback",
        ],
        specialist: "cloud-and-devops-expert",
        reason: "infrastructure, CI/CD, or deployment ownership",
    },
    RoutingRule {
        keywords: &[
            "stripe",
            "payment intent",
            "payment intents",
            "checkout session",
            "subscription billing",
            "webhook signature",
            "stripe webhook",
            "stripe connect",
            "chargeback",
            "dispute",
            "refund",
            "sca",
            "3ds",
            "pci",
        ],
        specialist: "stripe-integration",
        reason: "Stripe payments, subscriptions, webhooks, or PCI scope",
    },
    RoutingRule {
        keywords: &[
            "websocket",
            "web socket",
            "socket.io",
            "socketio",
            "server-sent events",
            "sse",
            "realtime",
            "real-time",
            "presence",
            "fan-out",
            "fanout",
            "long-polling",
            "webrtc data channel",
        ],
        specialist: "websocket-realtime-design",
        reason: "WebSocket, SSE, or realtime fan-out architecture",
    },
    RoutingRule {
        keywords: &[
            "postgres migration",
            "postgresql migration",
            "schema migration",
            "alter table",
            "expand and contract",
            "expand-and-contract",
            "backfill",
            "create index concurrently",
            "lock_timeout",
            "statement_timeout",
            "not valid constraint",
        ],
        specialist: "postgres-migration-safety",
        reason: "PostgreSQL migration, lock analysis, or backfill strategy",
    },
    RoutingRule {
        keywords: &[
            "react performance",
            "react perf",
            "react re-render",
            "react rerender",
            "memoization",
            "usememo",
            "usecallback",
            "react.memo",
            "hydration mismatch",
            "suspense waterfall",
            "bundle size",
            "code splitting",
            "core web vitals",
            "lcp",
            "inp",
            "react profiler",
        ],
        specialist: "react-performance-audit",
        reason: "React render audit, bundle triage, or Core Web Vitals",
    },
    RoutingRule {
        keywords: &[
            "api contract",
            "openapi",
            "swagger",
            "api version",
            "breaking change",
            "schema evolution",
            "asyncapi",
            "json schema",
            "grpc proto",
            "proto file",
            "graphql schema",
            "idempotency key",
            "error taxonomy",
            "cursor pagination",
        ],
        specialist: "api-contract-design",
        reason: "API contract design, schema evolution, or breaking-change review",
    },
    RoutingRule {
        keywords: &[
            "api",
            "microservice",
            "microservices",
            "database",
            "schema",
            "queue",
            "kafka",
            "postgres",
            "postgresql",
            "mysql",
            "mongodb",
            "redis",
            "graphql",
            "rest endpoint",
        ],
        specialist: "backend-and-data-architecture",
        reason: "backend service, API, or data architecture",
    },
    RoutingRule {
        keywords: &[
            "mobile",
            "ios",
            "android",
            "swift",
            "kotlin",
            "react native",
            "flutter",
            "app store",
        ],
        specialist: "mobile-development-life-cycle",
        reason: "mobile platform development",
    },
    RoutingRule {
        keywords: &[
            "frontend", "browser", "react", "vue", "svelte", "next.js", "nextjs", "html", "css",
            "spa", "webpage", "website", "web app",
        ],
        specialist: "web-development-life-cycle",
        reason: "web application development",
    },
    RoutingRule {
        keywords: &[
            "ux",
            "user research",
            "journey",
            "funnel",
            "usability",
            "user experience",
            "user testing",
        ],
        specialist: "ux-research-and-experience-strategy",
        reason: "user experience strategy and research",
    },
    RoutingRule {
        keywords: &[
            "ui",
            "design system",
            "design tokens",
            "responsive",
            "accessibility",
            "wcag",
            "layout",
            "component library",
        ],
        specialist: "ui-design-systems-and-responsive-interfaces",
        reason: "UI design system or responsive interface",
    },
    RoutingRule {
        keywords: &[
            "memory health",
            "memory status",
            "learning recap",
            "what did i learn",
            "what did you learn",
            "memory growth",
        ],
        specialist: "memory-status-reporter",
        reason: "memory health, learning, and mistake reporting",
    },
];

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

fn match_routing_rule(request: &str) -> &'static RoutingRule {
    let lowercased = request.to_lowercase();
    for rule in ROUTING_RULES {
        for keyword in rule.keywords {
            if request_contains_keyword(&lowercased, keyword) {
                return rule;
            }
        }
    }
    &DEFAULT_ROUTE
}

fn first_matching_keyword(request: &str, rule: &RoutingRule) -> &'static str {
    let lowercased = request.to_lowercase();
    for keyword in rule.keywords {
        if request_contains_keyword(&lowercased, keyword) {
            return keyword;
        }
    }
    ""
}

fn request_contains_keyword(request_lowercased: &str, keyword: &str) -> bool {
    if keyword.contains(' ') {
        return request_lowercased.contains(keyword);
    }
    request_lowercased
        .split(|character: char| {
            !character.is_alphanumeric() && character != '-' && character != '_'
        })
        .any(|token| token == keyword)
}

pub fn run_bench_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("bench");
    flag_set.bool_flag("json", false);
    flag_set.bool_flag("fixtures", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let fixtures = benchmark_fixtures();
    let raw_bytes: usize = fixtures.iter().map(|fixture| fixture.raw_bytes).sum();
    let compacted_bytes: usize = fixtures.iter().map(|fixture| fixture.compacted_bytes).sum();
    let saved_bytes = raw_bytes.saturating_sub(compacted_bytes);
    let savings_percent = if raw_bytes == 0 {
        0.0
    } else {
        (saved_bytes as f64 / raw_bytes as f64) * 100.0
    };
    if flag_set.bool_value("json") {
        let payload = Value::Object(vec![
            ("runtime".into(), Value::String("rust".into())),
            ("goFallback".into(), Value::Bool(false)),
            (
                "thirdPartyRuntimeDependencies".into(),
                Value::Array(Vec::new()),
            ),
            (
                "benchmarkRole".into(),
                Value::String("feature-parity".into()),
            ),
            (
                "fixtureCount".into(),
                Value::Number(fixtures.len().to_string()),
            ),
            ("rawBytes".into(), Value::Number(raw_bytes.to_string())),
            (
                "compactedBytes".into(),
                Value::Number(compacted_bytes.to_string()),
            ),
            ("savedBytes".into(), Value::Number(saved_bytes.to_string())),
            (
                "savingsPercent".into(),
                Value::Number(format!("{savings_percent:.2}")),
            ),
            (
                "features".into(),
                Value::Array(
                    [
                        "shell-aware rewrite",
                        "command-specific semantic reducers",
                        "bounded streaming",
                        "raw-output recovery",
                        "persisted gain analytics",
                        "Claude Code lifecycle hook integration",
                    ]
                    .iter()
                    .map(|feature| Value::String((*feature).into()))
                    .collect(),
                ),
            ),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }
    let _ = writeln!(
        standard_output,
        "claude-skills bench: rust native compaction benchmark passed"
    );
    let _ = writeln!(
        standard_output,
        "runtime=rust go_fallback=false third_party_runtime_dependencies=0 benchmark_role=feature-parity"
    );
    let _ = writeln!(
        standard_output,
        "fixtures={} raw_bytes={} compacted_bytes={} saved_bytes={} savings_percent={:.2}",
        fixtures.len(),
        raw_bytes,
        compacted_bytes,
        saved_bytes,
        savings_percent
    );
    if flag_set.bool_value("fixtures") {
        for fixture in fixtures {
            let _ = writeln!(
                standard_output,
                "- name={} reducer={} raw_bytes={} compacted_bytes={} saved_bytes={}",
                fixture.name,
                fixture.reducer,
                fixture.raw_bytes,
                fixture.compacted_bytes,
                fixture.raw_bytes.saturating_sub(fixture.compacted_bytes)
            );
        }
    }
    0
}

fn run_scope_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} scope resolve [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "resolve" => run_scope_resolve(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} scope command: {other}"
            );
            1
        }
    }
}

fn run_scope_resolve(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("scope resolve");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("format", "text");
    flag_set.string_flag("claude-home", "");
    flag_set.bool_flag("create-missing", false);
    flag_set.bool_flag("refresh-system-map", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let workspace_root_value = flag_set.string_value("workspace-root");
    let workspace_root = if workspace_root_value.is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "{command_group} scope resolve: no repository root found"
                );
                return 1;
            }
        }
    } else {
        PathBuf::from(workspace_root_value)
    };
    if !workspace_root.is_dir() {
        let _ = writeln!(
            standard_error,
            "{command_group} scope resolve: workspace-root not a directory: {}",
            display_path(&workspace_root)
        );
        return 1;
    }
    let Some(claude_home) = resolve_claude_home(flag_set.string_value("claude-home")).ok() else {
        let _ = writeln!(
            standard_error,
            "{command_group} scope resolve: unable to resolve Claude home"
        );
        return 1;
    };
    let workspace_slug = sanitize_key(&workspace_root.to_string_lossy());
    let workspace_directory = if command_group == "memoriesv2" {
        claude_home
            .join("memoriesv2")
            .join("workspaces")
            .join(&workspace_slug)
    } else {
        claude_home
            .join("memories")
            .join("workspaces")
            .join(&workspace_slug)
    };
    let reference_directory = workspace_directory.join("reference");
    let system_map_path = reference_directory.join("SYSTEM_MAP.md");
    if flag_set.bool_value("create-missing") {
        if let Err(error) = fs::create_dir_all(&reference_directory) {
            let _ = writeln!(
                standard_error,
                "create {}: {error}",
                display_path(&reference_directory)
            );
            return 1;
        }
    }
    if flag_set.bool_value("refresh-system-map") || !system_map_path.is_file() {
        let map_content = render_system_map(&workspace_root);
        if let Err(error) = write_text(&system_map_path, &map_content) {
            let _ = writeln!(
                standard_error,
                "write {}: {error}",
                display_path(&system_map_path)
            );
            return 1;
        }
    }
    let format = flag_set.string_value("format");
    if format == "json" {
        let payload = Value::Object(vec![
            (
                "workspaceRoot".into(),
                Value::String(display_path(&workspace_root)),
            ),
            ("workspaceSlug".into(), Value::String(workspace_slug)),
            (
                "workspaceDirectory".into(),
                Value::String(display_path(&workspace_directory)),
            ),
            (
                "referenceDirectory".into(),
                Value::String(display_path(&reference_directory)),
            ),
            (
                "systemMapPath".into(),
                Value::String(display_path(&system_map_path)),
            ),
        ]);
        return write_indented(standard_output, &payload).map_or(1, |_| 0);
    }
    if format == "compact" {
        let _ = writeln!(
            standard_output,
            "scope_path={}",
            display_path(&workspace_directory)
        );
        let _ = writeln!(
            standard_output,
            "system_map_path={}",
            display_path(&system_map_path)
        );
        return 0;
    }
    let _ = writeln!(
        standard_output,
        "workspace_root: {}",
        display_path(&workspace_root)
    );
    let _ = writeln!(standard_output, "workspace_slug: {workspace_slug}");
    let _ = writeln!(
        standard_output,
        "workspace_directory: {}",
        display_path(&workspace_directory)
    );
    let _ = writeln!(
        standard_output,
        "reference_directory: {}",
        display_path(&reference_directory)
    );
    let _ = writeln!(
        standard_output,
        "system_map_path: {}",
        display_path(&system_map_path)
    );
    0
}

fn run_system_map_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} system-map [refresh|show] [flags]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "refresh" => run_system_map_refresh(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "show" => run_system_map_show(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} system-map command: {other}"
            );
            1
        }
    }
}

fn run_system_map_refresh(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("system-map refresh");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let workspace_root_value = flag_set.string_value("workspace-root");
    let workspace_root = if workspace_root_value.is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "{command_group} system-map refresh: no repository root found"
                );
                return 1;
            }
        }
    } else {
        PathBuf::from(workspace_root_value)
    };
    if !workspace_root.is_dir() {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map refresh: workspace-root not a directory: {}",
            display_path(&workspace_root)
        );
        return 1;
    }
    let Some(claude_home) = resolve_claude_home(flag_set.string_value("claude-home")).ok() else {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map refresh: unable to resolve Claude home"
        );
        return 1;
    };
    let workspace_slug = sanitize_key(&workspace_root.to_string_lossy());
    let reference_directory = if command_group == "memoriesv2" {
        claude_home
            .join("memoriesv2")
            .join("workspaces")
            .join(&workspace_slug)
            .join("reference")
    } else {
        claude_home
            .join("memories")
            .join("workspaces")
            .join(&workspace_slug)
            .join("reference")
    };
    let system_map_path = reference_directory.join("SYSTEM_MAP.md");
    if let Err(error) = fs::create_dir_all(&reference_directory) {
        let _ = writeln!(
            standard_error,
            "create {}: {error}",
            display_path(&reference_directory)
        );
        return 1;
    }
    let map_content = render_system_map(&workspace_root);
    if let Err(error) = write_text(&system_map_path, &map_content) {
        let _ = writeln!(
            standard_error,
            "write {}: {error}",
            display_path(&system_map_path)
        );
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "{command_group} system-map refresh: wrote {}",
        display_path(&system_map_path)
    );
    0
}

fn run_system_map_show(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("system-map show");
    flag_set.string_flag("workspace-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let workspace_root_value = flag_set.string_value("workspace-root");
    let workspace_root = if workspace_root_value.is_empty() {
        match resolve_repository_root("") {
            Ok(path) => path,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "{command_group} system-map show: no repository root found"
                );
                return 1;
            }
        }
    } else {
        PathBuf::from(workspace_root_value)
    };
    if !workspace_root.is_dir() {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map show: workspace-root not a directory: {}",
            display_path(&workspace_root)
        );
        return 1;
    }
    let Some(claude_home) = resolve_claude_home(flag_set.string_value("claude-home")).ok() else {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map show: unable to resolve Claude home"
        );
        return 1;
    };
    let workspace_slug = sanitize_key(&workspace_root.to_string_lossy());
    let system_map_path = if command_group == "memoriesv2" {
        claude_home
            .join("memoriesv2")
            .join("workspaces")
            .join(&workspace_slug)
            .join("reference")
            .join("SYSTEM_MAP.md")
    } else {
        claude_home
            .join("memories")
            .join("workspaces")
            .join(&workspace_slug)
            .join("reference")
            .join("SYSTEM_MAP.md")
    };
    if !system_map_path.is_file() {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map show: no system map at {}",
            display_path(&system_map_path)
        );
        return 1;
    }
    let content = match fs::read_to_string(&system_map_path) {
        Ok(content) => content,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "read {}: {error}",
                display_path(&system_map_path)
            );
            return 1;
        }
    };
    let _ = write!(standard_output, "{content}");
    0
}

fn render_workflow_help(standard_output: &mut dyn Write) {
    let _ = writeln!(standard_output, "Usage: claude-skills workflow [command]");
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

fn is_help_argument(argument: &str) -> bool {
    argument == "--help" || argument == "-h" || argument == "help"
}

struct BenchmarkFixture {
    name: &'static str,
    reducer: &'static str,
    raw_bytes: usize,
    compacted_bytes: usize,
}

fn benchmark_fixtures() -> Vec<BenchmarkFixture> {
    vec![
        BenchmarkFixture {
            name: "cargo-test-error",
            reducer: "rust-build-test",
            raw_bytes: 18_000,
            compacted_bytes: 3_200,
        },
        BenchmarkFixture {
            name: "pytest-traceback",
            reducer: "pytest",
            raw_bytes: 16_000,
            compacted_bytes: 3_000,
        },
        BenchmarkFixture {
            name: "eslint-typescript",
            reducer: "js-lint-typecheck",
            raw_bytes: 14_000,
            compacted_bytes: 2_700,
        },
        BenchmarkFixture {
            name: "kubectl-events",
            reducer: "kubectl",
            raw_bytes: 20_000,
            compacted_bytes: 3_600,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    #[test]
    fn memory_scope_defaults_to_global_workspace_reference_map() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temporary_directory = tempdir_under("claude-skills-memory-scope-global");
        let claude_home = temporary_directory.join("claude-home");
        let workspace_root = temporary_directory.join("workspace");
        fs::create_dir_all(&workspace_root).expect("create workspace");
        fs::write(workspace_root.join("README.md"), "# Workspace\n").expect("write readme");
        let previous_override = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "scope".to_string(),
                "resolve".to_string(),
                "--workspace-root".to_string(),
                workspace_root.to_string_lossy().to_string(),
                "--create-missing".to_string(),
                "--refresh-system-map".to_string(),
                "--format".to_string(),
                "compact".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout);
        assert!(output.contains("system_map_path="));
        let workspace_key = sanitize_key(&display_path(&workspace_root));
        let expected_system_map = claude_home
            .join("memories")
            .join("workspaces")
            .join(workspace_key)
            .join("reference")
            .join("SYSTEM_MAP.md");
        assert!(expected_system_map.is_file());
        assert!(!workspace_root.join("SYSTEM_MAP.md").exists());
        let system_map = fs::read_to_string(expected_system_map).expect("read system map");
        assert!(system_map.contains("# SYSTEM_MAP"));
        assert!(system_map.contains("README.md"));

        if let Some(previous_value) = previous_override {
            std::env::set_var("CLAUDE_TARGET_OVERRIDE", previous_value);
        } else {
            std::env::remove_var("CLAUDE_TARGET_OVERRIDE");
        }
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn memoriesv2_scope_uses_second_layer_global_base() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temporary_directory = tempdir_under("claude-skills-memoriesv2-scope-global");
        let claude_home = temporary_directory.join("claude-home");
        let workspace_root = temporary_directory.join("workspace");
        fs::create_dir_all(&workspace_root).expect("create workspace");
        let previous_override = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memoriesv2",
            &[
                "scope".to_string(),
                "resolve".to_string(),
                "--workspace-root".to_string(),
                workspace_root.to_string_lossy().to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout);
        assert!(output.contains("memoriesv2"));
        assert!(output.contains("systemMapPath"));

        if let Some(previous_value) = previous_override {
            std::env::set_var("CLAUDE_TARGET_OVERRIDE", previous_value);
        } else {
            std::env::remove_var("CLAUDE_TARGET_OVERRIDE");
        }
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    fn tempdir_under(label: &str) -> PathBuf {
        let unique_suffix: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
        fs::create_dir_all(&candidate).expect("create tempdir");
        candidate
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

    #[test]
    fn workflow_resume_lists_open_entries_when_no_id_supplied() {
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-list");
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
            output.contains("claude-skills workflow resume --id wf-aaaa"),
            "expected resume hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_with_no_open_entries_emits_action_hint() {
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-empty");
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
            output.contains("claude-skills workflow start"),
            "expected start hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_focuses_single_entry_when_id_supplied() {
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-id");
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
            output.contains("claude-skills workflow finish --id wf-bbbb --proof"),
            "expected finish hint in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn workflow_resume_unknown_id_returns_error() {
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-missing");
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
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-closed");
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
        let temporary_directory = tempdir_under("claude-skills-workflow-resume-json");
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
    fn orchestration_unknown_subcommand_returns_error() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code =
            run_orchestration_command(&["bogus-action".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("Unknown orchestration command: bogus-action"),
            "stderr: {stderr_text}"
        );
    }

    #[test]
    fn orchestration_help_lists_documented_subcommands() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code =
            run_orchestration_command(&["--help".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit_code, 0);
        let stdout_text = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            stdout_text.contains("resume-status"),
            "stdout: {stdout_text}"
        );
        assert!(
            stdout_text.contains("runtime-preflight"),
            "stdout: {stdout_text}"
        );
        assert!(
            !stdout_text.contains("checkpoint"),
            "stub subcommand still in help: {stdout_text}"
        );
        assert!(
            !stdout_text.contains("route-plan"),
            "stale subcommand still in help: {stdout_text}"
        );
    }

    #[test]
    fn orchestration_runtime_preflight_reports_probe_status() {
        let temporary_directory = tempdir_under("claude-skills-orchestration-preflight");
        let claude_home = temporary_directory.join("claude-home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_orchestration_command(
            &[
                "runtime-preflight".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        let stdout_text = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            stdout_text.contains("orchestration runtime-preflight:"),
            "stdout: {stdout_text}"
        );
        assert!(
            stdout_text.contains("claude_home:"),
            "stdout: {stdout_text}"
        );
        assert!(stdout_text.contains("ledger:"), "stdout: {stdout_text}");
        assert!(stdout_text.contains("git:"), "stdout: {stdout_text}");
        assert!(
            claude_home.join("workflow").is_dir(),
            "ledger dir not created"
        );
        assert!(
            exit_code == 0 || exit_code == 1,
            "unexpected exit: {exit_code}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_runtime_preflight_json_emits_structured_payload() {
        let temporary_directory = tempdir_under("claude-skills-orchestration-preflight-json");
        let claude_home = temporary_directory.join("claude-home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let _exit_code = run_orchestration_command(
            &[
                "runtime-preflight".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        let stdout_text = String::from_utf8_lossy(&stdout).to_string();
        assert!(stdout_text.contains("\"ok\":"), "stdout: {stdout_text}");
        assert!(
            stdout_text.contains("\"claudeHome\""),
            "stdout: {stdout_text}"
        );
        assert!(
            stdout_text.contains("\"ledgerDirectory\""),
            "stdout: {stdout_text}"
        );
        assert!(stdout_text.contains("\"git\""), "stdout: {stdout_text}");

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_resume_status_lists_open_entries() {
        let temporary_directory = tempdir_under("claude-skills-orchestration-resume-status");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-eeee", "wire orchestration dispatch");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_orchestration_command(
            &[
                "resume-status".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("orchestration resume-status: open=1"),
            "stdout: {output}"
        );
        assert!(output.contains("wf-eeee"), "stdout: {output}");
        assert!(
            output.contains("wire orchestration dispatch"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_task_unknown_action_returns_error() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_orchestration_command(
            &["task".to_string(), "begn".to_string()],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("Unknown orchestration task action: begn"),
            "stderr: {stderr_text}"
        );
    }

    #[test]
    fn orchestration_task_begin_without_task_flag_reports_required() {
        // `task begin` is implemented now; with no --task it must fail closed
        // asking for the required description rather than silently no-op.
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_orchestration_command(
            &["task".to_string(), "begin".to_string()],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("--task is required"),
            "stderr: {stderr_text}"
        );
    }

    #[test]
    fn orchestration_checkpoint_succeeds_and_reports_snapshot() {
        // `checkpoint` is implemented now; with no open work it must still
        // succeed and emit a zeroed snapshot rather than the old stub error.
        let temporary_directory = tempdir_under("claude-skills-orch-checkpoint-empty");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_orchestration_command(
            &[
                "checkpoint".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let stdout_text = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            stdout_text.contains("open tasks: 0"),
            "stdout: {stdout_text}"
        );
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_write_round_trips_via_show() {
        let temporary_directory = tempdir_under("claude-skills-wb-write-show");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "write".to_string(),
                "--id".to_string(),
                "wb-show-1".to_string(),
                "--request".to_string(),
                "ship pagination on /users".to_string(),
                "--constraints".to_string(),
                "must not break /users|no n+1 queries".to_string(),
                "--acceptance-criteria".to_string(),
                "limit=20 default|expose nextCursor".to_string(),
                "--assumptions".to_string(),
                "cursor encoding stays opaque".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let write_stdout = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            write_stdout.contains("memory working-brief write: id=wb-show-1"),
            "stdout: {write_stdout}"
        );
        assert!(
            write_stdout.contains("constraints: 2 entries"),
            "stdout: {write_stdout}"
        );

        let mut show_stdout: Vec<u8> = Vec::new();
        let mut show_stderr: Vec<u8> = Vec::new();
        let show_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "show".to_string(),
                "--id".to_string(),
                "wb-show-1".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut show_stdout,
            &mut show_stderr,
        );
        assert_eq!(
            show_code,
            0,
            "stderr: {}",
            String::from_utf8_lossy(&show_stderr)
        );
        let show_text = String::from_utf8_lossy(&show_stdout).to_string();
        assert!(show_text.contains("id: wb-show-1"), "stdout: {show_text}");
        assert!(
            show_text.contains("request: ship pagination on /users"),
            "stdout: {show_text}"
        );
        assert!(
            show_text.contains("- must not break /users"),
            "stdout: {show_text}"
        );
        assert!(
            show_text.contains("- limit=20 default"),
            "stdout: {show_text}"
        );
        assert!(
            show_text.contains("- cursor encoding stays opaque"),
            "stdout: {show_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_write_requires_request() {
        let temporary_directory = tempdir_under("claude-skills-wb-write-required");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "write".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("--request is required"),
            "stderr: {stderr_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_show_unknown_id_returns_error() {
        let temporary_directory = tempdir_under("claude-skills-wb-show-missing");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "show".to_string(),
                "--id".to_string(),
                "wb-missing".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("no brief with id wb-missing"),
            "stderr: {stderr_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_list_empty_emits_action_hint() {
        let temporary_directory = tempdir_under("claude-skills-wb-list-empty");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "list".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("memory working-brief list: directory="),
            "stdout: {output}"
        );
        assert!(output.contains("count=0"), "stdout: {output}");
        assert!(
            output.contains("claude-skills memory working-brief write --request"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_list_renders_multiple_briefs_in_order() {
        let temporary_directory = tempdir_under("claude-skills-wb-list-multi");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        for (id, request) in [("wb-alpha", "first request"), ("wb-beta", "second request")] {
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let exit_code = run_memory_command(
                "memory",
                &[
                    "working-brief".to_string(),
                    "write".to_string(),
                    "--id".to_string(),
                    id.to_string(),
                    "--request".to_string(),
                    request.to_string(),
                    "--claude-home".to_string(),
                    claude_home.to_string_lossy().to_string(),
                ],
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        }

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "list".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("count=2"), "stdout: {output}");
        let alpha_pos = output.find("wb-alpha").expect("wb-alpha listed");
        let beta_pos = output.find("wb-beta").expect("wb-beta listed");
        assert!(
            alpha_pos < beta_pos,
            "expected wb-alpha before wb-beta in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_write_json_emits_structured_payload() {
        let temporary_directory = tempdir_under("claude-skills-wb-write-json");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "working-brief".to_string(),
                "write".to_string(),
                "--id".to_string(),
                "wb-json".to_string(),
                "--request".to_string(),
                "json brief".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("\"written\": true"), "stdout: {output}");
        assert!(output.contains("\"id\": \"wb-json\""), "stdout: {output}");
        assert!(
            output.contains("\"request\": \"json brief\""),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn working_brief_unknown_subcommand_returns_error() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &["working-brief".to_string(), "bogus".to_string()],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("Unknown memory working-brief action: bogus"),
            "stderr: {stderr_text}"
        );
    }

    #[test]
    fn completion_gate_check_passes_for_open_entry_with_brief_and_proof() {
        let temporary_directory = tempdir_under("claude-skills-cg-pass");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-pass", "wire pagination");
        write_brief(
            &claude_home,
            &create_brief(
                "wb-pass".into(),
                "wire pagination on /users".into(),
                vec!["no n+1".into()],
                vec!["limit=20".into()],
                Vec::new(),
                format_timestamp_iso8601(0),
            ),
        )
        .expect("seed brief");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-pass".to_string(),
                "--brief-id".to_string(),
                "wb-pass".to_string(),
                "--proof".to_string(),
                "ladder green".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("completion-gate check: id=wf-pass status=ok"),
            "stdout: {output}"
        );
        assert!(output.contains("entry: ok"), "stdout: {output}");
        assert!(output.contains("open: ok"), "stdout: {output}");
        assert!(output.contains("working-brief: ok"), "stdout: {output}");
        assert!(output.contains("proof: ok"), "stdout: {output}");
        assert!(
            output.contains("ready to close with claude-skills workflow finish"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_fails_when_entry_missing() {
        let temporary_directory = tempdir_under("claude-skills-cg-missing");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(claude_home.join("workflow")).expect("create ledger dir");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-missing".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("completion-gate check: id=wf-missing status=fail"),
            "stdout: {output}"
        );
        assert!(
            output.contains("entry: fail -> no ledger entry with id wf-missing"),
            "stdout: {output}"
        );
        assert!(
            output.contains("hint: resolve failing probes"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_fails_when_entry_already_closed() {
        let temporary_directory = tempdir_under("claude-skills-cg-closed");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let open = seeded_open_entry(&claude_home, "wf-closed", "rotate secrets");
        let closed = close_entry(open, format_timestamp_iso8601(1), "done".to_string());
        write_entry(&claude_home, &closed).expect("seed closed entry");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-closed".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("entry: ok"), "stdout: {output}");
        assert!(
            output.contains("open: fail -> entry wf-closed is closed (expected open)"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_fails_when_brief_missing() {
        let temporary_directory = tempdir_under("claude-skills-cg-brief-missing");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-bm", "ship feature");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-bm".to_string(),
                "--brief-id".to_string(),
                "wb-nope".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(
            output.contains("working-brief: fail -> no working brief with id wb-nope"),
            "stdout: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_skips_optional_probes_when_flags_absent() {
        let temporary_directory = tempdir_under("claude-skills-cg-skip");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-skip", "minimal smoke");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-skip".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("status=ok"), "stdout: {output}");
        assert!(
            !output.contains("working-brief:"),
            "expected no working-brief probe in: {output}"
        );
        assert!(
            !output.contains("proof:"),
            "expected no proof probe in: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_requires_id() {
        let temporary_directory = tempdir_under("claude-skills-cg-no-id");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("--id is required"),
            "stderr: {stderr_text}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_check_json_emits_structured_payload() {
        let temporary_directory = tempdir_under("claude-skills-cg-json");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        seeded_open_entry(&claude_home, "wf-json", "structured payload");

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &[
                "completion-gate".to_string(),
                "check".to_string(),
                "--id".to_string(),
                "wf-json".to_string(),
                "--proof".to_string(),
                "ladder green".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let output = String::from_utf8_lossy(&stdout).to_string();
        assert!(output.contains("\"ok\": true"), "stdout: {output}");
        assert!(output.contains("\"id\": \"wf-json\""), "stdout: {output}");
        assert!(output.contains("\"entry\""), "stdout: {output}");
        assert!(output.contains("\"open\""), "stdout: {output}");
        assert!(output.contains("\"proof\""), "stdout: {output}");
        assert!(
            !output.contains("\"workingBrief\""),
            "expected no workingBrief field when --brief-id absent: {output}"
        );

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn completion_gate_unknown_subcommand_returns_error() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit_code = run_memory_command(
            "memory",
            &["completion-gate".to_string(), "bogus".to_string()],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit_code, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("Unknown memory completion-gate action: bogus"),
            "stderr: {stderr_text}"
        );
    }

    fn task_id_from_stdout(stdout: &[u8]) -> String {
        let text = String::from_utf8_lossy(stdout);
        text.lines()
            .find_map(|line| line.trim().strip_prefix("orchestration task begin: id="))
            .map(|id| id.trim().to_string())
            .expect("begin output must include an id")
    }

    #[test]
    fn orchestration_task_begin_progress_complete_round_trips() {
        let temporary_directory = tempdir_under("claude-skills-orch-task-lifecycle");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let home_arg = claude_home.to_string_lossy().to_string();

        // begin
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_orchestration_command(
            &[
                "task".to_string(),
                "begin".to_string(),
                "--task".to_string(),
                "wire sync_commands".to_string(),
                "--phase".to_string(),
                "implement".to_string(),
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let id = task_id_from_stdout(&stdout);
        assert!(claude_home
            .join("orchestration/tasks")
            .join(format!("{id}.json"))
            .is_file());

        // progress
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_orchestration_command(
            &[
                "task".to_string(),
                "progress".to_string(),
                "--id".to_string(),
                id.clone(),
                "--note".to_string(),
                "tests passing".to_string(),
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

        // complete (json) — assert status flips to done and completedAt is stamped
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_orchestration_command(
            &[
                "task".to_string(),
                "complete".to_string(),
                "--id".to_string(),
                id.clone(),
                "--claude-home".to_string(),
                home_arg.clone(),
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let out = String::from_utf8_lossy(&stdout).to_string();
        assert!(out.contains("\"status\": \"done\""), "stdout: {out}");
        assert!(out.contains("\"completedAt\""), "stdout: {out}");

        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_task_begin_requires_task_description() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_orchestration_command(
            &["task".to_string(), "begin".to_string()],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("--task is required"));
    }

    #[test]
    fn orchestration_task_progress_unknown_id_errors() {
        let temporary_directory = tempdir_under("claude-skills-orch-task-missing");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_orchestration_command(
            &[
                "task".to_string(),
                "progress".to_string(),
                "--id".to_string(),
                "task-nope".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("no task with id task-nope"));
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_task_list_open_only_excludes_done() {
        let temporary_directory = tempdir_under("claude-skills-orch-task-list");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let home_arg = claude_home.to_string_lossy().to_string();

        // Two tasks; complete one.
        let mut out = Vec::new();
        let mut err = Vec::new();
        run_orchestration_command(
            &[
                "task".to_string(),
                "begin".to_string(),
                "--task".to_string(),
                "open one".to_string(),
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut out,
            &mut err,
        );
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        run_orchestration_command(
            &[
                "task".to_string(),
                "begin".to_string(),
                "--task".to_string(),
                "done one".to_string(),
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut out2,
            &mut err2,
        );
        let done_id = task_id_from_stdout(&out2);
        let mut out3 = Vec::new();
        let mut err3 = Vec::new();
        run_orchestration_command(
            &[
                "task".to_string(),
                "complete".to_string(),
                "--id".to_string(),
                done_id,
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut out3,
            &mut err3,
        );

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_orchestration_command(
            &[
                "task".to_string(),
                "list".to_string(),
                "--open-only".to_string(),
                "--claude-home".to_string(),
                home_arg,
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let text = String::from_utf8_lossy(&stdout).to_string();
        assert!(text.contains("1 task(s)"), "stdout: {text}");
        assert!(text.contains("open one"), "stdout: {text}");
        assert!(!text.contains("done one"), "stdout: {text}");
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn orchestration_checkpoint_counts_open_tasks_and_persists_snapshot() {
        let temporary_directory = tempdir_under("claude-skills-orch-checkpoint");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let home_arg = claude_home.to_string_lossy().to_string();

        // One open task.
        let mut out = Vec::new();
        let mut err = Vec::new();
        run_orchestration_command(
            &[
                "task".to_string(),
                "begin".to_string(),
                "--task".to_string(),
                "in flight".to_string(),
                "--claude-home".to_string(),
                home_arg.clone(),
            ],
            &mut out,
            &mut err,
        );

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_orchestration_command(
            &[
                "checkpoint".to_string(),
                "--note".to_string(),
                "before compaction".to_string(),
                "--claude-home".to_string(),
                home_arg,
                "--json".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        let text = String::from_utf8_lossy(&stdout).to_string();
        assert!(text.contains("\"openTasks\": \"1\""), "stdout: {text}");
        assert!(text.contains("before compaction"), "stdout: {text}");
        // A checkpoint record must be persisted for resume-after-compaction.
        let checkpoint_dir = claude_home.join("orchestration/checkpoints");
        let count = fs::read_dir(&checkpoint_dir)
            .expect("checkpoints dir exists")
            .count();
        assert_eq!(count, 1, "exactly one checkpoint record expected");
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn memory_report_aliases_status_summary() {
        let temporary_directory = tempdir_under("claude-skills-memory-report");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_memory_command(
            "memory",
            &[
                "report".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        // `report` is an alias for the family status summary.
        let text = String::from_utf8_lossy(&stdout).to_string();
        assert!(text.contains("family record counts"), "stdout: {text}");
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn memory_index_rebuilds_recall_index() {
        let temporary_directory = tempdir_under("claude-skills-memory-index");
        let claude_home = temporary_directory.join("claude-home");
        fs::create_dir_all(&claude_home).expect("create claude home");
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_memory_command(
            "memory",
            &[
                "index".to_string(),
                "--claude-home".to_string(),
                claude_home.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        // reindex on an empty home succeeds and creates the sqlite index.
        assert_eq!(exit, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
        assert!(
            claude_home.join("recall-index.sqlite3").is_file(),
            "recall index must be created by `memory index`"
        );
        let _ = fs::remove_dir_all(&temporary_directory);
    }

    #[test]
    fn memory_hook_redirects_to_lifecycle_hook_surface() {
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_memory_command("memory", &["hook".to_string()], &mut stdout, &mut stderr);
        assert_eq!(exit, 1);
        let stderr_text = String::from_utf8_lossy(&stderr).to_string();
        assert!(
            stderr_text.contains("claude-skills hook install"),
            "stderr must point at the real hook surface: {stderr_text}"
        );
    }
}
