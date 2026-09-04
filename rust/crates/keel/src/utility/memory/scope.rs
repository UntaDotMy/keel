//! Purpose: Scope resolution command handlers for workspace-scoped memory management
//! Caller: mod.rs run_memory_command
//! Dependencies: std::fs, std::path, crate::args::FlagSet, crate::json, crate::runtime, crate::utility::system_map
//! Main Functions: run_scope_command
//! Side Effects: Creates memory directories, writes system map files

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::args::FlagSet;
use crate::json::{write_indented, Value};
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root};

use super::shared::is_help_argument;

pub(super) fn run_scope_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} scope resolve [flags]"
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
            "{command_group} scope resolve: unable to resolve harness home"
        );
        return 1;
    };
    let reference_directory = super::system_map_cmd::system_map_reference_directory(
        &claude_home,
        command_group,
        &workspace_root,
    );
    let workspace_directory = reference_directory
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| reference_directory.clone());
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
    let mut system_map_changed = false;
    if flag_set.bool_value("refresh-system-map") || !system_map_path.is_file() {
        match super::system_map_cmd::refresh_system_map_with_status(
            &claude_home,
            command_group,
            &workspace_root,
        ) {
            Ok(report) => system_map_changed = report.changed,
            Err(error) => {
                let _ = writeln!(standard_error, "build indexed system map: {error}");
                return 1;
            }
        }
    }
    let format = flag_set.string_value("format");
    if format == "json" {
        let payload = Value::Object(vec![
            (
                "workspaceRoot".into(),
                Value::String(display_path(&workspace_root)),
            ),
            (
                "workspaceSlug".into(),
                Value::String(
                    workspace_directory
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                ),
            ),
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
            ("systemMapChanged".into(), Value::Bool(system_map_changed)),
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
    let _ = writeln!(
        standard_output,
        "workspace_slug: {}",
        workspace_directory
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );
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
