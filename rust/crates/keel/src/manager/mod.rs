//! Purpose: Thin dispatcher for manager submodules; re-exports public functions for commands.rs callers.
//! Caller: commands.rs top-level command dispatch.
//! Dependencies: manager::install, manager::doctor, manager::verify, manager::agent_config.
//! Main Functions: run_install_command, run_status_command (short dispatchers kept here).
//! Side Effects: Delegates to submodules for all heavy work.

pub mod agent_config;
pub mod doctor;
pub mod install;
pub mod mcp_register;
pub mod repair;
pub mod verify;

pub use doctor::run_doctor_command;
pub use install::{run_self_replace_command, run_uninstall_command, run_update_command};
pub use repair::run_repair_command;
pub use verify::{run_all_command, run_menu_command, run_validate_command, run_verify_command};

use std::io::Write;

use keel_platform::detect_current_target;

use crate::args::FlagSet;
use crate::runtime::{
    display_path, read_text_if_exists, resolve_claude_home, resolve_repository_root,
    skills_directory,
};

use install::{
    install_from_flags, repo_version_for_source, repo_version_from_metadata_or_build,
    write_install_summary,
};
use verify::{count_installed_skills, count_managed_skills, install_metadata_path, metadata_value};

pub fn run_install_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("install");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    match install_from_flags(build_version, &flag_set) {
        Ok(summary) => {
            write_install_summary(&summary, standard_output);
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Native Rust install failed: {error}");
            1
        }
    }
}

pub fn run_status_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("status");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let repository_root = match resolve_repository_root(flag_set.string_value("repo-root")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let installed_skill_count = count_installed_skills(&claude_home);
    let metadata = read_text_if_exists(&install_metadata_path(&claude_home)).unwrap_or_default();
    let layout = crate::runtime::discover_repository_layout(&repository_root);
    let source_skill_count = layout
        .as_ref()
        .ok()
        .map(|value| value.skills.len())
        .or_else(|| count_managed_skills(&claude_home));
    let source_display = if layout.is_ok() {
        display_path(&repository_root)
    } else if source_skill_count.is_some() {
        format!(
            "installed inventory (source unavailable from {})",
            display_path(&repository_root)
        )
    } else {
        format!("unavailable from {}", display_path(&repository_root))
    };
    let repo_version = if layout.is_ok() {
        repo_version_for_source(build_version, &repository_root)
    } else {
        repo_version_from_metadata_or_build(&metadata, build_version)
            .unwrap_or_else(|| "unavailable".to_string())
    };
    let update_status = match source_skill_count {
        Some(expected_count) if installed_skill_count == expected_count => "current",
        Some(_) => "refresh recommended",
        None if installed_skill_count == 0 => "not installed",
        None => "source unavailable",
    };
    let synced_skills = match source_skill_count {
        Some(expected_count) => format!("{installed_skill_count}/{expected_count}"),
        None => format!("{installed_skill_count}/unknown"),
    };
    let target = detect_current_target()
        .map(|value| value.directory_name())
        .unwrap_or_else(|error| format!("unknown ({error})"));
    let _ = writeln!(standard_output, "Claude Code Skill Pack Status");
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Summary:");
    let _ = writeln!(standard_output, "  Manager version: {build_version}");
    let _ = writeln!(standard_output, "  Repo version: {}", repo_version);
    let _ = writeln!(
        standard_output,
        "  Installed version: {}",
        metadata_value(&metadata, "manager_version").unwrap_or("not installed")
    );
    let _ = writeln!(standard_output, "  Install source: Rust-native manager");
    let _ = writeln!(
        standard_output,
        "  Skill pack update status: {}",
        update_status
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Claude Code Skills:");
    let _ = writeln!(standard_output, "  Source: {}", source_display);
    let _ = writeln!(
        standard_output,
        "  Target: {}",
        display_path(&skills_directory(&claude_home))
    );
    let _ = writeln!(standard_output, "  Platform: {target}");
    let _ = writeln!(standard_output, "  Synced skills: {synced_skills}");
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Runtime:");
    let _ = writeln!(standard_output, "  implementation: rust");
    0
}
