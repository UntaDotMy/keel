//! Purpose: Host-neutral plain-text bridge CLI surface that reuses the existing
//!   lifecycle handlers directly, bypassing the harness hook JSON envelope.
//! Caller: `commands.rs` via the top-level `bridge` subcommand.
//! Dependencies: crate::runner::hook_lifecycle, crate::utility::skill_match,
//!   crate::runner::observation, crate::runtime.
//! Main Functions: run_bridge_command dispatching subcommands (session-start,
//!   user-prompt, observe, session-end, pre-compact, post-compact, gate-status,
//!   pre-tool-use, rewrite). pre-tool-use emits the Iron Law edit-gate decision
//!   (KEEL_GATE_ALLOW / KEEL_GATE_DENY) as text; rewrite reroutes shell commands
//!   through compaction (KEEL_REWRITE) by reading the command on stdin. Both
//!   exit 0 — the host adapter enforces the deny, never the bridge itself.
//! Side Effects: Prints plain text to stdout; observe writes observation files.

use std::io::{Read, Write};

use crate::args::FlagSet;
use crate::runner::{hook_lifecycle, observation, shell_rewrite};
use crate::runtime::resolve_claude_home;

pub fn run_bridge_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        render_bridge_help(standard_output);
        return 0;
    }
    match arguments[0].as_str() {
        "session-start" => {
            run_bridge_session_start(&arguments[1..], standard_output, standard_error)
        }
        "user-prompt" => run_bridge_user_prompt(&arguments[1..], standard_output, standard_error),
        "observe" => run_bridge_observe(&arguments[1..], standard_output, standard_error),
        "session-end" => run_bridge_session_end(&arguments[1..], standard_output, standard_error),
        "pre-compact" => run_bridge_pre_compact(&arguments[1..], standard_output, standard_error),
        "post-compact" => run_bridge_post_compact(&arguments[1..], standard_output, standard_error),
        "gate-status" => run_bridge_gate_status(&arguments[1..], standard_output, standard_error),
        "pre-tool-use" => run_bridge_pre_tool_use(&arguments[1..], standard_output, standard_error),
        "rewrite" => run_bridge_rewrite(&arguments[1..], standard_output, standard_error),
        _ => {
            let _ = writeln!(
                standard_error,
                "Unknown bridge subcommand: {}",
                arguments[0]
            );
            render_bridge_help(standard_output);
            1
        }
    }
}

fn is_help_argument(value: &str) -> bool {
    matches!(value, "help" | "--help" | "-h")
}

fn render_bridge_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: keel bridge <subcommand> [flags]\n\
         Subcommands:\n\
         \x20 session-start --session <id> --cwd <path>\n\
         \x20 user-prompt   --session <id> --cwd <path> --prompt <text>\n\
         \x20 observe       --session <id> --cwd <path> --tool <name> [--failed]\n\
         \x20 session-end   --session <id> --cwd <path>\n\
         \x20 pre-compact   --session <id> --cwd <path>\n\
         \x20 post-compact  --session <id> --cwd <path>\n\
         \x20 gate-status   --session <id> --cwd <path>\n\
         \x20 pre-tool-use  --session <id> --cwd <path> --tool <name> [--command <text>] [--path <file>]\n\
         \x20 rewrite       --tool <name>  (command on stdin)"
    );
}

fn bridge_flag_set(name: &str) -> FlagSet {
    let mut flags = FlagSet::new(name);
    flags.string_flag("session", "");
    flags.string_flag("cwd", "");
    flags.string_flag("format", "text");
    flags
}

/// Run `body` with the process working directory temporarily set to the
/// host-passed `--cwd`, restoring the previous cwd afterward.
///
/// The context builders (`session_start_context`, `user_prompt_submit_context`,
/// `post_compact_context`) resolve the workspace — system map, working brief,
/// memory scope, instinct lane — from the process cwd. A host that spawns the
/// bridge from a different directory than the user's workspace (e.g. an editor
/// plugin launching keel from the plugin dir) would otherwise get the wrong
/// workspace's digest. Scoping the cwd to `--cwd` for the duration of the build
/// makes those subcommands honor it without changing the shared lifecycle code.
///
/// Backward compatible: an empty `--cwd`, or one that is not a usable directory,
/// leaves the process cwd unchanged (the previous behavior). The bridge runs one
/// short-lived subcommand per process, so the temporary switch is safe.
fn with_workspace_cwd<F: FnOnce() -> String>(cwd: &str, body: F) -> String {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return body();
    }
    let previous = std::env::current_dir().ok();
    let switched = std::env::set_current_dir(trimmed).is_ok();
    let result = body();
    if switched {
        if let Some(previous) = previous {
            let _ = std::env::set_current_dir(previous);
        }
    }
    result
}

