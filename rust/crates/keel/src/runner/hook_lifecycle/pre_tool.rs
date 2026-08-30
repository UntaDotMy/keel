//! Hook lifecycle pre_tool responsibility split.

use super::*;

pub(super) const IRON_LAW_GATE_ENV_VAR: &str = "KEEL_IRON_LAW_GATE";

/// Shared satisfaction marker dir used by Claude PreToolUse, bridge hosts, and
/// PostToolUse/observe (one source of truth across hosts).
pub(super) const IRON_LAW_SATISFIED_DIR: &str = "iron-law-satisfied";

/// Legacy one-shot acknowledge dir from the old "deny once then always allow"
/// gate. Still checked for back-compat so in-flight sessions mid-upgrade are not
/// re-blocked after they already cleared the old gate.
pub(super) const IRON_LAW_LEGACY_GATE_DIR: &str = "iron-law-gate";

pub(super) const IRON_LAW_GATE_DENIAL_STRICT: &str =
    "[keel] Iron Law gate (STRICT): Edit/Write/Bash (non-keel) and Agent/Task are \
        BLOCKED until this session used a keel research tool. Text reminders are not \
        enough — this is a hard deny.\n\
        Do ONE of these, then retry:\n\
        1. MCP `context_brief` or `system_map` (or `keel memory system-map` / `keel doctor`)\n\
        2. MCP `recall` or `skill_route` / `skill_get` (or `keel memory recall`)\n\
        3. MCP `code_search` (or `keel code-search search ...`)\n\
        Allowed while blocked: Read/Grep/Glob, and shell only if the command is a \
        keel research command. Plain Read alone does NOT clear STRICT. \
        Set KEEL_IRON_LAW_GATE=balanced or =off to relax.";

pub(super) const IRON_LAW_GATE_DENIAL_BALANCED: &str =
    "[keel] Iron Law gate: Edit/Write/Bash (non-keel) and Agent/Task are blocked \
        until this session has research evidence. Prefer keel tools first:\n\
        1. MCP `context_brief` / `system_map` / `recall` / `skill_route` (or CLI).\n\
        2. Or host Read/Grep/Glob of the owning file.\n\
        Retry after researching. Set KEEL_IRON_LAW_GATE=off to disable.";

pub(super) const IRON_LAW_GATE_DENIAL_VERIFIED: &str =
    "[keel] Iron Law gate (VERIFIED): Edit/Write/Bash (non-keel) and Agent/Task are \
        BLOCKED until this session did FRESH external research. Do not trust the \
        codebase, memory, or the model's own knowledge — verify against the live \
        source first.\n\
        Do ONE of these, then retry:\n\
        1. WebSearch for the current official docs/behavior\n\
        2. WebFetch the authoritative source page\n\
        3. The context7 MCP for up-to-date library docs\n\
        Recall/memory/keel reads do NOT clear VERIFIED — they are internal state, \
        not verification. Read/Grep/Glob stay allowed. Set KEEL_IRON_LAW_GATE=strict, \
        =balanced, or =off to relax.";

/// Iron-law edit-gate mode. Default is **Strict** (keel tool required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub(crate) enum IronLawGateMode {
    /// Disabled entirely.
    Off,
    /// Require keel MCP/CLI research evidence this session.
    Strict,
    /// Require any research evidence (keel tools OR host Read/Grep/Glob).
    Balanced,
    /// Require FRESH external research this session (WebSearch/WebFetch/context7).
    /// Recall/memory/keel tools do NOT clear it. The operator rule is "do not
    /// trust the codebase, memory, or the model's own knowledge; verify against the
    /// live source before editing." Strictest; still allows Read/Grep/Glob.
    Verified,
}

