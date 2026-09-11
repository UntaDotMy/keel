//! Purpose: Build and validate evidence-bound planner task tickets and RTM links.
//! Caller: utility::plan task compilation, plan checks, and named-plan review.
//! Dependencies: serde_json, chrono, stable hashing, and bounded filesystem reads.
//! Main Functions: prepare_task_artifacts, validate_task_artifacts, link_open_warning_subtasks.
//! Side Effects: None; the planner owner performs all writes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use chrono::DateTime;
use serde_json::{json, Map, Value};

const SCHEMA_VERSION: u64 = 1;
const MAX_TICKET_BYTES: u64 = 65_536;
const RTM_CHAIN: &str =
    "User request -> Requirement -> Acceptance criterion -> Task -> Checklist subtask -> Evidence";

const BASE_LAYERS: [&str; 11] = [
    "design",
    "implementation",
    "build",
    "tests",
    "lint_warnings",
    "security",
    "performance_tokens",
    "docs",
    "ui",
    "ux_accessibility",
    "release_rollback",
];

const ALL_LAYERS: [&str; 24] = [
    "design",
    "implementation",
    "build",
    "tests",
    "lint_warnings",
    "security",
    "performance_tokens",
    "docs",
    "ui",
    "ux_accessibility",
    "release_rollback",
    "compatibility",
    "screenshots",
    "test_fixtures",
    "license_audit",
    "reproducibility",
    "bloat_analysis",
    "benchmark",
    "fixed_context_runtime",
    "migration",
    "privacy",
    "integrity",
    "rollback",
    "host_adapter_contracts",
];

const INSTALL_PARITY_LAYER: &str = "install_provision_parity";

const EVIDENCE_TYPES: [&str; 7] = [
    "command",
    "named_test",
    "lint_diagnostic",
    "source_hash",
    "raw_store",
    "screenshot",
    "benchmark",
];

#[derive(Debug, Clone)]
pub(crate) struct AcceptanceSeed {
    pub id: String,
    pub verification_method: String,
    pub expected_evidence_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskSeed {
    pub id: String,
    pub title: String,
    pub requirement_refs: Vec<String>,
    pub acceptance: Vec<AcceptanceSeed>,
}

#[derive(Debug)]
pub(crate) struct TicketArtifact {
    pub file_name: String,
    pub value: Value,
    pub write_required: bool,
}

#[derive(Debug)]
pub(crate) struct PreparedTaskArtifacts {
    pub tasks: Value,
    pub rtm: Value,
    pub tickets: Vec<TicketArtifact>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskUpdateRequest {
    pub task_id: String,
    pub subtask_id: Option<String>,
    pub status: String,
    pub evidence_path: Option<String>,
    pub reason: Option<String>,
    pub verification_timestamp: Option<String>,
}

#[derive(Debug)]
pub(crate) struct PreparedTaskUpdate {
    pub ticket_file: String,
    pub ticket: Value,
    pub tasks: Value,
    pub rtm: Value,
    pub task_status: String,
}

pub(crate) struct ValidationContext<'a> {
    pub plan_id: &'a str,
    pub plan_directory: &'a Path,
    pub keel_home: &'a Path,
    pub workspace_root: &'a Path,
    pub specification: &'a str,
    pub architecture: &'a str,
    pub seeds: &'a [TaskSeed],
}

#[derive(Debug, Clone)]
pub(crate) struct WarningTicketLink {
    pub fingerprint: String,
    pub description: String,
}

pub(crate) fn link_open_warning_subtasks(
    keel_home: &Path,
    workspace_root: &Path,
    warnings: &[WarningTicketLink],
) -> Result<(), String> {
    if warnings.is_empty() {
        return Ok(());
    }
    let plans = keel_home
        .join("memories")
        .join("workspaces")
        .join(crate::utility::system_map::workspace_key(
            &workspace_root.to_string_lossy(),
        ))
        .join("plans");
    let Ok(entries) = fs::read_dir(&plans) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let plan_directory = entry.path();
        if plan_directory.is_dir() {
            link_plan_warning_subtasks(&plan_directory, warnings)?;
        }
    }
    Ok(())
}

fn link_plan_warning_subtasks(
    plan_directory: &Path,
    warnings: &[WarningTicketLink],
) -> Result<(), String> {
    let mut added = Vec::new();
    let Ok(entries) = fs::read_dir(plan_directory) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !file_name.starts_with("task-")
            || path.extension().and_then(|ext| ext.to_str()) != Some("json")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut ticket) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(new_ids) = append_warning_subtasks(&mut ticket, warnings) else {
            continue;
        };
        if new_ids.is_empty() {
            continue;
        }
        crate::runtime::write_text(
            &path,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&ticket)
                    .map_err(|error| format!("serialize {}: {error}", path.display()))?
            ),
        )?;
        added.extend(new_ids);
    }
    if !added.is_empty() {
        update_rtm_for_warning_subtasks(plan_directory, &added)?;
    }
    Ok(())
}

fn append_warning_subtasks(
    ticket: &mut Value,
    warnings: &[WarningTicketLink],
) -> Option<Vec<AddedWarningSubtask>> {
    let ticket_id = string_field(ticket, "id")?.to_string();
    let requirement_refs = ticket.get("requirement_refs")?.clone();
    let acceptance_refs = ticket.get("acceptance_refs")?.clone();
    let acceptance_ids = string_array(ticket, "acceptance_refs");
    let requirement_id = string_array(ticket, "requirement_refs")
        .into_iter()
        .next()
        .unwrap_or_default();
    let layers = ticket.get_mut("layers")?.as_object_mut()?;
    let lint_warnings = layers
        .entry("lint_warnings".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let subtasks = lint_warnings.as_array_mut()?;
    let existing: BTreeSet<String> = subtasks
        .iter()
        .flat_map(|subtask| {
            let mut keys = Vec::new();
            if let Some(id) = string_field(subtask, "id") {
                keys.push(id.to_string());
            }
            if let Some(description) = string_field(subtask, "description") {
                keys.push(description.to_string());
            }
            keys
        })
        .collect();
    let mut added = Vec::new();
    for warning in warnings {
        let subtask_id = format!("{ticket_id}-LINT-WARNINGS-{}", warning.fingerprint);
        if existing
            .iter()
            .any(|value| value.contains(&warning.fingerprint))
        {
            continue;
        }
        subtasks.push(json!({
            "id": subtask_id,
            "description": warning.description,
            "requirement_refs": requirement_refs,
            "acceptance_refs": acceptance_refs,
            "status": "open",
            "expected_evidence_type": "lint_diagnostic",
            "evidence_ref": Value::Null,
            "reason": Value::Null,
            "owner_role": "verifier",
            "verification_timestamp": Value::Null,
            "derived": true
        }));
        added.push(AddedWarningSubtask {
            task_id: ticket_id.clone(),
            subtask_id,
            requirement_id: requirement_id.clone(),
            acceptance_ids: acceptance_ids.clone(),
        });
    }
    Some(added)
}

struct AddedWarningSubtask {
    task_id: String,
    subtask_id: String,
    requirement_id: String,
    acceptance_ids: Vec<String>,
}

fn update_rtm_for_warning_subtasks(
    plan_directory: &Path,
    added: &[AddedWarningSubtask],
) -> Result<(), String> {
    let path = plan_directory.join("rtm.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(mut rtm) = serde_json::from_str::<Value>(&text) else {
        return Ok(());
    };
    if let Some(entries) = rtm.get_mut("entries").and_then(Value::as_array_mut) {
        for entry in entries {
            let task_ids = string_array(entry, "taskIds");
            let Some(ids) = entry
                .get_mut("checklistSubtaskIds")
                .and_then(Value::as_array_mut)
            else {
                continue;
            };
            for item in added {
                if task_ids.contains(&item.task_id)
                    && !ids
                        .iter()
                        .any(|value| value.as_str() == Some(&item.subtask_id))
                {
                    ids.push(Value::String(item.subtask_id.clone()));
                }
            }
        }
    }
    if let Some(traces) = rtm.get_mut("traces").and_then(Value::as_array_mut) {
        for item in added {
            for criterion_id in &item.acceptance_ids {
                let already = traces.iter().any(|trace| {
                    string_field(trace, "subtaskId") == Some(item.subtask_id.as_str())
                        && string_field(trace, "acceptanceCriterionId")
                            == Some(criterion_id.as_str())
                });
                if already {
                    continue;
                }
                traces.push(json!({
                    "userRequestRef": "request://submitted",
                    "requirementId": item.requirement_id,
                    "acceptanceCriterionId": criterion_id,
                    "taskId": item.task_id,
                    "subtaskId": item.subtask_id,
                    "expectedEvidenceType": "lint_diagnostic",
                    "evidenceRef": Value::Null
                }));
            }
        }
    }
    crate::runtime::write_text(
        &path,
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&rtm)
                .map_err(|error| format!("serialize {}: {error}", path.display()))?
        ),
    )
}

