//! Purpose: Implements the previously-planned memory command families on top of the
//!   shared scoped record store: research-cache, maintenance, agent-registry,
//!   agent-packets, loop-guard, entity, graph, retrieve, and a status summary.
//! Caller: utility::memory::run_memory_command dispatch for both the `memory` and
//!   `memoriesv2` command groups.
//! Dependencies: crate::args::FlagSet, crate::json::{write_indented, Value},
//!   crate::runtime::{display_path, resolve_claude_home}, crate::utility::record_store,
//!   crate::utility::workflow_ledger timestamp helpers.
//! Main Functions: run_memory_family_command (the single dispatch entry the memory module calls).
//! Side Effects: Reads and writes flat-string JSON records under
//!   `<claude-home>/<group>/<family>/<id>.json`. No global state.
//!
//! Design: every family is a thin handler over `RecordStore`, mirroring the
//! workflow ledger and working-brief storage shapes already in the tree rather
//! than introducing a new persistence concept. `<group>` is `memories` or
//! `memoriesv2` so the two command groups stay isolated on disk exactly like
//! their scope-resolve paths do.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::record_store::{field, join_lines, record_to_value, Record, RecordStore};
use crate::utility::workflow_ledger::{current_timestamp_millis, format_timestamp_iso8601};

/// Dispatch entry for the memory command families. `family` is the already-matched
/// subcommand name (`research-cache`, `entity`, ...) and `arguments` is everything
/// after it.
pub fn run_memory_family_command(
    command_group: &str,
    family: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    match family {
        "research-cache" => {
            run_research_cache(command_group, arguments, standard_output, standard_error)
        }
        "maintenance" => run_maintenance(command_group, arguments, standard_output, standard_error),
        "agent-registry" => {
            run_agent_registry(command_group, arguments, standard_output, standard_error)
        }
        "agent-packets" => {
            run_agent_packets(command_group, arguments, standard_output, standard_error)
        }
        "loop-guard" => run_loop_guard(command_group, arguments, standard_output, standard_error),
        "entity" => run_entity(command_group, arguments, standard_output, standard_error),
        "graph" => run_graph(command_group, arguments, standard_output, standard_error),
        "retrieve" => run_retrieve(command_group, arguments, standard_output, standard_error),
        "status" => run_status(command_group, arguments, standard_output, standard_error),
        "instincts" => run_instincts(command_group, arguments, standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "{command_group}: unknown family {other}");
            1
        }
    }
}

/// Resolve the claude home for the supplied `--claude-home` flag (empty = default).
fn resolve_home(
    flag_value: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<std::path::PathBuf> {
    match resolve_claude_home(flag_value) {
        Ok(path) => Some(path),
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            None
        }
    }
}

/// Build a store under `<claude_home>/<group>/<family>`.
fn family_store(claude_home: &Path, command_group: &str, family: &str) -> RecordStore {
    RecordStore::new(claude_home, &format!("{command_group}/{family}"))
}

fn now_id(prefix: &str) -> (String, String) {
    let millis = current_timestamp_millis();
    (
        format!("{prefix}-{millis:x}"),
        format_timestamp_iso8601(millis),
    )
}

fn render_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(error) = write_indented(standard_output, value) {
        let _ = writeln!(standard_error, "render JSON: {error}");
        return 1;
    }
    0
}

// ---------------------------------------------------------------------------
// research-cache: record | lookup | stale | reward | list
// ---------------------------------------------------------------------------