pub(super) fn iron_law_gate_mode() -> IronLawGateMode {
    match std::env::var(IRON_LAW_GATE_ENV_VAR)
        .ok()
        .as_deref()
        .map(str::trim)
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("off") | Some("0") | Some("false") | Some("no") => IronLawGateMode::Off,
        Some("balanced") | Some("balance") | Some("any") => IronLawGateMode::Balanced,
        Some("verified") | Some("verify") | Some("web") => IronLawGateMode::Verified,
        Some("strict") | Some("on") | Some("true") => IronLawGateMode::Strict,
        _ => IronLawGateMode::Strict,
    }
}

pub(super) fn iron_law_satisfied_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "default".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join(IRON_LAW_SATISFIED_DIR)
        .join(key)
}

pub(super) fn iron_law_legacy_path(claude_home: &Path, session_id: &str) -> PathBuf {
    // Legacy files used the raw session_id (not sanitized). Keep that shape.
    let name = if session_id.trim().is_empty() {
        "default"
    } else {
        session_id
    };
    claude_home
        .join("state")
        .join(IRON_LAW_LEGACY_GATE_DIR)
        .join(name)
}

/// Mark the session as iron-law satisfied (keel research evidence observed).
/// Best-effort: failures are silent so a disk error never wedges a tool hook.
pub(crate) fn mark_iron_law_satisfied(session_id: &str) {
    let Ok(claude_home) = crate::runtime::resolve_claude_home("") else {
        return;
    };
    let path = iron_law_satisfied_path(&claude_home, session_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, "satisfied");
}

/// Whether the session already has a satisfaction marker (or legacy clear).
pub(super) fn iron_law_marker_present(claude_home: &Path, session_id: &str) -> bool {
    iron_law_satisfied_path(claude_home, session_id).exists()
        || iron_law_legacy_path(claude_home, session_id).exists()
}

/// Substrings that identify a keel *research* tool name (MCP or host-neutral).
/// Management/install tools are excluded so installing keel does not clear the gate.
pub(super) fn is_keel_research_tool_name(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    // Namespaced MCP: mcp__keel__system_map, keel__system_map, etc.
    let looks_keel = lower.contains("mcp__keel__")
        || lower.contains("keel__")
        || lower == "keel"
        || lower.starts_with("keel_");
    if !looks_keel {
        return false;
    }
    // Exclude pure management surfaces.
    if lower.contains("install")
        || lower.contains("uninstall")
        || lower.contains("self-replace")
        || lower.contains("self_replace")
        || (lower.contains("repair") && lower.contains("hook"))
    {
        return false;
    }
    // Prefer research-shaped names; also accept generic keel MCP tools that
    // agents use to orient (status, doctor, cli, memory, skill_*, brief_*).
    true
}

/// Host tools that count as research under Balanced mode only.
pub(super) fn is_host_research_tool_name(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "read"
            | "glob"
            | "grep"
            | "search"
            | "semanticsearch"
            | "lsp_diagnostics"
            | "lsp_goto_definition"
            | "lsp_find_references"
            | "lsp_symbols"
            | "lsp_prepare_rename"
            | "websearch"
            | "web_search"
            | "webfetch"
            | "web_fetch"
            | "context7"
    ) || lower.contains("websearch")
        || lower.contains("web_fetch")
        || lower.contains("context7")
}

/// Tools that count as FRESH external research under Verified mode. Only a live
/// lookup against an external source qualifies. Recall/memory/keel reads are
/// internal state the operator rule says not to trust as sole evidence.
pub(super) fn is_web_research_tool_name(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    lower.contains("websearch")
        || lower.contains("web_search")
        || lower.contains("webfetch")
        || lower.contains("web_fetch")
        || lower.contains("context7")
}

/// Shell tools that may carry a `keel ...` research command. Delegates to
/// `shell_rewrite` so the gate and the rewriter read one list, guaranteeing every
/// admitted name has a shell mapping in `rewrite_shell_for_tool`.
pub(super) fn is_shell_tool_name(tool_name: &str) -> bool {
    crate::runner::shell_rewrite::is_shell_tool_name(tool_name)
}