pub(crate) fn prepare_task_artifacts(
    context: &ValidationContext<'_>,
) -> Result<PreparedTaskArtifacts, Vec<String>> {
    let scopes = derive_scopes(context.specification, context.architecture);
    let derived_layers = derived_layers(&scopes);
    let mut tickets = Vec::new();
    let mut issues = Vec::new();

    for (index, seed) in context.seeds.iter().enumerate() {
        let file_name = format!("task-{:03}.json", index + 1);
        let path = context.plan_directory.join(&file_name);
        let template = ticket_template(context.plan_id, seed, &scopes, &derived_layers);
        let (value, write_required) = if path.exists() {
            match read_bounded_json(&path, &file_name, &mut issues) {
                Some(value) => (value, false),
                None => (template, false),
            }
        } else {
            (template, true)
        };
        tickets.push(TicketArtifact {
            file_name,
            value,
            write_required,
        });
    }

    validate_unindexed_ticket_files(context.plan_directory, &tickets, &mut issues);
    validate_ticket_values(context, &tickets, &scopes, &derived_layers, &mut issues);
    let (tasks, rtm) = build_aggregate_and_rtm(context.plan_id, context.seeds, &tickets);
    validate_rtm_links(context.seeds, &tickets, &rtm, &mut issues);
    if issues.is_empty() {
        Ok(PreparedTaskArtifacts {
            tasks,
            rtm,
            tickets,
        })
    } else {
        Err(issues)
    }
}

/// Prepare one governed task or checklist-subtask update without publishing it.
///
/// The planner owns publication; this function keeps identity, evidence, layer,
/// and RTM validation in the task-ticket owner so callers cannot update a ticket
/// while leaving its aggregate artifacts out of sync.
pub(crate) fn prepare_task_update(
    context: &ValidationContext<'_>,
    request: &TaskUpdateRequest,
) -> Result<PreparedTaskUpdate, Vec<String>> {
    let scopes = derive_scopes(context.specification, context.architecture);
    let derived_layers = derived_layers(&scopes);
    let mut tickets = Vec::new();
    let mut issues = Vec::new();

    for (index, _seed) in context.seeds.iter().enumerate() {
        let file_name = format!("task-{:03}.json", index + 1);
        let path = context.plan_directory.join(&file_name);
        let value = read_bounded_json(&path, &file_name, &mut issues)
            .unwrap_or_else(|| Value::Object(Map::new()));
        tickets.push(TicketArtifact {
            file_name,
            value,
            write_required: false,
        });
    }

    validate_unindexed_ticket_files(context.plan_directory, &tickets, &mut issues);
    validate_ticket_values(context, &tickets, &scopes, &derived_layers, &mut issues);
    if !issues.is_empty() {
        return Err(issues);
    }

    let Some(ticket_index) = context
        .seeds
        .iter()
        .position(|seed| seed.id == request.task_id)
    else {
        return Err(vec![format!(
            "unknown task id {}; expected a generated task id",
            request.task_id
        )]);
    };
    {
        let ticket = &mut tickets[ticket_index];
        if let Some(subtask_id) = request.subtask_id.as_deref() {
            update_subtask(ticket, subtask_id, request, context, &mut issues);
        } else {
            update_task(ticket, request, &mut issues);
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    let (tasks, rtm) = build_aggregate_and_rtm(context.plan_id, context.seeds, &tickets);
    validate_ticket_values(context, &tickets, &scopes, &derived_layers, &mut issues);
    validate_rtm_links(context.seeds, &tickets, &rtm, &mut issues);
    if issues.is_empty() {
        let ticket = &tickets[ticket_index];
        Ok(PreparedTaskUpdate {
            ticket_file: ticket.file_name.clone(),
            ticket: ticket.value.clone(),
            task_status: string_field(&ticket.value, "status")
                .unwrap_or("planned")
                .to_string(),
            tasks,
            rtm,
        })
    } else {
        Err(issues)
    }
}

fn update_task(ticket: &mut TicketArtifact, request: &TaskUpdateRequest, issues: &mut Vec<String>) {
    if !matches!(
        request.status.as_str(),
        "planned" | "in_progress" | "blocked" | "done"
    ) {
        issues.push(format!("{} has invalid task status", request.task_id));
    }
    if request.subtask_id.is_none()
        && (request.evidence_path.is_some()
            || request.reason.is_some()
            || request.verification_timestamp.is_some())
    {
        issues.push("task status updates do not accept evidence, reason, or verification timestamp; target a subtask".to_string());
    }
    if request.status == "done" {
        let unfinished: Vec<&str> = ticket_subtasks(&ticket.value)
            .into_iter()
            .filter_map(|(_, subtask)| {
                let status = string_field(subtask, "status")?;
                (!matches!(status, "done" | "not_applicable"))
                    .then_some(string_field(subtask, "id").unwrap_or("subtask"))
            })
            .collect();
        if !unfinished.is_empty() {
            issues.push(format!(
                "{} cannot be done while subtasks are unfinished: {}",
                request.task_id,
                unfinished.join(", ")
            ));
        }
    }
    if issues.is_empty() {
        ticket
            .value
            .as_object_mut()
            .expect("validated task ticket is an object")
            .insert("status".to_string(), Value::String(request.status.clone()));
    }
}

fn update_subtask(
    ticket: &mut TicketArtifact,
    subtask_id: &str,
    request: &TaskUpdateRequest,
    context: &ValidationContext<'_>,
    issues: &mut Vec<String>,
) {
    let matches = ticket_subtasks(&ticket.value)
        .into_iter()
        .filter(|(_, subtask)| string_field(subtask, "id") == Some(subtask_id))
        .count();
    if matches == 0 {
        issues.push(format!("unknown subtask id {subtask_id}"));
        return;
    }
    if matches > 1 {
        issues.push(format!("subtask id {subtask_id} is duplicated"));
        return;
    }
    if !matches!(
        request.status.as_str(),
        "open" | "done" | "skipped" | "not_applicable" | "needs_human"
    ) {
        issues.push(format!("{subtask_id} has invalid subtask status"));
    }
    let reason_required = matches!(
        request.status.as_str(),
        "skipped" | "not_applicable" | "needs_human"
    );
    match (reason_required, request.reason.as_deref().map(str::trim)) {
        (true, Some("")) => {
            issues.push(format!(
                "{subtask_id} status {} requires a non-empty reason",
                request.status
            ));
        }
        (true, None) => {
            issues.push(format!(
                "{subtask_id} status {} requires --reason",
                request.status
            ));
        }
        (false, Some(reason)) if !reason.is_empty() => {
            issues.push(format!(
                "{subtask_id} status {} does not accept --reason",
                request.status
            ));
        }
        _ => {}
    }
    if request.status == "done" {
        if request
            .evidence_path
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            issues.push(format!("{subtask_id} done status requires --evidence-path"));
        }
        if request
            .verification_timestamp
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            issues.push(format!(
                "{subtask_id} done status requires --verification-timestamp"
            ));
        }
    } else {
        if request.evidence_path.is_some() {
            issues.push(format!(
                "{subtask_id} status {} does not accept --evidence-path",
                request.status
            ));
        }
        if request.verification_timestamp.is_some() {
            issues.push(format!(
                "{subtask_id} status {} does not accept --verification-timestamp",
                request.status
            ));
        }
    }
    if !issues.is_empty() {
        return;
    }

    let evidence_ref = if request.status == "done" {
        let relative = request
            .evidence_path
            .as_deref()
            .expect("done update checked evidence path");
        match evidence_reference(context.plan_directory, relative, issues) {
            Some(reference) => reference,
            None => return,
        }
    } else {
        Value::Null
    };
    let target = find_subtask_mut(&mut ticket.value, subtask_id)
        .expect("validated unique subtask exists in ticket");
    target["status"] = Value::String(request.status.clone());
    target["reason"] = request
        .reason
        .as_deref()
        .filter(|reason| !reason.trim().is_empty())
        .map(|reason| Value::String(reason.trim().to_string()))
        .unwrap_or(Value::Null);
    target["verification_timestamp"] = request
        .verification_timestamp
        .as_deref()
        .map(|timestamp| Value::String(timestamp.to_string()))
        .unwrap_or(Value::Null);
    target["evidence_ref"] = evidence_ref;
    if request.status != "done" {
        target["verification_timestamp"] = Value::Null;
    }
    let task_status = derive_ticket_status(&ticket.value).to_string();
    ticket.value["status"] = Value::String(task_status);
}

