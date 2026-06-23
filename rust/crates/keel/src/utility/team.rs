//! Purpose: Tmux-based team worker management — spawn, status, and kill named
//!   tmux panes running `claude` with a given prompt.
//! Caller: commands.rs `team` dispatch arm via utility::run_team_command.
//! Dependencies: crate::args::FlagSet, crate::runtime::{run_command, resolve_claude_home, display_path, safe_path_segment},
//!   crate::utility::record_store::{Record, RecordStore, field}.
//! Main Functions: run_team_command (spawn|status|kill|list|help).
//! Side Effects: Creates tmux sessions via `tmux new-session`, lists via
//!   `tmux list-sessions`, kills via `tmux kill-session`. Persists team
//!   worker records under `<claude-home>/team/<workspace-slug>/`.

use std::io::Write;

use crate::args::FlagSet;
use crate::runtime::{
    display_path, resolve_claude_home, resolve_repository_root, run_command, safe_path_segment,
};
use crate::utility::record_store::{field, Record, RecordStore};

/// Tmux session prefix for keel team workers.
const TMUX_SESSION_PREFIX: &str = "keel-team-";

/// Worker states mirrored from dispatch.rs pattern.
const STATE_RUNNING: &str = "running";
const STATE_KILLED: &str = "killed";

/// CLI: `keel team <spawn|status|kill|list> [flags]`.
pub fn run_team_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let action = arguments.first().map(String::as_str).unwrap_or("");
    if action.is_empty() || matches!(action, "help" | "--help" | "-h") {
        render_help(standard_output);
        return if action.is_empty() { 1 } else { 0 };
    }
    match action {
        "spawn" => run_spawn(&arguments[1..], standard_output, standard_error),
        "status" | "list" => run_status(&arguments[1..], standard_output, standard_error),
        "kill" => run_kill(&arguments[1..], standard_output, standard_error),
        other => {
            let _ = writeln!(standard_error, "team: unknown subcommand: {other}");
            render_help(standard_error);
            1
        }
    }
}

fn render_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: keel team <spawn|status|kill|list> [flags]\n\
         \n\
         spawn     Spawn a named tmux pane running claude with a prompt.\n\
         \x20          --name <name> --prompt \"<prompt>\" [--claude-home <path>]\n\
         status    Show all active team panes (tmux + record state).\n\
         \x20          [--claude-home <path>] [--json]\n\
         kill      Kill a specific team pane by name.\n\
         \x20          --name <name> [--claude-home <path>] [--json]\n\
         list      Alias for status.\n\
         \n\
         Common flags: --claude-home <path>  --json"
    );
}

/// Lowercase alphanumeric slug of a workspace path. Same pattern as dispatch.rs.
fn workspace_slug(path: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in path.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

/// Resolve the RecordStore for team state. Keyed by workspace slug so two
/// projects never share a team board.
fn resolve_store(
    claude_home_flag: &str,
    standard_error: &mut dyn Write,
) -> Option<(RecordStore, String)> {
    let home = match resolve_claude_home(claude_home_flag) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "team: {error}");
            return None;
        }
    };
    let repo_root = match resolve_repository_root(".") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "team: {error}");
            return None;
        }
    };
    let slug = workspace_slug(&display_path(&repo_root));
    let store = RecordStore::new(&home, &format!("team/{slug}"));
    Some((store, slug))
}

/// Tmux session name for a team worker.
fn tmux_session_name(name: &str) -> String {
    format!("{TMUX_SESSION_PREFIX}{name}")
}

/// Validate that a team worker name is a safe path segment.
fn validate_name(name: &str, label: &str, standard_error: &mut dyn Write) -> Option<String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        let _ = writeln!(standard_error, "{label}: --name is required");
        return None;
    }
    if safe_path_segment(&trimmed).is_none() {
        let _ = writeln!(
            standard_error,
            "{label}: --name {trimmed:?} contains invalid characters (use alphanumeric, dash, dot)"
        );
        return None;
    }
    Some(trimmed)
}

/// Check whether a tmux session exists by name.
fn tmux_session_exists(session_name: &str) -> bool {
    let result = run_command(
        "tmux",
        &[
            "has-session".to_string(),
            "-t".to_string(),
            session_name.to_string(),
        ],
        None,
    );
    matches!(result, Ok(ref r) if r.code == 0)
}

/// Run `tmux list-sessions` and return the raw output.
fn tmux_list_sessions() -> Result<String, String> {
    match run_command("tmux", &["list-sessions".to_string()], None) {
        Ok(result) if result.code == 0 => Ok(String::from_utf8_lossy(&result.stdout).to_string()),
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
            if stderr.contains("no server running") || result.stdout.is_empty() {
                Ok(String::new())
            } else {
                Err(stderr)
            }
        }
        Err(error) => Err(error),
    }
}

