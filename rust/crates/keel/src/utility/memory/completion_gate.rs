//! Purpose: Completion gate check command handler
//! Caller: mod.rs run_memory_command
//! Dependencies: crate::args::FlagSet, crate::json, crate::runtime, crate::utility::workflow_ledger, crate::utility::working_brief
//! Main Functions: run_completion_gate_command
//! Side Effects: None, read-only probes

use std::io::Write;

use crate::args::FlagSet;
use crate::json::Value;
use crate::runtime::resolve_claude_home;
use crate::utility::workflow_ledger::{read_entry, Entry, STATUS_OPEN};
use crate::utility::working_brief::read_brief;

use super::shared::{is_help_argument, probe_marker, probe_value, render_workflow_json};

pub(super) fn run_completion_gate_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} completion-gate [check|record-requirement] ..."
        );
        let _ = writeln!(
            standard_output,
            "  check --id <entry-id> [--brief-id <id>] [--proof <text>] [--claude-home <path>] [--json]"
        );
        let _ = writeln!(
            standard_output,
            "  record-requirement --id <entry-id> --requirement <text> [--status <pending|met|failed>] [--claude-home <path>] [--json]"
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
        "record-requirement" => run_completion_gate_record_requirement(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "Unknown {command_group} completion-gate action: {other} (expected check|record-requirement)"
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
            Err(error) => Err(error.to_string()),
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
            "  hint: resolve failing probes before running keel workflow finish --id {entry_id} --proof \"...\""
        );
        return 1;
    }
    let _ = writeln!(
        standard_output,
        "  hint: ready to close with keel workflow finish --id {entry_id} --proof \"...\""
    );
    0
}

fn run_completion_gate_record_requirement(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("completion-gate record-requirement");
    flag_set.string_flag("id", "");
    flag_set.string_flag("requirement", "");
    flag_set.string_flag("status", "pending");
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
            "{command_group} completion-gate record-requirement: --id is required"
        );
        return 1;
    }
    let requirement = flag_set.string_value("requirement").trim().to_string();
    if requirement.is_empty() {
        let _ = writeln!(
            standard_error,
            "{command_group} completion-gate record-requirement: --requirement is required"
        );
        return 1;
    }
    let status = flag_set.string_value("status").trim().to_string();
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} completion-gate record-requirement: {error}"
            );
            return 1;
        }
    };

    match read_entry(&claude_home, &entry_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = writeln!(
                standard_error,
                "{command_group} completion-gate record-requirement: no ledger entry with id {entry_id}"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "{command_group} completion-gate record-requirement: {error}"
            );
            return 1;
        }
    }

    let now_millis = crate::utility::workflow_ledger::current_timestamp_millis();
    let requirement_id = format!("cgr-{now_millis:x}");
    let dir = claude_home
        .join(command_group)
        .join("completion-gate-requirements");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        let _ = writeln!(
            standard_error,
            "{command_group} completion-gate record-requirement: create {}: {error}",
            crate::runtime::display_path(&dir)
        );
        return 1;
    }
    let file_path = dir.join(format!("{requirement_id}.json"));
    let payload = Value::Object(vec![
        (
            "requirementId".into(),
            Value::String(requirement_id.clone()),
        ),
        ("entryId".into(), Value::String(entry_id.clone())),
        ("requirement".into(), Value::String(requirement.clone())),
        ("status".into(), Value::String(status.clone())),
        (
            "createdAt".into(),
            Value::String(crate::utility::workflow_ledger::format_timestamp_iso8601(
                now_millis,
            )),
        ),
    ]);
    let mut serialized = Vec::<u8>::new();
    if let Err(error) = crate::json::write_indented(&mut serialized, &payload) {
        let _ = writeln!(
            standard_error,
            "{command_group} completion-gate record-requirement: serialize: {error}"
        );
        return 1;
    }
    if let Err(error) = std::fs::write(&file_path, &serialized) {
        let _ = writeln!(
            standard_error,
            "{command_group} completion-gate record-requirement: write {}: {error}",
            crate::runtime::display_path(&file_path)
        );
        return 1;
    }

    if flag_set.bool_value("json") {
        let json_payload = Value::Object(vec![
            ("recorded".into(), Value::Bool(true)),
            (
                "requirementId".into(),
                Value::String(requirement_id.clone()),
            ),
            ("entryId".into(), Value::String(entry_id.clone())),
            ("requirement".into(), Value::String(requirement.clone())),
            ("status".into(), Value::String(status.clone())),
        ]);
        return render_workflow_json(standard_output, standard_error, &json_payload);
    }

    let _ = writeln!(
        standard_output,
        "{command_group} completion-gate record-requirement: requirement_id={requirement_id} entry_id={entry_id}"
    );
    let _ = writeln!(standard_output, "  requirement: {requirement}");
    let _ = writeln!(standard_output, "  status: {status}");
    0
}