fn evidence_reference(
    plan_directory: &Path,
    relative: &str,
    issues: &mut Vec<String>,
) -> Option<Value> {
    let path = resolve_regular_plan_file(plan_directory, relative, issues)?;
    let body = read_bounded_text(&path, relative, issues)?;
    if serde_json::from_str::<Value>(&body).is_err() {
        issues.push(format!("parse evidence {relative}: expected a JSON object"));
        return None;
    }
    Some(json!({
        "path": relative,
        "content_hash": format!("fnv1a64:{}", crate::utility::hashing::fnv1a64_hex(&body))
    }))
}

fn find_subtask_mut<'a>(ticket: &'a mut Value, subtask_id: &str) -> Option<&'a mut Value> {
    let layers = ticket.get_mut("layers")?.as_object_mut()?;
    for values in layers.values_mut() {
        let values = values.as_array_mut()?;
        for node in values {
            if string_field(node, "id") == Some(subtask_id) {
                return Some(node);
            }
            if let Some(todos) = node.get_mut("todos").and_then(Value::as_array_mut) {
                for todo in todos {
                    if string_field(todo, "id") == Some(subtask_id) {
                        return Some(todo);
                    }
                }
            }
        }
    }
    None
}

fn derive_ticket_status(ticket: &Value) -> &'static str {
    let nodes = ticket_subtasks(ticket);
    if nodes.iter().any(|(_, node)| {
        matches!(
            string_field(node, "status"),
            Some("needs_human" | "skipped")
        )
    }) {
        return "blocked";
    }
    if !nodes.is_empty()
        && nodes.iter().all(|(_, node)| {
            matches!(
                string_field(node, "status"),
                Some("done" | "not_applicable")
            )
        })
    {
        return "done";
    }
    if nodes.iter().any(|(_, node)| {
        matches!(
            string_field(node, "status"),
            Some("done" | "not_applicable")
        )
    }) {
        "in_progress"
    } else {
        "planned"
    }
}

pub(crate) fn validate_task_artifacts(
    context: &ValidationContext<'_>,
    tasks: Option<&Value>,
    rtm: Option<&Value>,
    issues: &mut Vec<String>,
) {
    let Some(tasks) = tasks else {
        return;
    };
    let Some(ticket_files) = tasks.get("ticketFiles") else {
        return;
    };
    let Some(ticket_files) = ticket_files.as_array() else {
        issues.push("tasks.json ticketFiles is not an array".to_string());
        return;
    };
    if ticket_files.is_empty() {
        issues.push("tasks.json ticketFiles is empty".to_string());
        return;
    }

    let mut tickets = Vec::new();
    let mut seen_files = BTreeSet::new();
    for file in ticket_files {
        let Some(file_name) = file.as_str() else {
            issues.push("tasks.json ticketFiles contains a non-string entry".to_string());
            continue;
        };
        if !seen_files.insert(file_name.to_string()) {
            issues.push(format!("tasks.json repeats ticket file {file_name}"));
            continue;
        }
        if !is_ticket_file_name(file_name) {
            issues.push(format!("tasks.json has invalid ticket file {file_name}"));
            continue;
        }
        let path = context.plan_directory.join(file_name);
        if let Some(value) = read_bounded_json(&path, file_name, issues) {
            tickets.push(TicketArtifact {
                file_name: file_name.to_string(),
                value,
                write_required: false,
            });
        }
    }

    let scopes = derive_scopes(context.specification, context.architecture);
    let derived_layers = derived_layers(&scopes);
    validate_aggregate_ticket_links(tasks, context.seeds, &tickets, issues);
    validate_unindexed_ticket_files(context.plan_directory, &tickets, issues);
    validate_ticket_values(context, &tickets, &scopes, &derived_layers, issues);
    if let Some(rtm) = rtm {
        validate_rtm_links(context.seeds, &tickets, rtm, issues);
    }
}

fn derive_scopes(specification: &str, architecture: &str) -> Vec<&'static str> {
    let request = section_body(specification, "## 1. User request verbatim", "## 2.");
    let components = section_body(
        architecture,
        "## 3. Components/files/interfaces changed",
        "## 4.",
    );
    let text = format!("{request}\n{components}").to_ascii_lowercase();
    let mut scopes = Vec::new();
    if contains_any(
        &text,
        &[
            ".rs",
            ".go",
            ".py",
            ".js",
            ".jsx",
            ".ts",
            ".tsx",
            ".java",
            ".kt",
            ".swift",
            ".c",
            ".cpp",
            ".cs",
            ".rb",
            ".php",
            ".dart",
            ".sh",
            ".ps1",
            "source code",
        ],
    ) {
        scopes.push("source_code");
    }
    if text.contains("public behavior")
        || ["api", "cli", "command", "interface"]
            .iter()
            .any(|word| contains_word(&text, word))
    {
        scopes.push("public_behavior_api");
    }
    if [
        "ui",
        "screen",
        "widget",
        "visual",
        "touch",
        "accessibility",
        "ux",
    ]
    .iter()
    .any(|word| contains_word(&text, word))
    {
        scopes.push("ui_behavior");
    }
    if text.contains("cargo.toml")
        || ["dependency", "dependencies", "package", "pubspec", "npm"]
            .iter()
            .any(|word| contains_word(&text, word))
    {
        scopes.push("dependency_change");
    }
    if ["token", "performance", "latency", "throughput", "benchmark"]
        .iter()
        .any(|word| contains_word(&text, word))
    {
        scopes.push("token_performance_change");
    }
    if [
        "database",
        "data",
        "memory",
        "migration",
        "storage",
        "persistence",
    ]
    .iter()
    .any(|word| contains_word(&text, word))
    {
        scopes.push("data_memory_change");
    }
    if text.contains("host integration")
        || text.contains("host adapter")
        || ["install", "provision"]
            .iter()
            .any(|word| contains_word(&text, word))
    {
        scopes.push("host_integration_change");
    }
    scopes
}

fn section_body<'a>(body: &'a str, start: &str, next_prefix: &str) -> &'a str {
    let Some((_, after)) = body.split_once(start) else {
        return "";
    };
    let end = after.find(next_prefix).unwrap_or(after.len());
    &after[..end]
}

fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

fn contains_word(text: &str, expected: &str) -> bool {
    text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|word| word == expected)
}

fn derived_layers(scopes: &[&str]) -> Vec<&'static str> {
    let mut layers = BTreeSet::new();
    layers.insert("design");
    for scope in scopes {
        let required: &[&str] = match *scope {
            "source_code" => &["implementation", "build", "tests", "lint_warnings", "docs"],
            "public_behavior_api" => &["compatibility", "docs", "security", "release_rollback"],
            "ui_behavior" => &["ui", "ux_accessibility", "screenshots", "test_fixtures"],
            "dependency_change" => &[
                "security",
                "license_audit",
                "reproducibility",
                "bloat_analysis",
            ],
            "token_performance_change" => {
                &["benchmark", "performance_tokens", "fixed_context_runtime"]
            }
            "data_memory_change" => &["migration", "privacy", "integrity", "rollback"],
            "host_integration_change" => &["host_adapter_contracts", INSTALL_PARITY_LAYER],
            _ => &[],
        };
        layers.extend(required.iter().copied());
    }
    layers.into_iter().collect()
}