/// Get the list of active tmux session names matching our prefix.
fn list_active_team_sessions() -> Vec<String> {
    let mut names = Vec::new();
    let Ok(output) = tmux_list_sessions() else {
        return names;
    };
    for line in output.lines() {
        // tmux format: "keel-team-foo: 1 windows (created ...)"
        if let Some(session_name) = line.split(':').next() {
            let trimmed = session_name.trim();
            if trimmed.starts_with(TMUX_SESSION_PREFIX) {
                names.push(trimmed.to_string());
            }
        }
    }
    names
}

/// Extract the team worker name from a tmux session name (strip prefix).
#[cfg(test)]
fn worker_name_from_session(session_name: &str) -> String {
    session_name
        .strip_prefix(TMUX_SESSION_PREFIX)
        .unwrap_or(session_name)
        .to_string()
}

/// Spawn: create a named tmux pane running `claude` with the given prompt.
fn run_spawn(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("team spawn");
    flags.string_flag("name", "");
    flags.string_flag("prompt", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "team spawn: {}", parse_error.message);
        return 1;
    }

    let name = match validate_name(flags.string_value("name"), "team spawn", standard_error) {
        Some(name) => name,
        None => return 1,
    };
    let prompt = flags.string_value("prompt").trim().to_string();
    if prompt.is_empty() {
        let _ = writeln!(
            standard_error,
            "team spawn: --prompt is required (the prompt to send to claude)"
        );
        return 1;
    }

    let Some((store, _slug)) = resolve_store(flags.string_value("claude-home"), standard_error)
    else {
        return 1;
    };

    let session_name = tmux_session_name(&name);

    // Check if the session already exists.
    if tmux_session_exists(&session_name) {
        let _ = writeln!(
            standard_error,
            "team spawn: tmux session {session_name} already exists (kill it first or use a different name)"
        );
        return 1;
    }

    // Check if we already have a running record for this name.
    if let Ok(Some(existing)) = store.read_record(&name) {
        let state = field(&existing, "state").unwrap_or("");
        if state == STATE_RUNNING {
            let _ = writeln!(
                standard_error,
                "team spawn: worker {name} already exists in state `{state}`"
            );
            return 1;
        }
    }

    // Create tmux session: detached, running claude with the prompt.
    // On Windows, tmux may not be available — this is a best-effort approach.
    let tmux_args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session_name.clone(),
        format!("claude --prompt {prompt:?}"),
    ];
    match run_command("tmux", &tmux_args, None) {
        Ok(result) if result.code == 0 => {}
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
            let _ = writeln!(
                standard_error,
                "team spawn: tmux new-session failed: {stderr}"
            );
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "team spawn: {error}");
            return 1;
        }
    }

    // Persist the worker record.
    let record: Record = vec![
        ("name".into(), name.clone()),
        ("prompt".into(), prompt.clone()),
        ("session".into(), session_name.clone()),
        ("state".into(), STATE_RUNNING.into()),
    ];
    if let Err(error) = store.write_record(&name, &record) {
        let _ = writeln!(standard_error, "team spawn: {error}");
        // Attempt to clean up the tmux session we just created.
        let _ = run_command(
            "tmux",
            &["kill-session".to_string(), "-t".to_string(), session_name],
            None,
        );
        return 1;
    }

    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "spawned": true,
            "name": name,
            "session": session_name,
            "state": STATE_RUNNING,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "team spawn: json: {error}");
                return 1;
            }
        }
        0
    } else {
        let _ = writeln!(
            standard_output,
            "team spawn: worker {name} running in tmux session {session_name}"
        );
        let _ = writeln!(standard_output, "  prompt: {prompt}");
        let _ = writeln!(standard_output, "  view: tmux attach -t {session_name}");
        let _ = writeln!(standard_output, "  kill: keel team kill --name {name}");
        0
    }
}

/// Status: show all active team panes.
fn run_status(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("team status");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "team status: {}", parse_error.message);
        return 1;
    }

    let Some((store, _slug)) = resolve_store(flags.string_value("claude-home"), standard_error)
    else {
        return 1;
    };

    let records = store.list_records().unwrap_or_default();
    let active_sessions = list_active_team_sessions();

    if flags.bool_value("json") {
        let items: Vec<serde_json::Value> = records
            .iter()
            .map(|(id, record)| {
                let session_name = field(record, "session").unwrap_or("");
                let tmux_active = tmux_session_exists(session_name);
                serde_json::json!({
                    "name": id,
                    "prompt": field(record, "prompt").unwrap_or(""),
                    "session": session_name,
                    "state": field(record, "state").unwrap_or(STATE_RUNNING),
                    "tmuxActive": tmux_active,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "total": records.len(),
            "tmuxSessions": active_sessions.len(),
            "workers": items,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "team status: json: {error}");
                return 1;
            }
        }
        return 0;
    }

    let _ = writeln!(
        standard_output,
        "team status: {} worker(s) tracked, {} tmux session(s) active",
        records.len(),
        active_sessions.len()
    );

    if records.is_empty() {
        let _ = writeln!(
            standard_output,
            "  no workers (spawn one with `keel team spawn --name <name> --prompt \"...\"`)"
        );
        return 0;
    }

    for (id, record) in &records {
        let state = field(record, "state").unwrap_or(STATE_RUNNING);
        let session_name = field(record, "session").unwrap_or("");
        let prompt = field(record, "prompt").unwrap_or("");
        let tmux_active = tmux_session_exists(session_name);
        let status_icon = if state == STATE_KILLED {
            "x"
        } else if tmux_active {
            "+"
        } else {
            "-"
        };
        let _ = writeln!(
            standard_output,
            "  [{status_icon}] {id} :: {state} :: session={session_name}"
        );
        if !prompt.is_empty() {
            let truncated = if prompt.len() > 60 {
                format!("{}...", &prompt[..57])
            } else {
                prompt.to_string()
            };
            let _ = writeln!(standard_output, "      prompt: {truncated}");
        }
    }
    0
}