/// Whether a shell command is a keel research/read surface (not install/mutate).
pub(crate) fn is_keel_research_command(command: &str) -> bool {
    let trimmed = command.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return false;
    }
    // Strip common wrappers: keel run -- <cmd>, env prefixes left to contains checks.
    let body = trimmed
        .strip_prefix("keel run -- ")
        .or_else(|| trimmed.strip_prefix("keel.exe run -- "))
        .unwrap_or(trimmed.as_str());
    let has_keel = body.starts_with("keel ")
        || body.starts_with("keel.exe ")
        || body.contains("\\keel.exe ")
        || body.contains("/keel ")
        || body.contains("\\keel ");
    if !has_keel {
        return false;
    }
    // a compound/chained command can smuggle a non-keel tail past the gate
    // (`keel doctor && python exfil.py`); only a standalone keel invocation clears.
    if body.contains("&&")
        || body.contains("||")
        || body.contains(';')
        || body.contains('|')
        || body.contains('`')
        || body.contains("$(")
        || body.contains('\n')
    {
        return false;
    }
    // Research / orientation subcommands that clear the edit gate. Kept in lockstep
    const HITS: &[&str] = &[
        "system-map",
        "system_map",
        "recall",
        "doctor",
        "code-search",
        "code_search",
        "skill-route",
        "skill_route",
        "skill-list",
        "skill_list",
        "skill-get",
        "skill_get",
        "context-brief",
        "context_brief",
        "memory status",
        "memory recall",
        "memory system-map",
        "memory scope",
        "anvil prefix-check",
        "anvil sieve",
    ];
    HITS.iter().any(|h| body.contains(h))
}

pub(super) fn is_host_shell_tool_name(tool_name: &str) -> bool {
    is_shell_tool_name(tool_name) || tool_name.eq_ignore_ascii_case("run_terminal_command")
}

/// True when this tool call is evidence that clears the iron-law gate under `mode`.
pub(crate) fn tool_satisfies_iron_law(
    mode: IronLawGateMode,
    tool_name: &str,
    command: Option<&str>,
) -> bool {
    if mode == IronLawGateMode::Off {
        return false;
    }
    // Verified mode: only a fresh external research tool clears the gate. keel
    // research tools, recall, and host reads are internal state, not verification.
    if mode == IronLawGateMode::Verified {
        return is_web_research_tool_name(tool_name);
    }
    if is_keel_research_tool_name(tool_name) {
        return true;
    }
    if is_host_shell_tool_name(tool_name) {
        if let Some(cmd) = command {
            if is_keel_research_command(cmd) {
                return true;
            }
        }
    }
    if mode == IronLawGateMode::Balanced && is_host_research_tool_name(tool_name) {
        return true;
    }
    false
}

/// Extract a shell command string from a hook tool_input object when present.
pub(super) fn tool_input_command(input: &JsonDocument) -> Option<&str> {
    input
        .get("tool_input")
        .or_else(|| input.get("toolInput"))
        .and_then(|tool_input| tool_input.get("command"))
        .and_then(JsonDocument::as_str)
        .or_else(|| {
            input
                .get("input")
                .and_then(|inner| inner.get("command"))
                .and_then(JsonDocument::as_str)
        })
}

/// If this successful PostToolUse/observe event is keel research evidence, mark the session.
///
/// Anvil completion is deliberately not inferred here. A hook event proves only
/// that a tool was observed, not that the full dry-run pipeline succeeded. The
/// Anvil implementation records its own gate marker after successful completion.
pub(crate) fn maybe_mark_iron_law_from_tool_event(input: &JsonDocument) {
    let tool_name = hook_tool_name(input);
    let command = tool_input_command(input);
    let session_id = hook_session_id(input);
    let mode = iron_law_gate_mode();
    if mode == IronLawGateMode::Off {
        return;
    }
    if !tool_satisfies_iron_law(mode, tool_name, command) {
        return;
    }
    mark_iron_law_satisfied(session_id);
}

