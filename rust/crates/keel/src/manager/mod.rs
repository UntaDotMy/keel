//! Purpose: Thin dispatcher for manager submodules; re-exports public functions for commands.rs callers.
//! Caller: commands.rs top-level command dispatch.
//! Dependencies: manager::install, manager::doctor, manager::verify, manager::agent_config.
//! Main Functions: run_install_command, run_status_command (short dispatchers kept here).
//! Side Effects: Delegates to submodules for all heavy work.

pub mod agent_config;
pub mod doctor;
pub mod install;
pub mod mcp_register;
pub mod platform_detect;
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
    install_from_flags, install_from_paths, install_purge_stale_enabled, parse_overrides,
    repo_version_for_source, repo_version_from_metadata_or_build, resolve_install_repository_root,
    write_install_summary, InstallOverrides, PlatformName,
};

use verify::{
    count_installed_skills, count_learned_skills, count_managed_skills, install_metadata_path,
    metadata_value, stale_managed_skill_names,
};

pub fn run_install_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("install");
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("with", "");
    flag_set.string_flag("without", "");
    flag_set.bool_flag("interactive", false);
    // Default install never deletes orphans. Opt in to pack hygiene deletes
    // with --purge-stale (still never touches sessions/projects/memories/etc.).
    flag_set.bool_flag("no-purge", false);
    flag_set.bool_flag("purge-stale", false);
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    if flag_set.bool_value("interactive") {
        return run_interactive_install(build_version, &flag_set, standard_output, standard_error);
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

/// Guided interactive install flow. Prompts for an explicit host adapter and
/// verifies the same repository/home that was installed.
fn run_interactive_install(
    build_version: &str,
    flag_set: &FlagSet,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let _ = writeln!(standard_output, "keel interactive setup");
    let _ = writeln!(standard_output, "=====================");
    let _ = writeln!(standard_output);

    let detected_harness = detect_harness_type();
    let _ = writeln!(standard_output, "1. Detected harness: {detected_harness}");
    let _ = writeln!(standard_output);
    let harness_choice = read_line_from_stdin(
        "   Harness [claude/opencode/codex/pi/cursor/cowork/commandcode/grok] (Enter to accept): ",
    );
    let harness = if harness_choice.trim().is_empty() {
        detected_harness
    } else {
        harness_choice.trim().to_string()
    };
    let _ = writeln!(standard_output, "   -> Using: {harness}");
    let _ = writeln!(standard_output);

    let overrides = match interactive_overrides(flag_set, &harness) {
        Ok(overrides) => overrides,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let _ = writeln!(standard_output, "2. Configuration summary:");
    let _ = writeln!(standard_output, "   Harness: {harness}");
    let _ = writeln!(
        standard_output,
        "   Feature set: standard (hooks, compaction, memory, selected host adapter)"
    );
    let _ = writeln!(standard_output);

    let confirm = read_line_from_stdin("   Proceed with install? [Y/n]: ");
    if !confirm.trim().is_empty() && !matches!(confirm.trim(), "y" | "Y" | "yes" | "Yes" | "") {
        let _ = writeln!(standard_output, "   Install cancelled.");
        return 0;
    }
    let _ = writeln!(standard_output);

    let _ = writeln!(standard_output, "3. Running install...");
    let _ = writeln!(standard_output);

    let repository_root = match resolve_install_repository_root(flag_set.string_value("repo-root"))
    {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "Native Rust install failed: {error}");
            return 1;
        }
    };
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "Native Rust install failed: {error}");
            return 1;
        }
    };
    let purge_stale = install_purge_stale_enabled(
        flag_set.bool_value("no-purge"),
        flag_set.bool_value("purge-stale"),
    );
    match install_from_paths(
        build_version,
        &repository_root,
        &claude_home,
        &overrides,
        purge_stale,
    ) {
        Ok(summary) => {
            write_install_summary(&summary, standard_output);
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Native Rust install failed: {error}");
            return 1;
        }
    }

    let _ = writeln!(standard_output, "4. Verification: running keel status...");
    let _ = writeln!(standard_output);

    let status_args = vec![
        "--repo-root".to_string(),
        repository_root.to_string_lossy().to_string(),
        "--claude-home".to_string(),
        claude_home.to_string_lossy().to_string(),
    ];
    let status = run_status_command(build_version, &status_args, standard_output, standard_error);
    if status != 0 {
        return status;
    }

    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Interactive setup complete.");
    0
}

