//! Purpose: Host-neutral plain-text bridge CLI surface that reuses the existing
//!   lifecycle handlers directly, bypassing the Claude Code hook JSON envelope.
//! Caller: `commands.rs` via the top-level `bridge` subcommand.
//! Dependencies: crate::runner::hook_lifecycle, crate::utility::skill_match,
//!   crate::runner::observation, crate::runtime.
//! Main Functions: run_bridge_command dispatching subcommands (session-start,
//!   user-prompt, observe, session-end, post-compact, gate-status).
//! Side Effects: Prints plain text to stdout; observe writes observation files.

use std::io::{Read, Write};

use crate::args::FlagSet;
use crate::runner::{hook_lifecycle, observation};
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
        "post-compact" => run_bridge_post_compact(&arguments[1..], standard_output, standard_error),
        "gate-status" => run_bridge_gate_status(&arguments[1..], standard_output, standard_error),
        _ => {
            let _ = writeln!(
                standard_error,
                "Unknown bridge subcommand: {}",
                arguments[0]
            );
            render_bridge_help(standard_output);
            0
        }
    }
}

fn is_help_argument(value: &str) -> bool {
    matches!(value, "help" | "--help" | "-h")
}

fn render_bridge_help(standard_output: &mut dyn Write) {
    let _ = writeln!(
        standard_output,
        "Usage: claude-skills bridge <subcommand> [flags]\n\
         Subcommands:\n\
         \x20 session-start --session <id> --cwd <path>\n\
         \x20 user-prompt   --session <id> --cwd <path> --prompt <text>\n\
         \x20 observe       --session <id> --cwd <path> --tool <name> [--failed]\n\
         \x20 session-end   --session <id> --cwd <path>\n\
         \x20 post-compact  --session <id> --cwd <path>\n\
         \x20 gate-status   --session <id> --cwd <path>"
    );
}

fn bridge_flag_set(name: &str) -> FlagSet {
    let mut flags = FlagSet::new(name);
    flags.string_flag("session", "");
    flags.string_flag("cwd", "");
    flags.string_flag("format", "text");
    flags
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
    }
    let context = hook_lifecycle::session_start_context();
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
    }
    let prompt = flags.string_value("prompt");
    let context = hook_lifecycle::user_prompt_submit_context(prompt);
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
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
    }
    let (session, cwd) = resolve_bridge_args(&flags, standard_error);

    let tool_name = flags.string_value("tool");
    let failed = flags.bool_value("failed");

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
            return 0;
        }
    };

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
            0
        }
    }
}

fn run_bridge_session_end(
    arguments: &[String],
    _standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = bridge_flag_set("bridge session-end");
    if let Err(parse_error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
    }
    let (session, _cwd) = resolve_bridge_args(&flags, standard_error);

    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge session-end: resolve_claude_home failed: {error}"
            );
            return 0;
        }
    };

    hook_lifecycle::run_bridge_session_end(&claude_home, &session, standard_error);
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
    }
    let context = hook_lifecycle::post_compact_context();
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
    }
    let (session, _cwd) = resolve_bridge_args(&flags, standard_error);
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "bridge gate-status: resolve_claude_home failed: {error}"
            );
            return 0;
        }
    };

    let gates: &[(&str, &str)] = &[
        ("review-gate-blocks", "review"),
        ("brief-gate-blocks", "working-brief"),
        ("story-closeout-gate-blocks", "story-closeout"),
    ];

    let session_key = hook_lifecycle::sanitize_memory_key(&session);
    let _ = writeln!(standard_output, "gate status for session {session_key}:");

    for (dir, label) in gates {
        let counter_path = claude_home.join("state").join(dir).join(&session_key);
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
            n if n >= hook_lifecycle::default_max_blocks() => "capped",
            _ => "fired",
        };
        let _ = writeln!(standard_output, "  {label}: {cleared} (count: {count})");
    }
    0
}