/// Mark from bridge observe (tool name + optional stdin command JSON / raw).
pub(crate) fn maybe_mark_iron_law_from_parts(
    session_id: &str,
    tool_name: &str,
    command: Option<&str>,
) {
    let mode = iron_law_gate_mode();
    if mode == IronLawGateMode::Off {
        return;
    }
    if tool_satisfies_iron_law(mode, tool_name, command) {
        mark_iron_law_satisfied(session_id);
    }
}

/// Scan today's tool-timings for keel (or balanced host) research tools.
/// Fail-closed for the gate: returns false when timings are missing (no free pass).
pub(super) fn session_has_iron_law_evidence(
    claude_home: &Path,
    session_id: &str,
    mode: IronLawGateMode,
) -> bool {
    if mode == IronLawGateMode::Off {
        return true;
    }
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let Ok(body) = fs::read_to_string(&path) else {
        return false;
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // the day's file holds every session's rows; a line without the id as
        // a substring cannot match, so skip the parse rather than just the compare.
        if !line.contains(session_id) {
            continue;
        }
        let Ok(row) = serde_json::from_str::<JsonDocument>(line) else {
            continue;
        };
        if row.get("session_id").and_then(JsonDocument::as_str) != Some(session_id) {
            continue;
        }
        let tool = row
            .get("tool_name")
            .and_then(JsonDocument::as_str)
            .unwrap_or_default();
        // Timings rows may not carry the shell command; tool name alone is enough
        // for MCP keel tools. Shell keel commands rely on the live marker write.
        if tool_satisfies_iron_law(mode, tool, None) {
            return true;
        }
    }
    false
}

/// Whether this tool call is subject to the iron-law hard gate when the session
/// is not yet satisfied.
///
/// Gated: edit-class tools, shell commands that are **not** keel research, and
/// Agent/Task fan-out. Not gated: Read/Grep/Glob, keel research MCP/CLI, Skill.
pub(crate) fn tool_is_iron_law_gated(tool_name: &str, command: Option<&str>) -> bool {
    if tool_is_anvil_surface(tool_name, command) {
        return false;
    }
    if is_edit_class_tool(tool_name) {
        return true;
    }
    let lower = tool_name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "agent" | "task" | "teammate" | "taskcreate" | "task_create"
    ) {
        return true;
    }
    if is_host_shell_tool_name(tool_name) {
        // Keel research/anvil shell is the path that *clears* the gate ; never block it.
        if let Some(cmd) = command {
            if is_keel_research_command(cmd) || tool_is_anvil_surface(tool_name, Some(cmd)) {
                return false;
            }
        }
        return true;
    }
    false
}

/// Decide whether to deny a gated tool. Returns `Some(reason)` to deny.
///
/// Evidence-based: does **not** write a satisfaction marker on deny. The marker
/// is written only when PostToolUse/observe sees a qualifying research tool.
pub(crate) fn iron_law_gate_decision(session_id: &str) -> Option<&'static str> {
    let mode = iron_law_gate_mode();
    if mode == IronLawGateMode::Off {
        return None;
    }

    let claude_home = match crate::runtime::resolve_claude_home("") {
        Ok(home) => home,
        Err(error) => {
            // without the home dir the code cannot read the research marker; surface
            // the fail-open rather than silently disabling the gate.
            eprintln!(
                "[keel] Iron Law gate could not resolve the claude home directory ({error}); allowing this tool call unverified."
            );
            return None;
        }
    };

    if iron_law_marker_present(&claude_home, session_id) {
        return None;
    }

    // Recover if the marker write failed earlier but timings prove research ran.
    if session_has_iron_law_evidence(&claude_home, session_id, mode) {
        mark_iron_law_satisfied(session_id);
        return None;
    }

    Some(match mode {
        IronLawGateMode::Strict => IRON_LAW_GATE_DENIAL_STRICT,
        IronLawGateMode::Balanced => IRON_LAW_GATE_DENIAL_BALANCED,
        IronLawGateMode::Verified => IRON_LAW_GATE_DENIAL_VERIFIED,
        IronLawGateMode::Off => return None,
    })
}