/// Kill: terminate a specific team pane by name.
fn run_kill(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("team kill");
    flags.string_flag("name", "");
    flags.string_flag("claude-home", "");
    flags.bool_flag("json", false);
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "team kill: {}", parse_error.message);
        return 1;
    }

    let name = match validate_name(flags.string_value("name"), "team kill", standard_error) {
        Some(name) => name,
        None => return 1,
    };

    let Some((store, _slug)) = resolve_store(flags.string_value("claude-home"), standard_error)
    else {
        return 1;
    };

    let record = match store.read_record(&name) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let _ = writeln!(standard_error, "team kill: no worker with name {name}");
            return 1;
        }
        Err(error) => {
            let _ = writeln!(standard_error, "team kill: {error}");
            return 1;
        }
    };

    let session_name = field(&record, "session")
        .unwrap_or(&tmux_session_name(&name))
        .to_string();

    // Kill the tmux session if it exists.
    let tmux_killed = if tmux_session_exists(&session_name) {
        match run_command(
            "tmux",
            &[
                "kill-session".to_string(),
                "-t".to_string(),
                session_name.clone(),
            ],
            None,
        ) {
            Ok(result) => result.code == 0,
            Err(_) => false,
        }
    } else {
        false
    };

    // Update the record state to killed.
    let mut updated_record = record;
    if let Some(slot) = updated_record.iter_mut().find(|(k, _)| k == "state") {
        slot.1 = STATE_KILLED.to_string();
    }
    if let Err(error) = store.write_record(&name, &updated_record) {
        let _ = writeln!(standard_error, "team kill: {error}");
        return 1;
    }

    if flags.bool_value("json") {
        let payload = serde_json::json!({
            "killed": true,
            "name": name,
            "session": session_name,
            "tmuxKilled": tmux_killed,
            "state": STATE_KILLED,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                let _ = writeln!(standard_output, "{text}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "team kill: json: {error}");
                return 1;
            }
        }
        0
    } else {
        let status = if tmux_killed {
            "tmux session killed"
        } else {
            "tmux session not found (already stopped)"
        };
        let _ = writeln!(
            standard_output,
            "team kill: worker {name} killed ({status})"
        );
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_session_name_adds_prefix() {
        assert_eq!(tmux_session_name("foo"), "keel-team-foo");
        assert_eq!(tmux_session_name("my-worker"), "keel-team-my-worker");
    }

    #[test]
    fn worker_name_from_session_strips_prefix() {
        assert_eq!(worker_name_from_session("keel-team-foo"), "foo");
        assert_eq!(worker_name_from_session("keel-team-my-worker"), "my-worker");
        assert_eq!(worker_name_from_session("something-else"), "something-else");
    }

    #[test]
    fn workspace_slug_alphanumeric_to_lowercase() {
        assert_eq!(workspace_slug("/home/user/project"), "home-user-project");
        assert_eq!(
            workspace_slug("C:\\Users\\riezh\\test"),
            "c-users-riezh-test"
        );
    }

    #[test]
    fn render_help_does_not_panic() {
        let mut output = Vec::new();
        render_help(&mut output);
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("keel team"));
        assert!(text.contains("spawn"));
        assert!(text.contains("kill"));
    }

    #[test]
    fn run_team_command_empty_returns_1() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(&[], &mut stdout, &mut stderr);
        assert_eq!(code, 1);
    }

    #[test]
    fn run_team_command_help_returns_0() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(&["--help".to_string()], &mut stdout, &mut stderr);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&stdout).contains("keel team"));
    }

    #[test]
    fn run_team_command_unknown_returns_1() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(&["frobnicate".to_string()], &mut stdout, &mut stderr);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("unknown subcommand"));
    }

    #[test]
    fn run_spawn_missing_name_returns_1() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(
            &[
                "spawn".to_string(),
                "--prompt".to_string(),
                "hello".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("--name is required"));
    }

    #[test]
    fn run_spawn_missing_prompt_returns_1() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(
            &[
                "spawn".to_string(),
                "--name".to_string(),
                "test".to_string(),
            ],
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("--prompt is required"));
    }

    #[test]
    fn run_kill_missing_name_returns_1() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_team_command(&["kill".to_string()], &mut stdout, &mut stderr);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&stderr).contains("--name is required"));
    }
}
