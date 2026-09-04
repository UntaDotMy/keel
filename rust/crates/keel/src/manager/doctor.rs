//! Purpose: Doctor and hook probe logic for keel manager.
//! Caller: commands.rs via run_doctor_command.
//! Dependencies: std::fs, std::io, std::path, std::process, crate::runtime, crate::hooks, crate::runner, crate::proxy.
//! Main Functions: run_doctor_command, hook_rewrites_raw_command, hook_accepts_wrapped_command, run_hook_probe, write_doctor_check, find_on_path.
//! Side Effects: Runs hook probe commands, writes doctor check output.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::manager::install::{codex_plugin_installation, CodexPluginInstallation};
use crate::runtime::{
    display_path, installed_executable_path, resolve_claude_home,
    COMMAND_COMPACTION_EVENTS_FILE_NAME,
};

use super::run_status_command;

pub fn run_doctor_command(
    build_version: &str,
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flag_set = crate::args::FlagSet::new("doctor");
    flag_set.bool_flag("fix", false);
    flag_set.string_flag("repo-root", "");
    flag_set.string_flag("claude-home", "");
    if let Err(error) = flag_set.parse(arguments) {
        let _ = writeln!(standard_error, "doctor: {}", error.message);
        return 1;
    }
    let fix = flag_set.bool_value("fix");

    // Forward only the flags `status` understands; `status` rejects `--fix`.
    let mut status_args: Vec<String> = Vec::new();
    let repo_root = flag_set.string_value("repo-root").trim();
    if !repo_root.is_empty() {
        status_args.push("--repo-root".to_string());
        status_args.push(repo_root.to_string());
    }
    let claude_home_arg = flag_set.string_value("claude-home").trim();
    if !claude_home_arg.is_empty() {
        status_args.push("--claude-home".to_string());
        status_args.push(claude_home_arg.to_string());
    }
    let status_code =
        run_status_command(build_version, &status_args, standard_output, standard_error);
    if status_code != 0 {
        return status_code;
    }
    // why: the checks below must honor --claude-home, not just the status summary.
    let claude_home = match resolve_claude_home(claude_home_arg) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };
    let hooks_path = crate::runtime::claude_engagement_home(&claude_home)
        .join(crate::hooks::claude::SETTINGS_FILE_NAME);
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
        display_path(&std::env::current_exe().unwrap_or_else(|_| PathBuf::from("keel")))
    );
    let raw_store = crate::proxy::raw_store::RawStore::with_root(claude_home.join("raw-output"));
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
        "rewrite: cargo test -> keel run -- cargo test",
    );
    write_doctor_check(
        standard_output,
        per_shell_rewrite_prefixes_are_valid(),
        "rewrite prefix valid for every shell tool (bash/powershell/pwsh/cmd)",
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
    // Host-neutral home health: report the keel home, binary, legacy-binary
    // removal, and PATH so a partial migration is visible, not silent.
    let keel_home_label = display_path(&claude_home);
    write_doctor_check(
        standard_output,
        claude_home.is_dir(),
        &format!("keel home exists: {keel_home_label}"),
    );
    let keel_binary = installed_executable_path(&claude_home);
    write_doctor_check(
        standard_output,
        keel_binary.is_file(),
        &format!("keel binary present: {}", display_path(&keel_binary)),
    );
    let legacy_binary = crate::runtime::legacy_claude_executable_path(&claude_home);
    write_doctor_check(
        standard_output,
        legacy_binary.as_ref().map(|p| !p.exists()).unwrap_or(true),
        &match legacy_binary {
            Some(path) if path.exists() => {
                format!("legacy binary removed (still at {})", display_path(&path))
            }
            _ => "legacy ~/.claude binary removed".to_string(),
        },
    );
    write_doctor_check(
        standard_output,
        path_contains_dir(&claude_home),
        &format!("PATH includes keel home: {keel_home_label}"),
    );
    report_capture_gate(standard_output);
    report_mcp_registration(standard_output, &claude_home);
    probe_mcp_launch(standard_output, &claude_home);
    report_bridge_host_wiring(standard_output, &claude_home);
    // Primary interception path is PreToolUse Bash rewrite (probed above).
    // Do not emit a permanent false-warn for optional UnifiedExec surfaces.
    if dry_run_rewrites {
        write_doctor_check(
            standard_output,
            true,
            "shell interception path: PreToolUse rewrite healthy",
        );
    } else {
        write_doctor_check(
            standard_output,
            false,
            "shell interception path: PreToolUse rewrite probe failed",
        );
    }
    let orphan_pids = find_orphan_mcp_serve_pids();
    if orphan_pids.is_empty() {
        write_doctor_check(
            standard_output,
            true,
            "no orphaned keel mcp serve processes",
        );
    } else if fix {
        let reaped = reap_processes(&orphan_pids);
        write_doctor_check(
            standard_output,
            reaped == orphan_pids.len(),
            &format!(
                "reaped {reaped}/{} orphaned keel mcp serve process(es) (they hold the recall WAL; the idle self-reap now prevents recurrence)",
                orphan_pids.len()
            ),
        );
    } else {
        let _ = writeln!(
            standard_output,
            "[warn] {} orphaned keel mcp serve process(es) detected (pids: {}); they hold the recall SQLite WAL and cause Grok MCP tool timeouts. Run `keel doctor --fix` to reap them.",
            orphan_pids.len(),
            orphan_pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Many concurrent stdio servers means many processes sharing one recall
    // DB; the shared Streamable HTTP daemon is the documented scale path.
    let live_servers = running_mcp_serve_pids_with_ppid().len();
    if live_servers > 1 {
        let _ = writeln!(
            standard_output,
            "[info] {live_servers} keel mcp serve processes running (one per host window). \
             For many windows, run one shared `keel mcp serve-http` daemon and point hosts at \
             http://127.0.0.1:3920/mcp instead (see README 'MCP across many windows')."
        );
    }
    let _ = writeln!(
        standard_output,
        "Run `keel validate --profile smoke` for local proof."
    );
    0
}

/// PIDs of running `keel mcp serve` processes whose owning harness session has
/// died. A dropped Grok/Claude session leaves these as orphans; each holds the
/// recall SQLite WAL and is the root cause of intermittent MCP tool timeouts.
///
/// Orphan means the parent harness process is gone, not merely "not the
/// caller's PID": keel runs one `mcp serve` per live session, so a healthy
/// server owned by another running session must never be flagged. Otherwise
/// `doctor --fix` would `taskkill` a live, in-use server. The caller's own PID
/// is excluded as well. A PID whose parent cannot be determined is treated as
/// alive, because a false orphan leads to killing a healthy server.
fn find_orphan_mcp_serve_pids() -> Vec<u32> {
    let own_pid = std::process::id();
    running_mcp_serve_pids_with_ppid()
        .into_iter()
        .filter(|(pid, _)| *pid != own_pid)
        .filter(|(_, ppid)| !parent_is_alive(*ppid))
        .map(|(pid, _)| pid)
        .collect()
}

/// Enumerate `keel mcp serve` processes as `(pid, parent_pid)` pairs via the OS
/// process table. Windows uses PowerShell CIM; Unix uses `ps`. Returns an empty
/// vec on any probe failure; the orphan check is advisory and must never fail
/// the doctor run. A parent pid of 0 means "unknown" (conservatively treated as
/// alive by [`parent_is_alive`]).
#[cfg(windows)]
fn running_mcp_serve_pids_with_ppid() -> Vec<(u32, u32)> {
    let arguments = [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-CimInstance Win32_Process -Filter \"Name='keel.exe'\" | Where-Object { $_.CommandLine -match 'mcp serve' } | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId)\" }",
    ]
    .map(str::to_string);
    let output = crate::runtime::run_command_with_timeout(
        "powershell",
        &arguments,
        None,
        std::time::Duration::from_secs(10),
    );
    parse_pid_ppid_listing(
        output
            .ok()
            .filter(|result| result.code == 0)
            .map(|result| String::from_utf8_lossy(&result.stdout).to_string()),
    )
}

#[cfg(not(windows))]
fn running_mcp_serve_pids_with_ppid() -> Vec<(u32, u32)> {
    let arguments = [
        "-c".to_string(),
        "ps -eo pid=,ppid=,args | grep '[k]eel mcp serve' | awk '{print $1\" \"$2}'".to_string(),
    ];
    let output = crate::runtime::run_command_with_timeout(
        "sh",
        &arguments,
        None,
        std::time::Duration::from_secs(10),
    );
    parse_pid_ppid_listing(
        output
            .ok()
            .filter(|result| result.code == 0)
            .map(|result| String::from_utf8_lossy(&result.stdout).to_string()),
    )
}

fn parse_pid_ppid_listing(listing: Option<String>) -> Vec<(u32, u32)> {
    listing
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.trim().parse::<u32>().ok()?;
            let ppid = parts
                .next()
                .and_then(|p| p.trim().parse::<u32>().ok())
                .unwrap_or(0);
            Some((pid, ppid))
        })
        .collect()
}