fn ticket_template(
    plan_id: &str,
    seed: &TaskSeed,
    scopes: &[&str],
    derived_layers: &[&str],
) -> Value {
    let acceptance_refs: Vec<&str> = seed
        .acceptance
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect();
    let mut layers = Map::new();
    for layer in BASE_LAYERS {
        layers.insert(layer.to_string(), Value::Array(Vec::new()));
    }
    for layer in derived_layers {
        let subtask_id = format!(
            "{}-{}-001",
            seed.id,
            layer.to_ascii_uppercase().replace('_', "-")
        );
        layers.insert(
            (*layer).to_string(),
            json!([{
                "id": subtask_id,
                "parent_id": seed.id,
                "dependencies": [],
                "todos": [],
                "objective": format!("Produce {layer} evidence for {}.", seed.id),
                "expected_output": evidence_type_for_layer(layer),
                "verification": seed.acceptance.iter().map(|criterion| criterion.verification_method.as_str()).collect::<Vec<_>>(),
                "description": format!("Produce {layer} evidence for {}.", seed.id),
                "requirement_refs": seed.requirement_refs,
                "acceptance_refs": acceptance_refs,
                "status": "open",
                "expected_evidence_type": evidence_type_for_layer(layer),
                "evidence_ref": Value::Null,
                "reason": Value::Null,
                "owner_role": owner_for_layer(layer),
                "verification_timestamp": Value::Null,
                "derived": true
            }]),
        );
    }
    json!({
        "schema_version": SCHEMA_VERSION,
        "artifact": "task_ticket",
        "plan_id": plan_id,
        "id": seed.id,
        "parent_id": plan_id,
        "dependencies": [],
        "goal": seed.title,
        "title": seed.title,
        "requirement_refs": seed.requirement_refs,
        "acceptance_refs": acceptance_refs,
        "status": "planned",
        "derived_scopes": scopes,
        "derived_layers": derived_layers,
        "layers": layers
    })
}

fn evidence_type_for_layer(layer: &str) -> &'static str {
    match layer {
        "implementation" | "docs" | "design" | "release_rollback" => "source_hash",
        "tests"
        | "compatibility"
        | "test_fixtures"
        | "integrity"
        | "host_adapter_contracts"
        | INSTALL_PARITY_LAYER => "named_test",
        "lint_warnings" => "lint_diagnostic",
        "ui" | "ux_accessibility" | "screenshots" => "screenshot",
        "performance_tokens" | "bloat_analysis" | "benchmark" | "fixed_context_runtime" => {
            "benchmark"
        }
        _ => "command",
    }
}

fn owner_for_layer(layer: &str) -> &'static str {
    match layer {
        "implementation" | "docs" | "design" => "implementer",
        "security" | "privacy" | "ux_accessibility" => "reviewer",
        _ => "verifier",
    }
}

fn build_aggregate_and_rtm(
    plan_id: &str,
    seeds: &[TaskSeed],
    tickets: &[TicketArtifact],
) -> (Value, Value) {
    let mut task_entries = Vec::new();
    let mut rtm_entries = Vec::new();
    let mut traces = Vec::new();
    for (seed, ticket) in seeds.iter().zip(tickets) {
        let criterion_ids: Vec<&str> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.id.as_str())
            .collect();
        let verification_methods: Vec<&str> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.verification_method.as_str())
            .collect();
        let evidence_types: Vec<&str> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.expected_evidence_type.as_str())
            .collect();
        task_entries.push(json!({
            "taskId": seed.id,
            "parentId": plan_id,
            "title": seed.title,
            "requirementIds": seed.requirement_refs,
            "acceptanceCriterionIds": criterion_ids,
            "verificationMethod": verification_methods.first().copied().unwrap_or_default(),
            "expectedEvidenceType": evidence_types.first().copied().unwrap_or_default(),
            "ownerRole": "implementer",
            "status": aggregate_task_status(&ticket.value),
            "ticketFile": ticket.file_name
        }));

        let subtasks = ticket_subtasks(&ticket.value);
        let subtask_ids: Vec<String> = subtasks
            .iter()
            .filter_map(|(_, subtask)| string_field(subtask, "id").map(str::to_string))
            .collect();
        let evidence_refs: Vec<Value> = subtasks
            .iter()
            .filter_map(|(_, subtask)| {
                subtask
                    .get("evidence_ref")
                    .filter(|reference| !reference.is_null())
                    .cloned()
            })
            .collect();
        rtm_entries.push(json!({
            "userRequestRef": "request://submitted",
            "requirementId": seed.requirement_refs.first().cloned().unwrap_or_default(),
            "acceptanceCriterionIds": criterion_ids,
            "taskIds": [seed.id.clone()],
            "checklistSubtaskIds": subtask_ids,
            "evidenceRefs": evidence_refs,
            "verificationMethods": verification_methods,
            "expectedEvidenceTypes": evidence_types
        }));
        for criterion in &seed.acceptance {
            for (_, subtask) in &subtasks {
                if string_array(subtask, "acceptance_refs").contains(&criterion.id) {
                    traces.push(json!({
                        "userRequestRef": "request://submitted",
                        "requirementId": seed.requirement_refs.first().cloned().unwrap_or_default(),
                        "acceptanceCriterionId": criterion.id,
                        "taskId": seed.id,
                        "subtaskId": string_field(subtask, "id").unwrap_or_default(),
                        "expectedEvidenceType": string_field(subtask, "expected_evidence_type").unwrap_or_default(),
                        "evidenceRef": subtask.get("evidence_ref").cloned().unwrap_or(Value::Null)
                    }));
                }
            }
        }
    }
    let ticket_files: Vec<&str> = tickets
        .iter()
        .map(|ticket| ticket.file_name.as_str())
        .collect();
    (
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "artifact": "tasks",
            "planId": plan_id,
            "status": "ready",
            "tasks": task_entries,
            "ticketFiles": ticket_files
        }),
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "artifact": "rtm",
            "planId": plan_id,
            "status": "complete",
            "chain": RTM_CHAIN,
            "entries": rtm_entries,
            "traces": traces
        }),
    )
}

fn aggregate_task_status(ticket: &Value) -> &'static str {
    match string_field(ticket, "status") {
        Some("done") => "done",
        Some("blocked") => "blocked",
        Some("in_progress") => "in_progress",
        _ => "pending",
    }
}

fn validate_ticket_values(
    context: &ValidationContext<'_>,
    tickets: &[TicketArtifact],
    scopes: &[&str],
    derived_layers: &[&str],
    issues: &mut Vec<String>,
) {
    if tickets.len() != context.seeds.len() {
        issues.push(format!(
            "tasks.json has {} ticket file(s); expected {}",
            tickets.len(),
            context.seeds.len()
        ));
    }
    let mut all_subtask_ids = BTreeSet::new();
    for (index, seed) in context.seeds.iter().enumerate() {
        let Some(ticket) = tickets.get(index) else {
            issues.push(format!("{} has no task ticket file", seed.id));
            continue;
        };
        validate_ticket_identity(context.plan_id, seed, ticket, issues);
        validate_scope_contract(ticket, scopes, derived_layers, issues);
        validate_task_tree(ticket, issues);
        validate_ticket_subtasks(
            context,
            seed,
            ticket,
            derived_layers,
            &mut all_subtask_ids,
            issues,
        );
    }
}

fn validate_ticket_identity(
    plan_id: &str,
    seed: &TaskSeed,
    ticket: &TicketArtifact,
    issues: &mut Vec<String>,
) {
    if ticket.value.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        issues.push(format!("{} schema_version must be 1", ticket.file_name));
    }
    if string_field(&ticket.value, "artifact") != Some("task_ticket") {
        issues.push(format!("{} artifact must be task_ticket", ticket.file_name));
    }
    if string_field(&ticket.value, "plan_id") != Some(plan_id) {
        issues.push(format!(
            "{} plan_id does not match {plan_id}",
            ticket.file_name
        ));
    }
    if string_field(&ticket.value, "id") != Some(seed.id.as_str()) {
        issues.push(format!(
            "{} id does not match {}",
            ticket.file_name, seed.id
        ));
    }
    if string_field(&ticket.value, "title")
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        issues.push(format!("{} has no title", ticket.file_name));
    }
    if !matches!(
        string_field(&ticket.value, "status"),
        Some("planned" | "in_progress" | "blocked" | "done")
    ) {
        issues.push(format!("{} has invalid task status", ticket.file_name));
    }
    let expected_acceptance: Vec<String> = seed
        .acceptance
        .iter()
        .map(|criterion| criterion.id.clone())
        .collect();
    if sorted_strings(string_array(&ticket.value, "requirement_refs"))
        != sorted_strings(seed.requirement_refs.clone())
    {
        issues.push(format!(
            "{} requirement_refs do not match {}",
            ticket.file_name, seed.id
        ));
    }
    if sorted_strings(string_array(&ticket.value, "acceptance_refs"))
        != sorted_strings(expected_acceptance)
    {
        issues.push(format!(
            "{} acceptance_refs do not match {}",
            ticket.file_name, seed.id
        ));
    }
}