fn resolve_bridge_args(flag_set: &FlagSet, standard_error: &mut dyn Write) -> (String, String) {
    let session = flag_set.string_value("session").trim().to_string();
    let cwd = flag_set.string_value("cwd").trim().to_string();
    if session.is_empty() || cwd.is_empty() {
        let _ = writeln!(
            standard_error,
            "bridge: --session and --cwd are required; continuing with defaults"
        );
    }
    (session, cwd)
}

fn run_bridge_session_start(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge session-start");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let context = with_workspace_cwd(
        flags.string_value("cwd"),
        hook_lifecycle::session_start_context,
    );
    let _ = writeln!(standard_output, "{context}");
    0
}

fn run_bridge_user_prompt(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge user-prompt");
    flags.string_flag("prompt", "");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let prompt = flags.string_value("prompt");
    let context = with_workspace_cwd(flags.string_value("cwd"), || {
        hook_lifecycle::user_prompt_submit_context(prompt)
    });
    let _ = writeln!(standard_output, "{context}");
    0
}

fn run_bridge_observe(
    arguments: &[String],
    _standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge observe");
    flags.string_flag("tool", "");
    flags.bool_flag("failed", false);
    flags.string_flag("phase", "post");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let (session, cwd) = resolve_bridge_args(&flags, standard_error);

    let tool_name = flags.string_value("tool");
    let failed = flags.bool_value("failed");
    let phase = flags.string_value("phase");

    // Read tool input JSON from stdin.
    let mut tool_input_json = String::new();
    let _ = std::io::stdin().read_to_string(&mut tool_input_json);

    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge observe: resolve_claude_home failed: {error}"
            );
            return 1;
        }
    };

    // Iron Law evidence is written only from successful post-tool observations.
    if phase == "post" && !failed {
        let command = extract_command_from_observe_payload(&tool_input_json);
        let session_id = if session.is_empty() {
            "default"
        } else {
            session.as_str()
        };
        let input: serde_json::Value =
            serde_json::from_str(&tool_input_json).unwrap_or(serde_json::Value::Null);
        let nested = input.get("tool_input").or_else(|| input.get("toolInput"));
        let path = input
            .get("path")
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("filePath"))
            .or_else(|| nested.and_then(|value| value.get("path")))
            .or_else(|| nested.and_then(|value| value.get("file_path")))
            .or_else(|| nested.and_then(|value| value.get("filePath")))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let effective_tool = hook_lifecycle::effective_tool_name(tool_name, path);
        hook_lifecycle::maybe_mark_iron_law_from_parts(
            session_id,
            effective_tool,
            command.as_deref(),
        );
    }

    match observation::record_observation_from_parts(
        &claude_home,
        tool_name,
        &tool_input_json,
        &cwd,
        &session,
        failed,
    ) {
        Ok(_) => 0,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge observe: observation record failed: {error}"
            );
            1
        }
    }
}

/// Best-effort extract of a shell `command` field from bridge observe stdin.
fn extract_command_from_observe_payload(payload: &str) -> Option<String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Full tool_input JSON object.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(cmd) = value.get("command").and_then(|v| v.as_str()) {
            return Some(cmd.to_string());
        }
        if let Some(cmd) = value
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|v| v.as_str())
        {
            return Some(cmd.to_string());
        }
        return None;
    }
    // Raw command string on stdin (some adapters send just the command).
    Some(trimmed.to_string())
}

fn run_bridge_session_end(
    arguments: &[String],
    _standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge session-end");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let (session, _cwd) = resolve_bridge_args(&flags, standard_error);

    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge session-end: resolve_claude_home failed: {error}"
            );
            return 1;
        }
    };

    hook_lifecycle::run_bridge_session_end(&claude_home, &session, standard_error);
    0
}

fn run_bridge_pre_compact(
    arguments: &[String],
    _standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge pre-compact");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    // Honor --cwd so the workspace learning checkpoint runs against the host's
    // workspace, not the bridge process cwd (same contract as post-compact).
    let cwd = flags.string_value("cwd").trim().to_string();
    if !cwd.is_empty() {
        let _ = std::env::set_current_dir(&cwd);
    }
    // Pre-compact persists what was learned before the window rewrite. It
    // emits no context; the post-compact memory digest belongs on post-compact.
    hook_lifecycle::run_session_end_learning(standard_error);
    0
}

fn run_bridge_post_compact(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge post-compact");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let context = with_workspace_cwd(
        flags.string_value("cwd"),
        hook_lifecycle::post_compact_context,
    );
    let _ = writeln!(standard_output, "{context}");
    hook_lifecycle::run_session_end_learning(standard_error);
    0
}

