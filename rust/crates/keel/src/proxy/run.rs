//! Purpose: Execute commands through the capture-first token-saving proxy.
//! Caller: runner::run_run_command for `keel run -- <command>`.
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
use crate::runtime::{display_path, run_command, ProcessResult, MAX_CAPTURED_OUTPUT_BYTES};

/// Decide whether the proxy should run in capture mode (with compaction, raw
/// recovery, gain analytics) or fall back to a transparent passthrough.
///
/// The proxy exists to save tokens on output the *agent* will read. Running it
/// in a developer's plain shell is wrong: it captures their stdout, writes raw
/// recovery files they didn't ask for, and pollutes gain analytics with their
/// interactive runs. We detect a hook-launched invocation by checking for env
/// vars the harness reliably sets:
///
///   - `CLAUDE_SKILLS_HOOK`: our own opt-in marker — set by the install path,
///     manually exportable for testing, and immune to upstream rename.
///   - `CLAUDE_PROJECT_DIR`: documented the harness hook variable (see
///     code.claude.com/docs/en/hooks). Present whenever a hook fires.
///   - `CLAUDE_PLUGIN_ROOT`: present for plugin-scoped hooks.
///   - `CLAUDE_AGENT` / `CLAUDE_SKILLS_AGENT`: legacy markers some integrations
///     set when launching us; preserved so existing automations keep working.
///   - `CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_CODE_SESSION_ID` /
///     `AI_AGENT`: the vars the harness exports to the **Bash tool** child.
///
/// why: the first five are *hook*-process variables. `keel run` is spawned by
/// the Bash tool, not by a hook, and that environment carries none of them, so
/// every PreToolUse-rewritten command took the passthrough branch below and the
/// proxy silently did nothing: no compaction, no raw-store recovery artifact, no
/// compaction event (leaving `keel gain` empty), and no injection neutralization.
/// Measured on harness 2.1.220: the same command emitted 400 lines bare vs 50
/// with a signal present. The tool-process vars close that gap; the hook vars
/// stay so hook-launched runs keep working.
///
/// Any one of those being non-empty signals "this is a harness-driven run."
/// Absence means "user typed `keel run --` themselves" — passthrough.
pub(crate) const CLAUDE_CODE_SIGNAL_VARS: &[&str] = &[
    "CLAUDE_SKILLS_HOOK",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_AGENT",
    "CLAUDE_SKILLS_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "AI_AGENT",
];