/// Whether the parent harness process is still alive. Unknown parents (ppid 0
/// or a failed probe) count as alive so a healthy server is never killed on
/// inconclusive evidence.
#[cfg(windows)]
fn parent_is_alive(ppid: u32) -> bool {
    if ppid == 0 {
        return true; // Unknown parent: assume alive, never kill on doubt.
    }
    crate::runtime::process_is_alive(ppid).unwrap_or(true)
}

#[cfg(not(windows))]
fn parent_is_alive(ppid: u32) -> bool {
    if ppid == 0 {
        return true; // Unknown parent: assume alive, never kill on doubt.
    }
    if ppid == 1 {
        return false; // Reparented to init/launchd: the harness parent is gone.
    }
    crate::runtime::process_is_alive(ppid).unwrap_or(true)
}

/// Terminate the given PIDs, returning how many were actually reaped. Used only
/// by `doctor --fix`; the pids come from `find_orphan_mcp_serve_pids`, which
/// already excludes the caller's own process.
fn reap_processes(pids: &[u32]) -> usize {
    pids.iter().filter(|pid| terminate_pid(**pid)).count()
}

#[cfg(windows)]
fn terminate_pid(pid: u32) -> bool {
    let arguments = ["/PID", &pid.to_string(), "/F", "/T"].map(str::to_string);
    crate::runtime::run_command_with_timeout(
        "taskkill",
        &arguments,
        None,
        std::time::Duration::from_secs(10),
    )
    .map(|result| result.code == 0)
    .unwrap_or(false)
}

#[cfg(not(windows))]
fn terminate_pid(pid: u32) -> bool {
    crate::runtime::run_command_with_timeout(
        "kill",
        &[pid.to_string()],
        None,
        std::time::Duration::from_secs(10),
    )
    .map(|result| result.code == 0)
    .unwrap_or(false)
}

/// Probe the PreToolUse hook with a noisy command and confirm it produces a
/// transparent rewrite payload. The current contract (the harness hook schema)
/// is `hookSpecificOutput.permissionDecision = "allow"` plus
/// `hookSpecificOutput.updatedInput.command = "keel run -- ..."` — the
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
/// short-circuits — emitting empty stdout (no `hookSpecificOutput`) so the harness
/// Code runs the command unchanged. If the hook re-rewrote a wrapped command we
/// would loop on every turn.
fn hook_accepts_wrapped_command() -> bool {
    let executable = std::env::current_exe()
        .map(|path| display_path(&path))
        .unwrap_or_else(|_| "keel".to_string());
    let command = format!("{executable} run -- cargo test --workspace");
    run_hook_probe(&command)
        .map(|output| !output.contains("permissionDecision"))
        .unwrap_or(false)
}

fn run_hook_probe(command: &str) -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    // Isolate the rewrite-contract probe from Iron Law STRICT. Doctor's
    // sample payload is a plain `cargo test` Bash command; under the default
    // gate that is hard-denied before compaction rewrite runs, which makes
    // the rewrite health check always fail even when rewrite itself is fine.
    // Disabling the gate for this child only measures rewrite allow/updatedInput.
    let mut child = Command::new(executable)
        .args(["hook", "pre-tool-use"])
        .env("KEEL_IRON_LAW_GATE", "off")
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

/// Whether every shell tool the PreToolUse gate admits gets a prefix its own
/// shell can parse.
///
/// The single-command probe above only covers the platform-default shape, so it
/// reported healthy while the PowerShell tool path emitted a POSIX-quoted path
/// with no call operator, which PowerShell rejects outright.
/// Report whether the rewritten command would actually be captured.
/// why: doctor probed only the rewrite, so it passed while capture did nothing.
fn report_capture_gate(standard_output: &mut dyn Write) {
    if crate::proxy::run::running_under_claude_code() {
        let _ = writeln!(
            standard_output,
            "[ok] compaction capture active (agent-session signal present)"
        );
        return;
    }
    let _ = writeln!(
        standard_output,
        "[warn] compaction capture inactive: no agent-session env signal, so `keel run` \
         passes through without compacting. Expected when you run doctor from a plain \
         shell; inside an agent session it means rewritten commands save nothing."
    );
}

