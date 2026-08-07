//! Purpose: Dependency-aware work graph, backing `keel work`.
//!   A work item is an open unit of work carrying a status, priority, optional
//!   hard blockers (`depends_on`), and optional provenance (`discovered_from`).
//!   The graph answers "what can I start now" (`ready`) and "what is waiting"
//!   (`blocked`), so an agent that discovers ten things while fixing one cannot
//!   drop the other nine — each is captured as a node and surfaced until done.
//! Caller: commands.rs `work` dispatch arm; hook_lifecycle closeout gate.
//! Dependencies: std::io, crate::args::FlagSet, crate::runtime path helpers,
//!   crate::utility::record_store (durable per-item JSON records), serde_json.
//! Main Functions: run_work_command (add|list|ready|blocked|dep|discovered|close|show),
//!   open_work_items_for_workspace (closeout gate), and the pure graph queries
//!   compute_ready / compute_blocked / would_create_cycle.
//! Side Effects: Reads/writes item records under `<claude_home>/work/<workspace-slug>/`.
//!
//! Why a graph and not a flat list: the anti-drift property the agent needs is
//! "the work I discovered but did not finish is still reachable next session".
//! A flat todo loses dependency order and silently drops discovered work at
//! compaction. Storing items + edges as durable records makes `ready`/`blocked`
//! deterministic and the discovered-from link survives a fresh session.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root};
use crate::utility::record_store::{
    allocate_unique_record_id, field, join_lines, split_lines, Record, RecordStore,
};

/// Lifecycle states, in board order. `done` is the only state that takes an
/// item out of the open set; `blocked` is explicitly NOT done so a blocked item
/// keeps surfacing at closeout (a blocker is reported, never silently complete).
const STATES: &[&str] = &["open", "in-progress", "blocked", "done"];
const STATE_DONE: &str = "done";

/// An open (not-done) work item, for listings and the closeout gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenItem {
    pub id: String,
    pub title: String,
    pub status: String,
    /// Blocker ids that are not yet done (empty for a ready item).
    pub open_blockers: Vec<String>,
}

/// Closeout view of a workspace's work graph, mirroring the sprint gate:
/// - `None` when no work items exist (gate silent — ordinary turns untouched).
/// - `Some(vec![])` when items exist and every one is done (gate satisfied).
/// - `Some(open)` listing not-done items when work remains (gate reports gaps).
pub fn open_work_items_for_workspace(
    claude_home: &Path,
    workspace_root: &str,
) -> Result<Option<Vec<OpenItem>>, String> {
    let store = RecordStore::new(claude_home, &work_group_for_workspace(workspace_root));
    let items = store.list_records()?;
    if items.is_empty() {
        return Ok(None);
    }
    let done: HashSet<String> = items
        .iter()
        .filter(|(_, record)| field(record, "status") == Some(STATE_DONE))
        .map(|(id, _)| id.clone())
        .collect();
    let open = items
        .iter()
        .filter(|(_, record)| field(record, "status") != Some(STATE_DONE))
        .map(|(id, record)| OpenItem {
            id: id.clone(),
            title: field(record, "title").unwrap_or("").to_string(),
            status: field(record, "status").unwrap_or("open").to_string(),
            open_blockers: blockers_of(record)
                .into_iter()
                .filter(|blocker| !done.contains(blocker))
                .collect(),
        })
        .collect();
    Ok(Some(open))
}

/// Resolve the per-workspace work store group path, normalizing the path the same
/// way the CLI does so the closeout gate reads the exact directory `work add` wrote.
fn work_group_for_workspace(workspace_root: &str) -> String {
    // Absolutize+clean so the writer (work CLI) and the reader (closeout gate)
    // slug the SAME lane for every --workspace-root input form.
    let normalized = resolve_repository_root(workspace_root)
        .map(|path| display_path(&path))
        .unwrap_or_else(|_| display_path(&std::path::PathBuf::from(workspace_root)));
    format!("work/{}", workspace_slug(&normalized))
}

/// Slugify a workspace path into a single safe directory segment. Mirrors the
/// sprint store so two projects never share a work graph.
fn workspace_slug(workspace_root: &str) -> String {
    let mut slug = String::with_capacity(workspace_root.len());
    let mut last_dash = false;
    for ch in workspace_root.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        // Match the sprint/team/dispatch/SYSTEM_MAP fallback so a degenerate
        // path lands in the same shared lane everywhere, not a divergent one.
        "workspace".to_string()
    } else {
        trimmed
    }
}

