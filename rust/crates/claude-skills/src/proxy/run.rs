//! Purpose: Execute commands through the capture-first token-saving proxy.
//! Caller: runner::run_run_command for `claude-skills run -- <command>`.
//! Dependencies: args parsing, command adapters, raw store, event log, renderer, token meter, and runtime execution.
//! Main Functions: run_proxy.
//! Side Effects: Executes child commands, writes raw/compact recovery artifacts, appends gain events, and writes agent-facing output.

use std::io::Write;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::args::FlagSet;
use crate::proxy::event_log::record_compaction_event;
use crate::proxy::injection_guard::{neutralize_injection, InjectionFinding};
use crate::proxy::raw_store::{RawRun, RawStore, RunMeta};
use crate::proxy::token_meter::TokenMeter;
use crate::runtime::{display_path, run_command, ProcessResult};

/// Decide whether the proxy should run in capture mode (with compaction, raw
/// recovery, gain analytics) or fall back to a transparent passthrough.
///
/// The proxy exists to save tokens on output the *agent* will read. Running it
/// in a developer's plain shell is wrong: it captures their stdout, writes raw
/// recovery files they didn't ask for, and pollutes gain analytics with their
/// interactive runs. We detect a hook-launched invocation by checking for env
/// vars Claude Code reliably sets:
///
///   - `CLAUDE_SKILLS_HOOK`: our own opt-in marker — set by the install path,
///     manually exportable for testing, and immune to upstream rename.
///   - `CLAUDE_PROJECT_DIR`: documented Claude Code hook variable (see
///     code.claude.com/docs/en/hooks). Present whenever a hook fires.
///   - `CLAUDE_PLUGIN_ROOT`: present for plugin-scoped hooks.
///   - `CLAUDE_AGENT` / `CLAUDE_SKILLS_AGENT`: legacy markers some integrations
///     set when launching us; preserved so existing automations keep working.
///
/// Any one of those being non-empty signals "this is a Claude Code-driven run."
/// Absence means "user typed `claude-skills run --` themselves" — passthrough.
pub fn running_under_claude_code() -> bool {
    const SIGNALS: &[&str] = &[
        "CLAUDE_SKILLS_HOOK",
        "CLAUDE_PROJECT_DIR",
        "CLAUDE_PLUGIN_ROOT",
        "CLAUDE_AGENT",
        "CLAUDE_SKILLS_AGENT",
    ];
    SIGNALS.iter().any(|name| {
        std::env::var(name)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    })
}