fn interactive_overrides(flag_set: &FlagSet, harness: &str) -> Result<InstallOverrides, String> {
    let mut overrides = parse_overrides(
        flag_set.string_value("with"),
        flag_set.string_value("without"),
    );
    if harness.eq_ignore_ascii_case("claude") {
        return Ok(overrides);
    }
    let platform = PlatformName::parse(harness).ok_or_else(|| {
        format!(
            "Unsupported harness {harness:?}. Choose claude, opencode, codex, pi, cursor, cowork, commandcode, or grok."
        )
    })?;
    overrides.skip.remove(&platform);
    overrides.force.insert(platform);
    Ok(overrides)
}

fn detect_harness_type() -> String {
    if std::env::var("OPENCODE").is_ok() || std::env::var("OPENCODE_VERSION").is_ok() {
        return "opencode".to_string();
    }
    if std::env::var("CODEX").is_ok() || std::env::var("CODEX_VERSION").is_ok() {
        return "codex".to_string();
    }
    "claude".to_string()
}

/// Read a line from stdin. Returns empty string on read failure.
fn read_line_from_stdin(prompt: &str) -> String {
    use std::io::BufRead;
    let _ = write!(std::io::stdout(), "{prompt}");
    let _ = std::io::stdout().flush();
    let stdin = std::io::stdin();
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(_) => line,
        Err(_) => String::new(),
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
    // Managed installed only — excludes learned-* so loop-generated skills
    // never force "refresh recommended" against the source pack size.
    let installed_skill_count = count_installed_skills(&claude_home);
    let learned_skill_count = count_learned_skills(&claude_home);
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
    // Content drift (not just count): an install that never re-ran after skill
    // edits can still show 52/52 while SKILL.md bodies are stale. Matcher and
    // Skill() both read the installed tree — stale means wrong guidance.
    let stale_skills: Vec<String> = layout
        .as_ref()
        .ok()
        .map(|value| stale_managed_skill_names(value, &claude_home))
        .unwrap_or_default();
    let update_status = match (source_skill_count, stale_skills.is_empty()) {
        (Some(expected_count), true) if installed_skill_count == expected_count => "current",
        (Some(_), false) => "refresh recommended (content drift)",
        (Some(_), true) => "refresh recommended",
        (None, _) if installed_skill_count == 0 => "not installed",
        (None, _) => "source unavailable",
    };
    let synced_skills = match source_skill_count {
        Some(expected_count) => format!("{installed_skill_count}/{expected_count}"),
        None => format!("{installed_skill_count}/unknown"),
    };
    let target = detect_current_target()
        .map(|value| value.directory_name())
        .unwrap_or_else(|error| format!("unknown ({error})"));
    let _ = writeln!(standard_output, "the harness Skill Pack Status");
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
    let _ = writeln!(standard_output, "the harness Skills:");
    let _ = writeln!(standard_output, "  Source: {}", source_display);
    let _ = writeln!(
        standard_output,
        "  Target: {}",
        display_path(&skills_directory(&claude_home))
    );
    let _ = writeln!(standard_output, "  Platform: {target}");
    let _ = writeln!(standard_output, "  Synced skills: {synced_skills}");
    let _ = writeln!(standard_output, "  Learned skills: {learned_skill_count}");
    if !stale_skills.is_empty() {
        let preview: String = stale_skills
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if stale_skills.len() > 12 {
            format!(" (+{} more)", stale_skills.len() - 12)
        } else {
            String::new()
        };
        let _ = writeln!(
            standard_output,
            "  Stale skills (content drift): {}{} — run `keel install` to refresh",
            preview, more
        );
    }
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Runtime:");
    let _ = writeln!(standard_output, "  implementation: rust");
    let _ = writeln!(standard_output);
    doctor::report_bridge_host_wiring(standard_output, &claude_home);
    0
}

#[cfg(test)]
mod interactive_tests {
    use super::*;

    fn install_flags(with: &str, without: &str) -> FlagSet {
        let mut flags = FlagSet::new("install");
        flags.string_flag("with", with);
        flags.string_flag("without", without);
        flags
    }

    #[test]
    fn interactive_host_selection_forces_the_selected_adapter() {
        let flags = install_flags("cursor", "grok");
        let overrides = interactive_overrides(&flags, "grok").unwrap();
        assert!(overrides.force.contains(&PlatformName::Cursor));
        assert!(overrides.force.contains(&PlatformName::Grok));
        assert!(!overrides.skip.contains(&PlatformName::Grok));
    }

    #[test]
    fn interactive_host_selection_rejects_unknown_names() {
        let flags = install_flags("", "");
        let error = interactive_overrides(&flags, "imaginary-host").unwrap_err();
        assert!(error.contains("Unsupported harness"));
    }
}