fn validate_scope_contract(
    ticket: &TicketArtifact,
    scopes: &[&str],
    derived_layers: &[&str],
    issues: &mut Vec<String>,
) {
    let recorded_scopes = string_array(&ticket.value, "derived_scopes");
    if sorted_strings(recorded_scopes)
        != sorted_strings(scopes.iter().map(|s| s.to_string()).collect())
    {
        issues.push(format!(
            "{} derived_scopes do not match detected scope",
            ticket.file_name
        ));
    }
    let recorded_layers = string_array(&ticket.value, "derived_layers");
    if sorted_strings(recorded_layers)
        != sorted_strings(derived_layers.iter().map(|s| s.to_string()).collect())
    {
        issues.push(format!(
            "{} derived_layers do not match detected scope",
            ticket.file_name
        ));
    }
    let Some(layers) = ticket.value.get("layers").and_then(Value::as_object) else {
        issues.push(format!("{} layers is not an object", ticket.file_name));
        return;
    };
    for layer in BASE_LAYERS {
        if !layers.get(layer).is_some_and(Value::is_array) {
            issues.push(format!("{} missing base layer {layer}", ticket.file_name));
        }
    }
    for layer in derived_layers {
        match layers.get(*layer).and_then(Value::as_array) {
            Some(subtasks) if !subtasks.is_empty() => {}
            _ => issues.push(format!(
                "{} missing derived layer {layer}",
                ticket.file_name
            )),
        }
    }
    for layer in layers.keys() {
        if !ALL_LAYERS.contains(&layer.as_str()) && layer != INSTALL_PARITY_LAYER {
            issues.push(format!("{} has unknown layer {layer}", ticket.file_name));
        }
    }
    for (layer, subtasks) in layers {
        if !subtasks.is_array() {
            issues.push(format!(
                "{} layer {layer} is not an array",
                ticket.file_name
            ));
        }
    }
}

fn validate_ticket_subtasks(
    context: &ValidationContext<'_>,
    seed: &TaskSeed,
    ticket: &TicketArtifact,
    derived_layers: &[&str],
    all_subtask_ids: &mut BTreeSet<String>,
    issues: &mut Vec<String>,
) {
    for (layer, subtask) in ticket_subtasks(&ticket.value) {
        let id = string_field(subtask, "id").unwrap_or("subtask without id");
        for field in [
            "id",
            "description",
            "requirement_refs",
            "acceptance_refs",
            "status",
            "expected_evidence_type",
            "evidence_ref",
            "reason",
            "owner_role",
            "verification_timestamp",
        ] {
            if subtask.get(field).is_none() {
                issues.push(format!("{id} is missing field {field}"));
            }
        }
        if id == "subtask without id" || !all_subtask_ids.insert(id.to_string()) {
            issues.push(format!(
                "{} has missing or duplicate subtask id {id}",
                ticket.file_name
            ));
        }
        if string_field(subtask, "description")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            issues.push(format!("{id} has no description"));
        }
        if sorted_strings(string_array(subtask, "requirement_refs"))
            != sorted_strings(seed.requirement_refs.clone())
        {
            issues.push(format!("{id} requirement_refs do not match its task"));
        }
        let expected_acceptance: Vec<String> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.id.clone())
            .collect();
        if sorted_strings(string_array(subtask, "acceptance_refs"))
            != sorted_strings(expected_acceptance)
        {
            issues.push(format!("{id} acceptance_refs do not match its task"));
        }
        let evidence_type = string_field(subtask, "expected_evidence_type").unwrap_or_default();
        if !EVIDENCE_TYPES.contains(&evidence_type) {
            issues.push(format!("{id} has unsupported expected_evidence_type"));
        }
        if !matches!(
            string_field(subtask, "owner_role"),
            Some("implementer" | "verifier" | "reviewer" | "human")
        ) {
            issues.push(format!("{id} has invalid owner_role"));
        }
        let status = string_field(subtask, "status").unwrap_or_default();
        if !matches!(
            status,
            "open" | "done" | "skipped" | "not_applicable" | "needs_human"
        ) {
            issues.push(format!("{id} has invalid subtask status"));
            continue;
        }
        if matches!(status, "skipped" | "not_applicable" | "needs_human")
            && string_field(subtask, "reason")
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            issues.push(format!("{id} status {status} requires a non-empty reason"));
        }
        if derived_layers.contains(&layer)
            && subtask.get("derived").and_then(Value::as_bool) != Some(true)
        {
            issues.push(format!("{id} must retain derived=true"));
        }
        if status == "done" {
            if subtask.get("evidence_ref").map_or(true, Value::is_null) {
                issues.push(format!("{id} is done without evidence_ref"));
            }
            validate_timestamp(subtask, id, issues);
        }
        if let Some(reference) = subtask
            .get("evidence_ref")
            .filter(|reference| !reference.is_null())
        {
            if status != "done" {
                issues.push(format!("{id} has evidence_ref but status is not done"));
            }
            validate_evidence_reference(context, seed, id, subtask, reference, issues);
        }
    }
}

fn validate_timestamp(subtask: &Value, id: &str, issues: &mut Vec<String>) {
    let timestamp = string_field(subtask, "verification_timestamp").unwrap_or_default();
    if timestamp.is_empty() || DateTime::parse_from_rfc3339(timestamp).is_err() {
        issues.push(format!(
            "{id} done status requires an RFC3339 verification_timestamp"
        ));
    }
}

fn validate_evidence_reference(
    context: &ValidationContext<'_>,
    seed: &TaskSeed,
    subtask_id: &str,
    subtask: &Value,
    reference: &Value,
    issues: &mut Vec<String>,
) {
    let Some(reference) = reference.as_object() else {
        issues.push(format!("{subtask_id} evidence_ref is not an object"));
        return;
    };
    let path_value = reference
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expected_hash = reference
        .get("content_hash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(path) = resolve_regular_plan_file(context.plan_directory, path_value, issues) else {
        issues.push(format!("{subtask_id} evidence_ref path is not resolvable"));
        return;
    };
    let body = match read_bounded_text(&path, path_value, issues) {
        Some(body) => body,
        None => return,
    };
    let actual_hash = format!("fnv1a64:{}", crate::utility::hashing::fnv1a64_hex(&body));
    if expected_hash != actual_hash {
        issues.push(format!(
            "{subtask_id} content_hash does not match evidence artifact"
        ));
        return;
    }
    let evidence: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            issues.push(format!("parse evidence {path_value}: {error}"));
            return;
        }
    };
    if evidence.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION)
        || string_field(&evidence, "artifact") != Some("task_evidence")
    {
        issues.push(format!("{subtask_id} evidence artifact schema is invalid"));
    }
    for (field, expected) in [
        ("plan_id", context.plan_id),
        ("task_id", seed.id.as_str()),
        ("subtask_id", subtask_id),
        (
            "evidence_type",
            string_field(subtask, "expected_evidence_type").unwrap_or_default(),
        ),
        (
            "recorded_at",
            string_field(subtask, "verification_timestamp").unwrap_or_default(),
        ),
    ] {
        if string_field(&evidence, field) != Some(expected) {
            issues.push(format!(
                "{subtask_id} evidence {field} does not match ticket"
            ));
        }
    }
    let evidence_type = string_field(&evidence, "evidence_type").unwrap_or_default();
    validate_evidence_payload(
        context.keel_home,
        context.workspace_root,
        subtask_id,
        evidence_type,
        &evidence,
        issues,
    );
}

