//! Purpose: Doctor and hook probe logic for claude-skills manager.
//! Caller: commands.rs via run_doctor_command.
//! Dependencies: std::fs, std::io, std::path, std::process, crate::runtime, crate::hooks, crate::runner, crate::proxy.
//! Main Functions: run_doctor_command, hook_rewrites_raw_command, hook_accepts_wrapped_command, run_hook_probe, write_doctor_check, find_on_path.
//! Side Effects: Runs hook probe commands, writes doctor check output.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::runtime::{
    config_path, display_path, installed_executable_path, resolve_claude_home,
    COMMAND_COMPACTION_EVENTS_FILE_NAME,
};

use super::run_status_command;

pub fn run_doctor_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let status_code = run_status_command(build_version, arguments, standard_output, standard_error);
    if status_code != 0 {
        return status_code;
    }
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let _ = config_path(&claude_home);
    let hooks_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let hooks_text = fs::read_to_string(&hooks_path).unwrap_or_default();
    let claude_binary = find_on_path(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    let _ = writeln!(standard_output, "Doctor:");
    let _ = writeln!(
        standard_output,
        "[ok] binary: {}",
        display_path(&std::env::current_exe().unwrap_or_else(|_| PathBuf::from("claude-skills")))
    );
    let raw_store = crate::proxy::raw_store::RawStore::new();
    let raw_writable = fs::create_dir_all(raw_store.root())
        .and_then(|_| {
            let probe = raw_store.root().join(".doctor-write-probe");
            fs::write(&probe, b"ok").and_then(|_| fs::remove_file(probe))
        })
        .is_ok();
    write_doctor_check(
        standard_output,
        raw_writable,
        &format!("raw store writable: {}", display_path(raw_store.root())),
    );
    let event_path = claude_home.join(COMMAND_COMPACTION_EVENTS_FILE_NAME);
    let event_writable = fs::create_dir_all(&claude_home)
        .and_then(|_| {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&event_path)
        })
        .is_ok();
    write_doctor_check(
        standard_output,
        event_writable,
        &format!("event log writable: {}", display_path(&event_path)),
    );
    let _ = writeln!(
        standard_output,
        "[ok] adapters: {}",
        crate::proxy::adapters::adapter_names()
    );
    let rewrite_probe = crate::runner::rewrite_for_doctor("cargo test");
    write_doctor_check(
        standard_output,
        rewrite_probe.contains("run -- cargo test"),
        "rewrite: cargo test -> claude-skills run -- cargo test",
    );
    write_doctor_check(
        standard_output,
        claude_binary.is_some(),
        "claude binary found",
    );
    write_doctor_check(
        standard_output,
        hooks_path.exists(),
        "~/.claude/settings.json exists",
    );
    write_doctor_check(
        standard_output,
        hooks_text.contains("PreToolUse")
            && hooks_text.contains(crate::hooks::claude::pre_tool_matcher()),
        "PreToolUse Bash matcher installed",
    );
    let dry_run_rewrites = hook_rewrites_raw_command();
    write_doctor_check(
        standard_output,
        dry_run_rewrites,
        "raw command is transparently rewritten via PreToolUse",
    );
    write_doctor_check(
        standard_output,
        hook_accepts_wrapped_command() && installed_executable_path(&claude_home).exists(),
        "rerun wrapper command is accepted",
    );
    report_mcp_registration(standard_output, &claude_home);
    let _ = writeln!(
        standard_output,
        "[warn] unified_exec interception incomplete in current Claude Code"
    );
    let _ = writeln!(
        standard_output,
        "Run `claude-skills validate --profile smoke` for local proof."
    );
    0
}

/// Probe the PreToolUse hook with a noisy command and confirm it produces a
/// transparent rewrite payload. The current contract (Claude Code hook schema)
/// is `hookSpecificOutput.permissionDecision = "allow"` plus
/// `hookSpecificOutput.updatedInput.command = "claude-skills run -- ..."` — the
/// agent never sees a "Rerun that as:" string, so checking for that legacy text
/// would silently fail for everyone on the current contract. Asserting the
/// schema fields is what makes the doctor useful as a real health check.
fn hook_rewrites_raw_command() -> bool {
    run_hook_probe("cargo test --workspace")
        .map(|output| {
            output.contains("\"permissionDecision\"")
                && output.contains("\"allow\"")
                && output.contains("\"updatedInput\"")
                && output.contains("run -- ")
        })
        .unwrap_or(false)
}

/// Probe the PreToolUse hook with an already-wrapped command and confirm it
/// short-circuits — emitting empty stdout (no `hookSpecificOutput`) so Claude
/// Code runs the command unchanged. If the hook re-rewrote a wrapped command we
/// would loop on every turn.
fn hook_accepts_wrapped_command() -> bool {
    let executable = std::env::current_exe()
        .map(|path| display_path(&path))
        .unwrap_or_else(|_| "claude-skills".to_string());
    let command = format!("{executable} run -- cargo test --workspace");
    run_hook_probe(&command)
        .map(|output| !output.contains("permissionDecision"))
        .unwrap_or(false)
}

fn run_hook_probe(command: &str) -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    let mut child = Command::new(executable)
        .args(["hook", "pre-tool-use"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let input = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {
            "command": command
        }
    });
    if let Some(mut stdin) = child.stdin.take() {
        let _ = write!(stdin, "{}", input);
    }
    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