fn run_research_cache(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} research-cache");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} research-cache [record|lookup|stale|reward|list] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "record" => {
            let mut flags = FlagSet::new(format!("{label} record"));
            flags.string_flag("question", "");
            flags.string_flag("answer", "");
            flags.string_flag("source", "");
            flags.string_flag("freshness", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let question = flags.string_value("question").trim().to_string();
            let answer = flags.string_value("answer").trim().to_string();
            if question.is_empty() || answer.is_empty() {
                let _ = writeln!(
                    standard_error,
                    "{label} record: --question and --answer are required"
                );
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let (id, at) = now_id("rc");
            let record: Record = vec![
                ("id".into(), id.clone()),
                ("question".into(), question),
                ("answer".into(), answer),
                (
                    "source".into(),
                    flags.string_value("source").trim().to_string(),
                ),
                (
                    "freshness".into(),
                    flags.string_value("freshness").trim().to_string(),
                ),
                ("state".into(), "fresh".into()),
                ("recordedAt".into(), at),
            ];
            let store = family_store(&home, command_group, "research-cache");
            match store.write_record(&id, &record) {
                Ok(path) => emit_created(
                    &label,
                    &id,
                    &path,
                    &record,
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                ),
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "lookup" => {
            let mut flags = FlagSet::new(format!("{label} lookup"));
            flags.string_flag("query", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let mut query = flags.string_value("query").trim().to_lowercase();
            if query.is_empty() && !flags.positional.is_empty() {
                query = flags.positional.join(" ").to_lowercase();
            }
            if query.is_empty() {
                let _ = writeln!(standard_error, "{label} lookup: --query is required");
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let store = family_store(&home, command_group, "research-cache");
            let records = match store.list_records() {
                Ok(records) => records,
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    return 1;
                }
            };
            let matches: Vec<&Record> = records
                .iter()
                .map(|(_, record)| record)
                .filter(|record| {
                    let haystack = format!(
                        "{} {}",
                        field(record, "question").unwrap_or(""),
                        field(record, "answer").unwrap_or("")
                    )
                    .to_lowercase();
                    query.split_whitespace().all(|term| haystack.contains(term))
                })
                .collect();
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("count".into(), Value::Number(matches.len().to_string())),
                    (
                        "matches".into(),
                        Value::Array(
                            matches
                                .iter()
                                .map(|record| record_to_value(record))
                                .collect(),
                        ),
                    ),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(standard_output, "{label}: {} match(es)", matches.len());
            for record in &matches {
                let _ = writeln!(
                    standard_output,
                    "  [{}] {} -> {}",
                    field(record, "state").unwrap_or("?"),
                    field(record, "question").unwrap_or(""),
                    field(record, "answer").unwrap_or("")
                );
            }
            0
        }
        "stale" | "reward" => {
            let new_state = if arguments[0] == "stale" {
                "stale"
            } else {
                "rewarded"
            };
            let mut flags = FlagSet::new(format!("{label} {}", arguments[0]));
            flags.string_flag("id", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let id = flags.string_value("id").trim().to_string();
            if id.is_empty() {
                let _ = writeln!(standard_error, "{label} {}: --id is required", arguments[0]);
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let store = family_store(&home, command_group, "research-cache");
            let mut record = match store.read_record(&id) {
                Ok(Some(record)) => record,
                Ok(None) => {
                    let _ = writeln!(standard_error, "{label}: no cache entry with id {id}");
                    return 1;
                }
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    return 1;
                }
            };
            set_field(&mut record, "state", new_state.to_string());
            match store.write_record(&id, &record) {
                Ok(path) => {
                    if flags.bool_value("json") {
                        let payload = Value::Object(vec![
                            ("updated".into(), Value::Bool(true)),
                            ("entry".into(), record_to_value(&record)),
                        ]);
                        return render_json(standard_output, standard_error, &payload);
                    }
                    let _ = writeln!(standard_output, "{label}: {id} -> {new_state}");
                    let _ = writeln!(standard_output, "  {}", display_path(&path));
                    0
                }
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "list" => list_family(
            command_group,
            "research-cache",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected record|lookup|stale|reward|list)"
            );
            1
        }
    }
}

// ---------------------------------------------------------------------------
// maintenance: append-working-buffer | trim | recalibrate
// ---------------------------------------------------------------------------

fn run_maintenance(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} maintenance");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} maintenance [append-working-buffer|trim|recalibrate] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "append-working-buffer" => {
            let mut flags = FlagSet::new(format!("{label} append-working-buffer"));
            flags.string_flag("note", "");
            flags.string_flag("claude-home", "");
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let mut note = flags.string_value("note").trim().to_string();
            if note.is_empty() && !flags.positional.is_empty() {
                note = flags.positional.join(" ");
            }
            if note.trim().is_empty() {
                let _ = writeln!(
                    standard_error,
                    "{label} append-working-buffer: --note is required"
                );
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let buffer_path = home.join(command_group).join("working-buffer.md");
            if let Some(parent) = buffer_path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    let _ = writeln!(
                        standard_error,
                        "{label}: create {}: {error}",
                        display_path(parent)
                    );
                    return 1;
                }
            }
            let (_, at) = now_id("wb");
            let existing = std::fs::read_to_string(&buffer_path).unwrap_or_default();
            let appended = format!("{existing}- {at} {}\n", note.trim());
            if let Err(error) = std::fs::write(&buffer_path, appended) {
                let _ = writeln!(
                    standard_error,
                    "{label}: write {}: {error}",
                    display_path(&buffer_path)
                );
                return 1;
            }
            let _ = writeln!(
                standard_output,
                "{label}: appended to {}",
                display_path(&buffer_path)
            );
            0
        }
        "trim" => {
            let mut flags = FlagSet::new(format!("{label} trim"));
            flags.string_flag("max-lines", "200");
            flags.string_flag("claude-home", "");
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let max_lines: usize = flags
                .string_value("max-lines")
                .trim()
                .parse()
                .unwrap_or(200);
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let buffer_path = home.join(command_group).join("working-buffer.md");
            let existing = std::fs::read_to_string(&buffer_path).unwrap_or_default();
            let lines: Vec<&str> = existing.lines().collect();
            if lines.len() <= max_lines {
                let _ = writeln!(
                    standard_output,
                    "{label}: working buffer within {max_lines} lines ({} lines), no trim",
                    lines.len()
                );
                return 0;
            }
            let kept: String = lines[lines.len() - max_lines..].join("\n");
            if let Err(error) = std::fs::write(&buffer_path, format!("{kept}\n")) {
                let _ = writeln!(
                    standard_error,
                    "{label}: write {}: {error}",
                    display_path(&buffer_path)
                );
                return 1;
            }
            let _ = writeln!(
                standard_output,
                "{label}: trimmed working buffer to last {max_lines} lines"
            );
            0
        }
        "recalibrate" => {
            // Report-only: lists the durable L1 artifacts present so the agent can
            // re-read them against current behavior. It does not mutate state.
            let mut flags = FlagSet::new(format!("{label} recalibrate"));
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let group_dir = home.join(command_group);
            let candidates = ["working-buffer.md", "SESSION-STATE.md"];
            let present: Vec<&str> = candidates
                .iter()
                .copied()
                .filter(|name| group_dir.join(name).is_file())
                .collect();
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("group".into(), Value::String(command_group.to_string())),
                    (
                        "presentL1Files".into(),
                        Value::Array(
                            present
                                .iter()
                                .map(|name| Value::String(name.to_string()))
                                .collect(),
                        ),
                    ),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(
                standard_output,
                "{label}: re-read these L1 files against current behavior:"
            );
            if present.is_empty() {
                let _ = writeln!(
                    standard_output,
                    "  (none present under {})",
                    display_path(&group_dir)
                );
            } else {
                for name in &present {
                    let _ = writeln!(standard_output, "  {}", display_path(&group_dir.join(name)));
                }
            }
            0
        }
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected append-working-buffer|trim|recalibrate)"
            );
            1
        }
    }
}