fn validate_evidence_payload(
    keel_home: &Path,
    workspace_root: &Path,
    subtask_id: &str,
    evidence_type: &str,
    evidence: &Value,
    issues: &mut Vec<String>,
) {
    match evidence_type {
        "command" => {
            require_text(evidence, "command", subtask_id, issues);
            require_zero_exit(evidence, subtask_id, issues);
            require_sha256(evidence, "output_hash", subtask_id, issues);
        }
        "named_test" => {
            require_text(evidence, "test_name", subtask_id, issues);
            require_pass(evidence, subtask_id, issues);
            require_sha256(evidence, "output_hash", subtask_id, issues);
        }
        "lint_diagnostic" => {
            require_text(evidence, "tool", subtask_id, issues);
            require_zero_exit(evidence, subtask_id, issues);
            require_sha256(evidence, "output_hash", subtask_id, issues);
        }
        "source_hash" => {
            let source_path = require_text(evidence, "source_path", subtask_id, issues);
            require_sha256(evidence, "source_hash", subtask_id, issues);
            if let Some(source_path) = source_path {
                if resolve_regular_workspace_file(workspace_root, source_path).is_none() {
                    issues.push(format!(
                        "{subtask_id} source_path is not machine-resolvable"
                    ));
                }
            }
        }
        "raw_store" => {
            let raw_id = require_text(evidence, "raw_store_id", subtask_id, issues);
            if let Some(raw_id) = raw_id {
                let store =
                    crate::proxy::raw_store::RawStore::with_root(keel_home.join("raw-output"));
                match store.load_meta(raw_id) {
                    Ok(meta) if meta.exit_code == 0 => {}
                    Ok(meta) => issues.push(format!(
                        "{subtask_id} RawStore {raw_id} exit code is {}",
                        meta.exit_code
                    )),
                    Err(error) => issues.push(format!(
                        "{subtask_id} RawStore {raw_id} is not resolvable: {error}"
                    )),
                }
            }
        }
        "screenshot" => {
            require_text(evidence, "artifact_id", subtask_id, issues);
            if string_field(evidence, "visual_verdict") != Some("pass") {
                issues.push(format!(
                    "{subtask_id} screenshot visual_verdict is not pass"
                ));
            }
            require_sha256(evidence, "artifact_hash", subtask_id, issues);
        }
        "benchmark" => {
            require_text(evidence, "artifact_id", subtask_id, issues);
            require_pass(evidence, subtask_id, issues);
            require_sha256(evidence, "output_hash", subtask_id, issues);
        }
        _ => issues.push(format!(
            "{subtask_id} has unsupported evidence_type {evidence_type}"
        )),
    }
}

fn require_text<'a>(
    value: &'a Value,
    field: &str,
    id: &str,
    issues: &mut Vec<String>,
) -> Option<&'a str> {
    let text = string_field(value, field).unwrap_or_default();
    if text.trim().is_empty() {
        issues.push(format!("{id} evidence {field} is required"));
        None
    } else {
        Some(text)
    }
}

fn require_zero_exit(value: &Value, id: &str, issues: &mut Vec<String>) {
    if value.get("exit_code").and_then(Value::as_i64) != Some(0) {
        issues.push(format!("{id} evidence exit_code is not zero"));
    }
}

fn require_pass(value: &Value, id: &str, issues: &mut Vec<String>) {
    if string_field(value, "result") != Some("pass") {
        issues.push(format!("{id} evidence result is not pass"));
    }
}

fn require_sha256(value: &Value, field: &str, id: &str, issues: &mut Vec<String>) {
    let hash = string_field(value, field).unwrap_or_default();
    let Some(digest) = hash.strip_prefix("sha256:") else {
        issues.push(format!("{id} evidence {field} is not a sha256 hash"));
        return;
    };
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        issues.push(format!("{id} evidence {field} is not a sha256 hash"));
    }
}

fn validate_rtm_links(
    seeds: &[TaskSeed],
    tickets: &[TicketArtifact],
    rtm: &Value,
    issues: &mut Vec<String>,
) {
    if string_field(rtm, "chain") != Some(RTM_CHAIN) {
        issues.push("rtm.json is missing the full request-to-evidence chain".to_string());
    }
    let traces = rtm
        .get("traces")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut task_map = BTreeMap::new();
    let mut subtask_map = BTreeMap::new();
    for (seed, ticket) in seeds.iter().zip(tickets) {
        task_map.insert(seed.id.as_str(), seed);
        for (_, subtask) in ticket_subtasks(&ticket.value) {
            if let Some(id) = string_field(subtask, "id") {
                subtask_map.insert(id, (seed, subtask));
            }
        }
    }
    validate_rtm_entries(seeds, tickets, rtm, &subtask_map, issues);
    let mut seen_trace_keys = BTreeSet::new();
    for trace in traces {
        let request_ref = string_field(trace, "userRequestRef").unwrap_or_default();
        let requirement_id = string_field(trace, "requirementId").unwrap_or_default();
        let criterion_id = string_field(trace, "acceptanceCriterionId").unwrap_or_default();
        let task_id = string_field(trace, "taskId").unwrap_or_default();
        let subtask_id = string_field(trace, "subtaskId").unwrap_or_default();
        if request_ref != "request://submitted" {
            issues.push(format!(
                "RTM trace {task_id}/{subtask_id} has unknown user request"
            ));
        }
        let Some(seed) = task_map.get(task_id) else {
            issues.push(format!("RTM trace references unknown task {task_id}"));
            continue;
        };
        if !seed.requirement_refs.iter().any(|id| id == requirement_id) {
            issues.push(format!(
                "RTM trace references unknown requirement {requirement_id}"
            ));
        }
        if !seed
            .acceptance
            .iter()
            .any(|criterion| criterion.id == criterion_id)
        {
            issues.push(format!(
                "RTM trace references unknown acceptance criterion {criterion_id}"
            ));
        }
        let Some((subtask_seed, subtask)) = subtask_map.get(subtask_id) else {
            issues.push(format!("RTM trace references unknown subtask {subtask_id}"));
            continue;
        };
        if subtask_seed.id != seed.id
            || !string_array(subtask, "requirement_refs").contains(&requirement_id.to_string())
            || !string_array(subtask, "acceptance_refs").contains(&criterion_id.to_string())
        {
            issues.push(format!(
                "RTM trace {task_id}/{subtask_id} has inconsistent links"
            ));
        }
        if string_field(trace, "expectedEvidenceType")
            != string_field(subtask, "expected_evidence_type")
        {
            issues.push(format!(
                "RTM trace {task_id}/{subtask_id} evidence type drifted"
            ));
        }
        let expected_reference = subtask.get("evidence_ref").unwrap_or(&Value::Null);
        if trace.get("evidenceRef").unwrap_or(&Value::Null) != expected_reference {
            issues.push(format!(
                "RTM trace {task_id}/{subtask_id} evidence reference drifted"
            ));
        }
        let key = format!("{criterion_id}\0{task_id}\0{subtask_id}");
        if !seen_trace_keys.insert(key) {
            issues.push(format!(
                "RTM repeats trace {criterion_id}/{task_id}/{subtask_id}"
            ));
        }
    }
    for seed in seeds {
        for criterion in &seed.acceptance {
            if !traces.iter().any(|trace| {
                string_field(trace, "acceptanceCriterionId") == Some(criterion.id.as_str())
                    && string_field(trace, "taskId") == Some(seed.id.as_str())
                    && string_field(trace, "subtaskId")
                        .is_some_and(|id| subtask_map.contains_key(id))
            }) {
                issues.push(format!(
                    "{} has no evidence-producing RTM trace",
                    criterion.id
                ));
            }
        }
    }
    for (subtask_id, (seed, subtask)) in &subtask_map {
        for criterion_id in string_array(subtask, "acceptance_refs") {
            if !traces.iter().any(|trace| {
                string_field(trace, "acceptanceCriterionId") == Some(criterion_id.as_str())
                    && string_field(trace, "taskId") == Some(seed.id.as_str())
                    && string_field(trace, "subtaskId") == Some(*subtask_id)
            }) {
                issues.push(format!("{subtask_id} has no RTM trace for {criterion_id}"));
            }
        }
    }
}

fn validate_aggregate_ticket_links(
    tasks: &Value,
    seeds: &[TaskSeed],
    tickets: &[TicketArtifact],
    issues: &mut Vec<String>,
) {
    let entries = tasks
        .get("tasks")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if entries.len() != seeds.len() {
        issues.push(format!(
            "tasks.json has {} aggregate task(s); expected {}",
            entries.len(),
            seeds.len()
        ));
    }
    for (seed, ticket) in seeds.iter().zip(tickets) {
        let Some(entry) = entries
            .iter()
            .find(|entry| string_field(entry, "taskId") == Some(seed.id.as_str()))
        else {
            issues.push(format!("tasks.json missing aggregate {}", seed.id));
            continue;
        };
        if string_field(entry, "ticketFile") != Some(ticket.file_name.as_str()) {
            issues.push(format!(
                "tasks.json {} ticketFile does not resolve to {}",
                seed.id, ticket.file_name
            ));
        }
        if sorted_strings(string_array(entry, "requirementIds"))
            != sorted_strings(seed.requirement_refs.clone())
        {
            issues.push(format!(
                "tasks.json {} requirementIds do not match its ticket",
                seed.id
            ));
        }
        let expected_acceptance: Vec<String> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.id.clone())
            .collect();
        if sorted_strings(string_array(entry, "acceptanceCriterionIds"))
            != sorted_strings(expected_acceptance)
        {
            issues.push(format!(
                "tasks.json {} acceptanceCriterionIds do not match its ticket",
                seed.id
            ));
        }
    }
    for entry in entries {
        let task_id = string_field(entry, "taskId").unwrap_or_default();
        if !seeds.iter().any(|seed| seed.id == task_id) {
            issues.push(format!("tasks.json references unknown task {task_id}"));
        }
    }
}