/// The blocker ids of one item record (the `depends_on[]` list field).
fn blockers_of(record: &Record) -> Vec<String> {
    field(record, "depends_on[]")
        .map(split_lines)
        .unwrap_or_default()
}

// ---- pure graph queries (unit-tested without IO) ----

/// Ready items: status `open` or `in-progress` whose every blocker is done.
/// Returns ids in input order. A missing blocker record counts as not-done
/// (a dangling dependency keeps the item blocked rather than silently ready).
pub fn compute_ready(items: &[(String, Record)]) -> Vec<String> {
    let done = done_set(items);
    items
        .iter()
        .filter(|(_, record)| {
            let status = field(record, "status").unwrap_or("open");
            matches!(status, "open" | "in-progress")
                && blockers_of(record)
                    .iter()
                    .all(|blocker| done.contains(blocker))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// Blocked items: not done, and either explicitly `blocked` or carrying at least
/// one blocker that is not done. Each entry pairs the id with its open blockers.
pub fn compute_blocked(items: &[(String, Record)]) -> Vec<(String, Vec<String>)> {
    let done = done_set(items);
    items
        .iter()
        .filter(|(_, record)| field(record, "status") != Some(STATE_DONE))
        .filter_map(|(id, record)| {
            let open_blockers: Vec<String> = blockers_of(record)
                .into_iter()
                .filter(|blocker| !done.contains(blocker))
                .collect();
            let explicitly_blocked = field(record, "status") == Some("blocked");
            if open_blockers.is_empty() && !explicitly_blocked {
                None
            } else {
                Some((id.clone(), open_blockers))
            }
        })
        .collect()
}

/// Whether adding edge `from depends_on to` would create a cycle — i.e. `to`
/// already (transitively) depends on `from`. A self-edge is always a cycle.
/// Walks the existing `depends_on` edges from `to`; if it reaches `from`, the
/// new edge would close a loop and must be rejected.
pub fn would_create_cycle(items: &[(String, Record)], from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let edges: HashMap<&str, Vec<String>> = items
        .iter()
        .map(|(id, record)| (id.as_str(), blockers_of(record)))
        .collect();
    let mut stack = vec![to.to_string()];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if node == from {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(deps) = edges.get(node.as_str()) {
            stack.extend(deps.iter().cloned());
        }
    }
    false
}

fn done_set(items: &[(String, Record)]) -> HashSet<String> {
    items
        .iter()
        .filter(|(_, record)| field(record, "status") == Some(STATE_DONE))
        .map(|(id, _)| id.clone())
        .collect()
}

// ---- CLI ----

/// CLI: `keel work <add|list|ready|blocked|dep|discovered|close|show> [flags]`.
pub fn run_work_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        let _ = writeln!(
            standard_output,
            "Usage: keel work <add|list|ready|blocked|dep|discovered|close|show> [flags]\n\
             \n\
             add        Create a work item.\n\
             \x20          --title \"...\" [--priority 0-4] [--id <id>]\n\
             list       Show every item and its status.\n\
             ready      List items with no open blocker (what to start now).\n\
             blocked    List items still waiting on a blocker, with the blockers.\n\
             dep        Add a hard dependency: --id A depends on --on B (B blocks A).\n\
             \x20          Rejected if it would create a cycle.\n\
             discovered Capture work found mid-task so it is not dropped.\n\
             \x20          --title \"...\" --from <parent-id> [--priority 0-4]\n\
             close      Mark an item done.  --id <id>\n\
             show       Show one item.  --id <id>\n\
             \n\
             Common flags: --workspace-root <path>  --claude-home <path>  --json"
        );
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "add" => run_add(&arguments[1..], standard_output, standard_error),
        "list" => run_list(&arguments[1..], standard_output, standard_error),
        "ready" => run_ready(&arguments[1..], standard_output, standard_error),
        "blocked" => run_blocked(&arguments[1..], standard_output, standard_error),
        "dep" => run_dep(&arguments[1..], standard_output, standard_error),
        "discovered" => run_discovered(&arguments[1..], standard_output, standard_error),
        "close" => run_close(&arguments[1..], standard_output, standard_error),
        "show" => run_show(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "work: unknown subcommand: {other}");
            1
        }
    }
}

fn resolve_store(
    workspace_root: &str,
    claude_home: &str,
    label: &str,
    standard_error: &mut dyn Write,
) -> Option<RecordStore> {
    let claude_home = match resolve_claude_home(claude_home) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return None;
        }
    };
    Some(RecordStore::new(
        &claude_home,
        &work_group_for_workspace(workspace_root),
    ))
}

