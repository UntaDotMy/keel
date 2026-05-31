//! Purpose: One-shot recovery command — re-wire Claude Code lifecycle hooks and
//!   (re)register the `claude_core` MCP server, then report the model-compliance
//!   caveat the user cannot fix in code.
//! Caller: `commands.rs` `repair` arm (`claude-skills repair`).
//! Dependencies: crate::runner::hook_lifecycle for the hooks payload builder,
//!   crate::manager::mcp_register for MCP registration, crate::runtime for paths.
//! Main Functions: run_repair_command.
//! Side Effects: Writes `~/.claude/settings.json` (hooks) and `~/.claude.json`
//!   (MCP). Both writers preserve unrelated content. Prints a status report.
//!
//! Why this exists: install/update already wire hooks and MCP, but a partial or
//! interrupted install, a hand-edited settings.json, or an install that skipped
//! the bootstrap shell script can leave the surface half-wired. `repair` is the
//! single command that brings any install back to fully-automatic without a
//! full reinstall — the durable analog to "turn it off and on again".

use std::io::Write;
use std::path::Path;

use crate::manager::mcp_register::{register_mcp_server, McpRegistration};
use crate::runner::hook_lifecycle::build_hooks_payload;
use crate::runtime::{display_path, installed_executable_path, resolve_claude_home, write_text};

pub fn run_repair_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = crate::args::FlagSet::new("repair");
    flag_set.string_flag("claude-home", "");
    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }
    let claude_home = match resolve_claude_home(flag_set.string_value("claude-home")) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let _ = writeln!(standard_output, "claude-skills repair");
    let _ = writeln!(standard_output);

    let mut had_error = false;

    // 1. Re-wire lifecycle hooks into settings.json.
    match repair_hooks(&claude_home) {
        Ok(path) => {
            let _ = writeln!(
                standard_output,
                "[ok] hooks re-wired in {}",
                display_path(&path)
            );
        }
        Err(error) => {
            had_error = true;
            let _ = writeln!(standard_error, "[fail] hooks: {error}");
        }
    }

    // 2. Register the MCP server in ~/.claude.json (unconditional — repair is an
    //    explicit operator action, so no test-isolation guard here).
    match register_mcp_server(&claude_home) {
        Ok(McpRegistration::Added) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server registered in ~/.claude.json"
            );
        }
        Ok(McpRegistration::Updated) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server entry updated in ~/.claude.json"
            );
        }
        Ok(McpRegistration::AlreadyCurrent) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server already registered in ~/.claude.json"
            );
        }
        Err(error) => {
            had_error = true;
            let _ = writeln!(standard_error, "[fail] MCP registration: {error}");
        }
    }

    // 3. The caveat we cannot fix in code.
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Next steps:");
    let _ = writeln!(
        standard_output,
        "  - Restart Claude Code so it reloads settings.json and ~/.claude.json."
    );
    let _ = writeln!(
        standard_output,
        "  - Run `claude-skills doctor` to confirm the hook probe passes."
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Note: hooks and skills are advisory by design. They inject the iron law"
    );
    let _ = writeln!(
        standard_output,
        "and name the matching skill, but Claude Code's model must choose to act"
    );
    let _ = writeln!(
        standard_output,
        "on them — a hook cannot force a Skill() call. If skills still do not load"
    );
    let _ = writeln!(
        standard_output,
        "after a restart, the model behind your ANTHROPIC_BASE_URL may be ignoring"
    );
    let _ = writeln!(
        standard_output,
        "injected instructions; that is a model/gateway setting, not a wiring bug."
    );

    if had_error {
        1
    } else {
        0
    }
}

/// Rebuild and write the managed hook stanzas into `<claude_home>/settings.json`.
/// Returns the settings path on success. Reuses `build_hooks_payload`, the same
/// function `claude-skills hook install` uses, so repair and install stay in
/// lockstep.
fn repair_hooks(claude_home: &Path) -> Result<std::path::PathBuf, String> {
    let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let executable = installed_executable_path(claude_home);
    let payload = build_hooks_payload(&settings_path, &executable)?;
    write_text(&settings_path, &payload)?;
    Ok(settings_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn unique_home(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let claude_home = std::env::temp_dir()
            .join(format!(
                "claude-skills-repair-{label}-{}-{nanos}",
                std::process::id()
            ))
            .join(".claude");
        fs::create_dir_all(&claude_home).expect("create claude home");
        claude_home
    }

    #[test]
    fn repair_wires_hooks_and_registers_mcp() {
        let claude_home = unique_home("full");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let args = vec![
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
        ];
        let code = run_repair_command(&args, &mut stdout, &mut stderr);
        assert_eq!(
            code,
            0,
            "repair failed: {}",
            String::from_utf8_lossy(&stderr)
        );

        // Hooks written.
        let settings =
            fs::read_to_string(claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME))
                .expect("settings written");
        assert!(settings.contains("UserPromptSubmit"));
        assert!(settings.contains("SessionStart"));

        // MCP registered in the parent ~/.claude.json.
        let config = fs::read_to_string(super::super::mcp_register::mcp_config_path(&claude_home))
            .expect("claude.json written");
        assert!(config.contains("claude_core"));
        assert!(config.contains("mcp"));

        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }
}