fn validate_rtm_entries<'a>(
    seeds: &[TaskSeed],
    tickets: &[TicketArtifact],
    rtm: &Value,
    subtask_map: &BTreeMap<&'a str, (&'a TaskSeed, &'a Value)>,
    issues: &mut Vec<String>,
) {
    let entries = rtm
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if entries.len() != seeds.len() {
        issues.push(format!(
            "rtm.json has {} requirement entries; expected {}",
            entries.len(),
            seeds.len()
        ));
    }
    for (seed, ticket) in seeds.iter().zip(tickets) {
        let requirement_id = seed
            .requirement_refs
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        let Some(entry) = entries
            .iter()
            .find(|entry| string_field(entry, "requirementId") == Some(requirement_id))
        else {
            continue;
        };
        if string_field(entry, "userRequestRef") != Some("request://submitted") {
            issues.push(format!("RTM {requirement_id} has unknown user request"));
        }
        if string_array(entry, "taskIds") != [seed.id.clone()] {
            issues.push(format!(
                "RTM {requirement_id} taskIds do not match {}",
                seed.id
            ));
        }
        let expected_acceptance: Vec<String> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.id.clone())
            .collect();
        if sorted_strings(string_array(entry, "acceptanceCriterionIds"))
            != sorted_strings(expected_acceptance)
        {
            issues.push(format!(
                "RTM {requirement_id} acceptanceCriterionIds do not match {}",
                seed.id
            ));
        }
        let expected_verification: Vec<String> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.verification_method.clone())
            .collect();
        if sorted_strings(string_array(entry, "verificationMethods"))
            != sorted_strings(expected_verification)
        {
            issues.push(format!(
                "RTM {requirement_id} verificationMethods do not match {}",
                seed.id
            ));
        }
        let expected_evidence_types: Vec<String> = seed
            .acceptance
            .iter()
            .map(|criterion| criterion.expected_evidence_type.clone())
            .collect();
        if sorted_strings(string_array(entry, "expectedEvidenceTypes"))
            != sorted_strings(expected_evidence_types)
        {
            issues.push(format!(
                "RTM {requirement_id} expectedEvidenceTypes do not match {}",
                seed.id
            ));
        }
        let expected_subtasks: Vec<String> = ticket_subtasks(&ticket.value)
            .into_iter()
            .filter_map(|(_, subtask)| string_field(subtask, "id").map(str::to_string))
            .collect();
        if sorted_strings(string_array(entry, "checklistSubtaskIds"))
            != sorted_strings(expected_subtasks)
        {
            issues.push(format!(
                "RTM {requirement_id} checklistSubtaskIds do not match {}",
                seed.id
            ));
        }
        let expected_evidence: Vec<String> = ticket_subtasks(&ticket.value)
            .into_iter()
            .filter_map(|(_, subtask)| {
                subtask
                    .get("evidence_ref")
                    .filter(|reference| !reference.is_null())
                    .and_then(|reference| serde_json::to_string(reference).ok())
            })
            .collect();
        let recorded_evidence: Vec<String> = entry
            .get("evidenceRefs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|reference| serde_json::to_string(reference).ok())
            .collect();
        if sorted_strings(recorded_evidence) != sorted_strings(expected_evidence) {
            issues.push(format!(
                "RTM {requirement_id} evidenceRefs do not match {}",
                seed.id
            ));
        }
    }
    for entry in entries {
        let requirement_id = string_field(entry, "requirementId").unwrap_or_default();
        if !seeds.iter().any(|seed| {
            seed.requirement_refs
                .iter()
                .any(|requirement| requirement == requirement_id)
        }) {
            issues.push(format!(
                "rtm.json references unknown requirement {requirement_id}"
            ));
        }
        for subtask_id in string_array(entry, "checklistSubtaskIds") {
            if !subtask_map.contains_key(subtask_id.as_str()) {
                issues.push(format!(
                    "RTM {requirement_id} references unknown subtask {subtask_id}"
                ));
            }
        }
    }
}

fn ticket_subtasks(ticket: &Value) -> Vec<(&str, &Value)> {
    let mut subtasks = Vec::new();
    let Some(layers) = ticket.get("layers").and_then(Value::as_object) else {
        return subtasks;
    };
    for (layer, values) in layers {
        if let Some(values) = values.as_array() {
            for value in values {
                subtasks.push((layer.as_str(), value));
                if let Some(todos) = value.get("todos").and_then(Value::as_array) {
                    subtasks.extend(todos.iter().map(|todo| (layer.as_str(), todo)));
                }
            }
        }
    }
    subtasks
}

fn validate_task_tree(ticket: &TicketArtifact, issues: &mut Vec<String>) {
    let nodes = ticket_subtasks(&ticket.value);
    let by_id: BTreeMap<&str, &Value> = nodes
        .iter()
        .filter_map(|(_, node)| string_field(node, "id").map(|id| (id, *node)))
        .collect();
    let task_id = string_field(&ticket.value, "id").unwrap_or_default();
    for (_, node) in &nodes {
        let id = string_field(node, "id").unwrap_or_default();
        if let Some(dependencies) = node.get("dependencies") {
            if !dependencies.as_array().is_some_and(|values| {
                values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))
            }) {
                issues.push(format!(
                    "{id} dependencies must be an array of non-empty IDs"
                ));
            }
        }
        if let Some(todos) = node.get("todos") {
            let Some(todos) = todos.as_array() else {
                issues.push(format!("{id} todos must be an array"));
                continue;
            };
            for todo in todos {
                if string_field(todo, "parent_id") != Some(id) {
                    issues.push(format!(
                        "{id} todo parent_id must match its containing subtask"
                    ));
                }
                if todo
                    .get("todos")
                    .and_then(Value::as_array)
                    .is_some_and(|children| !children.is_empty())
                {
                    issues.push(format!(
                        "{id} task hierarchy exceeds parent/subtask/todo depth"
                    ));
                }
                if string_field(node, "status") == Some("done")
                    && !matches!(
                        string_field(todo, "status"),
                        Some("done" | "not_applicable")
                    )
                {
                    issues.push(format!("{id} is done with unfinished todos"));
                }
            }
        }
        if let Some(parent) = string_field(node, "parent_id") {
            if parent != task_id && !by_id.contains_key(parent) {
                issues.push(format!("{id} references unknown parent {parent}"));
            }
        }
        let mut pending = string_array(node, "dependencies");
        let mut visited = BTreeSet::new();
        while let Some(dependency) = pending.pop() {
            if dependency == id {
                issues.push(format!("{id} has a dependency cycle"));
                break;
            }
            if !visited.insert(dependency.clone()) {
                continue;
            }
            match by_id.get(dependency.as_str()) {
                Some(target) => {
                    if string_field(node, "status") == Some("done")
                        && !matches!(
                            string_field(target, "status"),
                            Some("done" | "not_applicable")
                        )
                    {
                        issues.push(format!(
                            "{id} is done with unfinished dependency {dependency}"
                        ));
                    }
                    pending.extend(string_array(target, "dependencies"));
                }
                None => issues.push(format!("{id} references unknown dependency {dependency}")),
            }
        }
    }
    if string_field(&ticket.value, "status") == Some("done")
        && nodes.iter().any(|(_, node)| {
            !matches!(
                string_field(node, "status"),
                Some("done" | "not_applicable")
            )
        })
    {
        issues.push(format!("{task_id} is done with unfinished subtasks"));
    }
}