fn common_flags() -> FlagSet {
    let mut flags = FlagSet::new("work");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    flags
}

fn normalize_priority(raw: &str) -> String {
    match raw.trim().parse::<u8>() {
        Ok(value) if value <= 4 => value.to_string(),
        _ => "2".to_string(),
    }
}

/// Validate a requested status against the board order, returning the
/// normalized value or an error naming the valid set. `add` defaults to `open`.
fn normalize_status(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("open".to_string());
    }
    if STATES.contains(&trimmed) {
        Ok(trimmed.to_string())
    } else {
        Err(format!(
            "invalid status {trimmed:?}: expected one of {}",
            STATES.join(", ")
        ))
    }
}

fn now_id_base() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("wi-{ms}")
}

fn run_add(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    flags.string_flag("title", "");
    flags.string_flag("priority", "2");
    flags.string_flag("status", "");
    flags.string_flag("id", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let title = flags.string_value("title").trim().to_string();
    if title.is_empty() {
        let _ = writeln!(standard_error, "work add: --title is required");
        return 1;
    }
    let status = match normalize_status(flags.string_value("status")) {
        Ok(status) => status,
        Err(message) => {
            let _ = writeln!(standard_error, "work add: {message}");
            return 1;
        }
    };
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work add",
        standard_error,
    ) else {
        return 1;
    };
    let requested_id = flags.string_value("id").trim().to_string();
    let base = if requested_id.is_empty() {
        now_id_base()
    } else {
        requested_id
    };
    let id = allocate_unique_record_id(&store, &base);
    let record: Record = vec![
        ("id".into(), id.clone()),
        ("title".into(), title.clone()),
        ("status".into(), status.clone()),
        (
            "priority".into(),
            normalize_priority(flags.string_value("priority")),
        ),
        ("depends_on[]".into(), String::new()),
        ("discovered_from".into(), String::new()),
    ];
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "work add: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        let _ = writeln!(
            standard_output,
            "{{\"id\":\"{id}\",\"status\":\"{status}\"}}"
        );
    } else {
        let _ = writeln!(
            standard_output,
            "work add: id={id} status={status}\n  {title}"
        );
    }
    0
}

fn run_discovered(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    flags.string_flag("title", "");
    flags.string_flag("from", "");
    flags.string_flag("priority", "2");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let title = flags.string_value("title").trim().to_string();
    let from = flags.string_value("from").trim().to_string();
    if title.is_empty() || from.is_empty() {
        let _ = writeln!(
            standard_error,
            "work discovered: --title and --from <parent-id> are both required"
        );
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work discovered",
        standard_error,
    ) else {
        return 1;
    };
    match store.read_record(&from) {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = writeln!(
                standard_error,
                "work discovered: parent {from} not found (create it first)"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "work discovered: {error}");
            return 1;
        }
    }
    let id = allocate_unique_record_id(&store, &now_id_base());
    let record: Record = vec![
        ("id".into(), id.clone()),
        ("title".into(), title.clone()),
        ("status".into(), "open".into()),
        (
            "priority".into(),
            normalize_priority(flags.string_value("priority")),
        ),
        ("depends_on[]".into(), String::new()),
        ("discovered_from".into(), from.clone()),
    ];
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "work discovered: {error}");
        return 1;
    }
    if flags.bool_value("json") {
        let _ = writeln!(
            standard_output,
            "{{\"id\":\"{id}\",\"discovered_from\":\"{from}\"}}"
        );
    } else {
        let _ = writeln!(
            standard_output,
            "work discovered: id={id} discovered_from={from}\n  {title}"
        );
    }
    0
}

fn run_dep(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    flags.string_flag("id", "");
    flags.string_flag("on", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    let on = flags.string_value("on").trim().to_string();
    if id.is_empty() || on.is_empty() {
        let _ = writeln!(
            standard_error,
            "work dep: --id A and --on B are both required (A depends on B)"
        );
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work dep",
        standard_error,
    ) else {
        return 1;
    };
    let items = match store.list_records() {
        Ok(items) => items,
        Err(error) => {
            let _ = writeln!(standard_error, "work dep: {error}");
            return 1;
        }
    };
    if !items.iter().any(|(item_id, _)| item_id == &id) {
        let _ = writeln!(standard_error, "work dep: item {id} not found");
        return 1;
    }
    if !items.iter().any(|(item_id, _)| item_id == &on) {
        let _ = writeln!(standard_error, "work dep: blocker {on} not found");
        return 1;
    }
    if would_create_cycle(&items, &id, &on) {
        let _ = writeln!(
            standard_error,
            "work dep: refused — {id} depends-on {on} would create a cycle"
        );
        return 1;
    }
    let mut record = match store.read_record(&id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "work dep: item {id} not found");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "work dep: {error}");
            return 1;
        }
    };
    let mut blockers = blockers_of(&record);
    if !blockers.contains(&on) {
        blockers.push(on.clone());
    }
    set_field(&mut record, "depends_on[]", join_lines(&blockers));
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "work dep: {error}");
        return 1;
    }
    let _ = writeln!(standard_output, "work dep: {id} now depends on {on}");
    0
}

