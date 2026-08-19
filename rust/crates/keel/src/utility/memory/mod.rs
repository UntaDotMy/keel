//! Purpose: Memory and scope command handlers for workspace-scoped memory management
//! Caller: commands.rs via utility dispatcher
//! Dependencies: std::fs, std::io, std::path, crate::args, crate::json, crate::runtime, crate::utility::system_map, crate::utility::workflow_ledger
//! Main Functions: run_memory_command, run_scope_command, run_system_map_command
//! Side Effects: Creates memory directories, reads/writes system map files, reads/writes brief and ledger helpers

mod bench;
mod completion_gate;
mod scope;
pub(crate) mod shared;
mod system_map_cmd;
mod working_brief_cmd;

#[cfg(test)]
mod tests;

// Re-exports used by mcp/tools.rs and utility/mod.rs
pub use system_map_cmd::{refresh_system_map, system_map_reference_directory};

use std::io::Write;

use shared::is_help_argument;

/// Top-level dispatcher for the `memory` command group.
pub fn run_memory_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} [scope|system-map|working-brief|completion-gate|consolidate] ..."
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    match arguments[0].as_str() {
        "scope" => scope::run_scope_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "system-map" => system_map_cmd::run_system_map_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "working-brief" => working_brief_cmd::run_working_brief_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "completion-gate" => completion_gate::run_completion_gate_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "consolidate" => run_consolidate_command(
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
        "remember" => run_remember_command(
            command_group,
            &arguments[1..],
            standard_output,
            standard_error,
        ),
        "report" => crate::utility::memory_families::run_memory_family_command(
            command_group,
            "status",
            &arguments[1..],
            standard_output,
            standard_error,
        ),
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
        "hook" => {
            let _ = writeln!(
                standard_error,
                "{command_group} hook: not a memory subcommand. The harness lifecycle hooks are managed by `keel hook install|list|instructions|diagnose`."
            );
            1
        }
        other => {
            let _ = writeln!(standard_error, "Unknown {command_group} command: {other}");
            1
        }
    }
}

/// `memory remember` — the intuitive "save this finding" verb.
fn run_remember_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} remember");
    if !arguments.is_empty() && is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} remember [--family research-cache] --title <text> --text <text> [--source <text>] [--freshness <text>]\n  Sugar over `{command_group} research-cache record`. --question/--answer are accepted as aliases for --title/--text."
        );
        return 0;
    }

    let mut flags = crate::args::FlagSet::new(label.clone());
    flags.string_flag("family", "research-cache");
    flags.string_flag("title", "");
    flags.string_flag("text", "");
    flags.string_flag("question", "");
    flags.string_flag("answer", "");
    flags.string_flag("source", "");
    flags.string_flag("freshness", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }

    let family = flags.string_value("family").trim();
    if family != "research-cache" {
        let _ = writeln!(
            standard_error,
            "{label}: --family {family} has no record verb. Only `research-cache` is supported by `remember` today (use `keel {command_group} {family} ...` directly for other families)."
        );
        return 1;
    }

    let title = {
        let t = flags.string_value("title").trim();
        if t.is_empty() {
            flags.string_value("question").trim()
        } else {
            t
        }
    };
    let text = {
        let t = flags.string_value("text").trim();
        if t.is_empty() {
            flags.string_value("answer").trim()
        } else {
            t
        }
    };
    if title.is_empty() || text.is_empty() {
        let _ = writeln!(
            standard_error,
            "{label}: --title (or --question) and --text (or --answer) are required"
        );
        return 1;
    }

    let mut forwarded: Vec<String> = vec![
        "record".to_string(),
        "--question".to_string(),
        title.to_string(),
        "--answer".to_string(),
        text.to_string(),
    ];
    let source = flags.string_value("source").trim();
    if !source.is_empty() {
        forwarded.push("--source".to_string());
        forwarded.push(source.to_string());
    }
    let freshness = flags.string_value("freshness").trim();
    if !freshness.is_empty() {
        forwarded.push("--freshness".to_string());
        forwarded.push(freshness.to_string());
    }
    let claude_home = flags.string_value("claude-home").trim();
    if !claude_home.is_empty() {
        forwarded.push("--claude-home".to_string());
        forwarded.push(claude_home.to_string());
    }
    if flags.bool_value("json") {
        forwarded.push("--json".to_string());
    }

    crate::utility::memory_families::run_memory_family_command(
        command_group,
        "research-cache",
        &forwarded,
        standard_output,
        standard_error,
    )
}

pub fn run_bench_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    bench::run_bench_command(arguments, standard_output, standard_error)
}

fn run_consolidate_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let label = format!("{command_group} consolidate");
    let mut flags = crate::args::FlagSet::new(label.clone());
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match crate::runtime::resolve_claude_home(flags.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{label}: {error}");
            return 1;
        }
    };
    let group_dir = claude_home.join(command_group);
    let families = [
        "research-cache",
        "entities",
        "graph",
        "loop-guard",
        "instincts",
        "working-brief-summaries",
        "completion-gate-requirements",
    ];
    let mut total_consolidated: usize = 0;
    let mut family_summaries: Vec<(String, usize, String)> = Vec::new();
    for family in families {
        let family_dir = group_dir.join(family);
        if !family_dir.is_dir() {
            continue;
        }
        let entries: Vec<String> = match std::fs::read_dir(&family_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect(),
            Err(_) => continue,
        };
        if entries.is_empty() {
            continue;
        }
        let count = entries.len();
        let preview = entries
            .iter()
            .take(3)
            .filter_map(|text| {
                let fields = crate::utility::workflow_ledger::parse_object_of_strings(text).ok()?;
                let title = fields
                    .iter()
                    .find(|(k, _)| {
                        k == "question"
                            || k == "trigger"
                            || k == "name"
                            || k == "summary"
                            || k == "requirement"
                    })
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("?");
                Some(title.to_string())
            })
            .collect::<Vec<_>>()
            .join("; ");
        total_consolidated += count;
        family_summaries.push((family.to_string(), count, preview));
    }
    if flags.bool_value("json") {
        let payload = crate::json::Value::Object(vec![
            (
                "families".into(),
                crate::json::Value::Array(
                    family_summaries
                        .iter()
                        .map(|(family, count, preview)| {
                            crate::json::Value::Object(vec![
                                ("family".into(), crate::json::Value::String(family.clone())),
                                (
                                    "count".into(),
                                    crate::json::Value::Number(count.to_string()),
                                ),
                                (
                                    "preview".into(),
                                    crate::json::Value::String(preview.clone()),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "totalRecords".into(),
                crate::json::Value::Number(total_consolidated.to_string()),
            ),
        ]);
        return shared::render_workflow_json(standard_output, standard_error, &payload);
    }
    let _ = writeln!(
        standard_output,
        "{label}: scanned {} family directories, {} total records",
        family_summaries.len(),
        total_consolidated
    );
    for (family, count, preview) in &family_summaries {
        let _ = writeln!(standard_output, "  {family}: {count} records — {preview}");
    }
    if total_consolidated == 0 {
        let _ = writeln!(standard_output, "  no records to consolidate");
    }
    0
}