fn per_shell_rewrite_prefixes_are_valid() -> bool {
    use crate::runner::shell_rewrite::{
        rewrite_command_text_for_shell, rewrite_shell_for_tool, RewriteShell, SHELL_TOOL_NAMES,
    };
    SHELL_TOOL_NAMES.iter().all(|tool| {
        let shell = rewrite_shell_for_tool(tool);
        let decision = rewrite_command_text_for_shell("cargo test", shell);
        if !decision.supported {
            return false;
        }
        let command = decision.rewritten_command.as_str();
        match shell {
            RewriteShell::PowerShell => command.starts_with("& '"),
            RewriteShell::Cmd => command.starts_with('"'),
            RewriteShell::Bash | RewriteShell::PlatformDefault => !command.starts_with("& "),
        }
    })
}

fn write_doctor_check(standard_output: &mut dyn Write, ok: bool, message: &str) {
    let status = if ok { "[ok]" } else { "[warn]" };
    let _ = writeln!(standard_output, "{status} {message}");
}

/// Spawn the MCP command exactly as registered in `~/.claude.json` and confirm it
/// answers an `initialize` request. This catches the silent failure mode where the
/// registered `command` (e.g. the plugin manifest's bare `keel`) does not
/// resolve on the host's PATH — the harness would then fail to start the server and
/// all of its always-on tools would be missing with no in-session signal. Reading the
/// entry from disk (rather than assuming the installed path) means we probe what
/// the harness will actually launch.
///
/// Read-only: spawns a short-lived `mcp serve` child, pipes one JSON-RPC line, and
/// reads the response. No files are mutated.
fn probe_mcp_launch(standard_output: &mut dyn Write, claude_home: &std::path::Path) {
    let config_path = super::mcp_register::mcp_config_path(claude_home);
    let text = fs::read_to_string(&config_path).unwrap_or_default();
    let parsed: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let entry = parsed
        .as_ref()
        .and_then(|doc| doc.get("mcpServers"))
        .and_then(|servers| servers.get(super::mcp_register::MCP_SERVER_KEY));
    let Some(entry) = entry else {
        // No registration — report_mcp_registration already warned; nothing to probe.
        return;
    };
    let command = entry
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if command.is_empty() {
        write_doctor_check(
            standard_output,
            false,
            "keel MCP launch (registered entry has no command; run `keel repair`)",
        );
        return;
    }
    // Probe with EXACTLY the registered args so the result reflects what the harness
    // Code will actually launch, not an assumed shape. Three cases:
    //   - `args` is a valid array  → use it verbatim.
    //   - `args` is absent          → the harness launches the bare command with
    //                                 no args, so probe that (an MCP server needs
    //                                 `mcp serve`, so a bare command correctly
    //                                 fails the probe and flags the broken entry).
    //   - `args` is present but not an array → malformed entry; warn and skip
    //                                 rather than guess.
    let args: Vec<String> = match entry.get("args") {
        None => Vec::new(),
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        Some(_) => {
            write_doctor_check(
                standard_output,
                false,
                "keel MCP launch (registered entry has non-array args; \
                 run `keel repair`)",
            );
            return;
        }
    };
    let launched = probe_mcp_initialize(&command, &args);
    write_doctor_check(
        standard_output,
        launched,
        if launched {
            "keel MCP launch (registered command starts and responds to initialize)"
        } else {
            "keel MCP launch (registered command did not start — check that it \
             resolves on PATH; run `keel repair` to pin an absolute path)"
        },
    );
}

/// Spawn `command args...`, send a single `initialize` JSON-RPC line, and return
/// true if the child emits a JSON-RPC response containing `protocolVersion`.
///
/// Bounded by a wall-clock timeout: a registered command that starts but never
/// answers (a wrong binary, or one that blocks waiting for more input) must not
/// hang `doctor`. A reader thread captures stdout while the main thread waits on
/// a channel with a deadline; on timeout the child is killed and reaped.
fn probe_mcp_initialize(command: &str, args: &[String]) -> bool {
    probe_mcp_initialize_with_timeout(command, args, std::time::Duration::from_secs(5))
}

fn probe_mcp_initialize_with_timeout(
    command: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> bool {
    use std::sync::mpsc;

    const MAX_PROBE_OUTPUT_BYTES: usize = 256 * 1024;
    let mut probe = Command::new(command);
    probe
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::runtime::configure_process_group(&mut probe);
    let mut child = match probe.spawn() {
        Ok(child) => child,
        // Command not found / not executable — the failure this probe exists for.
        Err(_) => return false,
    };
    let mut process_guard = match crate::runtime::own_process_tree(&mut child) {
        Ok(guard) => guard,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    };
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "doctor", "version": "1.0" }
        }
    });
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{request}");
        // stdin drops here so the server sees EOF and exits after answering.
    }

    let (sender, receiver) = mpsc::channel::<String>();
    let reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if buffer.len() < MAX_PROBE_OUTPUT_BYTES {
                            let remaining = MAX_PROBE_OUTPUT_BYTES - buffer.len();
                            buffer.extend_from_slice(&chunk[..count.min(remaining)]);
                        }
                    }
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&buffer).into_owned());
        })
    });

    let responded = match receiver.recv_timeout(timeout) {
        Ok(response) => response.contains("protocolVersion") && response.contains("\"result\""),
        // Timed out (or the reader thread vanished) — treat as no response.
        Err(_) => false,
    };

    // why: teardown errors are intentionally ignored after the probe result;
    // cleanup must not replace the health signal or strand a child process.
    let _ = crate::runtime::terminate_owned_process_tree(&mut child, &mut process_guard);
    let _ = child.wait();
    if let Some(reader) = reader {
        // why: the bounded reader has no useful result after the probe; join it
        // so its thread cannot outlive the doctor command.
        let _ = reader.join();
    }
    responded
}

/// Report the health of the `keel` MCP registration in `~/.claude.json`.
///
/// Two failure modes matter and look identical from inside a session — the
/// server's tools (`recall`, `system_map`, `run_command`, `recall_status`, the
/// `skill_*`/`brief_*` pair, `memory_status`, `system_map_refresh`) appear absent:
///
/// 1. **No entry at all** — the server was never registered, so the tools do not
///    exist for the harness.
/// 2. **Entry present but `alwaysLoad` missing/false** — the tools ARE registered
///    but the harness *defers* them behind `ToolSearch` (forced on whenever tool
///    search is enabled or `ANTHROPIC_BASE_URL` points at a non-first-party
///    gateway). A model that searches for them by bare name (`select:recall`)
///    finds nothing and wrongly concludes "MCP not registered". `alwaysLoad: true`
///    pins them into context so they are always available. See
///    `mcp_register::mcp_server_entry` for the authoritative rationale.
///
/// Both are repaired by `keel repair` (re-runs `register_mcp_server`,
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
                "keel MCP server registered in ~/.claude.json \
                 (run `keel repair` to register it)",
            );
        }
        Some(entry) => {
            write_doctor_check(standard_output, true, "keel MCP server registered");
            let always_load = entry
                .get("alwaysLoad")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            write_doctor_check(
                standard_output,
                always_load,
                if always_load {
                    "keel MCP tools pinned into context (alwaysLoad)"
                } else {
                    "keel MCP tools pinned into context (alwaysLoad missing — \
                     tools are deferred behind ToolSearch; run `keel repair`)"
                },
            );
        }
    }
}

