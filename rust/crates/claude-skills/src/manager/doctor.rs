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
    let _ = writeln!(
        standard_output,
        "[warn] unified_exec interception incomplete in current Claude Code"
    );
    let _ = writeln!(
        standard_output,
        "hook hosts: {}",
        crate::hooks::supported_hosts().join(", ")
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
