//! Purpose: System map command handlers and shared path resolution
//! Caller: mod.rs run_memory_command, mcp/tools.rs (refresh_system_map, system_map_reference_directory)
//! Dependencies: std::fs, std::io, std::path, crate::args::FlagSet, crate::json, crate::runtime, crate::utility::system_map
//! Main Functions: run_system_map_command, refresh_system_map, system_map_reference_directory
//! Side Effects: Writes system map files

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::args::FlagSet;
use crate::runtime::{display_path, resolve_claude_home, resolve_repository_root};
use crate::utility::workspace_index;

use super::shared::is_help_argument;

pub(super) fn run_system_map_command(
    command_group: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel {command_group} system-map [refresh|show] [flags]"
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

/// Resolve the cached SYSTEM_MAP.md reference directory for a workspace under a
/// command group. Shared so the CLI refresh and the MCP
/// `system_map_refresh` tool agree on where the map lives.
pub fn system_map_reference_directory(
    claude_home: &Path,
    _command_group: &str,
    workspace_root: &Path,
) -> PathBuf {
    let workspace_slug =
        crate::utility::system_map::workspace_key(&workspace_root.to_string_lossy());
    claude_home
        .join("memories")
        .join("workspaces")
        .join(&workspace_slug)
        .join("reference")
}

/// Render and persist the workspace SYSTEM_MAP.md, returning the path written.
/// Backs both the CLI `system-map refresh` subcommand and the MCP
/// `system_map_refresh` tool so the two share one render+write path. `Err`
/// carries a caller-ready message; the caller decides how to surface it.
pub fn refresh_system_map(
    claude_home: &Path,
    command_group: &str,
    workspace_root: &Path,
) -> Result<PathBuf, String> {
    if !workspace_root.is_dir() {
        return Err(format!(
            "workspace-root not a directory: {}",
            display_path(workspace_root)
        ));
    }
    let reference_directory =
        system_map_reference_directory(claude_home, command_group, workspace_root);
    let system_map_path = reference_directory.join("SYSTEM_MAP.md");
    fs::create_dir_all(&reference_directory)
        .map_err(|error| format!("create {}: {error}", display_path(&reference_directory)))?;
    let map_content = workspace_index::render_map(workspace_root, &claude_home.to_string_lossy())?;
    crate::runtime::write_text(&system_map_path, &map_content)
        .map_err(|error| format!("write {}: {error}", display_path(&system_map_path)))?;
    Ok(system_map_path)
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
    // Absolutize+clean default AND explicit roots so a relative --workspace-root
    // resolves to the same reference lane as the absolute form (see code_graph).
    let workspace_root = match resolve_repository_root(workspace_root_value) {
        Ok(path) => path,
        Err(_) => {
            let _ = writeln!(
                standard_error,
                "{command_group} system-map refresh: no repository root found"
            );
            return 1;
        }
    };
    let Some(claude_home) = resolve_claude_home(flag_set.string_value("claude-home")).ok() else {
        let _ = writeln!(
            standard_error,
            "{command_group} system-map refresh: unable to resolve harness home"
        );
        return 1;
    };
    match refresh_system_map(&claude_home, command_group, &workspace_root) {
        Ok(system_map_path) => {
            let _ = writeln!(
                standard_output,
                "{command_group} system-map refresh: wrote {}",
                display_path(&system_map_path)
            );
            0
        }
        Err(message) => {
            let _ = writeln!(
                standard_error,
                "{command_group} system-map refresh: {message}"
            );
            1
        }
    }
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
            "{command_group} system-map show: unable to resolve harness home"
        );
        return 1;
    };
    let system_map_path =
        system_map_reference_directory(&claude_home, command_group, &workspace_root)
            .join("SYSTEM_MAP.md");
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