fn run_close(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    flags.string_flag("id", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "work close: --id is required");
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work close",
        standard_error,
    ) else {
        return 1;
    };
    let mut record = match store.read_record(&id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "work close: item {id} not found");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "work close: {error}");
            return 1;
        }
    };
    set_field(&mut record, "status", STATE_DONE.to_string());
    if let Err(error) = store.write_record(&id, &record) {
        let _ = writeln!(standard_error, "work close: {error}");
        return 1;
    }
    let _ = writeln!(standard_output, "work close: {id} done");
    0
}

fn run_list(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work list",
        standard_error,
    ) else {
        return 1;
    };
    let items = match store.list_records() {
        Ok(items) => items,
        Err(error) => {
            let _ = writeln!(standard_error, "work list: {error}");
            return 1;
        }
    };
    if items.is_empty() {
        let _ = writeln!(standard_output, "work list: no items");
        return 0;
    }
    let _ = writeln!(standard_output, "work list: {} item(s)", items.len());
    for (id, record) in &items {
        let _ = writeln!(
            standard_output,
            "  {} [{}] p{} {}",
            id,
            field(record, "status").unwrap_or("open"),
            field(record, "priority").unwrap_or("2"),
            field(record, "title").unwrap_or("")
        );
    }
    0
}

fn run_ready(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work ready",
        standard_error,
    ) else {
        return 1;
    };
    let items = match store.list_records() {
        Ok(items) => items,
        Err(error) => {
            let _ = writeln!(standard_error, "work ready: {error}");
            return 1;
        }
    };
    let titles: HashMap<&str, &str> = items
        .iter()
        .map(|(id, record)| (id.as_str(), field(record, "title").unwrap_or("")))
        .collect();
    let ready = compute_ready(&items);
    if ready.is_empty() {
        let _ = writeln!(standard_output, "work ready: nothing ready");
        return 0;
    }
    let _ = writeln!(standard_output, "work ready: {} item(s)", ready.len());
    for id in &ready {
        let _ = writeln!(
            standard_output,
            "  {} {}",
            id,
            titles.get(id.as_str()).copied().unwrap_or("")
        );
    }
    0
}

fn run_blocked(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work blocked",
        standard_error,
    ) else {
        return 1;
    };
    let items = match store.list_records() {
        Ok(items) => items,
        Err(error) => {
            let _ = writeln!(standard_error, "work blocked: {error}");
            return 1;
        }
    };
    let blocked = compute_blocked(&items);
    if blocked.is_empty() {
        let _ = writeln!(standard_output, "work blocked: nothing blocked");
        return 0;
    }
    let _ = writeln!(standard_output, "work blocked: {} item(s)", blocked.len());
    for (id, open_blockers) in &blocked {
        let _ = writeln!(
            standard_output,
            "  {} waiting on: {}",
            id,
            if open_blockers.is_empty() {
                "(explicitly blocked)".to_string()
            } else {
                open_blockers.join(", ")
            }
        );
    }
    0
}