fn run_bridge_gate_status(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge gate-status");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let (session, _cwd) = resolve_bridge_args(&flags, standard_error);
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge gate-status: resolve_claude_home failed: {error}"
            );
            return 1;
        }
    };

    let session_key = if session.trim().is_empty() {
        "no-session".to_string()
    } else {
        hook_lifecycle::sanitize_memory_key(&session)
    };
    let _ = writeln!(standard_output, "gate status for session {session_key}:");

    for row in hook_lifecycle::gate_status_rows() {
        let counter_path = claude_home.join("state").join(row.dir).join(&session_key);
        let count = if counter_path.exists() {
            std::fs::read_to_string(&counter_path)
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
                .unwrap_or(0)
        } else {
            0
        };
        let cleared = match count {
            0 => "not fired",
            n if n >= row.max_blocks => "capped",
            _ => "fired",
        };
        let _ = writeln!(
            standard_output,
            "  {}: {} (count: {})",
            row.label, cleared, count
        );
    }
    0
}

fn run_bridge_pre_tool_use(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge pre-tool-use");
    flags.string_flag("tool", "");
    flags.string_flag("command", "");
    flags.string_flag("path", "");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let (session, cwd) = resolve_bridge_args(&flags, standard_error);
    let tool_name =
        hook_lifecycle::effective_tool_name(flags.string_value("tool"), flags.string_value("path"));
    let command_flag = flags.string_value("command");
    // why: only a *shell* tool's gate decision reads the command, and an
    // inherited open stdin made this block until the adapter timed out.
    let mut stdin_command = String::new();
    if command_flag.trim().is_empty() && shell_rewrite::is_shell_tool_name(tool_name) {
        let _ = std::io::stdin().read_to_string(&mut stdin_command);
    }
    let command = if !command_flag.trim().is_empty() {
        Some(command_flag.trim())
    } else if !stdin_command.trim().is_empty() {
        Some(stdin_command.trim())
    } else {
        None
    };

    if !hook_lifecycle::tool_is_iron_law_gated(tool_name, command) {
        let _ = writeln!(standard_output, "KEEL_GATE_ALLOW");
        return 0;
    }

    if session.trim().is_empty() {
        let _ = writeln!(
            standard_output,
            "KEEL_GATE_DENY\nkeel Iron Law gate: missing session identifier; research must be recorded under a valid session"
        );
        return 0;
    }
    let session_id = session.trim();
    match hook_lifecycle::pre_tool_gate_decision(session_id, tool_name, command, &cwd) {
        Some(reason) => {
            let _ = writeln!(standard_output, "KEEL_GATE_DENY\n{reason}");
        }
        None => {
            let _ = writeln!(standard_output, "KEEL_GATE_ALLOW");
        }
    }
    0
}