fn write_doctor_check(standard_output: &mut dyn Write, ok: bool, message: &str) {
    let status = if ok { "[ok]" } else { "[warn]" };
    let _ = writeln!(standard_output, "{status} {message}");
}

/// Report the health of the `claude_core` MCP registration in `~/.claude.json`.
///
/// Two failure modes matter and look identical from inside a session — the four
/// tools (`recall`, `system_map`, `run_command`, `recall_status`) appear absent:
///
/// 1. **No entry at all** — the server was never registered, so the tools do not
///    exist for Claude Code.
/// 2. **Entry present but `alwaysLoad` missing/false** — the tools ARE registered
///    but Claude Code *defers* them behind `ToolSearch` (forced on whenever tool
///    search is enabled or `ANTHROPIC_BASE_URL` points at a non-first-party
///    gateway). A model that searches for them by bare name (`select:recall`)
///    finds nothing and wrongly concludes "MCP not registered". `alwaysLoad: true`
///    pins them into context so they are always available. See
///    `mcp_register::mcp_server_entry` for the authoritative rationale.
///
/// Both are repaired by `claude-skills repair` (re-runs `register_mcp_server`,
/// which writes the entry *with* `alwaysLoad: true`). Doctor only reports — it
/// never mutates `~/.claude.json` here, since a doctor run should be read-only.
fn report_mcp_registration(standard_output: &mut dyn Write, claude_home: &std::path::Path) {
    let config_path = super::mcp_register::mcp_config_path(claude_home);
    let text = fs::read_to_string(&config_path).unwrap_or_default();
    let parsed: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let entry = parsed
        .as_ref()
        .and_then(|doc| doc.get("mcpServers"))
        .and_then(|servers| servers.get(super::mcp_register::MCP_SERVER_KEY));

    match entry {
        None => {
            write_doctor_check(
                standard_output,
                false,
                "claude_core MCP server registered in ~/.claude.json \
                 (run `claude-skills repair` to register it)",
            );
        }
        Some(entry) => {
            write_doctor_check(standard_output, true, "claude_core MCP server registered");
            let always_load = entry
                .get("alwaysLoad")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            write_doctor_check(
                standard_output,
                always_load,
                if always_load {
                    "claude_core MCP tools pinned into context (alwaysLoad)"
                } else {
                    "claude_core MCP tools pinned into context (alwaysLoad missing — \
                     tools are deferred behind ToolSearch; run `claude-skills repair`)"
                },
            );
        }
    }
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path_value = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path_value) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) && !executable.ends_with(".exe") {
            let candidate = directory.join(format!("{executable}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_home(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        // claude_home is <root>/.claude so the parent is the synthetic user home
        // where .claude.json lives (matches mcp_register::mcp_config_path).
        let claude_home = std::env::temp_dir()
            .join(format!(
                "claude-skills-doctor-{label}-{}-{nanos}",
                std::process::id()
            ))
            .join(".claude");
        fs::create_dir_all(&claude_home).expect("create claude home");
        claude_home
    }

    fn run_report(claude_home: &std::path::Path) -> String {
        let mut out = Vec::new();
        report_mcp_registration(&mut out, claude_home);
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn reports_ok_when_entry_has_always_load() {
        let claude_home = unique_home("ok");
        let config = super::super::mcp_register::mcp_config_path(&claude_home);
        // The exact shape register_mcp_server writes.
        fs::write(
            &config,
            r#"{"mcpServers":{"claude_core":{"type":"stdio","command":"x","args":["mcp","serve"],"env":{},"alwaysLoad":true}}}"#,
        )
        .unwrap();
        let report = run_report(&claude_home);
        assert!(
            report.contains("[ok] claude_core MCP server registered"),
            "{report}"
        );
        assert!(
            report.contains("[ok] claude_core MCP tools pinned into context"),
            "{report}"
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn warns_when_always_load_missing() {
        // The exact bug a stale install / `claude mcp add` leaves behind:
        // entry present, alwaysLoad absent -> tools deferred behind ToolSearch.
        let claude_home = unique_home("noalwaysload");
        let config = super::super::mcp_register::mcp_config_path(&claude_home);
        fs::write(
            &config,
            r#"{"mcpServers":{"claude_core":{"type":"stdio","command":"x","args":["mcp","serve"],"env":{}}}}"#,
        )
        .unwrap();
        let report = run_report(&claude_home);
        // Server is registered...
        assert!(
            report.contains("[ok] claude_core MCP server registered"),
            "{report}"
        );
        // ...but the alwaysLoad line must WARN and point at repair.
        assert!(
            report.contains("[warn] claude_core MCP tools pinned into context"),
            "{report}"
        );
        assert!(report.contains("claude-skills repair"), "{report}");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn warns_when_no_entry() {
        let claude_home = unique_home("noentry");
        let config = super::super::mcp_register::mcp_config_path(&claude_home);
        fs::write(&config, r#"{"mcpServers":{}}"#).unwrap();
        let report = run_report(&claude_home);
        assert!(
            report.contains("[warn] claude_core MCP server registered"),
            "{report}"
        );
        assert!(report.contains("claude-skills repair"), "{report}");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn warns_when_config_absent() {
        // No ~/.claude.json at all -> treated as "no entry", warns.
        let claude_home = unique_home("noconfig");
        let report = run_report(&claude_home);
        assert!(
            report.contains("[warn] claude_core MCP server registered"),
            "{report}"
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }
}