// ---------------------------------------------------------------------------
// agent-registry / agent-packets / loop-guard / entity: register/get/list shapes
// ---------------------------------------------------------------------------

fn run_agent_registry(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} agent-registry");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} agent-registry [register|list] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "register" => {
            let mut flags = FlagSet::new(format!("{label} register"));
            flags.string_flag("name", "");
            flags.string_flag("role", "");
            flags.string_flag("status", "active");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let name = flags.string_value("name").trim().to_string();
            if name.is_empty() {
                let _ = writeln!(standard_error, "{label} register: --name is required");
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let (_, at) = now_id("agent");
            // Registry is keyed by agent name (one record per name) so re-register updates.
            let id = sanitize_id(&name);
            let record: Record = vec![
                ("id".into(), id.clone()),
                ("name".into(), name),
                ("role".into(), flags.string_value("role").trim().to_string()),
                (
                    "status".into(),
                    flags.string_value("status").trim().to_string(),
                ),
                ("registeredAt".into(), at),
            ];
            let store = family_store(&home, command_group, "agent-registry");
            match store.write_record(&id, &record) {
                Ok(path) => emit_created(
                    &label,
                    &id,
                    &path,
                    &record,
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                ),
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "list" => list_family(
            command_group,
            "agent-registry",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected register|list)"
            );
            1
        }
    }
}

fn run_agent_packets(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} agent-packets");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} agent-packets [build|show|list] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "build" => {
            let mut flags = FlagSet::new(format!("{label} build"));
            flags.string_flag("objective", "");
            flags.string_flag("constraints", "");
            flags.string_flag("files", "");
            flags.string_flag("non-goals", "");
            flags.string_flag("expected-output", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let objective = flags.string_value("objective").trim().to_string();
            if objective.is_empty() {
                let _ = writeln!(standard_error, "{label} build: --objective is required");
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let (id, at) = now_id("packet");
            let record: Record = vec![
                ("id".into(), id.clone()),
                ("objective".into(), objective),
                (
                    "constraints[]".into(),
                    join_lines(&split_flag_list(flags.string_value("constraints"))),
                ),
                (
                    "files[]".into(),
                    join_lines(&split_flag_list(flags.string_value("files"))),
                ),
                (
                    "nonGoals[]".into(),
                    join_lines(&split_flag_list(flags.string_value("non-goals"))),
                ),
                (
                    "expectedOutput".into(),
                    flags.string_value("expected-output").trim().to_string(),
                ),
                ("builtAt".into(), at),
            ];
            let store = family_store(&home, command_group, "agent-packets");
            match store.write_record(&id, &record) {
                Ok(path) => emit_created(
                    &label,
                    &id,
                    &path,
                    &record,
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                ),
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "show" => show_family_record(
            command_group,
            "agent-packets",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "list" => list_family(
            command_group,
            "agent-packets",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected build|show|list)"
            );
            1
        }
    }
}