fn run_bridge_rewrite(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge rewrite");
    flags.string_flag("tool", "");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 2;
    }
    let tool_name = flags.string_value("tool");
    let lower = tool_name.to_ascii_lowercase();
    if !shell_rewrite::is_shell_tool_name(&lower) {
        return 0;
    }
    let mut command = String::new();
    let _ = std::io::stdin().read_to_string(&mut command);
    let command = command.trim();
    if command.is_empty() {
        return 0;
    }
    // why: the host substitutes this verbatim, and powershell/pwsh/cmd reject a
    // Bash-shaped prefix.
    let decision = shell_rewrite::rewrite_command_text_for_shell(
        command,
        shell_rewrite::rewrite_shell_for_tool(&lower),
    );
    if decision.supported {
        let _ = writeln!(
            standard_output,
            "KEEL_REWRITE {}",
            decision.rewritten_command
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    /// H2 regression: `keel bridge gate-status` must report all 8 PostToolBatch
    /// gates (previously hardcoded only 3: review/brief/story-closeout) and must
    /// not crash on an empty session id. Also confirms the "not fired" state for
    /// a fresh session with no counter files.
    #[test]
    fn bridge_gate_status_reports_remaining_gates() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let temp = std::env::temp_dir().join(format!(
            "keel-bridge-gate-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&temp).expect("create test claude home");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &temp);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_bridge_command(
            &[
                "gate-status".to_string(),
                "--session".to_string(),
                "s1".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );

        let out = String::from_utf8_lossy(&stdout);
        assert_eq!(code, 0, "gate-status must exit 0: {stderr:?}");
        for label in [
            "review",
            "working-brief",
            "memory",
            "learned-skill",
            "research",
        ] {
            assert!(
                out.contains(&format!("{label}:")),
                "gate-status must report the {label} gate; got: {out}"
            );
        }
        // A fresh session with no counter files must show "not fired" for each.
        assert!(
            out.contains("not fired"),
            "fresh session must show not fired: {out}"
        );

        // Restore.
        match previous_home {
            Some(v) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", v),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// #35 regression: the context subcommands (`session-start`, `user-prompt`,
    /// `post-compact`) must honor a host-passed `--cwd` so a bridge spawned from a
    /// different working directory than the user's workspace still resolves the
    /// correct workspace. Previously they ignored `--cwd` and always resolved from
    /// the bridge process's own cwd. We prove the fix by passing a workspace path
    /// carrying a distinctive marker and asserting its canonical hashed lane
    /// appears in the workspace-scope summary,
    /// then that omitting `--cwd` falls back to the process cwd (no marker).
    #[test]
    fn bridge_context_subcommands_honor_cwd() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let temp = std::env::temp_dir().join(format!("keel-bridge-cwd-home-{nanos}"));
        std::fs::create_dir_all(&temp).expect("create test claude home");
        // Distinctive workspace dir; the basename is all lowercase-alnum so it
        // survives sanitize_key unchanged and is unmistakable in the output.
        let marker = format!("keelcwdmarker{}{nanos}", std::process::id());
        let workspace = std::env::temp_dir().join(&marker);
        std::fs::create_dir_all(&workspace).expect("create test workspace");
        let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
        let expected_lane = crate::utility::system_map::workspace_key(
            &crate::runtime::display_path(&canonical_workspace),
        );

        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &temp);

        // With --cwd: the workspace-scope summary must reference the passed dir.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_bridge_command(
            &[
                "session-start".to_string(),
                "--session".to_string(),
                "s1".to_string(),
                "--cwd".to_string(),
                workspace.to_string_lossy().to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0, "session-start must exit 0: {stderr:?}");
        let out_with_cwd = String::from_utf8_lossy(&stdout);
        assert!(
            out_with_cwd.contains(&expected_lane),
            "session-start must resolve the passed --cwd workspace (lane {expected_lane}); got: {out_with_cwd}"
        );

        // Without --cwd: falls back to the process cwd (the crate dir), so the
        // marker must NOT appear — proving --cwd is actually consulted.
        let mut stdout_no_cwd = Vec::new();
        let mut stderr_no_cwd = Vec::new();
        let _ = run_bridge_command(
            &[
                "session-start".to_string(),
                "--session".to_string(),
                "s1".to_string(),
            ],
            &mut stdout_no_cwd,
            &mut stderr_no_cwd,
        );
        let out_no_cwd = String::from_utf8_lossy(&stdout_no_cwd);
        assert!(
            !out_no_cwd.contains(&expected_lane),
            "without --cwd the passed workspace lane must not appear (used process cwd): {out_no_cwd}"
        );

        match previous_home {
            Some(v) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", v),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = std::fs::remove_dir_all(&temp);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// H2: an empty session id must not panic and must use the no-session key
    /// (matching the gate writers' default path).
    #[test]
    fn bridge_gate_status_handles_empty_session() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let temp = std::env::temp_dir().join(format!(
            "keel-bridge-gate-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&temp).expect("create test claude home");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &temp);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_bridge_command(&["gate-status".to_string()], &mut stdout, &mut stderr);
        let out = String::from_utf8_lossy(&stdout);
        assert_eq!(code, 0, "empty-session gate-status must exit 0: {stderr:?}");
        assert!(
            out.contains("no-session"),
            "empty session must use the no-session key: {out}"
        );

        match previous_home {
            Some(v) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", v),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn malformed_bridge_commands_fail_nonzero_without_running_handlers() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let unknown = run_bridge_command(&["not-a-command".to_string()], &mut stdout, &mut stderr);
        assert_ne!(unknown, 0, "unknown bridge commands must fail closed");

        stdout.clear();
        stderr.clear();
        let malformed = run_bridge_command(
            &[
                "session-end".to_string(),
                "--not-a-flag".to_string(),
                "value".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_ne!(malformed, 0, "parse errors must not run lifecycle effects");
    }
    #[test]
    fn bridge_protocol_contract_test() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        // 1. `bridge rewrite` requires `--tool`; unrecognized flag `--command` returns error 2
        let code = run_bridge_command(
            &[
                "rewrite".to_string(),
                "--command".to_string(),
                "foo".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(
            code, 2,
            "bridge rewrite must reject unrecognized --command flag"
        );

        // 2. `bridge pre-tool-use` on ungated tool allows
        stdout.clear();
        stderr.clear();
        let code = run_bridge_command(
            &[
                "pre-tool-use".to_string(),
                "--tool".to_string(),
                "Read".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&stdout).trim(), "KEEL_GATE_ALLOW");

        // 3. `bridge pre-tool-use` on gated tool with empty session denies
        stdout.clear();
        stderr.clear();
        let code = run_bridge_command(
            &[
                "pre-tool-use".to_string(),
                "--tool".to_string(),
                "Edit".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 0);
        let out = String::from_utf8_lossy(&stdout);
        assert!(
            out.starts_with("KEEL_GATE_DENY"),
            "id-less session must deny: {out}"
        );
    }
}
