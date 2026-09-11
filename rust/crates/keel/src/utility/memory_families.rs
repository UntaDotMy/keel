//! Purpose: Implements the previously-planned memory command families on top of the
//!   shared scoped record store: research-cache, maintenance, agent-registry,
//!   agent-packets, loop-guard, entity, graph, retrieve, and a status summary.
//! Caller: utility::memory::run_memory_command dispatch for the `memory` command group.
//! Dependencies: crate::args::FlagSet, crate::json::{write_indented, Value},
//!   crate::runtime::{display_path, resolve_claude_home}, crate::utility::record_store,
//!   crate::utility::record_store timestamp helpers.
//! Main Functions: run_memory_family_command (the single dispatch entry the memory module calls).
//! Side Effects: Reads and writes flat-string JSON records under
//!   `<claude-home>/<group>/<family>/<id>.json`. No global state.
//!
//! Design: every family is a thin handler over `RecordStore`, mirroring the
//! workflow ledger and working-brief storage shapes already in the tree rather
//! than introducing a new persistence concept. `<group>` is `memory` so the
//! command group's records stay isolated on disk.

use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::proxy::token_meter::TokenMeter;
use crate::runtime::{display_path, resolve_claude_home};
use crate::utility::hashing::sha256_hex;
use crate::utility::memory::shared::{
    is_help_argument as is_help, render_workflow_json as render_json,
};
use crate::utility::record_store::{
    current_timestamp_millis, field, format_timestamp_iso8601, join_lines, record_to_value,
    unique_timestamped_id, Record, RecordStore,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResearchCacheHit {
    pub id: String,
    pub answer: String,
    pub source_url: String,
    pub source_type: String,
    pub publication_date: Option<String>,
    pub retrieved_at: String,
    pub freshness_class: String,
    pub used_by: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ResearchCacheLookup {
    pub fresh: Vec<ResearchCacheHit>,
    pub stale_matches: usize,
}

pub(crate) fn lookup_fresh_research_cache(
    claude_home: &Path,
    query: &str,
) -> Result<ResearchCacheLookup, String> {
    let store = family_store(claude_home, "memory", "research-cache");
    let records = store.list_records().map_err(|error| error.to_string())?;
    let now = format_timestamp_iso8601(current_timestamp_millis());
    let mut lookup = ResearchCacheLookup::default();
    for (_, record) in &records {
        if !research_cache_record_matches(record, query) {
            continue;
        }
        let Some(hit) = complete_research_cache_hit(record) else {
            continue;
        };
        if research_cache_record_is_stale(record, &now) {
            lookup.stale_matches += 1;
        } else {
            lookup.fresh.push(hit);
        }
    }
    Ok(lookup)
}

/// Increment a loop-guard signature and return `(count, exhausted)` for `budget`.
pub fn bump_loop_guard(
    claude_home: &Path,
    signature: &str,
    budget: u32,
) -> Result<(u32, bool), String> {
    let store = family_store(claude_home, "memory", "loop-guard");
    let id = sanitize_id(signature);
    let record_opt = store.read_record(&id).map_err(|e| e.to_string())?;
    let mut record = record_opt.unwrap_or_else(|| {
        vec![
            ("id".into(), id.clone()),
            ("signature".into(), signature.trim().to_string()),
            ("count".into(), "0".into()),
        ]
    });
    let parsed: Option<u32> = field(&record, "count").and_then(|v| v.parse().ok());
    let Some(previous) = parsed else {
        // why: a corrupt count must not silently reset the budget: refuse loudly.
        return Err(format!(
            "loop-guard count corrupt for signature {signature:?}; refusing to reset budget"
        ));
    };
    let count = previous.saturating_add(1);
    set_field(&mut record, "count", count.to_string());
    store
        .write_record(&id, &record)
        .map_err(|error| error.to_string())?;
    Ok((count, count >= budget))
}

pub fn loop_guard_exhausted(claude_home: &Path, signature: &str, budget: u32) -> bool {
    let store = family_store(claude_home, "memory", "loop-guard");
    let id = sanitize_id(signature);
    match store.read_record(&id) {
        Ok(Some(record)) => {
            // why: corrupt counts fail closed: an unreadable budget blocks the loop.
            let Some(count): Option<u32> = field(&record, "count").and_then(|v| v.parse().ok())
            else {
                return true;
            };
            count >= budget
        }
        Ok(None) => false,
        Err(_) => true,
    }
}

fn now_id(prefix: &str) -> (String, String) {
    unique_timestamped_id(prefix)
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
            "Usage: keel {command_group} research-cache <subcommand> [flags]\n\
             \n\
             record   --question \"...\" --answer \"...\" [--source ...] [--source-type ...]\n\
                      [--publication-date ...] [--retrieved-at ...] [--freshness-class ...]\n\
                      [--used-by REQ-001,AC-001] [--freshness <ttl>]\n\
                      aliases: --query = --question, --result = --answer\n\
             lookup   --query \"...\" [--include-stale]\n\
             stale    [--days N]\n\
             reward   --id <id>\n\
             list"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "record" => {
            let mut flags = FlagSet::new(format!("{label} record"));
            flags.string_flag("question", "");
            flags.string_flag("answer", "");
            // Agent-facing aliases (stale skill text taught --query/--result).
            flags.string_flag("query", "");
            flags.string_flag("result", "");
            flags.string_flag("source", "");
            flags.string_flag("source-type", "");
            flags.string_flag("publication-date", "");
            flags.string_flag("retrieved-at", "");
            flags.string_flag("freshness-class", "");
            flags.string_flag("used-by", "");
            flags.string_flag("freshness", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let question = {
                let q = flags.string_value("question").trim().to_string();
                if q.is_empty() {
                    flags.string_value("query").trim().to_string()
                } else {
                    q
                }
            };
            let answer = {
                let a = flags.string_value("answer").trim().to_string();
                if a.is_empty() {
                    flags.string_value("result").trim().to_string()
                } else {
                    a
                }
            };
            if question.is_empty() || answer.is_empty() {
                let _ = writeln!(
                    standard_error,
                    "{label} record: --question and --answer are required \
                     (aliases: --query for --question, --result for --answer)"
                );
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let (id, at) = now_id("rc");
            let freshness = flags.string_value("freshness").trim().to_string();
            let retrieved_at = match flags.string_value("retrieved-at").trim() {
                "" => at.clone(),
                value => value.to_string(),
            };
            let mut record: Record = vec![
                ("id".into(), id.clone()),
                ("question".into(), question),
                ("answer".into(), answer),
                (
                    "source".into(),
                    flags.string_value("source").trim().to_string(),
                ),
                (
                    "sourceType".into(),
                    flags.string_value("source-type").trim().to_string(),
                ),
                (
                    "publicationDate".into(),
                    flags.string_value("publication-date").trim().to_string(),
                ),
                ("retrievedAt".into(), retrieved_at),
                (
                    "freshnessClass".into(),
                    flags.string_value("freshness-class").trim().to_string(),
                ),
                (
                    "usedBy".into(),
                    flags.string_value("used-by").trim().to_string(),
                ),
                ("freshness".into(), freshness.clone()),
                ("state".into(), "fresh".into()),
                ("recordedAt".into(), at),
            ];
            if let Some(expires_at) = freshness_expiry(&freshness, current_timestamp_millis()) {
                record.push(("expiresAt".into(), expires_at));
            }
            let store = family_store(&home, command_group, "research-cache");
            match store.write_record(&id, &record) {
                Ok(path) => {
                    // Sync the recall FTS index now so the record is searchable
                    // on the very next `recall` without a separate trigger.
                    // Best-effort: the file on disk is durable regardless, and a
                    // failed sync is reconciled by the next read-path sync.
                    if let Err(error) =
                        crate::utility::recall::reindex_after_write_paths(&home, &[path.as_path()])
                    {
                        let _ = writeln!(
                            standard_error,
                            "{label}: recall index sync skipped ({error})"
                        );
                    }
                    emit_created(
                        &label,
                        &id,
                        &path,
                        &record,
                        flags.bool_value("json"),
                        standard_output,
                        standard_error,
                    )
                }
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
            flags.bool_flag("include-stale", false);
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
            let now = format_timestamp_iso8601(current_timestamp_millis());
            let include_stale = flags.bool_value("include-stale");
            let mut stale_matches = Vec::new();
            let mut matches = Vec::new();
            for (_, record) in &records {
                if !research_cache_record_matches(record, &query) {
                    continue;
                }
                if research_cache_record_is_stale(record, &now) {
                    stale_matches.push(record);
                    if include_stale {
                        matches.push(record);
                    }
                } else {
                    matches.push(record);
                }
            }
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("count".into(), Value::Number(matches.len().to_string())),
                    ("includeStale".into(), Value::Bool(include_stale)),
                    (
                        "staleMatches".into(),
                        Value::Array(
                            stale_matches
                                .iter()
                                .map(|record| record_to_value(record))
                                .collect(),
                        ),
                    ),
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
            if !include_stale && !stale_matches.is_empty() {
                let _ = writeln!(
                    standard_output,
                    "  skipped {} stale match(es); rerun with --include-stale to inspect",
                    stale_matches.len()
                );
            }
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
        "stale" => {
            // Documented contract (help + research-enforcement skill):
            // `stale [--days N]` lists research-cache entries older than N days
            // (default 30). `--id <id>` marks a single entry stale explicitly.
            let mut flags = FlagSet::new(format!("{label} stale"));
            flags.string_flag("id", "");
            flags.string_flag("days", "");
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
            let store = family_store(&home, command_group, "research-cache");
            let id = flags.string_value("id").trim().to_string();
            if !id.is_empty() {
                return mark_cache_entry_state(
                    &store,
                    &label,
                    &id,
                    "stale",
                    flags.bool_value("json"),
                    standard_output,
                    standard_error,
                );
            }
            let days: u128 = match flags.string_value("days").trim().parse::<u128>() {
                Ok(days) if days > 0 => days,
                Ok(_) => {
                    let _ = writeln!(
                        standard_error,
                        "{label} stale: --days must be a positive integer"
                    );
                    return 1;
                }
                Err(_) => 30,
            };
            let records = match store.list_records() {
                Ok(records) => records,
                Err(error) => {
                    let _ = writeln!(standard_error, "{label}: {error}");
                    return 1;
                }
            };
            // ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` strings compare lexicographically,
            // so an age cutoff reduces to a string comparison against the cutoff.
            let cutoff = format_timestamp_iso8601(current_timestamp_millis() - days * 86_400_000);
            let stale_records: Vec<&Record> = records
                .iter()
                .map(|(_, record)| record)
                .filter(|record| field(record, "recordedAt").unwrap_or("") < cutoff.as_str())
                .collect();
            if flags.bool_value("json") {
                let payload = Value::Object(vec![
                    ("days".into(), Value::Number(days.to_string())),
                    (
                        "count".into(),
                        Value::Number(stale_records.len().to_string()),
                    ),
                    (
                        "entries".into(),
                        Value::Array(
                            stale_records
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
                "{label} stale: {days} day(s) cutoff, {} entr{} older",
                stale_records.len(),
                if stale_records.len() == 1 { "y" } else { "ies" }
            );
            for record in &stale_records {
                let _ = writeln!(
                    standard_output,
                    "  [{}] {} recorded {} -> {}",
                    field(record, "state").unwrap_or("?"),
                    field(record, "id").unwrap_or("?"),
                    field(record, "recordedAt").unwrap_or("?"),
                    field(record, "question").unwrap_or("")
                );
            }
            0
        }
        "reward" => {
            let mut flags = FlagSet::new(format!("{label} reward"));
            flags.string_flag("id", "");
            flags.string_flag("claude-home", "");
            flags.bool_flag("json", false);
            if let Err(error) = flags.parse(&arguments[1..]) {
                let _ = writeln!(standard_error, "{}", error.message);
                return 1;
            }
            let id = flags.string_value("id").trim().to_string();
            if id.is_empty() {
                let _ = writeln!(standard_error, "{label} reward: --id is required");
                return 1;
            }
            let Some(home) =
                resolve_home(flags.string_value("claude-home"), &label, standard_error)
            else {
                return 1;
            };
            let store = family_store(&home, command_group, "research-cache");
            mark_cache_entry_state(
                &store,
                &label,
                &id,
                "rewarded",
                flags.bool_value("json"),
                standard_output,
                standard_error,
            )
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
            "Usage: keel {command_group} maintenance [append-working-buffer|trim|recalibrate] ..."
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
            "Usage: keel {command_group} agent-registry [register|list] ..."
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
            "Usage: keel {command_group} agent-packets [build|show|list] ..."
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
            "Usage: keel {command_group} loop-guard [record|check] --signature <text> ..."
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
    let record_opt = match store.read_record(&id) {
        Ok(record) => record,
        Err(e) => {
            let _ = writeln!(standard_error, "{label}: read error: {e}");
            return 2;
        }
    };
    let mut record = record_opt.unwrap_or_else(|| {
        vec![
            ("id".into(), id.clone()),
            ("signature".into(), signature.trim().to_string()),
            ("count".into(), "0".into()),
        ]
    });
    let parsed: Option<u32> = field(&record, "count").and_then(|v| v.parse().ok());
    let mut count = match parsed {
        Some(value) => value,
        // why: corrupt counts fail closed: record refuses, check reports exhausted.
        None if action == "record" => {
            let _ = writeln!(
                standard_error,
                "{label}: count corrupt for signature {signature:?}; refusing to reset budget"
            );
            return 1;
        }
        None => budget,
    };
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
            "Usage: keel {command_group} entity [upsert|list|query] ..."
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
            "Usage: keel {command_group} graph [add|list|query] ..."
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

/// Retrieve durable memory through the scoped FTS5 chunk index. Results include
/// source paths, line evidence, ranking reasons, and the indexed retrieval stage.
fn run_retrieve(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} retrieve");
    let mut flags = FlagSet::new(label.clone());
    flags.string_flag("query", "");
    flags.string_flag("limit", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let mut query = flags.string_value("query").trim().to_string();
    if query.is_empty() && !flags.positional.is_empty() {
        query = flags.positional.join(" ");
    }
    if query.trim().is_empty() {
        let _ = writeln!(standard_error, "{label}: --query is required");
        return 1;
    }
    if let Err(error) = crate::utility::recall::validate_recall_query(&query) {
        let _ = writeln!(standard_error, "{label}: {error}");
        return 2;
    }
    let limit = match crate::utility::recall::parse_recall_limit(flags.string_value("limit")) {
        Ok(limit) => limit,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 2;
        }
    };
    let Some(home) = resolve_home(flags.string_value("claude-home"), &label, standard_error) else {
        return 1;
    };
    let result = match crate::utility::recall::search_recall_index(&home, &query, limit, None) {
        Ok(Some(result)) => result,
        Ok(None) => {
            let _ = writeln!(standard_error, "{label}: query has no searchable terms");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    let stage = result.stage;
    let bounded_hits = bound_retrieve_hits(&query, limit, result.hits);
    let (projected_hits, projection) =
        bounded_retrieve_projection(&query, stage, limit, &bounded_hits);
    if flags.bool_value("json") {
        return render_json(standard_output, standard_error, &projection);
    }
    let _ = writeln!(
        standard_output,
        "{label}: {} hit(s) for \"{}\" stage={}",
        projected_hits.len(),
        query,
        stage
    );
    for hit in &projected_hits {
        let provenance_id = retrieve_provenance_id(&query, hit);
        let retrieval_ref = retrieve_ref(&query, limit);
        let _ = writeln!(
            standard_output,
            "  {}:{} score={:.4} {} {} provenanceId=prov-sha256:{} retrievalRef={}",
            hit.absolute_path,
            hit.line,
            hit.score,
            hit.snippet,
            result.stage,
            provenance_id,
            retrieval_ref
        );
    }
    0
}

const MEMORY_RETRIEVE_RESULT_OVERHEAD_BYTES: usize = 2 * 1024;
// Includes the enclosing object, array indentation, separators, and the
// provenance/recovery envelope in addition to the per-hit JSON.
const MEMORY_RETRIEVE_RESULT_OVERHEAD_TOKENS: usize = 192;
const MAX_MEMORY_QUERY_PROJECTION_CHARS: usize = 256;
const MAX_MEMORY_EXCERPT_CHARS: usize = 600;

fn retrieve_hit_value(query: &str, limit: usize, hit: &crate::utility::recall::RecallHit) -> Value {
    let provenance_id = retrieve_provenance_id(query, hit);
    Value::Object(vec![
        ("path".into(), Value::String(hit.absolute_path.clone())),
        ("line".into(), Value::Number(hit.line.to_string())),
        ("score".into(), Value::Number(format!("{:.4}", hit.score))),
        (
            "snippet".into(),
            Value::String(bounded_memory_excerpt(&hit.snippet)),
        ),
        (
            "provenanceId".into(),
            Value::String(format!("prov-sha256:{provenance_id}")),
        ),
        (
            "retrievalRef".into(),
            Value::String(retrieve_ref(query, limit)),
        ),
    ])
}

fn retrieve_provenance_id(query: &str, hit: &crate::utility::recall::RecallHit) -> String {
    sha256_hex(
        format!(
            "memory-retrieve\0{}\0{}\0{}",
            query, hit.absolute_path, hit.line
        )
        .as_bytes(),
    )
}

fn retrieve_ref(query: &str, limit: usize) -> String {
    format!(
        "keel memory retrieve --query {:?} --limit {}",
        query,
        limit.clamp(1, crate::utility::recall::MAX_RECALL_LIMIT)
    )
}

fn bounded_memory_excerpt(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_MEMORY_EXCERPT_CHARS {
        return compact;
    }
    let mut excerpt = compact
        .chars()
        .take(MAX_MEMORY_EXCERPT_CHARS)
        .collect::<String>();
    excerpt.push_str("… [truncated]");
    excerpt
}

fn serialized_value(value: &Value) -> Vec<u8> {
    let mut rendered = Vec::new();
    write_indented(&mut rendered, value).expect("render memory result into memory");
    rendered
}

fn bounded_retrieve_projection(
    query: &str,
    stage: &str,
    limit: usize,
    hits: &[crate::utility::recall::RecallHit],
) -> (Vec<crate::utility::recall::RecallHit>, Value) {
    let (projected_query, query_truncated) =
        bounded_projection_text(query, MAX_MEMORY_QUERY_PROJECTION_CHARS);
    let query_digest = sha256_hex(query.as_bytes());
    let mut selected = Vec::new();
    let mut dropped = false;
    for hit in hits {
        let mut candidate = selected.clone();
        candidate.push(hit.clone());
        let payload = retrieve_projection_payload(
            &projected_query,
            query_truncated,
            &query_digest,
            stage,
            limit,
            &candidate,
            dropped || candidate.len() < hits.len(),
        );
        if retrieve_projection_within_budget(&payload) {
            selected.push(hit.clone());
        } else {
            dropped = true;
        }
    }
    let payload = retrieve_projection_payload(
        &projected_query,
        query_truncated,
        &query_digest,
        stage,
        limit,
        &selected,
        dropped || selected.len() < hits.len(),
    );
    if retrieve_projection_within_budget(&payload) {
        return (selected, payload);
    }
    let fallback_query = bounded_projection_text(&projected_query, 64).0;
    let fallback = retrieve_projection_payload(
        &fallback_query,
        true,
        &query_digest,
        stage,
        limit,
        &[],
        true,
    );
    (Vec::new(), fallback)
}

fn retrieve_projection_payload(
    query: &str,
    query_truncated: bool,
    query_digest: &str,
    stage: &str,
    limit: usize,
    hits: &[crate::utility::recall::RecallHit],
    truncated: bool,
) -> Value {
    let values = hits
        .iter()
        .map(|hit| retrieve_hit_value(query, limit, hit))
        .collect();
    Value::Object(vec![
        ("query".into(), Value::String(query.to_string())),
        (
            "queryDigest".into(),
            Value::String(format!("sha256:{query_digest}")),
        ),
        ("queryTruncated".into(), Value::Bool(query_truncated)),
        ("stage".into(), Value::String(stage.to_string())),
        (
            "limit".into(),
            Value::Number(
                limit
                    .min(crate::utility::recall::MAX_RECALL_LIMIT)
                    .to_string(),
            ),
        ),
        ("count".into(), Value::Number(hits.len().to_string())),
        ("truncated".into(), Value::Bool(truncated)),
        ("hits".into(), Value::Array(values)),
    ])
}

fn bounded_projection_text(text: &str, max_chars: usize) -> (String, bool) {
    let mut chars = text.chars();
    let mut value = chars.by_ref().take(max_chars).collect::<String>();
    let truncated = chars.next().is_some();
    if truncated {
        value.push('…');
    }
    (value, truncated)
}

fn retrieve_projection_within_budget(value: &Value) -> bool {
    let rendered = serialized_value(value);
    rendered.len() <= crate::utility::recall::MAX_RECALL_RESULT_BYTES
        && TokenMeter::count_bytes(&rendered) <= crate::utility::recall::MAX_RECALL_RESULT_TOKENS
}

/// Keep the memory-family JSON projection inside the same result budgets as
/// low-level recall, including the extra provenance/recovery fields owned by
/// this command. The search owner already applies the limit; this second pass
/// measures the actual family response shape before it reaches stdout.
fn bound_retrieve_hits(
    query: &str,
    limit: usize,
    hits: Vec<crate::utility::recall::RecallHit>,
) -> Vec<crate::utility::recall::RecallHit> {
    let count_limit = limit.min(crate::utility::recall::MAX_RECALL_LIMIT);
    let byte_budget = crate::utility::recall::MAX_RECALL_RESULT_BYTES
        .saturating_sub(MEMORY_RETRIEVE_RESULT_OVERHEAD_BYTES);
    let token_budget = crate::utility::recall::MAX_RECALL_RESULT_TOKENS
        .saturating_sub(MEMORY_RETRIEVE_RESULT_OVERHEAD_TOKENS);
    let mut selected = Vec::new();
    let mut used_bytes = 0usize;
    let mut used_tokens = 0usize;

    for hit in hits {
        if selected.len() >= count_limit {
            break;
        }
        let rendered = serialized_value(&retrieve_hit_value(query, limit, &hit));
        let hit_bytes = rendered.len();
        let hit_tokens = TokenMeter::count_bytes(&rendered);
        if hit_bytes > byte_budget.saturating_sub(used_bytes)
            || hit_tokens > token_budget.saturating_sub(used_tokens)
        {
            continue;
        }
        used_bytes = used_bytes.saturating_add(hit_bytes);
        used_tokens = used_tokens.saturating_add(hit_tokens);
        selected.push(hit);
    }
    selected
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

/// Record counts per memory family for `command_group`. A family whose store cannot be read counts as 0 so a partial
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
            "Usage: keel {command_group} instincts [record|reinforce|penalize|list|promote] ..."
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
    // Thin wrapper over the canonical system_map slug; family record ids fall
    // back to "record" instead of the empty string.
    let id = crate::utility::system_map::sanitize_key(value);
    if id.is_empty() {
        "record".to_string()
    } else {
        id
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

/// Convert a compact freshness guidance value into an absolute expiry. The
/// research cache intentionally keeps the original guidance verbatim, but
/// recognizes unambiguous TTL forms so lookup can avoid silently reusing
/// time-sensitive findings. Accepted forms include `30d`, `30 days`, `12h`,
/// `90m`, `7w`, and an optional `ttl=` prefix. Free-form guidance such as
/// `refresh when the vendor releases a new version` remains advisory only.
fn freshness_expiry(freshness: &str, recorded_at_millis: u128) -> Option<String> {
    let ttl_millis = parse_freshness_ttl_millis(freshness)?;
    let expires_at_millis = recorded_at_millis.checked_add(ttl_millis)?;
    Some(format_timestamp_iso8601(expires_at_millis))
}

fn parse_freshness_ttl_millis(freshness: &str) -> Option<u128> {
    let normalized = freshness.trim().to_ascii_lowercase();
    let value = normalized
        .strip_prefix("ttl=")
        .or_else(|| normalized.strip_prefix("ttl:"))
        .unwrap_or(&normalized)
        .trim();
    if value.is_empty() {
        return None;
    }

    let (amount, unit) = if value.chars().all(|character| character.is_ascii_digit()) {
        // A bare number is deliberately not interpreted: its unit is unknown.
        return None;
    } else if value.split_whitespace().count() == 2 {
        let mut parts = value.split_whitespace();
        (parts.next()?, parts.next()?)
    } else {
        let split_at = value
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(value.len());
        value.split_at(split_at)
    };
    let amount = amount.parse::<u128>().ok()?;
    let unit_millis = match unit.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "wk" | "wks" | "week" | "weeks" => 7 * 86_400_000,
        "mo" | "month" | "months" => 30 * 86_400_000,
        _ => return None,
    };
    amount.checked_mul(unit_millis)
}

fn research_cache_record_matches(record: &Record, query: &str) -> bool {
    let haystack = format!(
        "{} {}",
        field(record, "question").unwrap_or(""),
        field(record, "answer").unwrap_or("")
    )
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|term| haystack.contains(term))
}

fn complete_research_cache_hit(record: &Record) -> Option<ResearchCacheHit> {
    let non_empty = |name| {
        field(record, name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some(ResearchCacheHit {
        id: non_empty("id")?.to_string(),
        answer: non_empty("answer")?.to_string(),
        source_url: non_empty("source")?.to_string(),
        source_type: non_empty("sourceType")?.to_string(),
        publication_date: non_empty("publicationDate").map(str::to_string),
        retrieved_at: non_empty("retrievedAt")?.to_string(),
        freshness_class: non_empty("freshnessClass")?.to_string(),
        used_by: non_empty("usedBy")?
            .split([',', ' '])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

fn research_cache_record_is_stale(record: &Record, now: &str) -> bool {
    matches!(
        field(record, "state")
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "stale" | "expired"
    ) || research_cache_record_expiry(record).is_some_and(|expires_at| expires_at.as_str() <= now)
}

/// Return the stored expiry, or derive one for records written before the
/// `expiresAt` field was introduced. Keeping the derivation on lookup makes the
/// freshness policy apply uniformly to legacy cache files without rewriting
/// user data just to migrate metadata.
fn research_cache_record_expiry(record: &Record) -> Option<String> {
    if let Some(expires_at) = field(record, "expiresAt")
        .map(str::trim)
        .filter(|expires_at| !expires_at.is_empty())
    {
        return Some(expires_at.to_string());
    }
    let freshness = field(record, "freshness")?.trim();
    let recorded_at = field(record, "recordedAt")?.trim();
    let recorded_at_millis = chrono::DateTime::parse_from_rfc3339(recorded_at)
        .ok()?
        .timestamp_millis()
        .try_into()
        .ok()?;
    freshness_expiry(freshness, recorded_at_millis)
}

/// Set the `state` field on one research-cache record (used by `stale --id` and `reward`).
fn mark_cache_entry_state(
    store: &RecordStore,
    label: &str,
    id: &str,
    new_state: &str,
    json: bool,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let record = match store.read_record(id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(
                standard_error,
                "{label}: no research-cache entry with id {id}"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    let mut record: Record = record
        .into_iter()
        .filter(|(key, _)| key != "state")
        .collect();
    record.push(("state".into(), new_state.to_string()));
    match store.write_record(id, &record) {
        Ok(path) => {
            if json {
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
        let directory = std::env::temp_dir().join(format!("keel-memfam-{label}-{pid}-{unique}"));
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
    fn research_cache_record_accepts_query_result_aliases() {
        // Agents/skills historically taught --query/--result for record.
        // Accept them as aliases so MCP/CLI stops fail-looping on stale flags.
        let home = temp_home("rc-alias");
        let h = home.to_string_lossy().to_string();
        let (code, _, err) = run(
            "memory",
            "research-cache",
            &[
                "record",
                "--query",
                "alias question",
                "--result",
                "alias answer body",
                "--source",
                "unit-test",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "aliases must succeed; stderr: {err}");
        let (code, out, err) = run(
            "memory",
            "research-cache",
            &["lookup", "--query", "alias question", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("alias answer body"), "stdout: {out}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn research_cache_lookup_excludes_expired_entries_until_opted_in() {
        let home = temp_home("rc-freshness");
        let h = home.to_string_lossy().to_string();
        let (code, _, err) = run(
            "memory",
            "research-cache",
            &[
                "record",
                "--question",
                "time-sensitive provider behavior",
                "--answer",
                "refresh this answer before reuse",
                "--freshness",
                "0s",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "record must succeed; stderr: {err}");
        let stored = family_store(&home, "memory", "research-cache")
            .list_records()
            .expect("list records");
        assert_eq!(stored.len(), 1);
        assert!(
            field(&stored[0].1, "expiresAt").is_some(),
            "recognized TTL must persist an absolute expiry"
        );

        let (code, out, err) = run(
            "memory",
            "research-cache",
            &[
                "lookup",
                "--query",
                "provider behavior",
                "--json",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "fresh lookup must succeed; stderr: {err}");
        assert!(out.contains("\"count\": 0"), "expired entry leaked: {out}");
        assert!(
            out.contains("\"staleMatches\"") && out.contains("refresh this answer"),
            "lookup must expose the omitted stale match: {out}"
        );

        let (code, out, err) = run(
            "memory",
            "research-cache",
            &[
                "lookup",
                "--query",
                "provider behavior",
                "--include-stale",
                "--json",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "include-stale lookup must succeed; stderr: {err}");
        assert!(
            out.contains("\"count\": 1"),
            "stale opt-in missing match: {out}"
        );
        assert!(
            out.contains("\"includeStale\": true"),
            "flag not exposed: {out}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn research_cache_lookup_derives_expiry_for_legacy_records() {
        let home = temp_home("rc-legacy-freshness");
        let store = family_store(&home, "memory", "research-cache");
        store
            .write_record(
                "legacy",
                &vec![
                    ("id".into(), "legacy".into()),
                    ("question".into(), "legacy provider behavior".into()),
                    ("answer".into(), "must be refreshed".into()),
                    ("freshness".into(), "0s".into()),
                    ("state".into(), "fresh".into()),
                    ("recordedAt".into(), "1970-01-01T00:00:00Z".into()),
                ],
            )
            .expect("write legacy record");
        let home_arg = home.to_string_lossy().to_string();
        let (code, out, err) = run(
            "memory",
            "research-cache",
            &[
                "lookup",
                "--query",
                "legacy provider",
                "--json",
                "--claude-home",
                &home_arg,
            ],
        );
        assert_eq!(code, 0, "legacy lookup must succeed; stderr: {err}");
        assert!(out.contains("\"count\": 0"), "legacy expiry leaked: {out}");
        assert!(
            out.contains("must be refreshed"),
            "omitted stale legacy match should remain inspectable: {out}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn freshness_ttl_parser_rejects_ambiguous_guidance() {
        assert_eq!(parse_freshness_ttl_millis("30d"), Some(30 * 86_400_000));
        assert_eq!(
            parse_freshness_ttl_millis("TTL: 12 hours"),
            Some(12 * 3_600_000)
        );
        assert_eq!(
            parse_freshness_ttl_millis("refresh when upstream changes"),
            None
        );
        assert_eq!(parse_freshness_ttl_millis("30"), None);
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
    fn bump_loop_guard_exhausts_at_budget() {
        let home = temp_home("lg-bump");
        let (count, exhausted) = bump_loop_guard(&home, "anvil-loop:all", 2).expect("bump 1");
        assert_eq!(count, 1);
        assert!(!exhausted);
        let (count, exhausted) = bump_loop_guard(&home, "anvil-loop:all", 2).expect("bump 2");
        assert_eq!(count, 2);
        assert!(exhausted);
        assert!(loop_guard_exhausted(&home, "anvil-loop:all", 2));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn loop_guard_corrupt_count_fails_closed() {
        let home = temp_home("lg-corrupt");
        let h = home.to_string_lossy().to_string();
        let store = family_store(&home, "memory", "loop-guard");
        let id = sanitize_id("same error");
        let seed = vec![
            ("id".to_string(), id.clone()),
            ("signature".to_string(), "same error".to_string()),
            ("count".to_string(), "garbage".to_string()),
        ];
        assert!(
            store.write_record(&id, &seed).is_ok(),
            "seed corrupt record"
        );
        assert!(
            bump_loop_guard(&home, "same error", 2).is_err(),
            "corrupt count must not silently reset the budget"
        );
        assert!(
            loop_guard_exhausted(&home, "same error", 2),
            "corrupt count must read as exhausted"
        );
        let (code, out, _) = run(
            "memory",
            "loop-guard",
            &["check", "--signature", "same error", "--claude-home", &h],
        );
        assert_eq!(code, 2, "corrupt check must exit 2; stdout: {out}");
        assert!(out.contains("exhausted=true"), "stdout: {out}");
        let (code, _, err) = run(
            "memory",
            "loop-guard",
            &["record", "--signature", "same error", "--claude-home", &h],
        );
        assert_eq!(code, 1, "corrupt record must refuse; stderr: {err}");
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
            "memory",
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
            "memory",
            "graph",
            &["query", "--node", "bug-42", "--claude-home", &h],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("PR-1 --[fixes]--> bug-42"), "stdout: {out}");
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
    fn retrieve_enforces_query_and_limit_bounds_and_keeps_provenance() {
        let home = temp_home("retr-bounds");
        let h = home.to_string_lossy().to_string();
        run(
            "memory",
            "research-cache",
            &[
                "record",
                "--question",
                "bounded recall query",
                "--answer",
                "bounded answer",
                "--claude-home",
                &h,
            ],
        );

        let (code, out, err) = run(
            "memory",
            "retrieve",
            &[
                "--query",
                "bounded",
                "--limit",
                "1000",
                "--json",
                "--claude-home",
                &h,
            ],
        );
        assert_eq!(code, 0, "stderr: {err}");
        assert!(
            out.contains("\"limit\": 100"),
            "limit must be capped: {out}"
        );
        assert!(
            out.contains("\"provenanceId\": \"prov-"),
            "provenance: {out}"
        );
        assert!(
            out.contains("\"retrievalRef\""),
            "recovery reference: {out}"
        );

        let oversized = "x".repeat(crate::utility::recall::MAX_RECALL_QUERY_BYTES + 1);
        let (code, _, err) = run("memory", "retrieve", &["--query", &oversized]);
        assert_eq!(code, 2, "oversized query must be rejected");
        assert!(err.contains("byte"), "stderr: {err}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn retrieve_hits_respect_serialized_result_budgets() {
        let hits = (0..crate::utility::recall::MAX_RECALL_LIMIT + 10)
            .map(|index| crate::utility::recall::RecallHit {
                absolute_path: format!("C:/memory/note-{index}.md"),
                score: 0.5,
                line: index + 1,
                snippet: format!("[match] memory result {index}"),
            })
            .collect();
        let bounded = bound_retrieve_hits("memory", usize::MAX, hits);
        assert!(bounded.len() <= crate::utility::recall::MAX_RECALL_LIMIT);
        let values = bounded
            .iter()
            .map(|hit| retrieve_hit_value("memory", usize::MAX, hit))
            .collect();
        let rendered = serialized_value(&Value::Array(values));
        assert!(
            rendered.len()
                <= crate::utility::recall::MAX_RECALL_RESULT_BYTES
                    .saturating_sub(MEMORY_RETRIEVE_RESULT_OVERHEAD_BYTES),
            "serialized hits exceeded byte bound: {}",
            rendered.len()
        );
        assert!(
            TokenMeter::count_bytes(&rendered)
                <= crate::utility::recall::MAX_RECALL_RESULT_TOKENS
                    .saturating_sub(MEMORY_RETRIEVE_RESULT_OVERHEAD_TOKENS),
            "serialized hits exceeded token bound"
        );
    }

    #[test]
    fn retrieve_json_projection_recounts_complete_envelope() {
        let query = "memory query "
            .repeat(crate::utility::recall::MAX_RECALL_QUERY_BYTES / "memory query ".len());
        let hits = (0..crate::utility::recall::MAX_RECALL_LIMIT)
            .map(|index| crate::utility::recall::RecallHit {
                absolute_path: format!("C:/memory/note-{index}.md"),
                score: 0.5,
                line: index + 1,
                snippet: "[match] long memory result ".repeat(80),
            })
            .collect::<Vec<_>>();
        let (_, payload) = bounded_retrieve_projection(&query, "exact", usize::MAX, &hits);
        let rendered = serialized_value(&payload);
        assert!(
            rendered.len() <= crate::utility::recall::MAX_RECALL_RESULT_BYTES,
            "complete retrieve envelope exceeded byte bound: {}",
            rendered.len()
        );
        assert!(
            TokenMeter::count_bytes(&rendered) <= crate::utility::recall::MAX_RECALL_RESULT_TOKENS,
            "complete retrieve envelope exceeded token bound"
        );
        let text = String::from_utf8(rendered).expect("json utf8");
        assert!(text.contains("provenanceId"), "provenance missing: {text}");
        assert!(text.contains("retrievalRef"), "recovery missing: {text}");
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok());
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