fn run_loop_guard(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} loop-guard");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} loop-guard [record|check] --signature <text> ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    let action = arguments[0].as_str();
    if action != "record" && action != "check" {
        let _ = writeln!(
            standard_error,
            "{label}: unknown action {action} (expected record|check)"
        );
        return 1;
    }
    let mut flags = FlagSet::new(format!("{label} {action}"));
    flags.string_flag("signature", "");
    flags.string_flag("budget", "2");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(&arguments[1..]) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let mut signature = flags.string_value("signature").trim().to_string();
    if signature.is_empty() && !flags.positional.is_empty() {
        signature = flags.positional.join(" ");
    }
    if signature.trim().is_empty() {
        let _ = writeln!(standard_error, "{label} {action}: --signature is required");
        return 1;
    }
    let budget: u32 = flags.string_value("budget").trim().parse().unwrap_or(2);
    let Some(home) = resolve_home(flags.string_value("claude-home"), &label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, "loop-guard");
    let id = sanitize_id(&signature);
    let mut record = store.read_record(&id).ok().flatten().unwrap_or_else(|| {
        vec![
            ("id".into(), id.clone()),
            ("signature".into(), signature.trim().to_string()),
            ("count".into(), "0".into()),
        ]
    });
    let mut count: u32 = field(&record, "count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if action == "record" {
        count += 1;
        set_field(&mut record, "count", count.to_string());
        if let Err(error) = store.write_record(&id, &record) {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    }
    let exhausted = count >= budget;
    if flags.bool_value("json") {
        let payload = Value::Object(vec![
            (
                "signature".into(),
                Value::String(signature.trim().to_string()),
            ),
            ("count".into(), Value::Number(count.to_string())),
            ("budget".into(), Value::Number(budget.to_string())),
            ("exhausted".into(), Value::Bool(exhausted)),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{label}: count={count} budget={budget} exhausted={exhausted}"
    );
    if exhausted {
        let _ = writeln!(
            standard_output,
            "  retry budget exhausted — change approach instead of repeating this failure"
        );
    }
    // `check` returns non-zero when the budget is exhausted so a caller script
    // can branch on it; `record` always returns 0 (the write succeeded).
    if action == "check" && exhausted {
        2
    } else {
        0
    }
}

fn run_entity(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} entity");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} entity [upsert|list|query] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "upsert" => {
            let mut flags = FlagSet::new(format!("{label} upsert"));
            flags.string_flag("name", "");
            flags.string_flag("type", "");
            flags.string_flag("summary", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let name = flags.string_value("name").trim().to_string();
            let entity_type = flags.string_value("type").trim().to_string();
            if name.is_empty() || entity_type.is_empty() {
                let _ = writeln!(
                    standard_error,
                    "{label} upsert: --name and --type are required"
                );
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            // Entities are keyed by type+name so upsert overwrites in place.
            let id = sanitize_id(&format!("{entity_type}-{name}"));
            let (_, at) = now_id("entity");
            let record: Record = vec![
                ("id".into(), id.clone()),
                ("name".into(), name),
                ("type".into(), entity_type),
                (
                    "summary".into(),
                    flags.string_value("summary").trim().to_string(),
                ),
                ("updatedAt".into(), at),
            ];
            let store = family_store(&home, command_group, "entities");
            match store.write_record(&id, &record) {
                Ok(path) => emit_created(
                    &label,
                    &id,
                    &path,
                    &record,
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                ),
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "list" => list_family(
            command_group,
            "entities",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "query" => {
            let mut flags = FlagSet::new(format!("{label} query"));
            flags.string_flag("type", "");
            flags.string_flag("contains", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let type_filter = flags.string_value("type").trim().to_lowercase();
            let contains = flags.string_value("contains").trim().to_lowercase();
            let store = family_store(&home, command_group, "entities");
            let records = match store.list_records() {
                Ok(records) => records,
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    return 1;
                }
            };
            let matches: Vec<&Record> = records
                .iter()
                .map(|(_, record)| record)
                .filter(|record| {
                    let type_ok = type_filter.is_empty()
                        || field(record, "type").unwrap_or("").to_lowercase() == type_filter;
                    let text = format!(
                        "{} {}",
                        field(record, "name").unwrap_or(""),
                        field(record, "summary").unwrap_or("")
                    )
                    .to_lowercase();
                    let contains_ok = contains.is_empty() || text.contains(&contains);
                    type_ok && contains_ok
                })
                .collect();
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("count".into(), Value::Number(matches.len().to_string())),
                    (
                        "entities".into(),
                        Value::Array(
                            matches
                                .iter()
                                .map(|record| record_to_value(record))
                                .collect(),
                        ),
                    ),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(standard_output, "{label}: {} match(es)", matches.len());
            for record in &matches {
                let _ = writeln!(
                    standard_output,
                    "  [{}] {} — {}",
                    field(record, "type").unwrap_or("?"),
                    field(record, "name").unwrap_or(""),
                    field(record, "summary").unwrap_or("")
                );
            }
            0
        }
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected upsert|list|query)"
            );
            1
        }
    }
}

fn run_graph(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} graph");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} graph [add|list|query] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "add" => {
            let mut flags = FlagSet::new(format!("{label} add"));
            flags.string_flag("from", "");
            flags.string_flag("relation", "");
            flags.string_flag("to", "");
            flags.string_flag("evidence", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let from = flags.string_value("from").trim().to_string();
            let relation = flags.string_value("relation").trim().to_string();
            let to = flags.string_value("to").trim().to_string();
            if from.is_empty() || relation.is_empty() || to.is_empty() {
                let _ = writeln!(
                    standard_error,
                    "{label} add: --from, --relation, and --to are required"
                );
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let (id, at) = now_id("edge");
            let record: Record = vec![
                ("id".into(), id.clone()),
                ("from".into(), from),
                ("relation".into(), relation),
                ("to".into(), to),
                (
                    "evidence".into(),
                    flags.string_value("evidence").trim().to_string(),
                ),
                ("addedAt".into(), at),
            ];
            let store = family_store(&home, command_group, "graph");
            match store.write_record(&id, &record) {
                Ok(path) => emit_created(
                    &label,
                    &id,
                    &path,
                    &record,
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                ),
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    1
                }
            }
        }
        "list" => list_family(
            command_group,
            "graph",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "query" => {
            let mut flags = FlagSet::new(format!("{label} query"));
            flags.string_flag("node", "");
            flags.string_flag("relation", "");
            flags.string_flag("contains", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let node = flags.string_value("node").trim().to_lowercase();
            let relation = flags.string_value("relation").trim().to_lowercase();
            let contains = flags.string_value("contains").trim().to_lowercase();
            let store = family_store(&home, command_group, "graph");
            let records = match store.list_records() {
                Ok(records) => records,
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    return 1;
                }
            };
            let matches: Vec<&Record> = records
                .iter()
                .map(|(_, record)| record)
                .filter(|record| {
                    let from = field(record, "from").unwrap_or("").to_lowercase();
                    let to = field(record, "to").unwrap_or("").to_lowercase();
                    let rel = field(record, "relation").unwrap_or("").to_lowercase();
                    let node_ok = node.is_empty() || from == node || to == node;
                    let relation_ok = relation.is_empty() || rel == relation;
                    let text = format!(
                        "{} {} {} {}",
                        from,
                        rel,
                        to,
                        field(record, "evidence").unwrap_or("").to_lowercase()
                    );
                    let contains_ok = contains.is_empty() || text.contains(&contains);
                    node_ok && relation_ok && contains_ok
                })
                .collect();
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("count".into(), Value::Number(matches.len().to_string())),
                    (
                        "edges".into(),
                        Value::Array(
                            matches
                                .iter()
                                .map(|record| record_to_value(record))
                                .collect(),
                        ),
                    ),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(standard_output, "{label}: {} edge(s)", matches.len());
            for record in &matches {
                let _ = writeln!(
                    standard_output,
                    "  {} --[{}]--> {}",
                    field(record, "from").unwrap_or("?"),
                    field(record, "relation").unwrap_or("?"),
                    field(record, "to").unwrap_or("?")
                );
            }
            0
        }
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected add|list|query)"
            );
            1
        }
    }
}