fn validate_unindexed_ticket_files(
    plan_directory: &Path,
    tickets: &[TicketArtifact],
    issues: &mut Vec<String>,
) {
    let indexed: BTreeSet<&str> = tickets
        .iter()
        .map(|ticket| ticket.file_name.as_str())
        .collect();
    let Ok(entries) = fs::read_dir(plan_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_ticket_file_name(name) && !indexed.contains(name) {
            issues.push(format!("unindexed task ticket {name}"));
        }
    }
}

fn is_ticket_file_name(file_name: &str) -> bool {
    let Some(number) = file_name
        .strip_prefix("task-")
        .and_then(|value| value.strip_suffix(".json"))
    else {
        return false;
    };
    number.len() == 3 && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_bounded_json(path: &Path, label: &str, issues: &mut Vec<String>) -> Option<Value> {
    let body = read_bounded_text(path, label, issues)?;
    match serde_json::from_str(&body) {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(format!("parse {label}: {error}"));
            None
        }
    }
}

fn read_bounded_text(path: &Path, label: &str, issues: &mut Vec<String>) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            issues.push(format!("{label} is not a regular file"));
            return None;
        }
        Ok(metadata) => metadata,
        Err(error) => {
            issues.push(format!("read {label}: {error}"));
            return None;
        }
    };
    if metadata.len() > MAX_TICKET_BYTES {
        issues.push(format!(
            "{label} exceeds the {MAX_TICKET_BYTES}-byte input bound"
        ));
        return None;
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            issues.push(format!("read {label}: {error}"));
            return None;
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.take(MAX_TICKET_BYTES + 1).read_to_end(&mut bytes) {
        issues.push(format!("read {label}: {error}"));
        return None;
    }
    if bytes.len() as u64 > MAX_TICKET_BYTES {
        issues.push(format!(
            "{label} exceeds the {MAX_TICKET_BYTES}-byte input bound"
        ));
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(body) => Some(body),
        Err(error) => {
            issues.push(format!("read {label}: invalid UTF-8: {error}"));
            None
        }
    }
}

fn resolve_regular_plan_file(
    plan_directory: &Path,
    relative: &str,
    issues: &mut Vec<String>,
) -> Option<PathBuf> {
    let path = Path::new(relative);
    if relative.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        issues.push(format!("unsafe evidence path {relative:?}"));
        return None;
    }
    let candidate = plan_directory.join(path);
    let metadata = fs::symlink_metadata(&candidate).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        issues.push(format!("evidence path is not a regular file: {relative}"));
        return None;
    }
    let canonical_plan = plan_directory.canonicalize().ok()?;
    let canonical_candidate = candidate.canonicalize().ok()?;
    if !canonical_candidate.starts_with(&canonical_plan) {
        issues.push(format!(
            "evidence path escapes the plan directory: {relative}"
        ));
        return None;
    }
    Some(canonical_candidate)
}

fn resolve_regular_workspace_file(workspace_root: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    if relative.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let candidate = workspace_root.join(path);
    let metadata = fs::symlink_metadata(&candidate).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let canonical_root = workspace_root.canonicalize().ok()?;
    let canonical_candidate = candidate.canonicalize().ok()?;
    canonical_candidate
        .starts_with(canonical_root)
        .then_some(canonical_candidate)
}

fn sorted_strings(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_todos_participate_in_traceability_and_completion() {
        let mut ticket = TicketArtifact {
            file_name: "task-001.json".into(),
            write_required: false,
            value: json!({"id":"TASK-1", "status":"planned", "layers":{"tests":[{
                "id":"SUB-1", "parent_id":"TASK-1", "status":"done", "todos":[{
                    "id":"TODO-1", "parent_id":"SUB-1", "status":"open", "dependencies":[]
                }]
            }]}}),
        };
        assert_eq!(ticket_subtasks(&ticket.value).len(), 2);
        let mut issues = Vec::new();
        validate_task_tree(&ticket, &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.contains("unfinished todos")));
        ticket.value["layers"]["tests"][0]["todos"][0]["status"] = json!("done");
        issues.clear();
        validate_task_tree(&ticket, &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn task_tree_rejects_unknown_dependencies_and_cycles() {
        let ticket = TicketArtifact {
            file_name: "task-001.json".into(),
            write_required: false,
            value: json!({"id":"TASK-1", "layers":{"tests":[
                {"id":"A", "dependencies":["B"]},
                {"id":"B", "dependencies":["A", "missing"]}
            ]}}),
        };
        let mut issues = Vec::new();
        validate_task_tree(&ticket, &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.contains("dependency cycle")));
        assert!(issues
            .iter()
            .any(|issue| issue.contains("unknown dependency missing")));
    }

    #[test]
    fn todo_evidence_is_checked_by_the_existing_ticket_owner_and_rtm() {
        let seed = TaskSeed {
            id: "TASK-1".into(),
            title: "Test behavior".into(),
            requirement_refs: vec!["REQ-1".into()],
            acceptance: vec![AcceptanceSeed {
                id: "AC-1".into(),
                verification_method: "named test".into(),
                expected_evidence_type: "named_test".into(),
            }],
        };
        let mut value = ticket_template("PLAN-1", &seed, &[], &["tests"]);
        let mut todo = value["layers"]["tests"][0].clone();
        todo["id"] = json!("TODO-1");
        todo["parent_id"] = json!("TASK-1-TESTS-001");
        todo["status"] = json!("done");
        value["layers"]["tests"][0]["todos"] = json!([todo]);
        let ticket = TicketArtifact {
            file_name: "task-001.json".into(),
            value,
            write_required: false,
        };
        let seeds = vec![seed];
        let context = ValidationContext {
            plan_id: "PLAN-1",
            plan_directory: Path::new("."),
            keel_home: Path::new("."),
            workspace_root: Path::new("."),
            specification: "",
            architecture: "",
            seeds: &seeds,
        };
        let mut issues = Vec::new();
        validate_ticket_subtasks(
            &context,
            &seeds[0],
            &ticket,
            &["tests"],
            &mut BTreeSet::new(),
            &mut issues,
        );
        assert!(issues
            .iter()
            .any(|issue| issue == "TODO-1 is done without evidence_ref"));
        let (_, rtm) = build_aggregate_and_rtm("PLAN-1", &seeds, &[ticket]);
        assert!(rtm["traces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|trace| trace["subtaskId"] == "TODO-1"));
    }

    #[test]
    fn scope_derivation_does_not_read_unrelated_architecture_sections() {
        let spec = "## 1. User request verbatim\n\n    Update docs.\n\n## 2. Restated";
        let architecture = "## 3. Components/files/interfaces changed\n\n- Component: docs/planner.md\n\n## 4. Data/control flow\n\nPerformance token host adapter UI dependency";
        assert!(derive_scopes(spec, architecture).is_empty());
    }

    #[test]
    fn scope_derivation_maps_source_and_public_cli() {
        let spec = "## 1. User request verbatim\n\n    Add a public CLI command.\n\n## 2. Restated";
        let architecture = "## 3. Components/files/interfaces changed\n\n- Component: src/main.rs\n\n## 4. Data/control flow";
        assert_eq!(
            derive_scopes(spec, architecture),
            ["source_code", "public_behavior_api"]
        );
    }

    #[test]
    fn scope_layer_matrix_covers_every_declared_requirement() {
        let spec = "## 1. User request verbatim\n\n    Change a public API UI dependency benchmark data migration host adapter install.\n\n## 2. Restated";
        let architecture = "## 3. Components/files/interfaces changed\n\n- Component: src/main.rs\n\n## 4. Data/control flow";
        let scopes = derive_scopes(spec, architecture);
        assert_eq!(
            scopes,
            [
                "source_code",
                "public_behavior_api",
                "ui_behavior",
                "dependency_change",
                "token_performance_change",
                "data_memory_change",
                "host_integration_change"
            ]
        );
        let layers = derived_layers(&scopes);
        for required in [
            "design",
            "implementation",
            "build",
            "tests",
            "lint_warnings",
            "docs",
            "compatibility",
            "security",
            "release_rollback",
            "ui",
            "ux_accessibility",
            "screenshots",
            "test_fixtures",
            "license_audit",
            "reproducibility",
            "bloat_analysis",
            "benchmark",
            "performance_tokens",
            "fixed_context_runtime",
            "migration",
            "privacy",
            "integrity",
            "rollback",
            "host_adapter_contracts",
            "install_provision_parity",
        ] {
            assert!(
                layers.contains(&required),
                "missing derived layer {required}"
            );
        }
    }
}
