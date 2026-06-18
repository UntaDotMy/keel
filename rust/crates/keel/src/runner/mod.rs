//! Purpose: Rust-native command execution, rewrite, and hook-management surfaces.
//! Caller: commands.rs for `run`, `rewrite`, `hook`, `raw`, and `replay` command groups.
//! Dependencies: args, json, runtime helpers, proxy, shell_rewrite, hook_lifecycle submodules.
//! Main Functions: run_run_command, run_rewrite_command, run_hook_command, run_raw_command, run_replay_command.
//! Side Effects: Spawns requested child commands, writes raw-output recovery logs, and may write or remove Claude Code hook configuration.

pub mod bridge;
pub mod hook_lifecycle;
pub mod learning;
pub mod observation;
pub mod shell_rewrite;
pub mod telemetry;
pub mod tool_timings;

use std::fs;
use std::io::Write;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, run_command};

// Re-export the public API callers depend on
pub use bridge::run_bridge_command;
pub use hook_lifecycle::run_hook_command;
pub use learning::run_learn_command;
pub use shell_rewrite::rewrite_for_doctor;
pub use telemetry::run_telemetry_command;

pub fn run_run_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    crate::proxy::run::run_proxy(arguments, standard_output, standard_error)
}

pub fn run_rewrite_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("rewrite");
    flag_set.bool_flag("json", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let command = flag_set.positional.join(" ");
    let command = command.trim();
    if command.is_empty() {
        let _ = writeln!(standard_error, "Usage: keel rewrite [--json] \"<command>\"");
        return 1;
    }
    let rewrite = shell_rewrite::rewrite_command_text(command);
    if flag_set.bool_value("json") {
        let adapter_name = shell_rewrite::adapter_name_for_rewrite(command);
        let risk = if rewrite.supported {
            "low"
        } else if rewrite.reason.contains("already") {
            "none"
        } else {
            "unsupported"
        };
        let payload = Value::Object(vec![
            (
                "original_command".into(),
                Value::String(command.to_string()),
            ),
            (
                "rewritten_command".into(),
                Value::String(rewrite.rewritten_command.clone()),
            ),
            ("originalCommand".into(), Value::String(command.to_string())),
            (
                "rewrittenCommand".into(),
                Value::String(rewrite.rewritten_command),
            ),
            ("supported".into(), Value::Bool(rewrite.supported)),
            ("reason".into(), Value::String(rewrite.reason)),
            (
                "adapter_name".into(),
                Value::String(adapter_name.to_string()),
            ),
            (
                "adapterName".into(),
                Value::String(adapter_name.to_string()),
            ),
            ("risk".into(), Value::String(risk.to_string())),
        ]);
        let _ = write_indented(standard_output, &payload);
        return if rewrite.supported { 0 } else { 1 };
    }
    if !rewrite.supported {
        let _ = writeln!(standard_error, "{}", rewrite.reason);
        return 1;
    }
    let _ = writeln!(standard_output, "{}", rewrite.rewritten_command);
    0
}

pub fn run_raw_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.first().map(String::as_str) == Some("list") {
        return run_raw_list(standard_output, standard_error);
    }
    if arguments.first().map(String::as_str) == Some("prune") {
        return run_raw_prune(&arguments[1..], standard_output, standard_error);
    }
    let mut flag_set = FlagSet::new("raw");
    flag_set.bool_flag("path", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let Some(raw_id) = flag_set.positional.first() else {
        let _ = writeln!(
            standard_error,
            "Usage: keel raw [--path] <raw_id> | raw list | raw prune --older-than <Nd>"
        );
        return 1;
    };
    let store = crate::proxy::raw_store::RawStore::new();
    let raw_dir = match store.find_dir(raw_id) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    if flag_set.bool_value("path") {
        let _ = writeln!(standard_output, "{}", display_path(&raw_dir));
        return 0;
    }
    let command = fs::read_to_string(raw_dir.join("command.txt")).unwrap_or_default();
    let stdout = fs::read(raw_dir.join("stdout.log")).unwrap_or_default();
    let stderr = fs::read(raw_dir.join("stderr.log")).unwrap_or_default();
    let _ = writeln!(standard_output, "raw_id: {raw_id}");
    let _ = writeln!(standard_output, "path: {}", display_path(&raw_dir));
    let _ = writeln!(standard_output, "command: {}", command.trim());
    let _ = writeln!(standard_output, "\n[stdout]");
    let _ = standard_output.write_all(&stdout);
    if !stdout.ends_with(b"\n") {
        let _ = writeln!(standard_output);
    }
    let _ = writeln!(standard_output, "\n[stderr]");
    let _ = standard_output.write_all(&stderr);
    if !stderr.ends_with(b"\n") {
        let _ = writeln!(standard_output);
    }
    0
}

pub fn run_replay_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let Some(raw_id) = arguments.first() else {
        let _ = writeln!(standard_error, "Usage: keel replay <raw_id>");
        return 1;
    };
    let store = crate::proxy::raw_store::RawStore::new();
    let meta = match store.load_meta(raw_id) {
        Ok(meta) => meta,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    if !meta.cwd.is_dir() {
        let _ = writeln!(
            standard_error,
            "Saved cwd no longer exists: {}",
            display_path(&meta.cwd)
        );
        return 1;
    }
    let (program, args) = crate::runtime::platform_shell_command_parts(&meta.command);
    match run_command(&program, &args, Some(&meta.cwd)) {
        Ok(result) => {
            let _ = standard_output.write_all(&result.stdout);
            let _ = standard_error.write_all(&result.stderr);
            result.code.clamp(0, 255) as u8
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Unable to replay command: {error}");
            1
        }
    }
}

fn run_raw_list(standard_output: &mut dyn Write, standard_error: &mut dyn Write) -> u8 {
    let store = crate::proxy::raw_store::RawStore::new();
    let entries = match store.list() {
        Ok(entries) => entries,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let _ = writeln!(standard_output, "raw store: {}", display_path(store.root()));
    if entries.is_empty() {
        let _ = writeln!(standard_output, "no raw outputs found");
        return 0;
    }
    for entry in entries.iter().take(50) {
        let command = entry
            .meta
            .as_ref()
            .map(|meta| meta.command.as_str())
            .unwrap_or("unknown");
        let _ = writeln!(
            standard_output,
            "{} exit={} adapter={} {}",
            entry.raw_id,
            entry.meta.as_ref().map(|meta| meta.exit_code).unwrap_or(0),
            entry
                .meta
                .as_ref()
                .map(|meta| meta.adapter_name.as_str())
                .unwrap_or("unknown"),
            command
        );
    }
    if entries.len() > 50 {
        let _ = writeln!(
            standard_output,
            "omitted {} older entries",
            entries.len() - 50
        );
    }
    0
}

fn run_raw_prune(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("raw prune");
    flag_set.string_flag("older-than", "30d");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let days = parse_days(flag_set.string_value("older-than")).unwrap_or(30);
    let store = crate::proxy::raw_store::RawStore::new();
    match store.prune_older_than(days) {
        Ok(count) => {
            let _ = writeln!(
                standard_output,
                "raw prune: removed {count} entries older than {days}d"
            );
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            1
        }
    }
}

fn parse_days(value: &str) -> Option<u64> {
    value.trim_end_matches('d').parse().ok()
}