/// Report bridge-host wiring health. Unlike the native host probes above,
/// these are file-presence checks
/// — doctor reads the installed artifacts and reports which are present/absent
/// so an operator can see, at a glance, which bridge hosts are wired. Read-only.
/// `keel_home` is the host-neutral root; host files live under the user home.
pub(crate) fn report_bridge_host_wiring(
    standard_output: &mut dyn Write,
    keel_home: &std::path::Path,
) {
    let _ = writeln!(standard_output, "Bridge hosts:");
    let home = match crate::manager::install::host_user_home(keel_home) {
        Some(path) => path,
        None => return,
    };
    let shared_gateway = home
        .join(".agents")
        .join("skills")
        .join("using-keel")
        .join("SKILL.md");
    write_doctor_check(
        standard_output,
        shared_gateway.is_file(),
        &format!(
            "shared gateway skill: {} ({})",
            if shared_gateway.is_file() {
                "installed"
            } else {
                "missing - run `keel install` so Codex/OpenCode can discover Keel"
            },
            display_path(&shared_gateway)
        ),
    );

    // OpenCode: plugin file + mcp.keel entry in opencode.json.
    let opencode_plugin = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("keel.ts");
    let opencode_json = home.join(".config").join("opencode").join("opencode.json");
    let opencode_core = home
        .join(".config")
        .join("opencode")
        .join("_shared")
        .join("ts")
        .join("bridge-core.ts");
    let opencode_mcp = if opencode_json.is_file() {
        fs::read_to_string(&opencode_json)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map(|doc| doc.get("mcp").and_then(|m| m.get("keel")).is_some())
            .unwrap_or(false)
    } else {
        false
    };
    report_host(
        standard_output,
        "opencode",
        opencode_plugin.is_file() && opencode_core.is_file(),
        opencode_mcp,
    );

    // Pi: AGENTS.md + mcp.json keel entry + keel-pi.ts extension.
    let pi_agents = home.join(".pi").join("agent").join("AGENTS.md");
    let pi_mcp_json = home.join(".pi").join("agent").join("mcp.json");
    let pi_mcp = if pi_mcp_json.is_file() {
        fs::read_to_string(&pi_mcp_json)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map(|doc| doc.get("mcpServers").and_then(|s| s.get("keel")).is_some())
            .unwrap_or(false)
    } else {
        false
    };
    let pi_ext = home
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join("keel-pi.ts");
    let pi_core = home
        .join(".pi")
        .join("agent")
        .join("_shared")
        .join("ts")
        .join("bridge-core.ts");
    report_host(
        standard_output,
        "pi",
        pi_agents.is_file() && pi_ext.is_file() && pi_core.is_file(),
        pi_mcp,
    );

    let commandcode_root = home.join(".commandcode");
    if commandcode_root.is_dir() {
        let commandcode_mcp = fs::read_to_string(commandcode_root.join("mcp.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|doc| {
                doc.get("mcpServers")
                    .and_then(|servers| servers.get("keel"))
                    .is_some()
            });
        let commandcode_wired = commandcode_root.join("mods").join("keel-cmdc.ts").is_file()
            && commandcode_root
                .join("_shared")
                .join("ts")
                .join("bridge-core.ts")
                .is_file();
        report_host(
            standard_output,
            "commandcode",
            commandcode_wired,
            commandcode_mcp,
        );
    }

    // Codex: plugin dir with manifest + bundled .mcp.json (plugin MCP server).
    // File presence alone is NOT enough: Codex discovers plugins through the
    // marketplace manifest and loads only plugins enabled in config.toml.
    let codex_plugin = home
        .join(".codex")
        .join("plugins")
        .join("keel")
        .join(".codex-plugin")
        .join("plugin.json");
    let codex_marketplace_registered = fs::read_to_string(
        home.join(".agents")
            .join("plugins")
            .join("marketplace.json"),
    )
    .ok()
    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    .and_then(|doc| doc.get("plugins").and_then(|p| p.as_array()).cloned())
    .map(|entries| {
        entries.iter().any(|entry| {
            entry.get("name").and_then(|n| n.as_str()) == Some("keel")
                && entry
                    .get("source")
                    .and_then(|source| source.get("source"))
                    .and_then(|value| value.as_str())
                    == Some("local")
                && entry
                    .get("source")
                    .and_then(|source| source.get("path"))
                    .and_then(|value| value.as_str())
                    == Some("./.codex/plugins/keel")
        })
    })
    .unwrap_or(false);
    let codex_config_text = fs::read_to_string(home.join(".codex").join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
    let codex_enabled = codex_config_text
        .as_ref()
        .and_then(|doc| doc.get("plugins"))
        .and_then(|p| p.get("keel@personal-keel"))
        .and_then(|entry| entry.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // Native MCP registration: this config.toml entry is what actually makes
    // the tools reachable on Windows, where plugin-bundled MCP never loads.
    let codex_native_mcp = codex_config_text.as_ref().is_some_and(|document| {
        native_mcp_is_current(document, &installed_executable_path(keel_home))
    });
    let codex_agents_md = fs::read_to_string(home.join(".codex").join("AGENTS.md"))
        .map(|text| text.contains("keel:begin"))
        .unwrap_or(false);
    let codex_installation = codex_plugin_installation(&home);
    let codex_installed = codex_installation == CodexPluginInstallation::Current;
    report_host(
        standard_output,
        "codex",
        codex_installed && codex_agents_md,
        codex_native_mcp,
    );
    write_doctor_check(
        standard_output,
        codex_installed || !codex_plugin.is_file(),
        &format!(
            "codex plugin installation: {}",
            match codex_installation {
                CodexPluginInstallation::Current => "installed and current",
                CodexPluginInstallation::Stale => "stale - run `keel install` to refresh",
                CodexPluginInstallation::Missing if codex_plugin.is_file() => {
                    "not installed - run `keel install`"
                }
                CodexPluginInstallation::Missing => "n/a - source plugin not installed",
            }
        ),
    );
    write_doctor_check(
        standard_output,
        codex_marketplace_registered || !codex_plugin.is_file(),
        &format!(
            "codex marketplace entry ({}): {}",
            display_path(
                &home
                    .join(".agents")
                    .join("plugins")
                    .join("marketplace.json")
            ),
            if codex_marketplace_registered {
                "registered"
            } else if codex_plugin.is_file() {
                "missing - run `keel install` to register"
            } else {
                "n/a - plugin not installed"
            }
        ),
    );
    write_doctor_check(
        standard_output,
        codex_enabled || !codex_plugin.is_file(),
        &format!(
            "codex plugin enablement (config.toml [plugins.\"keel@personal-keel\"]): {}",
            if codex_enabled {
                "enabled"
            } else if codex_plugin.is_file() {
                "not enabled - run `keel install` or enable via /plugins"
            } else {
                "n/a - plugin not installed"
            }
        ),
    );
    write_doctor_check(
        standard_output,
        codex_native_mcp || !codex_plugin.is_file(),
        &format!(
            "codex native MCP (config.toml [mcp_servers.keel]): {}",
            if codex_native_mcp {
                "registered"
            } else if codex_plugin.is_file() {
                "missing - run `keel install` to register (required on Windows)"
            } else {
                "n/a - plugin not installed"
            }
        ),
    );
    write_doctor_check(
        standard_output,
        codex_agents_md || !codex_plugin.is_file(),
        &format!(
            "codex AGENTS.md iron law contract: {}",
            if codex_agents_md {
                "present"
            } else if codex_plugin.is_file() {
                "missing - run `keel install` to write it"
            } else {
                "n/a - plugin not installed"
            }
        ),
    );

    // Cursor: .cursorrules + hooks.json + hook script + mcp.json keel entry.
    let cursor_rules = home.join(".cursorrules");
    let cursor_hooks = home.join(".cursor").join("hooks.json");
    let cursor_script = home.join(".cursor").join("hooks").join("keel-cursor.sh");
    let cursor_mcp_json = home.join(".cursor").join("mcp.json");
    let cursor_mcp = if cursor_mcp_json.is_file() {
        fs::read_to_string(&cursor_mcp_json)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map(|doc| doc.get("mcpServers").and_then(|s| s.get("keel")).is_some())
            .unwrap_or(false)
    } else {
        false
    };
    report_host(
        standard_output,
        "cursor",
        cursor_rules.is_file() && cursor_hooks.is_file() && cursor_script.is_file(),
        cursor_mcp,
    );

    let grok_dir = crate::manager::install::grok_config_home(&home);
    if grok_dir.is_dir() {
        let grok_hooks = grok_dir.join("hooks").join("keel.json");
        let expected_executable = installed_executable_path(keel_home);
        let grok_mcp = fs::read_to_string(grok_dir.join("config.toml"))
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .is_some_and(|document| native_mcp_is_current(&document, &expected_executable));
        let grok_wired = crate::manager::install::grok_hooks_are_effective(
            &grok_dir,
            keel_home,
            &expected_executable,
        ) && grok_mcp;
        let state = if grok_wired {
            if grok_hooks.exists() {
                "wired (native hooks + MCP)"
            } else {
                "wired (Claude-compatible hooks + MCP, no duplicates)"
            }
        } else {
            "not wired (run `keel install --with grok` to repair hooks + MCP)"
        };
        write_doctor_check(standard_output, grok_wired, &format!("grok host: {state}"));
    }

    let omp_root = home.join(".omp").join("agent");
    if omp_root.is_dir() {
        let omp_mcp = fs::read_to_string(omp_root.join("mcp.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|doc| {
                doc.get("mcpServers")
                    .and_then(|servers| servers.get("keel"))
                    .is_some()
            });
        let omp_wired = omp_root.join("AGENTS.md").is_file()
            && omp_root.join("extensions").join("keel-pi.ts").is_file()
            && omp_root
                .join("_shared")
                .join("ts")
                .join("bridge-core.ts")
                .is_file()
            && omp_root
                .join("skills")
                .join("using-keel")
                .join("SKILL.md")
                .is_file();
        report_host(standard_output, "omp", omp_wired, omp_mcp);
    }

    let zcode_root = home.join(".zcode");
    if zcode_root.is_dir() {
        let zcode_config = fs::read_to_string(zcode_root.join("cli").join("config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let zcode_mcp = zcode_config.as_ref().is_some_and(|doc| {
            doc.get("mcp")
                .and_then(|mcp| mcp.get("servers"))
                .and_then(|servers| servers.get("keel"))
                .is_some()
        });
        let zcode_hooks = zcode_config.as_ref().is_some_and(|doc| {
            doc.get("hooks")
                .and_then(|hooks| hooks.get("enabled"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
                && doc.to_string().contains("Keel managed lifecycle hook")
        });
        let zcode_wired = zcode_hooks
            && fs::read_to_string(zcode_root.join("AGENTS.md"))
                .map(|text| text.contains("keel:begin"))
                .unwrap_or(false)
            && zcode_root
                .join("skills")
                .join("using-keel")
                .join("SKILL.md")
                .is_file();
        report_host(standard_output, "zcode", zcode_wired, zcode_mcp);
    }

    let antigravity_node = which::which("node").is_ok();
    let mut antigravity_present = false;
    for (name, antigravity_root, antigravity) in [
        (
            "antigravity",
            home.join(".gemini").join("config"),
            home.join(".gemini")
                .join("config")
                .join("plugins")
                .join("keel"),
        ),
        (
            "antigravity-cli",
            home.join(".gemini").join("antigravity-cli"),
            home.join(".gemini")
                .join("antigravity-cli")
                .join("plugins")
                .join("keel"),
        ),
    ] {
        if !antigravity.is_dir() {
            continue;
        }
        antigravity_present = true;
        let antigravity_mcp = fs::read_to_string(antigravity_root.join("mcp_config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|doc| {
                doc.get("mcpServers")
                    .and_then(|servers| servers.get("keel"))
                    .is_some()
            });
        let antigravity_wired = antigravity_node
            && antigravity.join("plugin.json").is_file()
            && antigravity.join("hooks.json").is_file()
            && antigravity.join("keel-antigravity.js").is_file()
            && antigravity.join("rules").join("keel.md").is_file()
            && antigravity
                .join("skills")
                .join("using-keel")
                .join("SKILL.md")
                .is_file();
        report_host(
            standard_output,
            name,
            antigravity_wired && antigravity_mcp,
            antigravity_mcp,
        );
    }
    if antigravity_present {
        write_doctor_check(
            standard_output,
            antigravity_node,
            if antigravity_node {
                "antigravity hook runtime (node): available"
            } else {
                "antigravity hook runtime (node): missing from PATH"
            },
        );
    }
}

pub(crate) fn native_mcp_is_current(document: &toml::Value, executable: &std::path::Path) -> bool {
    let Some(entry) = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("keel"))
    else {
        return false;
    };
    let command_matches = entry.get("command").and_then(toml::Value::as_str)
        == Some(display_path(executable).as_str());
    let args: Vec<&str> = entry
        .get("args")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect();
    command_matches && args == ["mcp", "serve"]
}

/// One summary line per bridge host: name + wired state + MCP state.
/// `wired` is the host's primary artifact presence; `mcp` is whether an MCP
/// entry was registered (all four bridge hosts now register one).
fn report_host(standard_output: &mut dyn Write, name: &str, wired: bool, mcp: bool) {
    let state = if wired {
        if mcp {
            "wired (rules + MCP)".to_string()
        } else {
            "wired (rules)".to_string()
        }
    } else {
        format!("not wired (opt in with `keel install --with {name}`)")
    };
    // why: render [ok] only when the host is actually wired. Passing `true`
    // unconditionally painted an unconfigured — or failed-to-wire — host as healthy;
    // a not-wired host is now a [warn] so doctor tells the truth about host state.
    write_doctor_check(standard_output, wired, &format!("{name} host: {state}"));
}

/// True when `directory` is one of the entries on the current PATH.
/// Case-insensitive on Windows (PATH entries ignore case there).
fn path_contains_dir(directory: &std::path::Path) -> bool {
    let Some(path_value) = std::env::var_os("PATH") else {
        return false;
    };
    for entry in std::env::split_paths(&path_value) {
        if cfg!(windows) {
            if entry.to_string_lossy().to_lowercase() == directory.to_string_lossy().to_lowercase()
            {
                return true;
            }
        } else if entry == directory {
            return true;
        }
    }
    false
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
                "keel-doctor-{label}-{}-{nanos}",
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
    fn parse_pid_ppid_listing_reads_pairs_and_skips_noise() {
        let pairs = parse_pid_ppid_listing(Some(
            "1234 100\n  5678   200 \nnot-a-pid x\n\n90\n".to_string(),
        ));
        // "90" has no parent token -> ppid defaults to 0 (unknown -> treated alive).
        assert_eq!(pairs, vec![(1234, 100), (5678, 200), (90, 0)]);
        assert!(parse_pid_ppid_listing(None).is_empty());
        assert!(parse_pid_ppid_listing(Some(String::new())).is_empty());
    }

    #[test]
    fn orphan_scan_never_includes_own_pid() {
        // why: the reaper must never target the calling process; its own PID is
        // filtered out by construction regardless of the live process table.
        let own = std::process::id();
        let filtered: Vec<u32> = vec![own, 999_999]
            .into_iter()
            .filter(|p| *p != own)
            .collect();
        assert!(!filtered.contains(&own));
    }

    #[test]
    fn unknown_parent_is_treated_as_alive_never_orphaned() {
        // why: inconclusive parent evidence must resolve to alive, or a false
        // orphan gets a healthy live server killed.
        assert!(parent_is_alive(0));
    }

    #[cfg(not(windows))]
    #[test]
    fn reparented_to_init_is_orphaned_on_unix() {
        // why: on Unix a process whose harness parent died is reparented to
        // init/launchd (ppid 1), the only unambiguous orphan signal.
        assert!(!parent_is_alive(1));
    }

    #[test]
    fn orphan_detection_keeps_live_and_unknown_parents() {
        // why: the filter must drop the caller's own PID and any live-or-unknown
        // parent, flagging only confirmed-dead parents.
        let own = std::process::id();
        let table: Vec<(u32, u32)> = vec![
            (own, 0),  // Own PID: excluded regardless of parent.
            (5000, 0), // Unknown parent -> alive -> keep out.
        ];
        let orphans: Vec<u32> = table
            .into_iter()
            .filter(|(pid, _)| *pid != own)
            .filter(|(_, ppid)| !parent_is_alive(*ppid))
            .map(|(pid, _)| pid)
            .collect();
        assert!(orphans.is_empty(), "no live/unknown parent may be orphaned");
    }

    #[test]
    fn mcp_probe_timeout_reaps_descendant_held_pipes() {
        let (program, arguments) = if cfg!(windows) {
            (
                "powershell",
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "$p=Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -NoNewWindow -PassThru; exit 0"
                        .to_string(),
                ],
            )
        } else {
            (
                "sh",
                vec!["-c".to_string(), "sleep 30 & exit 0".to_string()],
            )
        };
        let started = std::time::Instant::now();
        assert!(!probe_mcp_initialize_with_timeout(
            program,
            &arguments,
            std::time::Duration::from_millis(100),
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn reports_ok_when_entry_has_always_load() {
        let claude_home = unique_home("ok");
        let config = super::super::mcp_register::mcp_config_path(&claude_home);
        // The exact shape register_mcp_server writes.
        fs::write(
            &config,
            r#"{"mcpServers":{"keel":{"type":"stdio","command":"x","args":["mcp","serve"],"env":{},"alwaysLoad":true}}}"#,
        )
        .unwrap();
        let report = run_report(&claude_home);
        assert!(
            report.contains("[ok] keel MCP server registered"),
            "{report}"
        );
        assert!(
            report.contains("[ok] keel MCP tools pinned into context"),
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
            r#"{"mcpServers":{"keel":{"type":"stdio","command":"x","args":["mcp","serve"],"env":{}}}}"#,
        )
        .unwrap();
        let report = run_report(&claude_home);
        // Server is registered...
        assert!(
            report.contains("[ok] keel MCP server registered"),
            "{report}"
        );
        // ...but the alwaysLoad line must WARN and point at repair.
        assert!(
            report.contains("[warn] keel MCP tools pinned into context"),
            "{report}"
        );
        assert!(report.contains("keel repair"), "{report}");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn warns_when_no_entry() {
        let claude_home = unique_home("noentry");
        let config = super::super::mcp_register::mcp_config_path(&claude_home);
        fs::write(&config, r#"{"mcpServers":{}}"#).unwrap();
        let report = run_report(&claude_home);
        assert!(
            report.contains("[warn] keel MCP server registered"),
            "{report}"
        );
        assert!(report.contains("keel repair"), "{report}");
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn warns_when_config_absent() {
        // No ~/.claude.json at all -> treated as "no entry", warns.
        let claude_home = unique_home("noconfig");
        let report = run_report(&claude_home);
        assert!(
            report.contains("[warn] keel MCP server registered"),
            "{report}"
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }
    #[test]
    fn codex_bridge_status_rejects_source_only_plugin_as_uninstalled() {
        let claude_home = unique_home("codex-config");
        let home = claude_home.parent().unwrap();
        let config_path = home.join(".codex").join("config.toml");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            format!(
                "[plugins.\"keel@personal-keel\"]\nenabled = true\n\n[mcp_servers.keel]\ncommand = {:?}\nargs = [\"mcp\", \"serve\"]\n",
                display_path(&installed_executable_path(&claude_home))
            ),
        )
        .unwrap();
        let plugin_root = home.join(".codex").join("plugins").join("keel");
        fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        fs::write(
            plugin_root.join(".codex-plugin").join("plugin.json"),
            r#"{"name":"keel","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(plugin_root.join(".mcp.json"), "{}").unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[warn] codex host: not wired"), "{output}");
        assert!(
            output.contains("[warn] codex plugin installation: not installed"),
            "{output}"
        );
        assert!(output.contains(
            "[ok] codex plugin enablement (config.toml [plugins.\"keel@personal-keel\"]): enabled"
        ));
        assert!(
            output.contains("[ok] codex native MCP (config.toml [mcp_servers.keel]): registered")
        );
        let _ = fs::remove_dir_all(claude_home.parent().unwrap());
    }

    #[test]
    fn opencode_host_warns_when_imported_bridge_core_is_missing() {
        let claude_home = unique_home("opencode-missing-core");
        let home = claude_home.parent().unwrap();
        let plugin = home.join(".config").join("opencode").join("plugins");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(
            plugin.join("keel.ts"),
            "import '../_shared/ts/bridge-core';",
        )
        .unwrap();
        fs::write(
            home.join(".config").join("opencode").join("opencode.json"),
            r#"{"mcp":{"keel":{}}}"#,
        )
        .unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("[warn] opencode host: not wired"),
            "{output}"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn requested_new_hosts_have_truthful_doctor_rows() {
        let claude_home = unique_home("new-host-status");
        let home = claude_home.parent().unwrap();

        let gateway = home.join(".agents").join("skills").join("using-keel");
        fs::create_dir_all(&gateway).unwrap();
        fs::write(gateway.join("SKILL.md"), "keel").unwrap();

        let omp = home.join(".omp").join("agent");
        fs::create_dir_all(omp.join("extensions")).unwrap();
        fs::create_dir_all(omp.join("_shared").join("ts")).unwrap();
        fs::write(omp.join("extensions").join("keel-pi.ts"), "keel").unwrap();
        fs::write(
            omp.join("_shared").join("ts").join("bridge-core.ts"),
            "keel",
        )
        .unwrap();
        fs::write(omp.join("AGENTS.md"), "keel:begin").unwrap();
        fs::write(omp.join("mcp.json"), r#"{"mcpServers":{"keel":{}}}"#).unwrap();
        fs::create_dir_all(omp.join("skills").join("using-keel")).unwrap();
        fs::write(
            omp.join("skills").join("using-keel").join("SKILL.md"),
            "keel",
        )
        .unwrap();

        let zcode_config = home.join(".zcode").join("cli").join("config.json");
        fs::create_dir_all(zcode_config.parent().unwrap()).unwrap();
        fs::write(
            zcode_config,
            r#"{"mcp":{"servers":{"keel":{}}},"hooks":{"enabled":true,"events":{"PreToolUse":[{"hooks":[{"statusMessage":"Keel managed lifecycle hook"}]}]}}}"#,
        )
        .unwrap();
        fs::write(home.join(".zcode").join("AGENTS.md"), "keel:begin").unwrap();
        fs::create_dir_all(home.join(".zcode").join("skills").join("using-keel")).unwrap();
        fs::write(
            home.join(".zcode")
                .join("skills")
                .join("using-keel")
                .join("SKILL.md"),
            "keel",
        )
        .unwrap();

        let antigravity = home
            .join(".gemini")
            .join("config")
            .join("plugins")
            .join("keel");
        fs::create_dir_all(&antigravity).unwrap();
        fs::write(antigravity.join("plugin.json"), r#"{"name":"keel"}"#).unwrap();
        fs::write(
            antigravity.join("mcp_config.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();
        fs::write(
            home.join(".gemini").join("config").join("mcp_config.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();
        fs::write(antigravity.join("hooks.json"), r#"{"keel":{}}"#).unwrap();
        fs::write(antigravity.join("keel-antigravity.js"), "keel").unwrap();
        fs::create_dir_all(antigravity.join("rules")).unwrap();
        fs::create_dir_all(antigravity.join("skills").join("using-keel")).unwrap();
        fs::write(antigravity.join("rules").join("keel.md"), "keel").unwrap();
        fs::write(
            antigravity
                .join("skills")
                .join("using-keel")
                .join("SKILL.md"),
            "keel",
        )
        .unwrap();

        let antigravity_cli = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("plugins")
            .join("keel");
        fs::create_dir_all(antigravity_cli.join("rules")).unwrap();
        fs::create_dir_all(antigravity_cli.join("skills").join("using-keel")).unwrap();
        fs::write(antigravity_cli.join("plugin.json"), r#"{"name":"keel"}"#).unwrap();
        fs::write(
            antigravity_cli.join("mcp_config.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();
        fs::write(
            home.join(".gemini")
                .join("antigravity-cli")
                .join("mcp_config.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();
        fs::write(antigravity_cli.join("hooks.json"), r#"{"keel":{}}"#).unwrap();
        fs::write(antigravity_cli.join("keel-antigravity.js"), "keel").unwrap();
        fs::write(antigravity_cli.join("rules").join("keel.md"), "keel").unwrap();
        fs::write(
            antigravity_cli
                .join("skills")
                .join("using-keel")
                .join("SKILL.md"),
            "keel",
        )
        .unwrap();

        let commandcode = home.join(".commandcode");
        fs::create_dir_all(commandcode.join("mods")).unwrap();
        fs::create_dir_all(commandcode.join("_shared").join("ts")).unwrap();
        fs::write(commandcode.join("mods").join("keel-cmdc.ts"), "keel").unwrap();
        fs::write(
            commandcode
                .join("_shared")
                .join("ts")
                .join("bridge-core.ts"),
            "keel",
        )
        .unwrap();
        fs::write(
            commandcode.join("mcp.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[ok] shared gateway skill:"), "{output}");
        assert!(output.contains("[ok] commandcode host:"), "{output}");
        assert!(output.contains("[ok] omp host:"), "{output}");
        assert!(output.contains("[ok] zcode host:"), "{output}");
        assert!(output.contains("[ok] antigravity host:"), "{output}");
        assert!(output.contains("[ok] antigravity-cli host:"), "{output}");
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn antigravity_host_warns_when_plugin_mcp_exists_but_global_mcp_is_missing() {
        let claude_home = unique_home("antigravity-missing-global-mcp");
        let home = claude_home.parent().unwrap();
        let plugin = home
            .join(".gemini")
            .join("config")
            .join("plugins")
            .join("keel");
        fs::create_dir_all(plugin.join("rules")).unwrap();
        fs::create_dir_all(plugin.join("skills").join("using-keel")).unwrap();
        fs::write(plugin.join("plugin.json"), r#"{"name":"keel"}"#).unwrap();
        fs::write(
            plugin.join("mcp_config.json"),
            r#"{"mcpServers":{"keel":{}}}"#,
        )
        .unwrap();
        fs::write(plugin.join("hooks.json"), r#"{"keel":{}}"#).unwrap();
        fs::write(plugin.join("keel-antigravity.js"), "keel").unwrap();
        fs::write(plugin.join("rules").join("keel.md"), "keel").unwrap();
        fs::write(
            plugin.join("skills").join("using-keel").join("SKILL.md"),
            "keel",
        )
        .unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("[warn] antigravity host: not wired"),
            "doctor must not report a plugin-only MCP registration as active: {output}"
        );
        let _ = fs::remove_dir_all(home);
    }
    #[test]
    fn grok_host_reports_wired_when_hooks_and_mcp_are_current() {
        let claude_home = unique_home("grok-wired");
        let home = claude_home.parent().unwrap();
        let grok_hooks = home.join(".grok").join("hooks");
        fs::create_dir_all(&grok_hooks).unwrap();
        fs::write(
            grok_hooks.join("keel.json"),
            serde_json::to_string_pretty(&crate::manager::install::grok_hooks_payload(
                &installed_executable_path(&claude_home),
            ))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            home.join(".grok").join("config.toml"),
            format!(
                "[mcp_servers.keel]\ncommand = {:?}\nargs = [\"mcp\", \"serve\"]\n",
                display_path(&installed_executable_path(&claude_home))
            ),
        )
        .unwrap();
        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("[ok] grok host: wired (native hooks + MCP)"),
            "{output}"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn grok_host_reuses_claude_compatible_hooks_without_native_duplicate() {
        let claude_home = unique_home("grok-claude-compatible");
        let home = claude_home.parent().unwrap();
        let executable = installed_executable_path(&claude_home);
        fs::create_dir_all(home.join(".grok")).unwrap();
        fs::write(
            home.join(".grok").join("config.toml"),
            format!(
                "[mcp_servers.keel]\ncommand = {:?}\nargs = [\"mcp\", \"serve\"]\n",
                display_path(&executable)
            ),
        )
        .unwrap();
        let settings = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        fs::create_dir_all(&claude_home).unwrap();
        fs::write(
            &settings,
            crate::runner::hook_lifecycle::build_hooks_payload(&settings, &executable).unwrap(),
        )
        .unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("[ok] grok host: wired (Claude-compatible hooks + MCP, no duplicates)"),
            "{output}"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn grok_host_warns_when_native_and_claude_compatible_hooks_would_duplicate() {
        let claude_home = unique_home("grok-duplicate-hooks");
        let home = claude_home.parent().unwrap();
        let executable = installed_executable_path(&claude_home);
        let grok_hooks = home.join(".grok").join("hooks");
        fs::create_dir_all(&grok_hooks).unwrap();
        fs::write(
            grok_hooks.join("keel.json"),
            serde_json::to_string_pretty(&crate::manager::install::grok_hooks_payload(&executable))
                .unwrap(),
        )
        .unwrap();
        fs::write(
            home.join(".grok").join("config.toml"),
            format!(
                "[mcp_servers.keel]\ncommand = {:?}\nargs = [\"mcp\", \"serve\"]\n",
                display_path(&executable)
            ),
        )
        .unwrap();
        let settings = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
        fs::create_dir_all(&claude_home).unwrap();
        fs::write(
            &settings,
            crate::runner::hook_lifecycle::build_hooks_payload(&settings, &executable).unwrap(),
        )
        .unwrap();

        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[warn] grok host: not wired"), "{output}");
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn grok_host_warns_when_hook_contract_is_stale() {
        let claude_home = unique_home("grok-stale-hook");
        let home = claude_home.parent().unwrap();
        let grok_hooks = home.join(".grok").join("hooks");
        fs::create_dir_all(&grok_hooks).unwrap();
        fs::write(grok_hooks.join("keel.json"), "{\"hooks\":{}}").unwrap();
        fs::write(
            home.join(".grok").join("config.toml"),
            format!(
                "[mcp_servers.keel]\ncommand = {:?}\nargs = [\"mcp\", \"serve\"]\n",
                display_path(&installed_executable_path(&claude_home))
            ),
        )
        .unwrap();
        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[warn] grok host: not wired"), "{output}");
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn grok_host_warns_when_detected_but_unwired() {
        let claude_home = unique_home("grok-unwired");
        let home = claude_home.parent().unwrap();
        fs::create_dir_all(home.join(".grok")).unwrap();
        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("[warn] grok host: not wired"), "{output}");
        assert!(output.contains("keel install"), "{output}");
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn grok_host_omitted_when_not_detected() {
        let claude_home = unique_home("grok-absent");
        let home = claude_home.parent().unwrap();
        let mut output = Vec::new();
        report_bridge_host_wiring(&mut output, &claude_home);
        let output = String::from_utf8(output).unwrap();
        assert!(
            !output.contains("grok host:"),
            "undetected Grok must not appear, {output}"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn claude_home_flag_reaches_doctor_checks_not_just_status_summary() {
        // Doctor checks must use the custom home, not the default home.
        // This test ensures every probe honors `--claude-home`.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let custom_home = std::env::temp_dir().join(format!(
            "keel-doctor-flaghome-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&custom_home).expect("create custom home");
        // A non-`.keel` root doubles as its own engagement home, so
        // settings.json sits directly inside it (claude_engagement_home).
        fs::write(
            custom_home.join(crate::hooks::claude::SETTINGS_FILE_NAME),
            b"{}",
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_doctor_command(
            "test",
            &[
                "--claude-home".to_string(),
                custom_home.to_string_lossy().to_string(),
            ],
            &mut out,
            &mut err,
        );
        let output = String::from_utf8(out).expect("utf8");
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        let expected_label = crate::runtime::display_path(&custom_home);
        assert!(
            output.contains(&format!("[ok] keel home exists: {expected_label}")),
            "home check must target the --claude-home value; output: {output}"
        );
        assert!(
            output.contains("[ok] ~/.claude/settings.json exists"),
            "settings.json check must target the --claude-home value; output: {output}"
        );
        let _ = fs::remove_dir_all(&custom_home);
    }
}