pub fn run_proxy(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = FlagSet::new("run");
    flag_set.bool_flag("json", false);
    flag_set.bool_flag("stream", false);
    flag_set.bool_flag("full", false);
    flag_set.bool_flag("no-compact", false);
    flag_set.bool_flag("no-raw", false);
    flag_set.string_flag("max-lines", "0");
    flag_set.string_flag("recovery-dir", "");
    flag_set.string_flag("adapter", "");
    flag_set.bool_flag("list-adapters", false);

    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    let registry = crate::proxy::adapters::build_adapter_registry();

    if flag_set.bool_value("list-adapters") {
        let _ = writeln!(
            standard_output,
            "Available adapters: {}",
            crate::proxy::adapters::adapter_names()
        );
        return 0;
    }

    let command_arguments = flag_set.positional.clone();
    if command_arguments.is_empty() {
        let _ = writeln!(
            standard_error,
            "Usage: claude-skills run -- <command> [args...]"
        );
        return 1;
    }

    // Phase B gate: only run the capture+compaction proxy when Claude Code
    // (or our own opt-in marker) launched us. A developer typing
    // `claude-skills run -- cargo test` in a plain shell expects to see their
    // command's output, not a "[claude-skills] compacted command output" wrapper
    // and a recovery artifact they never asked for. The token-saver is only
    // valuable when the consumer is the agent transcript; when it is the
    // human terminal, passthrough is the correct behaviour.
    if !running_under_claude_code() {
        return run_proxy_passthrough(&command_arguments, standard_error);
    }

    let ast = match crate::proxy::classify::classify_command(&command_arguments) {
        Some(ast) => ast,
        None => {
            let _ = writeln!(
                standard_error,
                "Usage: claude-skills run -- <command> [args...]"
            );
            return 1;
        }
    };
    let cwd = ast.cwd.clone();
    let adapter = if flag_set.string_value("adapter").trim().is_empty() {
        registry
            .best_match(&ast)
            .expect("generic adapter registered")
    } else {
        let requested = flag_set.string_value("adapter").trim();
        match registry.find_by_name(requested) {
            Some(adapter) => adapter,
            None => {
                let _ = writeln!(
                    standard_error,
                    "Unknown adapter: {requested}. Available adapters: {}",
                    crate::proxy::adapters::adapter_names()
                );
                return 1;
            }
        }
    };
    let executable_ast = adapter.rewrite_args(&ast);
    let (program, args) = if let Some(executable_ast) = executable_ast {
        (executable_ast.program, executable_ast.args)
    } else if ast.has_shell_syntax && !ast.shell_wrapped {
        crate::runtime::platform_shell_command_parts(&shell_join(&command_arguments))
    } else {
        (
            command_arguments.first().cloned().unwrap_or_default(),
            command_arguments.iter().skip(1).cloned().collect(),
        )
    };

    let start_time = Instant::now();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let run_result = if flag_set.bool_value("stream") {
        run_command_streaming_proxy(&program, &args, standard_error)
    } else {
        run_command(&program, &args, None)
    };
    let duration = start_time.elapsed();

    match run_result {
        Ok(result) => {
            let raw_id = RawStore::generate_id();
            let mut meta = RunMeta {
                raw_id: raw_id.clone(),
                command: shell_join(&command_arguments),
                program: ast.program.clone(),
                args: ast.args.clone(),
                cwd: cwd.clone(),
                started_at,
                duration_ms: duration.as_millis() as u64,
                exit_code: result.code,
                adapter_name: adapter.name().to_string(),
                raw_path: std::path::PathBuf::new(),
                compact_path: std::path::PathBuf::new(),
                // The capture path only runs when `running_under_claude_code()`
                // returned true above, so defaulting to "claude-code" reflects
                // verified state — not an assumption. CLAUDE_SKILLS_AGENT and
                // CLAUDE_AGENT remain explicit overrides for forks or test
                // harnesses that want a custom label.
                agent: std::env::var("CLAUDE_SKILLS_AGENT")
                    .or_else(|_| std::env::var("CLAUDE_AGENT"))
                    .unwrap_or_else(|_| "claude-code".to_string()),
                workspace: cwd.clone(),
                stdout_bytes: result.stdout.len(),
                stderr_bytes: result.stderr.len(),
                compact_stdout_bytes: 0,
                compact_stderr_bytes: 0,
                estimated_tokens_before: TokenMeter::estimate_bytes(&result.stdout)
                    + TokenMeter::estimate_bytes(&result.stderr),
                estimated_tokens_after: 0,
                estimated_tokens_saved: 0,
                savings_pct: 0.0,
                compacted: false,
            };

            let raw_run = RawRun {
                stdout: result.stdout.clone(),
                stderr: result.stderr.clone(),
                exit_code: result.code,
            };

            let store = if flag_set.string_value("recovery-dir").trim().is_empty() {
                RawStore::new()
            } else {
                RawStore::with_root(std::path::PathBuf::from(
                    flag_set.string_value("recovery-dir"),
                ))
            };
            if !flag_set.bool_value("no-raw") {
                let _ = store.save(&mut meta, &raw_run);
            }

            let compact_result =
                adapter.compact(&raw_run.stdout, &raw_run.stderr, raw_run.exit_code, &meta);
            let (compact_result, compact_findings) =
                neutralize_compact_result(compact_result, &meta.raw_id);
            let rendered = cap_lines(
                &crate::proxy::render::render_compact_result(&compact_result),
                flag_set.string_value("max-lines").parse().unwrap_or(0),
            );
            let use_compact_output = !flag_set.bool_value("full")
                && !flag_set.bool_value("no-compact")
                && compact_result.compacted;

            meta.adapter_name = compact_result.adapter_name.clone();
            meta.compacted = use_compact_output;
            meta.compact_path = meta.raw_path.join("compact.txt");

            let (agent_output, raw_findings) = if use_compact_output {
                (rendered.clone(), Vec::new())
            } else {
                let merged = format!(
                    "{}{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
                let (cleaned, findings) = neutralize_injection(&merged, &meta.raw_id);
                (cleaned, findings)
            };
            let mut all_findings = raw_findings;
            all_findings.extend(compact_findings);
            report_injection_findings(&all_findings, &meta.raw_id, standard_error);
            meta.compact_stdout_bytes = agent_output.len();
            meta.compact_stderr_bytes = 0;
            let measurement =
                TokenMeter::measure(&result.stdout, &result.stderr, agent_output.as_bytes());
            meta.estimated_tokens_before = measurement.tokens_before;
            meta.estimated_tokens_after = measurement.tokens_after;
            meta.estimated_tokens_saved = measurement.tokens_saved as isize;
            meta.savings_pct = measurement.savings_pct;
            if !flag_set.bool_value("no-raw") {
                let _ = store.save_compact(&meta, &agent_output);
            }
            record_compaction_event(&meta, &compact_result, &all_findings);

            if flag_set.bool_value("json") {
                let json_result = serde_json::json!({
                    "command": meta.command,
                    "exit_code": meta.exit_code,
                    "adapter_name": compact_result.adapter_name,
                    "compacted": use_compact_output,
                    "raw_id": meta.raw_id,
                    "raw_path": display_path(&meta.raw_path),
                    "compact_path": display_path(&meta.compact_path),
                    "estimated_tokens_before": meta.estimated_tokens_before,
                    "estimated_tokens_after": meta.estimated_tokens_after,
                    "estimated_tokens_saved": meta.estimated_tokens_saved,
                    "exact_tokens_before": meta.estimated_tokens_before,
                    "exact_tokens_after": meta.estimated_tokens_after,
                    "exact_tokens_saved": meta.estimated_tokens_saved,
                    "tokenizer": "o200k_base",
                    "token_counting": "exact",
                    "savings_pct": meta.savings_pct,
                    "summary": compact_result.summary,
                    "stdout": compact_result.stdout,
                    "stderr": compact_result.stderr,
                });
                let _ = writeln!(
                    standard_output,
                    "{}",
                    serde_json::to_string_pretty(&json_result).unwrap()
                );
            } else {
                if !use_compact_output {
                    let _ = standard_output.write_all(&result.stdout);
                    let _ = standard_error.write_all(&result.stderr);
                } else {
                    let _ = writeln!(standard_output, "{}", rendered);
                }
            }

            result.code.clamp(0, 255) as u8
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Unable to execute command: {error}");
            1
        }
    }
}

/// Transparent passthrough used when the proxy is invoked outside Claude Code.
/// Mirrors what the user would see if they had run the program directly: stdio
/// inherited, no capture, no analytics, exit code propagated. Falls back to a
/// platform shell wrapper if the arguments contain shell metacharacters that
/// only a shell can interpret (`|`, `&&`, `>`, env-var assignments, etc.).
fn run_proxy_passthrough(command_arguments: &[String], standard_error: &mut dyn Write) -> u8 {
    let needs_shell = command_arguments.iter().any(|argument| {
        argument.chars().any(|character| {
            matches!(
                character,
                '|' | '&' | ';' | '<' | '>' | '`' | '$' | '(' | ')'
            )
        })
    });

    let (program, args) = if needs_shell {
        crate::runtime::platform_shell_command_parts(&shell_join(command_arguments))
    } else {
        (
            command_arguments.first().cloned().unwrap_or_default(),
            command_arguments.iter().skip(1).cloned().collect(),
        )
    };

    match crate::runtime::run_command_inherit(&program, &args, None) {
        Ok(code) => code.clamp(0, 255) as u8,
        Err(error) => {
            let _ = writeln!(standard_error, "Unable to execute command: {error}");
            1
        }
    }
}

fn neutralize_compact_result(
    mut result: crate::proxy::adapter::CompactResult,
    raw_id: &str,
) -> (crate::proxy::adapter::CompactResult, Vec<InjectionFinding>) {
    let mut findings = Vec::new();
    let (clean_stdout, stdout_findings) = neutralize_injection(&result.stdout, raw_id);
    let (clean_stderr, stderr_findings) = neutralize_injection(&result.stderr, raw_id);
    findings.extend(stdout_findings);
    findings.extend(stderr_findings);
    result.stdout = clean_stdout;
    result.stderr = clean_stderr;
    if !findings.is_empty() {
        result.warnings.push(format!(
            "neutralized {} prompt-injection block(s)",
            findings.len()
        ));
    }
    (result, findings)
}

fn report_injection_findings(
    findings: &[InjectionFinding],
    raw_id: &str,
    standard_error: &mut dyn Write,
) {
    if findings.is_empty() {
        return;
    }
    let _ = writeln!(
        standard_error,
        "[claude-skills] neutralized {} prompt-injection block(s) in command output (raw_id={raw_id}):",
        findings.len()
    );
    for finding in findings {
        let _ = writeln!(standard_error, "  - {finding}");
    }
    let _ = writeln!(
        standard_error,
        "  Recover the original bytes with: claude-skills raw {raw_id}"
    );
}

fn shell_join(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if matches!(
                argument.as_str(),
                "|" | "||" | "&&" | ";" | "<" | ">" | ">>" | "2>" | "2>>"
            ) {
                argument.to_string()
            } else if argument.is_empty()
                || argument.chars().any(|character| {
                    character.is_whitespace()
                        || matches!(
                            character,
                            '\'' | '"' | '$' | '`' | '&' | '|' | ';' | '<' | '>' | '(' | ')'
                        )
                })
            {
                format!("'{}'", argument.replace('\'', "'\\''"))
            } else {
                argument.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cap_lines(text: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let keep = max_lines.saturating_sub(1);
    let mut rendered = lines[..keep].join("\n");
    rendered.push_str(&format!(
        "\n... omitted {} compact lines due to --max-lines ...",
        lines.len().saturating_sub(keep)
    ));
    rendered
}

struct StreamChunk {
    label: &'static str,
    bytes: Vec<u8>,
    high_signal: bool,
}

fn run_command_streaming_proxy(
    program: &str,
    arguments: &[String],
    live_output: &mut dyn Write,
) -> Result<ProcessResult, String> {
    let mut child = Command::new(program);
    child.args(arguments);
    child.stdin(Stdio::inherit());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|error| format!("execute {program}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "capture child stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "capture child stderr".to_string())?;
    let (sender, receiver) = mpsc::channel::<StreamChunk>();
    let stdout_sender = sender.clone();
    let stdout_handle = thread::spawn(move || read_stream("stdout", stdout, stdout_sender));
    let stderr_handle = thread::spawn(move || read_stream("stderr", stderr, sender));

    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_live = 0usize;
    let mut stderr_live = 0usize;
    let mut stdout_capped = false;
    let mut stderr_capped = false;
    for chunk in receiver {
        let live_count = if chunk.label == "stdout" {
            &mut stdout_live
        } else {
            &mut stderr_live
        };
        let should_show = chunk.high_signal || *live_count < 24;
        if should_show {
            let _ = write!(live_output, "[claude-skills stream:{}] ", chunk.label);
            let _ = live_output.write_all(&chunk.bytes);
            if !chunk.bytes.ends_with(b"\n") {
                let _ = writeln!(live_output);
            }
        } else if chunk.label == "stdout" && !stdout_capped {
            let _ = writeln!(
                live_output,
                "[claude-skills stream:stdout] ... live output capped; full output captured for compaction ..."
            );
            stdout_capped = true;
        } else if chunk.label == "stderr" && !stderr_capped {
            let _ = writeln!(
                live_output,
                "[claude-skills stream:stderr] ... live output capped; full output captured for compaction ..."
            );
            stderr_capped = true;
        }
        *live_count += 1;
        if chunk.label == "stdout" {
            stdout_bytes.extend_from_slice(&chunk.bytes);
        } else {
            stderr_bytes.extend_from_slice(&chunk.bytes);
        }
    }
    let status = child.wait().map_err(|error| format!("wait: {error}"))?;
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    Ok(ProcessResult {
        code: status.code().unwrap_or(1),
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

fn read_stream<R: std::io::Read + Send + 'static>(
    label: &'static str,
    reader: R,
    sender: mpsc::Sender<StreamChunk>,
) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(read) = reader.read_until(b'\n', &mut line) else {
            break;
        };
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let lower = text.to_ascii_lowercase();
        let high_signal = [
            "error",
            "failed",
            "failure",
            "panic",
            "exception",
            "traceback",
            "warning",
            "denied",
            "timeout",
            "killed",
        ]
        .iter()
        .any(|needle| lower.contains(needle));
        if sender
            .send(StreamChunk {
                label,
                bytes: line.clone(),
                high_signal,
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;

    const SIGNAL_VARS: &[&str] = &[
        "CLAUDE_SKILLS_HOOK",
        "CLAUDE_PROJECT_DIR",
        "CLAUDE_PLUGIN_ROOT",
        "CLAUDE_AGENT",
        "CLAUDE_SKILLS_AGENT",
    ];

    fn snapshot_signals() -> Vec<(&'static str, Option<String>)> {
        SIGNAL_VARS
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect()
    }

    fn restore_signals(snapshot: &[(&'static str, Option<String>)]) {
        for (name, value) in snapshot {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }

    fn clear_signals() {
        for name in SIGNAL_VARS {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn gate_blocks_capture_when_no_claude_code_signal_is_present() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();

        assert!(
            !running_under_claude_code(),
            "with all CLAUDE_* signals cleared, the gate must report 'not under Claude Code'"
        );

        restore_signals(&snapshot);
    }

    #[test]
    fn gate_allows_capture_when_claude_project_dir_is_set() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();
        std::env::set_var("CLAUDE_PROJECT_DIR", "/tmp/example-project");

        assert!(
            running_under_claude_code(),
            "Claude Code documents CLAUDE_PROJECT_DIR as a hook-execution variable; \
             setting it must satisfy the gate"
        );

        restore_signals(&snapshot);
    }

    #[test]
    fn gate_allows_capture_when_explicit_opt_in_marker_is_set() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();
        std::env::set_var("CLAUDE_SKILLS_HOOK", "1");

        assert!(
            running_under_claude_code(),
            "operators must be able to opt into capture mode for tests and tooling"
        );

        restore_signals(&snapshot);
    }

    #[test]
    fn gate_treats_blank_signal_value_as_absence() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();
        std::env::set_var("CLAUDE_PROJECT_DIR", "   ");

        assert!(
            !running_under_claude_code(),
            "an empty or whitespace-only env value is the same as 'not set' — otherwise \
             a stale export from a previous shell could silently re-enable capture"
        );

        restore_signals(&snapshot);
    }

    #[test]
    fn shell_command_parts_uses_platform_appropriate_shell() {
        let (program, args) =
            crate::runtime::platform_shell_command_parts("cargo test --workspace");
        if cfg!(windows) {
            assert_eq!(program, "cmd");
            assert_eq!(args[0], "/C");
        } else {
            assert_eq!(program, "bash");
            assert_eq!(args[0], "-lc");
        }
        assert_eq!(args[1], "cargo test --workspace");
    }
}