pub fn running_under_claude_code() -> bool {
    CLAUDE_CODE_SIGNAL_VARS.iter().any(|name| {
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
    // Audit G-3/G-4: generic error-only filter (works without a dedicated adapter).
    flag_set.bool_flag("errors-only", false);
    // Audit G-2: ultra-compact tier — shorter body, failure-first line keep.
    flag_set.bool_flag("ultra", false);
    flag_set.string_flag("max-lines", "0");
    flag_set.string_flag("recovery-dir", "");
    flag_set.string_flag("adapter", "");
    flag_set.bool_flag("list-adapters", false);

    if let Err(parse_error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "{}", parse_error.message);
        return 1;
    }

    // Validate --max-lines: a non-numeric or negative value previously parsed to
    // 0 (via unwrap_or(0)) and silently meant "no cap" — the opposite of what a
    // user typing --max-lines garbage expects. Reject it explicitly. A value of
    // 0 (the default) still means "no cap".
    let max_lines_raw = flag_set.string_value("max-lines");
    let max_lines: usize = if max_lines_raw.trim() == "0" || max_lines_raw.trim().is_empty() {
        0
    } else {
        match max_lines_raw.trim().parse::<usize>() {
            Ok(value) => value,
            Err(_) => {
                let _ = writeln!(
                    standard_error,
                    "keel run: --max-lines expects a non-negative integer, got {:?}",
                    max_lines_raw
                );
                return 1;
            }
        }
    };

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
        let _ = writeln!(standard_error, "Usage: keel run -- <command> [args...]");
        return 1;
    }

    // Phase B gate: only run the capture+compaction proxy when the harness
    // (or our own opt-in marker) launched us. A developer typing
    // `keel run -- cargo test` in a plain shell expects to see their
    // command's output, not a "[keel] compacted command output" wrapper
    // and a recovery artifact they never asked for. Explicit filter modes
    // (`--errors-only`, `--ultra`) are intentional opt-ins and always capture.
    let force_capture = flag_set.bool_value("errors-only") || flag_set.bool_value("ultra");
    if !running_under_claude_code() && !force_capture {
        // why: passthrough honors none of the capture-only flags, and silently
        // ignoring an explicit `--json` reads as a broken flag rather than a mode.
        const CAPTURE_ONLY_FLAGS: &[&str] = &["json", "stream", "full", "no-compact", "no-raw"];
        let ignored: Vec<&str> = CAPTURE_ONLY_FLAGS
            .iter()
            .copied()
            .filter(|name| flag_set.bool_value(name))
            .chain(
                ["adapter", "recovery-dir"]
                    .into_iter()
                    .filter(|name| !flag_set.string_value(name).trim().is_empty()),
            )
            .chain(std::iter::once("max-lines").filter(|_| max_lines > 0))
            .collect();
        if !ignored.is_empty() {
            let _ = writeln!(
                standard_error,
                "keel run: ignoring {} outside an agent session (no capture, so nothing to \
                 compact or report). Pass --ultra or --errors-only to force capture.",
                ignored
                    .iter()
                    .map(|name| format!("--{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return run_proxy_passthrough(&command_arguments, standard_error);
    }

    let ast = match crate::proxy::classify::classify_command(&command_arguments) {
        Some(ast) => ast,
        None => {
            let _ = writeln!(standard_error, "Usage: keel run -- <command> [args...]");
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
                // Use the original (pre-cap) byte counts so gain analytics stay
                // honest when a runaway command's output was capped.
                stdout_bytes: result.original_stdout_bytes,
                stderr_bytes: result.original_stderr_bytes,
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

            let compact_result = if flag_set.bool_value("errors-only") {
                errors_only_compact(&raw_run, &meta)
            } else {
                adapter.compact(&raw_run.stdout, &raw_run.stderr, raw_run.exit_code, &meta)
            };
            let (compact_result, compact_findings) =
                neutralize_compact_result(compact_result, &meta.raw_id);
            let rendered_base = if flag_set.bool_value("ultra") {
                crate::proxy::render::render_ultra_compact_result(&compact_result)
            } else {
                crate::proxy::render::render_compact_result(&compact_result)
            };
            // Ultra defaults to a tight line cap when the caller did not set one.
            let effective_max_lines = if max_lines == 0 && flag_set.bool_value("ultra") {
                40
            } else {
                max_lines
            };
            let rendered = cap_lines(&rendered_base, effective_max_lines);
            // Break-even guard: never emit compacted output that is larger than
            // the raw it replaces. On small or already-terse command output the
            // fixed wrapper overhead (the PASS/FAIL prefix + the raw-recovery
            // footer) can exceed what compaction saves, which would INFLATE the
            // agent's context — the opposite of this proxy's purpose. The real
            // eval (`keel eval`) measured this on small fixtures
            // (npm install, kubectl get, a small failing test) before the guard:
            // they grew by up to 30%. When compaction does not actually shrink
            // the exact o200k_base token count, fall through to the neutralized
            // raw passthrough so a command that did not benefit pays no penalty.
            let raw_tokens =
                TokenMeter::count_bytes(&result.stdout) + TokenMeter::count_bytes(&result.stderr);
            let rendered_tokens = TokenMeter::count_text(&rendered);
            let compaction_reduces_tokens = rendered_tokens < raw_tokens;
            let use_compact_output = !flag_set.bool_value("full")
                && !flag_set.bool_value("no-compact")
                && compact_result.compacted
                && compaction_reduces_tokens;

            meta.adapter_name = compact_result.adapter_name.clone();
            meta.compacted = use_compact_output;
            meta.compact_path = meta.raw_path.join("compact.txt");

            // On the non-compact path we still neutralize prompt-injection in the
            // command output before the agent sees it — the raw bytes are the
            // exact attack surface the guard exists for. Each stream is cleaned
            // separately so the stdout/stderr split is preserved when written
            // below; `agent_output` (the merged form) backs the on-disk compact
            // copy and the token measurement.
            let (agent_output, clean_stdout, clean_stderr, raw_findings) = if use_compact_output {
                (rendered.clone(), String::new(), String::new(), Vec::new())
            } else {
                let (cleaned_stdout, mut findings) =
                    neutralize_injection(&String::from_utf8_lossy(&result.stdout), &meta.raw_id);
                let (cleaned_stderr, stderr_findings) =
                    neutralize_injection(&String::from_utf8_lossy(&result.stderr), &meta.raw_id);
                findings.extend(stderr_findings);
                let merged = format!("{cleaned_stdout}{cleaned_stderr}");
                (merged, cleaned_stdout, cleaned_stderr, findings)
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
            // Housekeeping after the capture completes. Throttled and fail-open
            // inside auto_prune, so the wrapped command is never slowed or failed.
            store.auto_prune();

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
                    // Write the NEUTRALIZED streams, never the raw bytes: this is
                    // the agent-visible output path the injection guard protects.
                    let _ = standard_output.write_all(clean_stdout.as_bytes());
                    let _ = standard_error.write_all(clean_stderr.as_bytes());
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

/// Transparent passthrough used when the proxy is invoked outside the harness.
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

/// Build a compact result that keeps only error/failure-class lines from the
/// raw streams. Adapter-agnostic — closes the `rtk err` gap for any command.
fn errors_only_compact(raw: &RawRun, meta: &RunMeta) -> crate::proxy::adapter::CompactResult {
    use crate::adapters::common::{error_only_lines, make_result, merge_streams};

    let merged = merge_streams(&raw.stdout, &raw.stderr);
    let lines = error_only_lines(&merged, 80);
    let body = if lines.is_empty() {
        if raw.exit_code == 0 {
            "(no error lines; exit 0)".to_string()
        } else {
            format!(
                "(no error-class lines matched; exit {})\nsee: keel raw {}",
                raw.exit_code, meta.raw_id
            )
        }
    } else {
        lines.join("\n")
    };
    let summary = format!(
        "[keel] errors-only\ncommand: {}\nreducer: errors-only; lines: {}",
        meta.command,
        body.lines().count()
    );
    make_result(
        "errors-only",
        summary,
        body,
        String::new(),
        raw.exit_code,
        meta,
        true,
    )
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
        "[keel] neutralized {} prompt-injection block(s) in command output (raw_id={raw_id}):",
        findings.len()
    );
    for finding in findings {
        let _ = writeln!(standard_error, "  - {finding}");
    }
    let _ = writeln!(
        standard_error,
        "  Recover the original bytes with: keel raw {raw_id}"
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

/// Maximum ordinary (non-high-signal) lines shown live per stream before the
/// "output capped" notice; the full stream is still captured for compaction.
const STREAM_LIVE_CAP: usize = 24;

/// Maximum high-signal lines allowed to bypass the live cap per stream. Bounds
/// the bypass so output that tags every line with an error/warning keyword
/// cannot defeat the live cap by flooding high-signal lines.
const STREAM_HIGH_SIGNAL_CAP: usize = 50;

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
    let mut stdout_original = 0usize;
    let mut stderr_original = 0usize;
    let mut stdout_live = 0usize;
    let mut stderr_live = 0usize;
    let mut stdout_high_signal = 0usize;
    let mut stderr_high_signal = 0usize;
    let mut stdout_capped = false;
    let mut stderr_capped = false;
    for chunk in receiver {
        // Always capture the raw bytes for post-run compaction (which neutralizes
        // the captured copy). The captured stream is the source of truth — but
        // cap it at MAX_CAPTURED_OUTPUT_BYTES so a runaway command cannot exhaust
        // memory. CRITICAL: keep draining the receiver even after the cap (discard
        // bytes) so the child does not deadlock on a full OS pipe.
        if chunk.label == "stdout" {
            stdout_original += chunk.bytes.len();
            if stdout_bytes.len() < MAX_CAPTURED_OUTPUT_BYTES {
                let room = MAX_CAPTURED_OUTPUT_BYTES - stdout_bytes.len();
                if chunk.bytes.len() <= room {
                    stdout_bytes.extend_from_slice(&chunk.bytes);
                } else {
                    stdout_bytes.extend_from_slice(&chunk.bytes[..room]);
                }
            }
        } else {
            stderr_original += chunk.bytes.len();
            if stderr_bytes.len() < MAX_CAPTURED_OUTPUT_BYTES {
                let room = MAX_CAPTURED_OUTPUT_BYTES - stderr_bytes.len();
                if chunk.bytes.len() <= room {
                    stderr_bytes.extend_from_slice(&chunk.bytes);
                } else {
                    stderr_bytes.extend_from_slice(&chunk.bytes[..room]);
                }
            }
        }

        let (live_count, high_signal_count) = if chunk.label == "stdout" {
            (&mut stdout_live, &mut stdout_high_signal)
        } else {
            (&mut stderr_live, &mut stderr_high_signal)
        };
        // A high-signal line may show past the normal cap, but only up to a
        // bounded high-signal budget — otherwise output that tags every line with
        // an "error"/"warning" keyword defeats the live cap entirely.
        let show_high_signal = chunk.high_signal && *high_signal_count < STREAM_HIGH_SIGNAL_CAP;
        let should_show = show_high_signal || *live_count < STREAM_LIVE_CAP;
        if should_show {
            if show_high_signal {
                *high_signal_count += 1;
            } else {
                // Only non-high-signal lines consume the normal live cap. A
                // high-signal line showed via its own budget (STREAM_HIGH_SIGNAL_CAP)
                // and must not also count against STREAM_LIVE_CAP, or output that
                // tags every line with "error"/"warning" would exhaust both caps.
                *live_count += 1;
            }
            // Neutralize prompt-injection before the chunk reaches the live
            // display — the live path is agent-visible just like the captured one.
            let (clean, _) =
                neutralize_injection(&String::from_utf8_lossy(&chunk.bytes), "live-stream");
            let _ = write!(live_output, "[keel stream:{}] ", chunk.label);
            let _ = live_output.write_all(clean.as_bytes());
            if !clean.ends_with('\n') {
                let _ = writeln!(live_output);
            }
        } else if chunk.label == "stdout" && !stdout_capped {
            let _ = writeln!(
                live_output,
                "[keel stream:stdout] ... live output capped; full output captured for compaction ..."
            );
            stdout_capped = true;
        } else if chunk.label == "stderr" && !stderr_capped {
            let _ = writeln!(
                live_output,
                "[keel stream:stderr] ... live output capped; full output captured for compaction ..."
            );
            stderr_capped = true;
        }
    }
    let status = child.wait().map_err(|error| format!("wait: {error}"))?;
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    Ok(ProcessResult {
        code: status.code().unwrap_or(1),
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        original_stdout_bytes: stdout_original,
        original_stderr_bytes: stderr_original,
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

    // why: a copy here would let a new gate signal survive `clear_signals`,
    // silently turning the "no signal present" tests into no-ops.
    const SIGNAL_VARS: &[&str] = super::CLAUDE_CODE_SIGNAL_VARS;

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
            "with all CLAUDE_* signals cleared, the gate must report 'not under the harness'"
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
            "the harness documents CLAUDE_PROJECT_DIR as a hook-execution variable; \
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

    /// Regression: the gate listed only *hook*-process vars, so every rewritten
    /// command hit passthrough. Each var below is live in a 2.1.220 Bash tool env.
    #[test]
    fn gate_allows_capture_for_bash_tool_environment_variables() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();

        for (name, value) in [
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            (
                "CLAUDE_CODE_SESSION_ID",
                "78b79ef6-1775-4dd3-b63c-9ccef1251fb7",
            ),
            ("AI_AGENT", "claude-code_2-1-220_agent"),
        ] {
            clear_signals();
            std::env::set_var(name, value);
            assert!(
                running_under_claude_code(),
                "{name} is exported to the Bash tool child; it must satisfy the capture gate \
                 or the compaction proxy silently no-ops on every rewritten command"
            );
        }

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

    #[test]
    fn no_compact_path_neutralizes_injection_before_writing_to_agent() {
        // Regression: the non-compact branch wrote the RAW result.stdout to the
        // agent instead of the neutralized output, so a command emitting a
        // "--- SYSTEM PROMPT ---" block reached the model verbatim. Run a real
        // command under the proxy with --no-compact and assert the marker block
        // is neutralized in the agent-visible output.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();
        // Satisfy the capture gate so the proxy actually runs.
        std::env::set_var("CLAUDE_SKILLS_HOOK", "test");

        // A portable command that prints an injection-shaped block. echo is not
        // a standalone executable on Windows (it is a cmd.exe builtin), so route
        // through the platform shell exactly as the proxy runs shell commands.
        let payload = "--- SYSTEM PROMPT ---";
        let recovery_dir = std::env::temp_dir().join(format!(
            "keel-inject-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let (program, shell_args) =
            crate::runtime::platform_shell_command_parts(&format!("echo {payload}"));
        let mut arguments = vec![
            "--no-compact".to_string(),
            "--recovery-dir".to_string(),
            recovery_dir.to_string_lossy().to_string(),
            "--".to_string(),
            program,
        ];
        arguments.extend(shell_args);
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let _ = run_proxy(&arguments, &mut stdout, &mut stderr);
        let rendered = String::from_utf8_lossy(&stdout);

        assert!(
            rendered.contains("neutralized prompt-injection"),
            "expected neutralized marker in agent output, got: {rendered} (stderr: {})",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            !rendered.contains("--- SYSTEM PROMPT ---"),
            "raw injection marker must NOT reach the agent, got: {rendered}"
        );

        let _ = std::fs::remove_dir_all(&recovery_dir);
        restore_signals(&snapshot);
    }

    /// Regression: the compact-ON branch of `run_proxy` (use_compact_output ==
    /// true) was never exercised end-to-end. The sole existing test passes
    /// `--no-compact`, so it only walks the neutralized-raw passthrough. This
    /// test runs a real command whose output is large enough to trip the
    /// generic adapter's head/tail reducer AND the break-even guard
    /// (`rendered_tokens < raw_tokens`), then asserts the four pieces of
    /// wiring that only the compact branch touches:
    ///   1. the agent sees the rendered wrapper (PASS/FAIL + `raw: keel raw`),
    ///      not the raw bytes;
    ///   2. `save_compact` wrote the rendered wrapper to `compact.txt`
    ///      (not the neutralized raw the --no-compact path writes);
    ///   3. `record_compaction_event` appended a JSONL line with
    ///      `compacted: true` to the event log that feeds `keel gain`;
    ///   4. the persisted `meta.json` carries `compacted: true`.
    ///
    /// A regression in the break-even comparison, the compact-branch save, or
    /// the event-log write would fail one of these.
    #[test]
    fn compact_on_path_emits_rendered_wrapper_and_records_compacted_event() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = snapshot_signals();
        clear_signals();
        std::env::set_var("CLAUDE_SKILLS_HOOK", "test");

        // Redirect resolve_claude_home (used by record_compaction_event) to a
        // private temp dir so the event-log write is isolated and observable,
        // mirroring runner/tool_timings.rs' with_isolated_claude_home.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let claude_home = std::env::temp_dir().join(format!(
            "keel-compact-on-home-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&claude_home);
        std::fs::create_dir_all(&claude_home).expect("create test claude home");
        let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
        std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

        let recovery_dir = std::env::temp_dir().join(format!(
            "keel-compact-on-recovery-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&recovery_dir);

        // 200 repetitive lines exceeds the generic adapter's LINE_LIMIT (80),
        // so compact_stream reduces it to ~20 head + omission notice + 20 tail.
        // That rendered wrapper is far smaller than the raw 200 lines, so the
        // break-even guard (rendered_tokens < raw_tokens) selects the compact
        // branch. The generator command is shell-specific: cmd.exe has no
        // printf/seq, so branch on the platform the way the existing
        // shell_command_parts test does. platform_shell_command_parts already
        // selects cmd /C vs bash -lc, so we hand it native syntax.
        let generator = if cfg!(windows) {
            // `for /L %i in (start,step,end)` is the cmd.exe counted loop; the
            // leading @ suppresses per-iteration command echo so only the
            // `line N` text reaches stdout.
            "for /L %i in (1,1,200) do @echo line %i"
        } else {
            // bash brace expansion needs no external tool (seq/jot are not
            // guaranteed on macOS), so this works on bash 3.2+ everywhere.
            "for i in {1..200}; do echo \"line $i\"; done"
        };
        let (program, shell_args) = crate::runtime::platform_shell_command_parts(generator);
        let mut arguments = vec![
            "--recovery-dir".to_string(),
            recovery_dir.to_string_lossy().to_string(),
            "--".to_string(),
            program,
        ];
        arguments.extend(shell_args);
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = run_proxy(&arguments, &mut stdout, &mut stderr);
        // The generator (for-loop printing lines) exits 0; if it failed to run
        // the rendered-output assertions below would also fail, but asserting
        // the exit code up front gives a clearer signal when the command itself
        // did not execute (e.g. a shell-syntax portability regression).
        assert_eq!(
            exit,
            0,
            "generator command must exit 0, got {exit} (stderr: {})",
            String::from_utf8_lossy(&stderr)
        );
        // Normalize CRLF (Windows cmd output) to LF so line-sensitive contains
        // checks behave identically on ubuntu/windows/macos CI.
        let rendered = String::from_utf8_lossy(&stdout).replace("\r\n", "\n");

        // (1) The compact branch emits render_compact_result's wrapper, which
        // always carries the `raw: keel raw <id>` footer and a `saved:` line.
        // The raw command output is 200 `line N` lines; if the compact branch
        // were NOT taken, stdout would contain `line 100` (a middle line the
        // head/tail reducer drops) and lack the wrapper footer.
        assert!(
            rendered.contains("raw: keel raw "),
            "compact branch must emit the rendered wrapper footer, got: {rendered} (stderr: {})",
            String::from_utf8_lossy(&stderr)
        );
        assert!(
            rendered.contains("saved: "),
            "compact branch must emit the token-savings line, got: {rendered}"
        );
        assert!(
            !rendered.contains("line 100\n"),
            "a middle raw line must NOT survive compaction, got: {rendered}"
        );
        // `line 1` survives as a head edge line.
        assert!(
            rendered.contains("line 1\n"),
            "a head edge line must survive compaction, got: {rendered}"
        );

        // (2) save_compact wrote the rendered wrapper to compact.txt. The
        // --no-compact path writes the neutralized raw instead, so asserting
        // the wrapper footer is present distinguishes the branches.
        let store = RawStore::with_root(recovery_dir.clone());
        let entries = store.list().expect("list raw entries");
        let entry = entries
            .first()
            .expect("at least one raw entry must be persisted");
        let compact = String::from_utf8(
            std::fs::read(entry.path.join("compact.txt")).expect("compact.txt written"),
        )
        .unwrap();
        assert!(
            compact.contains("raw: keel raw "),
            "save_compact must persist the rendered wrapper, got: {compact}"
        );

        // (4) The persisted meta.json carries compacted: true — the field
        // record_compaction_event reads and that keel gain surfaces. The
        // --no-compact path writes compacted: false here.
        let meta = store.load_meta(&entry.raw_id).expect("load meta");
        assert!(
            meta.compacted,
            "meta.compacted must be true on the compact branch, got false (meta: {meta:?})"
        );

        // (3) record_compaction_event appended a JSONL line with
        // compacted: true to the event log. This is the line keel gain reads
        // (gain.rs joins COMMAND_COMPACTION_EVENTS_FILE_NAME); a broken
        // compact-branch write would starve gain reporting.
        let event_path = claude_home.join("command-compaction-events.jsonl");
        let log = std::fs::read_to_string(&event_path)
            .expect("event log must be written on the compact branch");
        let last_line = log
            .lines()
            .last()
            .expect("at least one event line must be appended");
        let payload: serde_json::Value =
            serde_json::from_str(last_line).expect("event line is valid JSON");
        assert_eq!(
            payload["compacted"],
            serde_json::json!(true),
            "event log must record compacted: true on the compact branch, got: {last_line}"
        );
        assert_eq!(
            payload["adapter_name"],
            serde_json::json!("generic"),
            "event log must record the resolved adapter, got: {last_line}"
        );

        let _ = std::fs::remove_dir_all(&recovery_dir);
        let _ = std::fs::remove_dir_all(&claude_home);
        match previous_home {
            Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
            None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
        }
        restore_signals(&snapshot);
    }
}