fn run_show(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = common_flags();
    flags.string_flag("id", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let id = flags.string_value("id").trim().to_string();
    if id.is_empty() {
        let _ = writeln!(standard_error, "work show: --id is required");
        return 1;
    }
    let Some(store) = resolve_store(
        flags.string_value("workspace-root"),
        flags.string_value("claude-home"),
        "work show",
        standard_error,
    ) else {
        return 1;
    };
    match store.read_record(&id) {
        Ok(Some(record)) => {
            let _ = writeln!(standard_output, "id: {id}");
            let _ = writeln!(
                standard_output,
                "title: {}",
                field(&record, "title").unwrap_or("")
            );
            let _ = writeln!(
                standard_output,
                "status: {}",
                field(&record, "status").unwrap_or("open")
            );
            let _ = writeln!(
                standard_output,
                "priority: {}",
                field(&record, "priority").unwrap_or("2")
            );
            let blockers = blockers_of(&record);
            let _ = writeln!(
                standard_output,
                "depends_on: {}",
                if blockers.is_empty() {
                    "(none)".to_string()
                } else {
                    blockers.join(", ")
                }
            );
            let from = field(&record, "discovered_from").unwrap_or("");
            if !from.is_empty() {
                let _ = writeln!(standard_output, "discovered_from: {from}");
            }
            0
        }
        Ok(None) => {
            let _ = writeln!(standard_error, "work show: item {id} not found");
            1
        }
        Err(error) => {
            let _ = writeln!(standard_error, "work show: {error}");
            1
        }
    }
}

fn set_field(record: &mut Record, key: &str, value: String) {
    if let Some(existing) = record.iter_mut().find(|(field_key, _)| field_key == key) {
        existing.1 = value;
    } else {
        record.push((key.to_string(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: &str, depends_on: &[&str]) -> (String, Record) {
        let deps = depends_on.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        (
            id.to_string(),
            vec![
                ("id".into(), id.into()),
                ("title".into(), format!("title {id}")),
                ("status".into(), status.into()),
                ("priority".into(), "2".into()),
                ("depends_on[]".into(), join_lines(&deps)),
                ("discovered_from".into(), String::new()),
            ],
        )
    }

    #[test]
    fn ready_excludes_items_with_open_blockers() {
        // a is open with no blocker -> ready. b is open but blocked by a (open) -> not ready.
        let items = vec![item("a", "open", &[]), item("b", "open", &["a"])];
        let ready = compute_ready(&items);
        assert_eq!(ready, vec!["a".to_string()]);
    }

    #[test]
    fn ready_includes_item_once_blocker_done() {
        // a done -> b becomes ready.
        let items = vec![item("a", "done", &[]), item("b", "open", &["a"])];
        let ready = compute_ready(&items);
        assert_eq!(ready, vec!["b".to_string()]);
    }

    #[test]
    fn ready_treats_missing_blocker_as_not_done() {
        // b depends on ghost which has no record -> b stays blocked, not ready.
        let items = vec![item("b", "open", &["ghost"])];
        assert!(compute_ready(&items).is_empty());
    }

    #[test]
    fn blocked_lists_open_blockers_only() {
        let items = vec![
            item("a", "done", &[]),
            item("b", "open", &["a", "c"]),
            item("c", "open", &[]),
        ];
        let blocked = compute_blocked(&items);
        // b is blocked, its only OPEN blocker is c (a is done).
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].0, "b");
        assert_eq!(blocked[0].1, vec!["c".to_string()]);
    }

    #[test]
    fn explicitly_blocked_status_counts_even_without_edges() {
        let items = vec![item("a", "blocked", &[])];
        let blocked = compute_blocked(&items);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].0, "a");
        assert!(blocked[0].1.is_empty());
    }

    #[test]
    fn cycle_detection_rejects_self_and_loops() {
        let items = vec![item("a", "open", &["b"]), item("b", "open", &[])];
        // self edge
        assert!(would_create_cycle(&items, "a", "a"));
        // a already depends on b; adding b depends-on a would close a 2-cycle.
        assert!(would_create_cycle(&items, "b", "a"));
        // a depends-on b is already present and is acyclic in the other direction.
        assert!(!would_create_cycle(&items, "c", "a"));
    }

    #[test]
    fn open_work_items_none_when_empty() {
        let home = std::env::temp_dir().join(format!(
            "keel-wg-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let result = open_work_items_for_workspace(&home, "C:/proj").unwrap();
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn open_work_items_surfaces_not_done() {
        let home = std::env::temp_dir().join(format!(
            "keel-wg-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let workspace = "C:/proj";
        let store = RecordStore::new(&home, &work_group_for_workspace(workspace));
        store.write_record("a", &item("a", "done", &[]).1).unwrap();
        store
            .write_record("b", &item("b", "open", &["a"]).1)
            .unwrap();
        let result = open_work_items_for_workspace(&home, workspace)
            .unwrap()
            .unwrap();
        // a is done -> only b is open.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "b");
        // b's blocker a is done, so it has no OPEN blockers.
        assert!(result[0].open_blockers.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn states_constant_is_board_order() {
        assert_eq!(STATES, &["open", "in-progress", "blocked", "done"]);
    }

    #[test]
    fn normalize_status_defaults_and_validates() {
        assert_eq!(normalize_status("").unwrap(), "open");
        assert_eq!(normalize_status("in-progress").unwrap(), "in-progress");
        assert!(normalize_status("frobnicate").is_err());
    }
}
