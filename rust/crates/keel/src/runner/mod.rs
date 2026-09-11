//! Purpose: Rust-native command execution, rewrite, and hook-management surfaces.
//! Caller: commands.rs for `run`, `rewrite`, `hook`, `raw`, and `replay` command groups.
//! Dependencies: args, json, runtime helpers, proxy, shell_rewrite, hook_lifecycle submodules.
//! Main Functions: run_run_command, run_rewrite_command, run_hook_command, run_raw_command, run_replay_command.
//! Side Effects: Spawns requested child commands, writes raw-output recovery logs, and may write or remove the harness hook configuration.

pub mod bridge;
pub mod hook_lifecycle;
pub mod learning;
pub mod observation;
pub mod shell_rewrite;
pub mod telemetry;
pub mod tool_timings;

use std::cell::RefCell;
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

thread_local! {
    /// Scoped raw-store identity for in-process callers such as MCP. CLI
    /// invocations without an override retain their existing host-signal
    /// behavior, while a shared daemon can never accidentally read another
    /// session's artifact through the raw subcommand.
    static RAW_NAMESPACE_OVERRIDE: RefCell<Option<crate::proxy::raw_store::RawNamespace>> =
        const { RefCell::new(None) };
}

pub(crate) fn with_raw_namespace<F, T>(
    namespace: crate::proxy::raw_store::RawNamespace,
    work: F,
) -> T
where
    F: FnOnce() -> T,
{
    RAW_NAMESPACE_OVERRIDE.with(|active| {
        let previous = active.replace(Some(namespace));
        let result = work();
        active.replace(previous);
        result
    })
}

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
    let store = raw_recovery_store();
    // Verify metadata before exposing even a path, so `raw --path` cannot
    // advertise a tampered or incomplete artifact.
    let raw_dir = match store.load_meta(raw_id) {
        Ok(meta) => meta.raw_path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    if flag_set.bool_value("path") {
        let _ = writeln!(standard_output, "{}", display_path(&raw_dir));
        return 0;
    }
    // Read through RawStore::read_file: reading files directly after find_dir
    // bypassed integrity verification and treated missing files as empty.
    let command = match store.read_file(raw_id, "command.txt") {
        Ok(bytes) => String::from_utf8(bytes).map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    let command = match command {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(standard_error, "raw command: {error}");
            return 1;
        }
    };
    let stdout = match store.read_file(raw_id, "stdout.log") {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = writeln!(standard_error, "raw stdout: {error}");
            return 1;
        }
    };
    let stderr = match store.read_file(raw_id, "stderr.log") {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = writeln!(standard_error, "raw stderr: {error}");
            return 1;
        }
    };
    let _ = writeln!(standard_output, "raw_id: {raw_id}");
    let _ = writeln!(standard_output, "path: {}", display_path(&raw_dir));
    let screenshot_path = raw_dir.join("screenshot.png");
    if screenshot_path.is_file() {
        let _ = writeln!(
            standard_output,
            "screenshot: {}",
            display_path(&screenshot_path)
        );
    }
    let _ = writeln!(standard_output, "command: {}", command.trim());
    let _ = writeln!(standard_output, "\n[stdout]");
    let stdout_text = String::from_utf8_lossy(&stdout);
    let (neutralized_stdout, _) =
        crate::proxy::injection_guard::neutralize_injection(&stdout_text, raw_id);
    let _ = standard_output.write_all(neutralized_stdout.as_bytes());
    if !neutralized_stdout.ends_with('\n') {
        let _ = writeln!(standard_output);
    }
    let _ = writeln!(standard_output, "\n[stderr]");
    let stderr_text = String::from_utf8_lossy(&stderr);
    let (neutralized_stderr, _) =
        crate::proxy::injection_guard::neutralize_injection(&stderr_text, raw_id);
    let _ = standard_output.write_all(neutralized_stderr.as_bytes());
    if !neutralized_stderr.ends_with('\n') {
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
    let store = raw_recovery_store();
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
    let store = raw_recovery_store();
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
    let store = raw_recovery_store();
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

/// Build the recovery view for an operator or a harness-launched session.
/// Explicit CLI use outside a host session remains backward-compatible and
/// can inspect the local unscoped store. Once a host session signal is present,
/// bind every read/list/replay/prune operation to the same workspace/session
/// namespace used by `proxy::run`; a raw id alone must not cross that boundary.
fn raw_recovery_store() -> crate::proxy::raw_store::RawStore {
    let store = crate::proxy::raw_store::RawStore::new();
    let explicit_namespace = RAW_NAMESPACE_OVERRIDE.with(|active| active.borrow().clone());
    if let Some(namespace) = explicit_namespace {
        return crate::proxy::raw_store::RawStore::with_namespace(store.root().clone(), namespace);
    }
    let mcp_session = std::env::var("KEEL_MCP_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mcp_workspace = std::env::var("KEEL_MCP_WORKSPACE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let (Some(session_id), Some(workspace_id)) = (mcp_session, mcp_workspace) {
        return crate::proxy::raw_store::RawStore::with_namespace(
            store.root().clone(),
            crate::proxy::raw_store::RawNamespace {
                workspace_id,
                session_id,
            },
        );
    }
    if !crate::proxy::run::running_under_claude_code() {
        return store;
    }
    let workspace_id = match std::env::current_dir() {
        Ok(path) if !path.as_os_str().is_empty() => path.to_string_lossy().to_string(),
        _ => return store,
    };
    let session_id = ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "default".to_string());
    crate::proxy::raw_store::RawStore::with_namespace(
        store.root().clone(),
        crate::proxy::raw_store::RawNamespace {
            workspace_id,
            session_id,
        },
    )
}

fn parse_days(value: &str) -> Option<u64> {
    value.trim_end_matches('d').parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::raw_store::{RawRun, RawStore, RunMeta};
    use crate::test_support::ENV_LOCK;
    use std::path::PathBuf;

    #[test]
    fn raw_cli_rejects_tampered_artifacts_before_emitting_bytes() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        let signal_snapshot = crate::proxy::run::CLAUDE_CODE_SIGNAL_VARS
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect::<Vec<_>>();
        for name in crate::proxy::run::CLAUDE_CODE_SIGNAL_VARS {
            std::env::remove_var(name);
        }

        let root = crate::test_support::unique_temp_dir("keel-runner-raw-integrity");
        let _home_precedence = crate::test_support::HomePrecedenceGuard::clear_keel_home();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", root.join("home"));
        std::env::set_var("CLAUDE_SKILLS_HOOK", "test");
        std::env::set_var("CLAUDE_CODE_SESSION_ID", "runner-session-a");
        let raw_id = "20260512-runner-raw-integrity";
        let workspace_id = std::env::current_dir()
            .expect("current workspace")
            .to_string_lossy()
            .to_string();
        let mut meta = RunMeta {
            raw_id: raw_id.to_string(),
            command: "echo verified".to_string(),
            program: "echo".to_string(),
            args: vec!["verified".to_string()],
            cwd: PathBuf::from("."),
            started_at: 1,
            duration_ms: 1,
            exit_code: 0,
            adapter_name: "generic".to_string(),
            raw_path: PathBuf::new(),
            compact_path: PathBuf::new(),
            agent: "test".to_string(),
            workspace: PathBuf::from(&workspace_id),
            stdout_bytes: 8,
            stderr_bytes: 0,
            compact_stdout_bytes: 0,
            compact_stderr_bytes: 0,
            estimated_tokens_before: 2,
            estimated_tokens_after: 0,
            estimated_tokens_saved: 0,
            savings_pct: 0.0,
            compacted: false,
        };
        let base_store = RawStore::new();
        let store = RawStore::with_namespace(
            base_store.root().clone(),
            crate::proxy::raw_store::RawNamespace {
                workspace_id: workspace_id.clone(),
                session_id: "runner-session-a".to_string(),
            },
        );
        store
            .save(
                &mut meta,
                &RawRun {
                    stdout: b"verified\n".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save raw fixture");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            run_raw_command(&[raw_id.to_string()], &mut stdout, &mut stderr),
            0,
            "untampered recovery should succeed: {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(String::from_utf8_lossy(&stdout).contains("verified"));

        std::fs::write(meta.raw_path.join("stdout.log"), b"tampered\n")
            .expect("tamper stdout fixture");
        stdout.clear();
        stderr.clear();
        assert_eq!(
            run_raw_command(&[raw_id.to_string()], &mut stdout, &mut stderr),
            1,
            "tampered recovery must fail closed"
        );
        assert!(
            String::from_utf8_lossy(&stderr).contains("integrity mismatch"),
            "tampered recovery error should identify integrity failure: {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            stdout.is_empty(),
            "tampered recovery must not emit any raw bytes"
        );

        let mut foreign_meta = meta.clone();
        foreign_meta.raw_id = "20260512-runner-raw-foreign".to_string();
        let foreign_store = RawStore::with_namespace(
            base_store.root().clone(),
            crate::proxy::raw_store::RawNamespace {
                workspace_id,
                session_id: "runner-session-b".to_string(),
            },
        );
        foreign_store
            .save(
                &mut foreign_meta,
                &RawRun {
                    stdout: b"foreign\n".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
            )
            .expect("save foreign raw fixture");
        stdout.clear();
        stderr.clear();
        assert_eq!(
            run_raw_command(&[foreign_meta.raw_id.clone()], &mut stdout, &mut stderr),
            1,
            "a session must not recover another session's raw artifact"
        );
        assert!(
            String::from_utf8_lossy(&stderr).contains("namespace")
                || String::from_utf8_lossy(&stderr).contains("not found"),
            "cross-session recovery should fail with a scoped lookup error: {}",
            String::from_utf8_lossy(&stderr)
        );

        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        for (name, value) in signal_snapshot {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}