pub(super) fn run_iron_law_gate(
    input: &JsonDocument,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let session_id = input
        .get("session_id")
        .and_then(JsonDocument::as_str)
        .unwrap_or("default");

    let Some(reason) = iron_law_gate_decision(session_id) else {
        return 0;
    };
    emit_pretool_deny(reason, standard_output, standard_error);
    0
}

pub(super) fn run_hook_pre_tool_use(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let input_text = match std::io::read_to_string(std::io::stdin()) {
        Ok(text) => text,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to read the harness hook input: {error}"
            );

            return 1;
        }
    };

    let input: JsonDocument = match serde_json::from_str(&input_text) {
        Ok(value) => value,

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to decode the harness hook input: {error}"
            );

            return 1;
        }
    };

    let tool_name = hook_tool_name(&input);

    let command = tool_input_command(&input).unwrap_or("");
    let command_opt = if command.is_empty() {
        None
    } else {
        Some(command)
    };
    let session_id = hook_session_id(&input);

    // Hard Iron Law blocks edit tools until research evidence exists.
    if tool_is_iron_law_gated(tool_name, command_opt) {
        if iron_law_gate_decision(session_id).is_some() {
            return run_iron_law_gate(&input, standard_output, standard_error);
        }
        if anvil_gate_enabled()
            && is_edit_class_tool(tool_name)
            && !tool_is_anvil_surface(tool_name, command_opt)
        {
            let claude_home = resolve_claude_home("").ok();
            let cwd = std::env::current_dir()
                .ok()
                .map(|path| display_path(&path))
                .unwrap_or_default();
            let satisfied = claude_home
                .as_ref()
                .map(|home| anvil_satisfied_this_session(home, session_id, &cwd))
                .unwrap_or(false);
            if !satisfied {
                emit_pretool_deny(ANVIL_GATE_DENIAL, standard_output, standard_error);
                return 0;
            }
        }
    }

    // Compaction rewrite only applies to shell tools.
    if !is_shell_tool_name(tool_name) {
        return 0;
    }

    // Inspect EVERY segment of a compound command, not just the first supported
    if let Some(finding) = crate::runner::shell_rewrite::detect_destructive_in_command(command) {
        let reason = match finding.severity {
            crate::runner::shell_rewrite::DestructiveSeverity::Block => format!(
                "[keel] Destructive command blocked: {}. This command is almost certainly unsafe. \
                 Use a safer alternative.",
                finding.pattern
            ),
            crate::runner::shell_rewrite::DestructiveSeverity::Warn => format!(
                "[keel] Destructive command detected: {}. Confirm this is intentional before proceeding.",
                finding.pattern
            ),
        };
        emit_pretool_deny(&reason, standard_output, standard_error);
        return 0;
    }

    // updatedInput.command goes back to the originating tool, and a
    // Bash-shaped prefix is a parse error in PowerShell.
    let rewrite = rewrite_command_text_for_shell(command, rewrite_shell_for_tool(tool_name));

    if !rewrite.supported {
        return 0;
    }

    let payload = serde_json::json!({

        "hookSpecificOutput": {

            "hookEventName": MANAGED_PRE_TOOL_USE_EVENT,

            "permissionDecision": "allow",

            "updatedInput": {

                "command": rewrite.rewritten_command,

            },

            "allowRules": [
                // a rule is ToolName(pattern), so Bash(...) never matches a
                // PowerShell call; skip the leading `&` to reach the executable.
                format!(
                    "{tool_name}({}:*)",
                    rewrite
                        .rewritten_command
                        .split_whitespace()
                        .find(|token| *token != "&")
                        .unwrap_or("keel")
                ),
            ],

        }

    });

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");

            0
        }

        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render the harness hook output: {error}"
            );

            0
        }
    }
}