/// retrieve: a cross-family read. Searches research-cache answers and entity
/// summaries for the query terms and returns the merged hits. This is the
/// honest, lexical version of the planned semantic retrieve — it reuses the
/// same stores rather than introducing a vector index.
fn run_retrieve(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} retrieve");
    let mut flags = FlagSet::new(label.clone());
    flags.string_flag("query", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let mut query = flags.string_value("query").trim().to_lowercase();
    if query.is_empty() && !flags.positional.is_empty() {
        query = flags.positional.join(" ").to_lowercase();
    }
    if query.is_empty() {
        let _ = writeln!(standard_error, "{label}: --query is required");
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), &label, standard_error) else {
        return 1;
    };
    let terms: Vec<String> = query.split_whitespace().map(|s| s.to_string()).collect();
    let mut hits: Vec<(String, Value)> = Vec::new();
    for (family, text_fields) in [
        ("research-cache", vec!["question", "answer"]),
        ("entities", vec!["name", "summary"]),
    ] {
        let store = family_store(&home, command_group, family);
        if let Ok(records) = store.list_records() {
            for (_, record) in records {
                let haystack = text_fields
                    .iter()
                    .map(|key| field(&record, key).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                if terms.iter().all(|term| haystack.contains(term)) {
                    hits.push((family.to_string(), record_to_value(&record)));
                }
            }
        }
    }
    if flags.bool_value("json") {
        let payload = Value::Object(vec![
            ("count".into(), Value::Number(hits.len().to_string())),
            (
                "hits".into(),
                Value::Array(
                    hits.iter()
                        .map(|(family, value)| {
                            Value::Object(vec![
                                ("family".into(), Value::String(family.clone())),
                                ("record".into(), value.clone()),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{label}: {} hit(s) for \"{query}\"",
        hits.len()
    );
    for (family, _) in &hits {
        let _ = writeln!(standard_output, "  [{family}] match");
    }
    0
}

/// The family names `status` summarizes, in display order. Shared between the
/// CLI `status` handler and the programmatic `family_counts` surface so the two
/// can never drift on which families are counted.
const STATUS_FAMILIES: &[&str] = &[
    "research-cache",
    "agent-registry",
    "agent-packets",
    "loop-guard",
    "entities",
    "graph",
    "instincts",
];

/// Record counts per memory family for `command_group` (`memories` or
/// `memoriesv2`). A family whose store cannot be read counts as 0 so a partial
/// store never fails the whole summary. Backs both the CLI `status` subcommand
/// and the MCP `memory_status` tool, so the two share one definition of "what
/// families exist and how many records each holds".
pub fn family_counts(claude_home: &Path, command_group: &str) -> Vec<(String, usize)> {
    STATUS_FAMILIES
        .iter()
        .map(|family| {
            let store = family_store(claude_home, command_group, family);
            let count = store
                .list_records()
                .map(|records| records.len())
                .unwrap_or(0);
            ((*family).to_string(), count)
        })
        .collect()
}

/// status: a compact health summary of every implemented family for this group.
fn run_status(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} status");
    let mut flags = FlagSet::new(label.clone());
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), &label, standard_error) else {
        return 1;
    };
    let counts = family_counts(&home, command_group);
    if flags.bool_value("json") {
        let payload = Value::Object(
            counts
                .iter()
                .map(|(family, count)| (family.clone(), Value::Number(count.to_string())))
                .collect(),
        );
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "{label}: family record counts");
    for (family, count) in &counts {
        let _ = writeln!(standard_output, "  {family}: {count}");
    }
    0
}

// ---------------------------------------------------------------------------
// instincts: the learning loop. Confidence-scored behavioral patterns that
// reinforce/penalize over time and promote into reusable guidance once trusted.
//
// Each instinct is keyed by a sanitized form of its trigger so recording the
// same pattern twice updates one record rather than duplicating. Confidence is
// an integer score: `record` seeds it, `reinforce` raises it, `penalize` lowers
// it. `promote` surfaces (and optionally writes a markdown digest of) the
// instincts whose confidence meets a threshold — the ECC-style "instincts
// evolve into durable guidance" move, kept honest and lexical rather than ML.
// ---------------------------------------------------------------------------

const INSTINCT_SEED_CONFIDENCE: i64 = 1;
const INSTINCT_PROMOTE_THRESHOLD: i64 = 3;

fn run_instincts(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} instincts");
    if arguments.is_empty() || is_help(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: claude-skills {command_group} instincts [record|reinforce|penalize|list|promote] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "record" => instincts_record(
            command_group,
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "reinforce" => instincts_adjust(
            command_group,
            &label,
            &arguments[1..],
            1,
            standard_output,
            standard_error,
        ),
        "penalize" => instincts_adjust(
            command_group,
            &label,
            &arguments[1..],
            -1,
            standard_output,
            standard_error,
        ),
        "list" => list_family(
            command_group,
            "instincts",
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "promote" => instincts_promote(
            command_group,
            &label,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        other => {
            let _ = writeln!(
                standard_error,
                "{label}: unknown action {other} (expected record|reinforce|penalize|list|promote)"
            );
            1
        }
    }
}

fn instincts_record(
    command_group: &str,
    label: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new(format!("{label} record"));
    flags.string_flag("trigger", "");
    flags.string_flag("guidance", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let trigger = flags.string_value("trigger").trim().to_string();
    let guidance = flags.string_value("guidance").trim().to_string();
    if trigger.is_empty() || guidance.is_empty() {
        let _ = writeln!(
            standard_error,
            "{label} record: --trigger and --guidance are required"
        );
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, "instincts");
    // Keyed by trigger so re-recording the same pattern reinforces one record.
    let id = sanitize_id(&trigger);
    let (_, at) = now_id("instinct");
    let (confidence, observations) = match store.read_record(&id) {
        Ok(Some(existing)) => {
            let prior_conf: i64 = field(&existing, "confidence")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let prior_obs: i64 = field(&existing, "observations")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            (prior_conf + INSTINCT_SEED_CONFIDENCE, prior_obs + 1)
        }
        _ => (INSTINCT_SEED_CONFIDENCE, 1),
    };
    let record: Record = vec![
        ("id".into(), id.clone()),
        ("trigger".into(), trigger),
        ("guidance".into(), guidance),
        ("confidence".into(), confidence.to_string()),
        ("observations".into(), observations.to_string()),
        ("updatedAt".into(), at),
    ];
    match store.write_record(&id, &record) {
        Ok(path) => {
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("recorded".into(), Value::Bool(true)),
                    ("instinct".into(), record_to_value(&record)),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(
                standard_output,
                "{label}: {id} (confidence {confidence}, {observations} obs)"
            );
            let _ = writeln!(standard_output, "  saved: {}", display_path(&path));
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            1
        }
    }
}

fn instincts_adjust(
    command_group: &str,
    label: &str,
    arguments: &[String],
    delta: i64,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let verb = if delta >= 0 { "reinforce" } else { "penalize" };
    let mut flags = FlagSet::new(format!("{label} {verb}"));
    flags.string_flag("id", "");
    flags.string_flag("trigger", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let mut id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let trigger = flags.string_value("trigger").trim().to_string();
        if !trigger.is_empty() {
            id = sanitize_id(&trigger);
        }
    }
    if id.is_empty() {
        let _ = writeln!(
            standard_error,
            "{label} {verb}: --id or --trigger is required"
        );
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, "instincts");
    let mut record = match store.read_record(&id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "{label} {verb}: no instinct with id {id}");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    let confidence: i64 = field(&record, "confidence")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let new_confidence = confidence + delta;
    set_field(&mut record, "confidence", new_confidence.to_string());
    let (_, at) = now_id("instinct");
    set_field(&mut record, "updatedAt", at);
    match store.write_record(&id, &record) {
        Ok(_) => {
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("updated".into(), Value::Bool(true)),
                    ("instinct".into(), record_to_value(&record)),
                ]);
                return render_json(standard_output, standard_error, &payload);
            }
            let _ = writeln!(
                standard_output,
                "{label} {verb}: {id} -> confidence {new_confidence}"
            );
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            1
        }
    }
}

fn instincts_promote(
    command_group: &str,
    label: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new(format!("{label} promote"));
    flags.string_flag("threshold", INSTINCT_PROMOTE_THRESHOLD.to_string());
    flags.string_flag("write", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let threshold: i64 = flags
        .string_value("threshold")
        .trim()
        .parse()
        .unwrap_or(INSTINCT_PROMOTE_THRESHOLD);
    let Some(home) = resolve_home(flags.string_value("claude-home"), label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, "instincts");
    let records = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    let promoted: Vec<&Record> = records
        .iter()
        .map(|(_, record)| record)
        .filter(|record| {
            field(record, "confidence")
                .and_then(|v| v.parse::<i64>().ok())
                .map(|confidence| confidence >= threshold)
                .unwrap_or(false)
        })
        .collect();

    // Optionally write a durable markdown digest of the promoted instincts so
    // they read like reusable guidance rather than raw records.
    let write_target = flags.string_value("write").trim().to_string();
    if !write_target.is_empty() {
        let mut digest = String::from("# Promoted Instincts\n\n");
        digest.push_str(&format!(
            "Instincts at or above confidence {threshold}, promoted to reusable guidance.\n\n"
        ));
        for record in &promoted {
            digest.push_str(&format!(
                "- When **{}**: {} _(confidence {}, {} obs)_\n",
                field(record, "trigger").unwrap_or("?"),
                field(record, "guidance").unwrap_or(""),
                field(record, "confidence").unwrap_or("0"),
                field(record, "observations").unwrap_or("0"),
            ));
        }
        let path = std::path::PathBuf::from(&write_target);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                let _ = writeln!(
                    standard_error,
                    "{label}: create {}: {error}",
                    display_path(parent)
                );
                return 1;
            }
        }
        if let Err(error) = std::fs::write(&path, digest) {
            let _ = writeln!(
                standard_error,
                "{label}: write {}: {error}",
                display_path(&path)
            );
            return 1;
        }
    }

    if flags.bool_value("json") {
        let payload = Value::Object(vec![
            ("threshold".into(), Value::Number(threshold.to_string())),
            ("count".into(), Value::Number(promoted.len().to_string())),
            (
                "promoted".into(),
                Value::Array(
                    promoted
                        .iter()
                        .map(|record| record_to_value(record))
                        .collect(),
                ),
            ),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{label}: {} instinct(s) at or above confidence {threshold}",
        promoted.len()
    );
    for record in &promoted {
        let _ = writeln!(
            standard_output,
            "  [{}] when {} -> {}",
            field(record, "confidence").unwrap_or("0"),
            field(record, "trigger").unwrap_or("?"),
            field(record, "guidance").unwrap_or("")
        );
    }
    if !write_target.is_empty() {
        let _ = writeln!(standard_output, "  wrote digest: {write_target}");
    }
    0
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn is_help(argument: &str) -> bool {
    matches!(argument, "help" | "--help" | "-h")
}

fn set_field(record: &mut Record, key: &str, value: String) {
    if let Some(slot) = record.iter_mut().find(|(field_key, _)| field_key == key) {
        slot.1 = value;
    } else {
        record.push((key.to_string(), value));
    }
}

/// Turn arbitrary text into a filesystem-safe record id (used for keyed records
/// like registry-by-name, entity-by-type-name, loop-guard-by-signature).
fn sanitize_id(value: &str) -> String {
    let mut id = String::new();
    let mut previous_dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            id.push('-');
            previous_dash = true;
        }
    }
    let trimmed = id.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "record".to_string()
    } else {
        trimmed
    }
}

/// Split a comma- or semicolon-separated flag value into trimmed non-empty parts.
fn split_flag_list(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn emit_created(
    label: &str,
    id: &str,
    path: &Path,
    record: &Record,
    json: bool,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if json {
        let payload = Value::Object(vec![
            ("created".into(), Value::Bool(true)),
            ("path".into(), Value::String(display_path(path))),
            ("record".into(), record_to_value(record)),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "{label}: id={id}");
    let _ = writeln!(standard_output, "  saved: {}", display_path(path));
    0
}

fn list_family(
    command_group: &str,
    family: &str,
    label: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new(format!("{label} list"));
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, family);
    let records = match store.list_records() {
        Ok(records) => records,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    if flags.bool_value("json") {
        let payload = Value::Object(vec![
            ("count".into(), Value::Number(records.len().to_string())),
            (
                "records".into(),
                Value::Array(
                    records
                        .iter()
                        .map(|(_, record)| record_to_value(record))
                        .collect(),
                ),
            ),
        ]);
        return render_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(standard_output, "{label}: {} record(s)", records.len());
    for (id, _) in &records {
        let _ = writeln!(standard_output, "  {id}");
    }
    0
}

fn show_family_record(
    command_group: &str,
    family: &str,
    label: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new(format!("{label} show"));
    flags.string_flag("id", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "{label} show: --id is required");
        return 1;
    }
    let Some(home) = resolve_home(flags.string_value("claude-home"), label, standard_error) else {
        return 1;
    };
    let store = family_store(&home, command_group, family);
    match store.read_record(&id) {
        Ok(Some(record)) => {
            if flags.bool_value("json") {
                return render_json(standard_output, standard_error, &record_to_value(&record));
            }
            for (key, value) in &record {
                let _ = writeln!(standard_output, "  {key}: {value}");
            }
            0
        }
        Ok(None) => {
            let _ = writeln!(standard_error, "{label} show: no record with id {id}");
            1
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_home(label: &str) -> PathBuf {
        let unique: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let pid = std::process::id();
        let directory =
            std::env::temp_dir().join(format!("claude-skills-memfam-{label}-{pid}-{unique}"));
        std::fs::create_dir_all(&directory).expect("create tempdir");
        directory
    }

    fn run(group: &str, family: &str, args: &[&str]) -> (u8, String, String) {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_memory_family_command(group, family, &owned, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
        )
    }

    #[test]
    fn research_cache_record_then_lookup_finds_match() {
        let home = temp_home("rc");
        let h = home.to_string_lossy().to_string();
        let (code, _, err) = run(
            "memory",
            "research-cache",
            &[
                "record",
                "--question",
                "how to widen postgres column",
                "--answer",
                "use expand-contract",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "stderr: {err}");
        let (code, out, err) = run(
            "memory",
            "research-cache",
            &["lookup", "--query", "postgres column", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("expand-contract"), "stdout: {out}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn loop_guard_check_returns_two_when_budget_exhausted() {
        let home = temp_home("lg");
        let h = home.to_string_lossy().to_string();
        // record twice (budget default 2)
        run(
            "memory",
            "loop-guard",
            &["record", "--signature", "same error", "--claude-home", &h],
        );
        run(
            "memory",
            "loop-guard",
            &["record", "--signature", "same error", "--claude-home", &h],
        );
        let (code, out, _) = run(
            "memory",
            "loop-guard",
            &["check", "--signature", "same error", "--claude-home", &h],
        );
        assert_eq!(code, 2, "exhausted check must exit 2; stdout: {out}");
        assert!(out.contains("exhausted=true"), "stdout: {out}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn entity_upsert_is_idempotent_by_type_and_name() {
        let home = temp_home("ent");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "entity",
            &[
                "upsert",
                "--name",
                "Stripe",
                "--type",
                "tool",
                "--summary",
                "v1",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "entity",
            &[
                "upsert",
                "--name",
                "Stripe",
                "--type",
                "tool",
                "--summary",
                "v2",
                "--claude-home",
                &h,
            ],
        );
        let (code, out, err) = run("memory", "entity", &["list", "--claude-home", &h]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(
            out.contains("1 record"),
            "upsert must not duplicate; stdout: {out}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn graph_add_then_query_by_node() {
        let home = temp_home("graph");
        let h = home.to_string_lossy().to_string();
        run(
            "memoriesv2",
            "graph",
            &[
                "add",
                "--from",
                "PR-1",
                "--relation",
                "fixes",
                "--to",
                "bug-42",
                "--claude-home",
                &h,
            ],
        );
        let (code, out, err) = run(
            "memoriesv2",
            "graph",
            &["query", "--node", "bug-42", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("PR-1 --[fixes]--> bug-42"), "stdout: {out}");
        // group isolation: the memory group must not see the memoriesv2 edge
        let (_, out_other, _) = run(
            "memory",
            "graph",
            &["query", "--node", "bug-42", "--claude-home", &h],
        );
        assert!(
            out_other.contains("0 edge"),
            "groups must be isolated; stdout: {out_other}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn retrieve_merges_research_cache_and_entities() {
        let home = temp_home("retr");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "research-cache",
            &[
                "record",
                "--question",
                "auth flow",
                "--answer",
                "use oauth pkce",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "entity",
            &[
                "upsert",
                "--name",
                "oauth",
                "--type",
                "concept",
                "--summary",
                "pkce flow",
                "--claude-home",
                &h,
            ],
        );
        let (code, out, err) = run(
            "memory",
            "retrieve",
            &["--query", "pkce", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("hit(s)"), "stdout: {out}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn maintenance_append_and_trim_working_buffer() {
        let home = temp_home("maint");
        let h = home.to_string_lossy().to_string();
        for i in 0..5 {
            run(
                "memory",
                "maintenance",
                &[
                    "append-working-buffer",
                    "--note",
                    &format!("note {i}"),
                    "--claude-home",
                    &h,
                ],
            );
        }
        let (code, out, err) = run(
            "memory",
            "maintenance",
            &["trim", "--max-lines", "2", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("trimmed"), "stdout: {out}");
        let buffer =
            std::fs::read_to_string(home.join("memory/working-buffer.md")).expect("buffer exists");
        assert_eq!(
            buffer.lines().count(),
            2,
            "buffer must be trimmed to 2 lines"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn status_reports_family_counts() {
        let home = temp_home("status");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "agent-registry",
            &[
                "register",
                "--name",
                "reviewer",
                "--role",
                "qa",
                "--claude-home",
                &h,
            ],
        );
        let (code, out, err) = run("memory", "status", &["--claude-home", &h]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("agent-registry: 1"), "stdout: {out}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn unknown_action_is_rejected() {
        let (code, _, err) = run("memory", "research-cache", &["bogus"]);
        assert_eq!(code, 1);
        assert!(err.contains("unknown action bogus"), "stderr: {err}");
    }

    #[test]
    fn instincts_record_reinforce_and_promote_by_confidence() {
        let home = temp_home("instincts");
        let h = home.to_string_lossy().to_string();
        // Record the same trigger twice (each record +1 confidence) then reinforce.
        run(
            "memory",
            "instincts",
            &[
                "record",
                "--trigger",
                "flaky test retries",
                "--guidance",
                "quarantine and trace root cause",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "record",
                "--trigger",
                "flaky test retries",
                "--guidance",
                "quarantine and trace root cause",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "reinforce",
                "--trigger",
                "flaky test retries",
                "--claude-home",
                &h,
            ],
        );
        // confidence now 3 (1+1 from records, +1 reinforce). Promote at threshold 3.
        let (code, out, err) = run(
            "memory",
            "instincts",
            &["promote", "--threshold", "3", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("1 instinct(s)"), "stdout: {out}");
        assert!(
            out.contains("quarantine and trace root cause"),
            "stdout: {out}"
        );
        // A weaker instinct must be excluded from promotion.
        run(
            "memory",
            "instincts",
            &[
                "record",
                "--trigger",
                "weak hunch",
                "--guidance",
                "maybe",
                "--claude-home",
                &h,
            ],
        );
        let (_, out2, _) = run(
            "memory",
            "instincts",
            &["promote", "--threshold", "3", "--claude-home", &h],
        );
        assert!(
            out2.contains("1 instinct(s)"),
            "weak instinct must not promote; stdout: {out2}"
        );
        assert!(!out2.contains("weak hunch"), "stdout: {out2}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn instincts_penalize_lowers_confidence_below_promotion() {
        let home = temp_home("instincts-pen");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "instincts",
            &[
                "record",
                "--trigger",
                "guess pattern",
                "--guidance",
                "do x",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "reinforce",
                "--trigger",
                "guess pattern",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "reinforce",
                "--trigger",
                "guess pattern",
                "--claude-home",
                &h,
            ],
        );
        // confidence 3 now; penalize twice -> 1, below threshold 3.
        run(
            "memory",
            "instincts",
            &[
                "penalize",
                "--trigger",
                "guess pattern",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "penalize",
                "--trigger",
                "guess pattern",
                "--claude-home",
                &h,
            ],
        );
        let (code, out, err) = run("memory", "instincts", &["promote", "--claude-home", &h]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(
            out.contains("0 instinct(s)"),
            "penalized instinct must drop out; stdout: {out}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn instincts_promote_write_emits_markdown_digest() {
        let home = temp_home("instincts-write");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "instincts",
            &[
                "record",
                "--trigger",
                "merge garbage",
                "--guidance",
                "dispatch parallel agents only on disjoint files",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "reinforce",
                "--trigger",
                "merge garbage",
                "--claude-home",
                &h,
            ],
        );
        run(
            "memory",
            "instincts",
            &[
                "reinforce",
                "--trigger",
                "merge garbage",
                "--claude-home",
                &h,
            ],
        );
        let digest_path = home.join("digest.md");
        let dp = digest_path.to_string_lossy().to_string();
        let (code, _, err) = run(
            "memory",
            "instincts",
            &[
                "promote",
                "--threshold",
                "3",
                "--write",
                &dp,
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "stderr: {err}");
        let digest = std::fs::read_to_string(&digest_path).expect("digest written");
        assert!(digest.contains("# Promoted Instincts"), "digest: {digest}");
        assert!(digest.contains("merge garbage"), "digest: {digest}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
