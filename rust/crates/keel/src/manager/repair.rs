//! Purpose: One-shot recovery command — re-wire the harness lifecycle hooks and
//!   (re)register the `keel` MCP server, then report the model-compliance
//!   caveat the user cannot fix in code.
//! Caller: `commands.rs` `repair` arm (`keel repair`).
//! Dependencies: crate::runner::hook_lifecycle for the hooks payload builder,
//!   crate::manager::mcp_register for MCP registration, crate::runtime for paths.
//! Main Functions: run_repair_command.
//! Side Effects: Writes hooks into the resolved home's `settings.json` and the
//!   MCP entry into the resolved home's `.claude.json` (never outside it), and
//!   restores a missing `keel` executable from the repo's build artifact. Both
//!   writers preserve unrelated content. Prints a status report.
//!
//! Why this exists: install/update already wire hooks and MCP, but a partial or
//! interrupted install, a hand-edited settings.json, or an install that skipped
//! the bootstrap shell script can leave the surface half-wired. `repair` is the
//! single command that brings any install back to fully-automatic without a
//! full reinstall — the durable analog to "turn it off and on again".

use std::io::Write;
use std::path::Path;

use crate::manager::mcp_register::{mcp_config_path, register_mcp_server, McpRegistration};
use crate::runner::hook_lifecycle::build_hooks_payload;
use crate::runtime::{display_path, installed_executable_path, resolve_claude_home, write_text};

pub fn run_repair_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = crate::args::FlagSet::new("repair");
    flag_set.string_flag("claude-home", "");
    flag_set.string_flag("repo-root", "");
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

    let _ = writeln!(standard_output, "keel repair");
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
    let mcp_config = display_path(&mcp_config_path(&claude_home));
    match register_mcp_server(&claude_home) {
        Ok(McpRegistration::Added) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server registered in {mcp_config}"
            );
        }
        Ok(McpRegistration::Updated) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server entry updated in {mcp_config}"
            );
        }
        Ok(McpRegistration::AlreadyCurrent) => {
            let _ = writeln!(
                standard_output,
                "[ok] MCP server already registered in {mcp_config}"
            );
        }
        Err(error) => {
            had_error = true;
            let _ = writeln!(standard_error, "[fail] MCP registration: {error}");
        }
    }

    // 3. Resolve the repository root; it anchors both the executable restore
    //    below and the bridge-host wiring after it.
    let repo_root =
        match crate::runtime::resolve_repository_root(flag_set.string_value("repo-root")) {
            Ok(path) => path,
            Err(error) => {
                had_error = true;
                let _ = writeln!(
                standard_error,
                "[fail] resolve repo root: {error} (executable restore and bridge wiring skipped)"
            );
                return if had_error { 1 } else { 0 };
            }
        };

    // 4. Restore a deleted executable into the requested home. install/update
    //    publish it; repair must bring it back so hooks and MCP keep working.
    let executable = installed_executable_path(&claude_home);
    match crate::manager::install::restore_missing_executable(&repo_root, &claude_home) {
        Ok(true) => {
            let _ = writeln!(
                standard_output,
                "[ok] executable restored at {}",
                display_path(&executable)
            );
        }
        Ok(false) if executable.is_file() => {
            let _ = writeln!(
                standard_output,
                "[ok] executable already present at {}",
                display_path(&executable)
            );
        }
        Ok(false) => {
            had_error = true;
            let _ = writeln!(
                standard_error,
                "[fail] executable missing and no release artifact under repo root"
            );
        }
        Err(error) => {
            had_error = true;
            let _ = writeln!(standard_error, "[fail] executable: {error}");
        }
    }

    // 5. Re-wire the four bridge hosts (OpenCode, Pi, Codex, Cursor). Like the
    //    native MCP step, repair is an explicit operator action, so we force
    //    detected=true for each host the operator has installed — re-running its
    //    wiring idempotently. Cursor is never auto-detected, so we always force
    //    it (the maybe_wire call is a no-op if the source files are absent).
    for (name, status) in [
        (
            "opencode",
            crate::manager::install::maybe_wire_opencode(&repo_root, &claude_home, true),
        ),
        (
            "pi",
            crate::manager::install::maybe_wire_pi(&repo_root, &claude_home, true),
        ),
        (
            "codex",
            crate::manager::install::maybe_wire_codex(&repo_root, &claude_home, true),
        ),
        (
            "cursor",
            crate::manager::install::maybe_wire_cursor(&repo_root, &claude_home, true),
        ),
    ] {
        match status {
            Some(detail)
                if !detail.starts_with("skipped") && !detail.starts_with("source absent") =>
            {
                let _ = writeln!(standard_output, "[ok] {name}: {detail}");
            }
            Some(detail) => {
                let _ = writeln!(standard_output, "[--] {name}: {detail}");
            }
            None => {
                let _ = writeln!(
                    standard_output,
                    "[--] {name}: not a standard ~/.claude home"
                );
            }
        }
    }

    // 4. Status note. The skill brief is now inlined into per-prompt context,
    //    so a matched skill's guidance lands regardless of model compliance.
    let _ = writeln!(standard_output);
    let _ = writeln!(standard_output, "Next steps:");
    let _ = writeln!(
        standard_output,
        "  - Restart the harness so it reloads settings.json and {}.",
        display_path(&mcp_config_path(&claude_home))
    );
    let _ = writeln!(
        standard_output,
        "  - Run `keel doctor` to confirm the hook probe passes."
    );
    let _ = writeln!(standard_output);
    let _ = writeln!(
        standard_output,
        "Note: when a prompt distinctively matches a skill, that skill's guidance"
    );
    let _ = writeln!(
        standard_output,
        "is now inlined directly into the turn's context — it applies whether or"
    );
    let _ = writeln!(
        standard_output,
        "not the model chooses to make a Skill() call, so it no longer depends on"
    );
    let _ = writeln!(
        standard_output,
        "the gateway model honoring an injected tool-call instruction. The Skill()"
    );
    let _ = writeln!(
        standard_output,
        "call remains available to load the full skill body when the brief is not"
    );
    let _ = writeln!(
        standard_output,
        "enough. MCP tools are likewise offered to the model by the harness; this"
    );
    let _ = writeln!(
        standard_output,
        "command guarantees the wiring, and the inlined brief guarantees the guidance."
    );

    if had_error {
        1
    } else {
        0
    }
}