/// PostToolUse handler.
///
/// Two responsibilities:
///   1. Count edit-class tool calls (Edit, Write, MultiEdit, NotebookEdit) in a
///      per-workspace counter file under `<claude_home>/state/system-map-edit-counter/<key>`.
///   2. Refresh SYSTEM_MAP.md every N edits so the workspace pointer stays in
///      sync with the repo. N defaults to 10; override via
///      `CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL` (`0` disables).
///
/// PostToolUse stays silent on `additionalContext` (the model already sees the
/// tool result), so we never emit JSON — only do the side-effect and return 0.
pub(super) fn emit_pretool_deny(
    reason: &str,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) {
    // Claude reads hookSpecificOutput.permissionDecision. Grok reads top-level
    // decision/reason. Emit both so one payload blocks every host.
    let deny_payload = serde_json::json!({
        "decision": "deny",
        "reason": reason,
        "hookSpecificOutput": {
            "hookEventName": MANAGED_PRE_TOOL_USE_EVENT,
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    match serde_json::to_string(&deny_payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Unable to render PreToolUse deny: {error}");
        }
    }
}

pub(super) fn tool_is_anvil_surface(tool_name: &str, command: Option<&str>) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    if lower == "anvil"
        || lower.ends_with("__anvil")
        || lower.contains("keel__anvil")
        || lower.ends_with("_anvil")
    {
        return true;
    }
    if let Some(cmd) = command {
        let body = cmd.to_ascii_lowercase();
        if body.contains("keel anvil") || body.contains("keel.exe anvil") {
            return true;
        }
    }
    false
}

pub(super) const ANVIL_SATISFIED_DIR: &str = "anvil-satisfied";

pub(super) fn anvil_satisfied_path(claude_home: &Path, session_id: &str) -> PathBuf {
    claude_home
        .join("state")
        .join(ANVIL_SATISFIED_DIR)
        .join(sanitize_memory_key(session_id))
}

/// Workspace marker so MCP `anvil` (no host session id) still clears the gate.
pub fn record_anvil_gate_clear() {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let key = sanitize_memory_key(&display_path(&cwd));
    let dir = claude_home.join("state").join("anvil-gate");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(format!("{key}.compiled")), now_ms().to_string());
}

pub(super) fn anvil_workspace_marker_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let key = sanitize_memory_key(workspace_cwd);
    let path = claude_home
        .join("state")
        .join("anvil-gate")
        .join(format!("{key}.compiled"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

pub(super) fn anvil_satisfied_this_session(
    claude_home: &Path,
    session_id: &str,
    workspace_cwd: &str,
) -> bool {
    if anvil_satisfied_path(claude_home, session_id).exists() {
        return true;
    }
    let Some(marker) = anvil_workspace_marker_ms(claude_home, workspace_cwd) else {
        return false;
    };
    match session_start_ms(claude_home, session_id) {
        Some(start) => marker.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        None => true,
    }
}

pub(super) fn anvil_gate_enabled() -> bool {
    match std::env::var("KEEL_ANVIL_GATE")
        .ok()
        .or_else(|| std::env::var("CLAUDE_SKILLS_ANVIL_GATE").ok())
    {
        Some(value) => {
            let trimmed = value.trim().to_ascii_lowercase();
            !matches!(trimmed.as_str(), "off" | "0" | "false" | "no")
        }
        None => true,
    }
}

pub(super) const ANVIL_GATE_DENIAL: &str = "\
Anvil gate: call `anvil` (compile, then run --dry-run) before editing. \
This is the only keel delivery loop. MCP: keel__anvil action=compile then action=run args=[--dry-run]. \
CLI: keel anvil compile --goal \"...\" --bar \"echo ok\" then keel anvil run --dry-run. \
Set KEEL_ANVIL_GATE=off to disable.";