/// Rebuild and write the managed hook stanzas into `<claude_home>/settings.json`.
/// Returns the settings path on success. Reuses `build_hooks_payload`, the same
/// function `keel hook install` uses, so repair and install stay in
/// lockstep.
fn repair_hooks(claude_home: &Path) -> Result<std::path::PathBuf, String> {
    let settings_path = crate::runtime::claude_engagement_home(claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);
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
                "keel-repair-{label}-{}-{nanos}",
                std::process::id()
            ))
            .join(".claude");
        fs::create_dir_all(&claude_home).expect("create claude home");
        claude_home
    }

    /// Stage a minimal repo root with a release artifact so repair's
    /// executable-restore step has something to publish.
    fn stage_repo(root: &std::path::Path, artifact_bytes: &[u8]) {
        let release_dir = root.join("target").join("release");
        fs::create_dir_all(&release_dir).expect("create release dir");
        fs::write(
            release_dir.join(crate::runtime::executable_file_name()),
            artifact_bytes,
        )
        .expect("stage artifact");
    }

    #[test]
    fn repair_wires_hooks_registers_mcp_and_restores_executable() {
        let claude_home = unique_home("full");
        let repo_root = claude_home.parent().unwrap().join("repo");
        stage_repo(&repo_root, b"fake-keel");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let args = vec![
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--repo-root".to_string(),
            repo_root.to_string_lossy().to_string(),
        ];
        let code = run_repair_command(&args, &mut stdout, &mut stderr);
        assert_eq!(
            code,
            0,
            "repair failed: {}",
            String::from_utf8_lossy(&stderr)
        );

        // Hooks written.
        let settings = fs::read_to_string(
            crate::runtime::claude_engagement_home(&claude_home)
                .join(crate::hooks::claude::SETTINGS_FILE_NAME),
        )
        .expect("settings written");
        assert!(settings.contains("UserPromptSubmit"));
        assert!(settings.contains("SessionStart"));

        // MCP registered in the resolved home's .claude.json.
        let config =
            fs::read_to_string(mcp_config_path(&claude_home)).expect("claude.json written");
        assert!(config.contains("keel"));
        assert!(config.contains("mcp"));

        // Deleted executable restored into the requested home.
        let executable = installed_executable_path(&claude_home);
        assert_eq!(
            fs::read(&executable).expect("executable restored"),
            b"fake-keel"
        );

        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn repair_non_standard_home_keeps_mcp_config_inside() {
        // A home NOT named `.claude` (temp fixtures, `--claude-home` overrides)
        // must keep its `.claude.json` INSIDE the requested home: the old
        // parent-derived path wrote OUTSIDE it — straight into the real
        // `~/.claude.json` when the temp home sat directly under the real
        // user home.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("keel-repair-nonstd-{}-{nanos}", std::process::id()));
        let claude_home = root.join("home");
        fs::create_dir_all(&claude_home).expect("create home");
        let repo_root = root.join("repo");
        stage_repo(&repo_root, b"fake-keel");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let args = vec![
            "--claude-home".to_string(),
            claude_home.to_string_lossy().to_string(),
            "--repo-root".to_string(),
            repo_root.to_string_lossy().to_string(),
        ];
        let code = run_repair_command(&args, &mut stdout, &mut stderr);
        assert_eq!(
            code,
            0,
            "repair failed: {}",
            String::from_utf8_lossy(&stderr)
        );

        // Config lives inside the requested home, never beside it.
        assert!(
            claude_home.join(".claude.json").is_file(),
            "non-standard home must keep its .claude.json inside itself"
        );
        assert!(
            !root.join(".claude.json").exists(),
            "repair must never write outside the requested home"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
