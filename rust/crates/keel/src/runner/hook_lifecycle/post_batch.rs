//! Hook lifecycle post_batch responsibility split.

use super::*;

pub(super) const GATE_DEFAULT_MAX_BLOCKS: u64 = 1;

/// One row per PostToolBatch gate, for the bridge `gate-status` surface. Each
/// row carries the counter directory name, a display label, and the gate's
/// env-aware max-blocks cap (the real cap, not the flat default). This is the
/// single source the native hook path and the bridge host path must both use so
/// `keel bridge gate-status` reports the same 6 gates the native PostToolBatch
/// fires; the bridge previously hardcoded a subset and compared against a flat 1.
pub(crate) struct GateStatusRow {
    pub dir: &'static str,
    pub label: &'static str,
    pub max_blocks: u64,
}

pub(crate) fn gate_status_rows() -> Vec<GateStatusRow> {
    vec![
        GateStatusRow {
            dir: "review-gate-blocks",
            label: "review",
            max_blocks: review_gate_max_blocks(),
        },
        GateStatusRow {
            dir: "brief-gate-blocks",
            label: "working-brief",
            max_blocks: brief_gate_max_blocks(),
        },
        GateStatusRow {
            dir: "memory-gate-blocks",
            label: "memory",
            max_blocks: memory_gate_max_blocks(),
        },
        GateStatusRow {
            dir: "learned-skill-gate-blocks",
            label: "learned-skill",
            max_blocks: learned_skill_gate_max_blocks(),
        },
        GateStatusRow {
            dir: "research-gate-blocks",
            label: "research",
            max_blocks: research_gate_max_blocks(),
        },
        GateStatusRow {
            dir: "completeness-gate-blocks",
            label: "completeness",
            max_blocks: completeness_gate_max_blocks(),
        },
    ]
}

/// Default per-session fire cap for a gate, chosen by mode. `Escalate` needs at
/// least 2 (fire 0 nudges, fire 1 blocks) or it could never escalate past the
/// opening nudge; every other mode keeps the historical single fire. An explicit
/// `…_MAX_BLOCKS` env var always overrides this.
pub(super) fn default_max_blocks_for(mode: GateMode) -> u64 {
    match mode {
        // Escalate needs 2 (nudge then block). Block defaults to 3 so harder
        // closeout keeps insisting a few times before falling to advisory.
        GateMode::Escalate => 2,
        GateMode::Block => 3,
        _ => GATE_DEFAULT_MAX_BLOCKS,
    }
}

/// How a PostToolBatch gate behaves when it fires (code changed, requirement
/// unmet, under the per-session cap). Three modes, parsed from the gate's env
/// var by [`gate_mode`]:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub(super) enum GateMode {
    /// Fully disabled. The gate never fires; only the generic advisory reminder
    /// is emitted. Selected by `off` / `0` / `false` / `no`.
    Off,
    /// Inject the gate's message via `hookSpecificOutput.additionalContext` — the
    /// agent is *told* to run the review / write the brief, but the turn is never
    /// halted (no `decision` field). Still bounded by the per-session counter so
    /// the reminder shows at most `…_MAX_BLOCKS` time(s) and never spams every
    /// batch. Opt-down from the escalating default: select with `nudge`.
    Nudge,
    /// Escalated feed-forward. Emit an imperative reminder via
    /// `hookSpecificOutput.additionalContext` on every fire (up to the cap) so the
    /// agent is told in strong terms to satisfy the requirement — but the turn is
    /// NEVER halted (no `decision: "block"`). Select with `block` (case-insensitive).
    Block,
    /// DEFAULT. The honest answer to "not optional": a hook cannot force a
    /// `Skill()`/Agent call, but it can feed corrective context forward so the turn
    /// does not close cheaply. The FIRST fire is an advisory nudge (warn, do not
    /// interrupt mid-task); if the requirement is STILL unmet on a later
    /// end-of-turn the gate ESCALATES to an imperative reminder (still via
    /// `additionalContext`, never a blocking decision). Strictly bounded by the
    /// per-session counter, so the worst case is "one nudge, then one imperative,
    /// then advisory forever" — it can neither be ignored for free nor wedge the
    /// session, and it never stops the turn. Selected by an unset var
    /// or any unrecognized value, so a typo fails safe toward this default.
    Escalate,
}

/// Parse a gate's behavior from its env var. Default-on as an ESCALATING gate
/// (nudge first, block if still unmet).
///
/// Mapping (value trimmed, compared case-insensitively):
///   * `off` / `0` / `false` / `no` → [`GateMode::Off`]
///   * `nudge` → [`GateMode::Nudge`] (opt-down: warn only, never block)
///   * `block` → [`GateMode::Block`] (opt-up: block on every fire)
///   * unset, or anything else → [`GateMode::Escalate`] (the default)
///
/// A typo therefore lands on `Escalate` (warn first, then block if ignored),
/// not on silent disablement and not on an immediate surprise stop — the safest
/// failure direction for a gate whose whole point is to make the requirement
/// progressively harder to skip without ever wedging the session.
pub(super) fn gate_mode(env_var: &str) -> GateMode {
    match std::env::var(env_var).ok().as_deref() {
        Some(value) => gate_mode_value(value),
        None => GateMode::Escalate,
    }
}

/// Review-gate behavior. Precedence: an explicit `CLAUDE_SKILLS_REVIEW_GATE`
/// wins (operator escape hatch); otherwise the harness userConfig
/// `review_strictness` (`advisory`→Nudge, `strict`→Block, `off`→Off); otherwise
/// the default **`Block`** (harder closeout: imperative feed-forward until the
/// reviewer marker exists or the per-session cap is spent).
pub(super) fn review_gate_mode() -> GateMode {
    if let Ok(value) = std::env::var(REVIEW_GATE_ENV_VAR) {
        return gate_mode_value(&value);
    }
    user_config_review_strictness().unwrap_or(GateMode::Block)
}

/// `gate_mode` split out so a value already read from a specific env var can be
/// resolved without re-reading the environment.
pub(super) fn gate_mode_value(value: &str) -> GateMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" | "no" => GateMode::Off,
        "nudge" => GateMode::Nudge,
        "block" => GateMode::Block,
        _ => GateMode::Escalate,
    }
}

pub(super) fn review_gate_max_blocks() -> u64 {
    std::env::var(REVIEW_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(review_gate_mode()))
}

// ---- Research gate (PostToolBatch) ----

pub(super) const RESEARCH_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE";

pub(super) const RESEARCH_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_RESEARCH_GATE_MAX_BLOCKS";

/// Research-gate behavior. Default `Escalate` (nudge first, block if ignored);
/// `CLAUDE_SKILLS_RESEARCH_GATE=nudge` keeps it advisory-only; `=off` disables.
pub(super) fn research_gate_mode() -> GateMode {
    gate_mode(RESEARCH_GATE_ENV_VAR)
}

pub(super) fn research_gate_max_blocks() -> u64 {
    std::env::var(RESEARCH_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(research_gate_mode()))
}

pub(super) const COMPLETENESS_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_COMPLETENESS_GATE";

pub(super) const COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR: &str =
    "CLAUDE_SKILLS_COMPLETENESS_GATE_MAX_BLOCKS";

pub(super) fn completeness_gate_mode() -> GateMode {
    match std::env::var(COMPLETENESS_GATE_ENV_VAR) {
        Ok(value) => gate_mode_value(&value),
        Err(_) => GateMode::Block,
    }
}

pub(super) fn completeness_gate_max_blocks() -> u64 {
    std::env::var(COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(completeness_gate_mode()))
}

pub(super) fn completeness_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("completeness-gate-blocks")
        .join(key)
}

pub fn record_completeness_gate_clear_for(workspace: &Path) {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    let key = sanitize_memory_key(&display_path(workspace));
    let dir = claude_home.join("state").join("completeness-gate");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(format!("{key}.scanned")), now_ms().to_string());
}

/// True when `keel code-search siblings` ran for this workspace at or after `after_ms`.
pub fn completeness_scan_satisfies(workspace_cwd: &str, after_ms: u64) -> bool {
    let Ok(claude_home) = resolve_claude_home("") else {
        return false;
    };
    completeness_marker_ms(&claude_home, workspace_cwd)
        .map(|marker_ms| marker_ms >= after_ms)
        .unwrap_or(false)
}

pub(super) fn completeness_marker_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let key = sanitize_memory_key(workspace_cwd);
    let path = claude_home
        .join("state")
        .join("completeness-gate")
        .join(format!("{key}.scanned"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

pub(super) fn completeness_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Completeness gate (CLAUDE_SKILLS_COMPLETENESS_GATE): code changed without a sibling scan — escalated (bounded, cannot loop). Run `keel code-search siblings` (or MCP code_search action=siblings). Fix every copy of the same shape — other hosts, CLIs, tests, install/update/uninstall — or mark it out of scope. A one-site fix is unfinished. Set CLAUDE_SKILLS_COMPLETENESS_GATE=nudge, =off.".to_string(),
        _ => "Completeness reminder (CLAUDE_SKILLS_COMPLETENESS_GATE): code changed without scanning siblings. Run `keel code-search siblings --query \"<the bug shape>\"` and handle every hit. This first reminder does not stop the turn, but will escalate.".to_string(),
    }
}

pub(super) fn research_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("research-gate-blocks")
        .join(key)
}

/// Whether any research tool was called this session. Scans the tool_timings
/// JSONL for `session_id` and checks whether any record's tool_name contains
/// one of the research-tool substrings: "websearch", "web_fetch", "context7",
/// or "recall". Fail-open: any read/parse problem returns `true` so the gate
/// degrades to advisory.
pub(super) fn session_has_research_tool(claude_home: &Path, session_id: &str) -> bool {
    // read yesterday too so research done before midnight in a session that
    // crosses midnight still counts (matches session_start_ms's two-day span).
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let yesterday = (now - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let mut any_readable = false;
    for date in [today, yesterday] {
        let path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{date}.jsonl"));
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        any_readable = true;
        for line in body.lines() {
            if line.trim().is_empty() {
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
                .unwrap_or_default()
                .to_ascii_lowercase();
            // `webfetch` (Claude Code's tool name) has no underscore, so the
            // `web_fetch` substring alone missed it ; count both spellings.
            if tool.contains("websearch")
                || tool.contains("web_search")
                || tool.contains("webfetch")
                || tool.contains("web_fetch")
                || tool.contains("context7")
                || tool.contains("recall")
            {
                return true;
            }
        }
    }
    // Fail-open: if no timing file was readable the code cannot prove research did not
    // happen, so keep the gate silent rather than firing spuriously.
    !any_readable
}

pub(super) fn research_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Research gate (CLAUDE_SKILLS_RESEARCH_GATE): code changed without web search or recall evidence — escalated (imperative, still feed-forward — not a turn halt). Use WebSearch/WebFetch, the context7 MCP, or the keel `recall` tool before implementing. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_RESEARCH_GATE=nudge, =block, =off.".to_string(),
        _ => "Research gate (CLAUDE_SKILLS_RESEARCH_GATE): code changed without web search or recall evidence. Use WebSearch/WebFetch, the context7 MCP, or the keel `recall` tool before implementing. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_RESEARCH_GATE=nudge, =block, =off.".to_string(),
    }
}

pub(super) const BRIEF_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE";

pub(super) const BRIEF_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_BRIEF_GATE_MAX_BLOCKS";

/// Grace window (ms) applied when deciding whether a working brief "belongs to"
/// the current session. A brief written as the session's very first action has a
/// file mtime a few ms BEFORE the session-start timestamp (which is taken from
/// the PostToolUse timing row recorded *after* that write tool completes), so a
/// zero-grace `mtime >= session_start` comparison would falsely reject correct
/// brief-first behavior. 60s comfortably covers tool-execution skew while still
/// rejecting prior-session briefs, which are minutes-to-hours older. Erring on
/// the generous side is deliberate: the gate fails open toward NOT blocking.
pub(super) const BRIEF_GATE_SESSION_GRACE_MS: u64 = 60_000;

/// Working-brief gate behavior. Default **`Block`** (harder closeout: imperative
/// feed-forward when code changed with no working brief). Opt-down with
/// `CLAUDE_SKILLS_BRIEF_GATE=nudge` or `=escalate`; `=off` disables.
pub(super) fn brief_gate_mode() -> GateMode {
    match std::env::var(BRIEF_GATE_ENV_VAR) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "escalate" => GateMode::Escalate,
            other => gate_mode_value(other),
        },
        // Unset to Block (stricter than the generic gate_mode default of Escalate).
        Err(_) => GateMode::Block,
    }
}

pub(super) fn brief_gate_max_blocks() -> u64 {
    std::env::var(BRIEF_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(brief_gate_mode()))
}

pub(super) fn brief_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("brief-gate-blocks")
        .join(key)
}

/// Working-brief gate message. `Nudge` (default) is framed as a non-blocking
/// reminder; `Block` (opt-in) is framed as a hard stop. Both name the clearing
/// action and the off-switch, and both reassure the reminder is bounded.
pub(super) fn brief_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Working-brief gate (CLAUDE_SKILLS_BRIEF_GATE): code changed without a working brief — escalated (imperative, still feed-forward — not a turn halt). Write one: `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"`. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_BRIEF_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Working-brief reminder (CLAUDE_SKILLS_BRIEF_GATE): code changed without a working brief. Write one: `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"`. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_BRIEF_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Memory-save gate (PostToolBatch) ----

pub(super) const MEMORY_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE";

pub(super) const MEMORY_GATE_MAX_BLOCKS_ENV_VAR: &str = "CLAUDE_SKILLS_MEMORY_GATE_MAX_BLOCKS";

pub(super) fn memory_gate_mode() -> GateMode {
    gate_mode(MEMORY_GATE_ENV_VAR)
}

pub(super) fn memory_gate_max_blocks() -> u64 {
    std::env::var(MEMORY_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(memory_gate_mode()))
}

pub(super) fn memory_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("memory-gate-blocks")
        .join(key)
}

/// Memory-save gate message, keyed on the emitted decision. Both variants name
/// the clearing action (research-cache record or maintenance working-buffer),
/// the bound, and the off-switch.
pub(super) fn memory_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Memory-save gate (CLAUDE_SKILLS_MEMORY_GATE): code changed without saving to memory — escalated (imperative, still feed-forward — not a turn halt). Use `keel memory research-cache record` for research findings, or `keel memory maintenance append-working-buffer` for working notes. Either clears the gate. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_MEMORY_GATE=nudge, =off.".to_string(),
        _ => "Memory-save reminder (CLAUDE_SKILLS_MEMORY_GATE): code changed without saving to memory. Use `keel memory research-cache record` for research findings, or `keel memory maintenance append-working-buffer` for working notes. Either clears the gate. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_MEMORY_GATE=nudge, =block, =off.".to_string(),
    }
}

// ---- Learned-skill reminder gate (PostToolBatch) ----

pub(super) const LEARNED_SKILL_GATE_ENV_VAR: &str = "CLAUDE_SKILLS_LEARNED_SKILL_GATE";

pub(super) const LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR: &str =
    "CLAUDE_SKILLS_LEARNED_SKILL_GATE_MAX_BLOCKS";

pub(super) fn learned_skill_gate_mode() -> GateMode {
    gate_mode(LEARNED_SKILL_GATE_ENV_VAR)
}

pub(super) fn learned_skill_gate_max_blocks() -> u64 {
    std::env::var(LEARNED_SKILL_GATE_MAX_BLOCKS_ENV_VAR)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_else(|| default_max_blocks_for(learned_skill_gate_mode()))
}

pub(super) fn learned_skill_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("learned-skill-gate-blocks")
        .join(key)
}

/// Learned-skill reminder message listing each pending learned skill as a
/// `Skill("...")` load action. Advisory in both variants (a reminder, not
/// enforcement); names the gate, the action, the bound, and the off-switch.
pub(super) fn learned_skill_gate_message(
    decision: GateDecision,
    briefs: &[crate::runner::learning::SynthesisBrief],
) -> String {
    let mut actions = String::new();
    for brief in briefs {
        actions.push_str(&format!("\n  - Skill(\"{}\")", brief.skill_name));
    }
    let preamble = match decision {
        GateDecision::Block => "Learned-skill reminder (CLAUDE_SKILLS_LEARNED_SKILL_GATE): learned skill(s) not yet loaded — reminder repeated.",
        _ => "Learned-skill reminder (CLAUDE_SKILLS_LEARNED_SKILL_GATE): learned skill(s) not yet loaded.",
    };
    format!("{preamble} Load and refine:{actions}\nAdvisory, bounded per session, never halts the turn. Set CLAUDE_SKILLS_LEARNED_SKILL_GATE=nudge, =off.")
}

/// Newest mtime (ms) across the memory surfaces a session can write to, or `None`
/// when none exist. Scans research-cache records, working-brief files, and the
/// maintenance working buffer — the targets the gate's clearing actions write to.
pub(super) fn newest_memory_write_ms(claude_home: &Path) -> Option<u64> {
    let candidates = [
        newest_file_mtime_in_dir(&claude_home.join("memory").join("research-cache")),
        newest_file_mtime_in_dir(&crate::utility::working_brief::brief_directory(claude_home)),
        file_mtime_ms(&claude_home.join("memory").join("working-buffer.md")),
    ];
    candidates.into_iter().flatten().max()
}

/// Newest file mtime (ms) directly under `directory`, or `None` when it is
/// missing/unreadable or has no files. Non-recursive: the record stores write
/// flat `<id>.json` files.
pub(super) fn newest_file_mtime_in_dir(directory: &Path) -> Option<u64> {
    let entries = fs::read_dir(directory).ok()?;
    let mut newest: Option<u64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ms) = file_mtime_ms(&path) {
            newest = Some(newest.map_or(ms, |current| current.max(ms)));
        }
    }
    newest
}

/// Whether durable memory was written for this session. Mirrors
/// [`brief_written_this_session`]: an unknown session start reports satisfied
/// (never block a session we cannot time); otherwise satisfied iff the newest
/// memory write is at or after `session_start_ms` minus the shared grace. A
/// missing/unreadable surface counts as "no write" so the gate still fires.
pub(super) fn memory_written_this_session(
    claude_home: &Path,
    session_start_ms: Option<u64>,
) -> bool {
    let Some(start) = session_start_ms else {
        return true;
    };
    match newest_memory_write_ms(claude_home) {
        Some(write_ms) => write_ms.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        None => false,
    }
}

/// Unix-ms modification time of `path`, or `None` on any error. Fail-open: an
/// unreadable mtime is treated by callers as "no usable timestamp" rather than
/// surfacing an error into the hook.
pub(super) fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
}

/// Most recent working-brief file mtime (ms) for a brief that applies to
/// `workspace_cwd`, or `None` when no such brief exists. Scans
/// `<claude_home>/working-briefs/*.json` — the same directory the
/// `keel memory working-brief write` surface writes to.
///
/// A brief applies when it has at least one non-empty acceptance criterion and
/// its stored `workspace` matches `workspace_cwd` (compared through
/// [`sanitize_memory_key`] so path-separator and case differences normalize
/// out) OR its workspace is empty. Empty workspace means a legacy brief written
/// before the field existed and remains a compatibility match; empty acceptance
/// criteria do not satisfy closeout because they provide no definition of done.
/// A missing or unreadable directory, or a brief that fails to parse, yields no
/// match for that entry rather than an error.
pub(super) fn newest_brief_mtime_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let directory = crate::utility::working_brief::brief_directory(claude_home);
    let entries = fs::read_dir(&directory).ok()?;
    let current_key = sanitize_memory_key(workspace_cwd);
    let mut newest: Option<u64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(brief) = crate::utility::working_brief::parse_brief_text(&text) else {
            continue;
        };
        if !brief
            .acceptance_criteria
            .iter()
            .any(|criterion| !criterion.trim().is_empty())
        {
            continue;
        }
        let applies = brief.workspace.trim().is_empty()
            || sanitize_memory_key(&brief.workspace) == current_key;
        if !applies {
            continue;
        }
        if let Some(ms) = file_mtime_ms(&path) {
            newest = Some(newest.map_or(ms, |current| current.max(ms)));
        }
    }
    newest
}

/// Earliest recorded tool-timing (ms) for `session_id`, i.e. an approximation
/// of when this session started doing work. Scans today's AND yesterday's
/// per-day JSONL so a session that began before midnight and continued past it
/// still resolves its true start, rather than taking today's first
/// post-midnight row (which would post-date a brief written late yesterday and
/// trigger one spurious block). Returns `None` when the session has no recorded
/// rows in that window (empty session id, older CC, or unreadable telemetry) so
/// the caller can fail open.
pub(super) fn session_start_ms(claude_home: &Path, session_id: &str) -> Option<u64> {
    if session_id.trim().is_empty() {
        return None;
    }
    let today = chrono::Local::now().date_naive();
    let mut earliest: Option<u64> = None;
    // offset 0 = today, 1 = yesterday. Two days is enough to span one midnight
    for offset in 0..2u64 {
        let Some(date) = today.checked_sub_days(chrono::Days::new(offset)) else {
            break;
        };
        let path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{}.jsonl", date.format("%Y-%m-%d")));
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(row) = serde_json::from_str::<JsonDocument>(line) else {
                continue;
            };
            if row.get("session_id").and_then(JsonDocument::as_str) != Some(session_id) {
                continue;
            }
            if let Some(ms) = row.get("recorded_at_ms").and_then(JsonDocument::as_u64) {
                earliest = Some(earliest.map_or(ms, |current| current.min(ms)));
            }
        }
    }
    earliest
}

/// Whether a working brief exists that plausibly covers this session's work in
/// `workspace_cwd`.
///
/// True when the newest brief applying to `workspace_cwd` (see
/// [`newest_brief_mtime_ms`] for the workspace-match rule) has an mtime at or
/// after `session_start_ms` minus [`BRIEF_GATE_SESSION_GRACE_MS`]. Fail-open in
/// two ways: when the session start is unknown (`None`, e.g. empty session id)
/// we report satisfied so the gate never blocks a session it cannot time; the
/// only way to be unsatisfied is a known session start with no applicable brief
/// recent enough to match it.
pub(super) fn brief_written_this_session(
    claude_home: &Path,
    workspace_cwd: &str,
    session_start_ms: Option<u64>,
) -> bool {
    let Some(start) = session_start_ms else {
        return true;
    };
    match newest_brief_mtime_ms(claude_home, workspace_cwd) {
        Some(brief_ms) => brief_ms.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub(super) enum GateDecision {
    /// Emit the normal generic advisory reminder (the gate did not fire).
    Advisory,
    /// Emit the gate-specific message via `hookSpecificOutput.additionalContext`
    /// — tell the agent to do the work, but do NOT halt the turn. Increments the
    /// per-session counter so it shows at most `max_blocks` time(s). Emitted by
    /// `Nudge` mode always, and by the default `Escalate` mode on its first fire.
    Nudge,
    /// Emit an imperative `additionalContext` reminder and increment the
    /// per-session counter — escalated feed-forward, NOT a turn halt (no
    /// `decision: "block"`). Emitted by `Block` mode always, and by the default
    /// `Escalate` mode once its opening nudge was issued and the requirement is
    /// still unmet.
    Block,
}

/// Pure decision core (no IO, no env) shared by the review gate and the
/// working-brief gate, so the termination guarantee is unit-testable in
/// isolation and identical for both.
///
/// `satisfied` is the gate-specific "requirement already met" signal: for the
/// review gate it means a reviewer pass ran after the last edit; for the brief
/// gate it means a working brief covers this session's work.
///
/// `mode` selects what a fired gate emits:
///   * [`GateMode::Nudge`] → always a non-blocking message ([`GateDecision::Nudge`]).
///   * [`GateMode::Block`] → an imperative feed-forward reminder on every fire
///     ([`GateDecision::Block`]; still `additionalContext`, never a turn halt).
///   * [`GateMode::Escalate`] (default) → the FIRST fire (`blocks_issued == 0`)
///     is a [`GateDecision::Nudge`]; every later fire is a [`GateDecision::Block`].
///     This is the "warn once, then refuse to close cheaply" behavior that makes
///     skipping the requirement progressively harder without interrupting work
///     mid-task on first contact.
///   * [`GateMode::Off`] → never fires.
///
/// The cap check (`blocks_issued >= max_blocks`) is the termination proof: the
/// caller increments `blocks_issued` on every Nudge OR Block, so the value is
/// strictly monotonic across a session and the function returns `Advisory`
/// forever once the cap is reached. Escalate's default cap is 2 (one nudge + one
/// block), so its worst case is "nudge, then block, then advisory forever" — no
/// infinite loop is possible in any mode.
pub(super) fn decide_gate(
    mode: GateMode,
    max_blocks: u64,
    blocks_issued: u64,
    edit_count: usize,
    satisfied: bool,
) -> GateDecision {
    if mode == GateMode::Off || max_blocks == 0 {
        return GateDecision::Advisory;
    }
    // No code changed this session ; nothing to gate. Pure-research and
    // question-answering turns never fire a gate.
    if edit_count == 0 {
        return GateDecision::Advisory;
    }
    // The gate-specific requirement is already met ; nothing to fire on.
    if satisfied {
        return GateDecision::Advisory;
    }
    // Hard cap: stop firing after the allowed number of nudges or blocks.
    // This prevents gate loops and repeated reminders.
    if blocks_issued >= max_blocks {
        return GateDecision::Advisory;
    }
    match mode {
        GateMode::Nudge => GateDecision::Nudge,
        GateMode::Block => GateDecision::Block,
        // Escalate: warn on first contact, then refuse to close cheaply. The
        GateMode::Escalate => {
            if blocks_issued == 0 {
                GateDecision::Nudge
            } else {
                GateDecision::Block
            }
        }
        // Unreachable: handled by the early return above. Mapped to Advisory so a
        // future refactor that removes the early return fails safe, not loud.
        GateMode::Off => GateDecision::Advisory,
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn read_counter_value(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

pub(super) fn review_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home
        .join("state")
        .join("review-gate-blocks")
        .join(key)
}

/// Number of edit-class tool calls recorded for `session_id` today, plus the
/// timestamp of the most recent one and the cwd it ran in. `count == 0` means
/// no code changed this session. Fail-open: any read/parse problem yields a
/// zero-count result so the gate degrades to advisory.
pub(super) struct SessionEditStats {
    count: usize,
    last_edit_ms: u64,
    last_cwd: String,
}

pub(super) fn session_edit_stats(claude_home: &Path, session_id: &str) -> SessionEditStats {
    let mut stats = SessionEditStats {
        count: 0,
        last_edit_ms: 0,
        last_cwd: String::new(),
    };
    // read yesterday too so a session that crosses midnight keeps its
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let yesterday = (now - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    for date in [today, yesterday] {
        let path = claude_home
            .join("state")
            .join("tool-timings")
            .join(format!("{date}.jsonl"));
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) => {
                // a missing file is the normal "no timings yet" case; an
                if path.exists() {
                    eprintln!(
                        "[keel] gate edit-count could not read {}: {error}",
                        path.display()
                    );
                }
                continue;
            }
        };
        for line in body.lines() {
            if line.trim().is_empty() {
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
            if !is_edit_class_tool(tool) {
                continue;
            }
            stats.count += 1;
            let ms = row
                .get("recorded_at_ms")
                .and_then(JsonDocument::as_u64)
                .unwrap_or(0);
            if ms >= stats.last_edit_ms {
                stats.last_edit_ms = ms;
                stats.last_cwd = row
                    .get("cwd")
                    .and_then(JsonDocument::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
        }
    }
    stats
}

/// Timestamp (ms) of the last recorded review for `workspace_cwd`, or `None`
/// when no review marker exists. Written by `record_review_gate_clear` from the
/// `keel review` surface.
pub(super) fn review_marker_ms(claude_home: &Path, workspace_cwd: &str) -> Option<u64> {
    let key = sanitize_memory_key(workspace_cwd);
    let path = claude_home
        .join("state")
        .join("review-gate")
        .join(format!("{key}.reviewed"));
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// Record that a reviewer pass ran for the current workspace, clearing the
/// review gate for edits made up to now. Called from the `keel review`
/// surface (pre-pr / pre-commit / gates). Best-effort: any failure is silently
/// ignored — a missing marker only means the gate may block once more, which
/// the per-session cap still bounds.
pub fn record_review_gate_clear() {
    let Ok(claude_home) = resolve_claude_home("") else {
        return;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let key = sanitize_memory_key(&display_path(&cwd));
    let dir = claude_home.join("state").join("review-gate");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(format!("{key}.reviewed")), now_ms().to_string());
}

/// Review gate message, keyed on the EMITTED decision (not the mode) so an
/// escalating gate renders nudge phrasing on its first fire and block phrasing
/// once it escalates. `Block` is framed as a hard stop, `Nudge` as a
/// non-blocking reminder; both name the clearing action and the off-switch and
/// reassure the reminder is bounded. `Advisory` never reaches here.
pub(super) fn review_gate_message(decision: GateDecision) -> String {
    match decision {
        GateDecision::Block => "Review gate (CLAUDE_SKILLS_REVIEW_GATE): code changed without a reviewer pass — escalated (imperative, still feed-forward — not a turn halt). Run `keel review pre-pr` or invoke the reviewer skill on the diff. Bounded per session, then lets the turn through so it cannot loop. Set CLAUDE_SKILLS_REVIEW_GATE=nudge, =off.".to_string(),
        // Nudge / Advisory both render the non-blocking phrasing; Advisory never reaches here.
        _ => "Review reminder (CLAUDE_SKILLS_REVIEW_GATE): code changed without a reviewer pass. Run `keel review pre-pr` or invoke the reviewer skill before closing. This first reminder does not stop the turn, but will escalate. Set CLAUDE_SKILLS_REVIEW_GATE=nudge, =block, =off.".to_string(),
    }
}

/// Emit the advisory PostToolBatch reminder (the default, gate-disabled path and
/// every fail-open branch). Mirrors the lifecycle render so the output is
/// identical to what `run_hook_lifecycle("post-tool-batch")` produces.
pub(super) fn emit_post_tool_batch_advisory(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let Some(event) = event_by_name("PostToolBatch") else {
        return 0;
    };
    let payload = render_lifecycle_payload(event, &post_tool_batch_context());
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch advisory output: {error}"
            );
            1
        }
    }
}

/// Emit a NON-BLOCKING feed-forward PostToolBatch payload with IMPERATIVE tone.
///
/// Previously emitted `decision: "block"` which halted the turn. Now emits
/// `hookSpecificOutput.additionalContext` (identical shape to the nudge) but
/// with imperative language ("Do NOT present this work as done") so the gate
/// still asserts its requirement without stopping the turn. The per-session
/// counter and cap logic are unchanged — the monotonic termination guarantee
/// remains intact. Falls back to the advisory reminder on render failure.
pub(super) fn emit_post_tool_batch_block(
    reason: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolBatch",
            "additionalContext": reason,
        },
        "suppressOutput": true,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch block output: {error}"
            );
            emit_post_tool_batch_advisory(standard_output, standard_error)
        }
    }
}

/// Emit a NON-BLOCKING PostToolBatch nudge: the gate's `message` is injected via
/// `hookSpecificOutput.additionalContext` so the agent is told to do the work,
/// but the turn is never halted (no `decision` field). This is the default
/// firing path — the fix for "stop mid-task": the agent gets the reminder and
/// keeps going. Falls back to the generic advisory reminder if rendering fails.
/// The caller increments the gate's counter BEFORE calling this so the
/// per-session cap advances even on a render error (the monotonic counter is the
/// termination guarantee).
pub(super) fn emit_post_tool_batch_nudge(
    message: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolBatch",
            "additionalContext": message,
        },
        "suppressOutput": true,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render PostToolBatch nudge output: {error}"
            );
            emit_post_tool_batch_advisory(standard_output, standard_error)
        }
    }
}

/// PostToolBatch dispatcher running the enforcement gates: working-brief,
/// completeness, review, memory-save, research, and learned-skill.
///
/// Reads stdin for `session_id` (the harness delivers the hook payload there,
/// same as UserPromptSubmit). Evaluates the working-brief gate first, then
/// review, completeness, memory-save, research, and learned-skill checks.
/// The working-brief gate covers understanding before building; review covers
/// closeout.
///
/// Each gate fires at most once per turn and has an independent session cap.
/// The default escalation starts with a non-blocking `additionalContext` nudge.
/// Later fires use an imperative reminder through `additionalContext`, never
/// a `decision: "block"` halt. `nudge`, `block`, and `off` select the behavior.
///
/// The worst case across a whole session is, per gate, one nudge then one block
/// (the escalate cap of 2), after which it falls through to the generic advisory
/// forever. When all gates are off this is byte-identical to the advisory reminder.
pub(super) fn run_hook_post_tool_batch(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let review_mode = review_gate_mode();
    let brief_mode = brief_gate_mode();
    let memory_mode = memory_gate_mode();
    let learned_mode = learned_skill_gate_mode();
    let research_mode = research_gate_mode();
    let completeness_mode = completeness_gate_mode();
    let review_on = review_mode != GateMode::Off && review_gate_max_blocks() > 0;
    let brief_on = brief_mode != GateMode::Off && brief_gate_max_blocks() > 0;
    let memory_on = memory_mode != GateMode::Off && memory_gate_max_blocks() > 0;
    let learned_on = learned_mode != GateMode::Off && learned_skill_gate_max_blocks() > 0;
    let research_on = research_mode != GateMode::Off && research_gate_max_blocks() > 0;
    let completeness_on = completeness_mode != GateMode::Off && completeness_gate_max_blocks() > 0;

    // All gates off: skip stdin entirely and emit the advisory reminder. This
    // keeps the fully-disabled path cheap and side-effect-free.
    if !review_on && !brief_on && !memory_on && !learned_on && !research_on && !completeness_on {
        return emit_post_tool_batch_advisory(standard_output, standard_error);
    }

    let stdin_payload = read_json_stdin_fail_open(standard_input);
    let session_id = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("session_id"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    // Fail-open: without a claude_home the code cannot read telemetry or the gate
    // counters, so degrade to advisory rather than risk a wedged turn.
    let Ok(claude_home) = resolve_claude_home("") else {
        return emit_post_tool_batch_advisory(standard_output, standard_error);
    };

    let stats = session_edit_stats(&claude_home, session_id);

    // Edit-count gates only fire when code changed this session ; pure research
    if stats.count > 0 {
        // Brief gate FIRST (front of the law: understand/plan before building).
        if brief_on {
            let start = session_start_ms(&claude_home, session_id);
            let satisfied = brief_written_this_session(&claude_home, &stats.last_cwd, start);
            let blocks_path = brief_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                brief_mode,
                brief_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                // Increment before rendering so the per-session cap advances
                // even if output rendering fails or the message is ignored.
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    brief_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        if completeness_on {
            let scanned = completeness_marker_ms(&claude_home, &stats.last_cwd)
                .map(|marker_ms| marker_ms >= stats.last_edit_ms)
                .unwrap_or(false);
            let blocks_path = completeness_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                completeness_mode,
                completeness_gate_max_blocks(),
                blocks_issued,
                stats.count,
                scanned,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    completeness_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Review gate (back of the law: review before close).
        if review_on {
            let reviewed = review_marker_ms(&claude_home, &stats.last_cwd)
                .map(|marker_ms| marker_ms >= stats.last_edit_ms)
                .unwrap_or(false);
            let blocks_path = review_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                review_mode,
                review_gate_max_blocks(),
                blocks_issued,
                stats.count,
                reviewed,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    review_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Memory-save gate THIRD (record what you learned before forgetting it).
        if memory_on {
            let start = session_start_ms(&claude_home, session_id);
            let satisfied = memory_written_this_session(&claude_home, start);
            let blocks_path = memory_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                memory_mode,
                memory_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    memory_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }

        // Research gate: fires when code changed but no web search or recall
        // tool was used this session. Satisfied when any research tool fired.
        if research_on {
            let satisfied = session_has_research_tool(&claude_home, session_id);
            let blocks_path = research_gate_blocks_path(&claude_home, session_id);
            let blocks_issued = read_counter_value(&blocks_path);
            let decision = decide_gate(
                research_mode,
                research_gate_max_blocks(),
                blocks_issued,
                stats.count,
                satisfied,
            );
            if decision != GateDecision::Advisory {
                let _ = increment_counter_file(&blocks_path);
                return emit_gate_decision(
                    decision,
                    research_gate_message(decision),
                    standard_output,
                    standard_error,
                );
            }
        }
    }

    // Learned-skill reminder (apply the loop's captured conventions). Independent
    if learned_on {
        if let Some(decision_and_message) =
            evaluate_learned_skill_gate(&claude_home, session_id, learned_mode)
        {
            let (decision, message, blocks_path) = decision_and_message;
            let _ = increment_counter_file(&blocks_path);
            return emit_gate_decision(decision, message, standard_output, standard_error);
        }
    }

    // No gate fired to advisory reminder.
    emit_post_tool_batch_advisory(standard_output, standard_error)
}
pub(super) fn stop_gate_blocks_path(claude_home: &Path, session_id: &str) -> PathBuf {
    let key = if session_id.trim().is_empty() {
        "no-session".to_string()
    } else {
        sanitize_memory_key(session_id)
    };
    claude_home.join("state").join("stop-gate-blocks").join(key)
}

fn stop_gate_is_enforcing(mode: GateMode, max_blocks: u64, blocks_issued: u64) -> bool {
    max_blocks > 0
        && blocks_issued < max_blocks
        && matches!(mode, GateMode::Block | GateMode::Escalate)
}

pub(super) fn run_hook_stop(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let Some(input) = read_json_stdin_fail_open(standard_input) else {
        return 0;
    };
    let session_id = hook_session_id(&input);
    let Ok(claude_home) = resolve_claude_home("") else {
        return 0;
    };
    let stats = session_edit_stats(&claude_home, session_id);
    if stats.count == 0 {
        return 0;
    }

    let session_start = session_start_ms(&claude_home, session_id);
    let stop_counter = stop_gate_blocks_path(&claude_home, session_id);
    let stop_blocks = read_counter_value(&stop_counter);
    if stop_blocks >= 3 {
        return 0;
    }

    let mut blockers: Vec<&str> = Vec::new();
    let brief_counter = brief_gate_blocks_path(&claude_home, session_id);
    let brief_blocks = read_counter_value(&brief_counter);
    if stop_gate_is_enforcing(brief_gate_mode(), brief_gate_max_blocks(), brief_blocks)
        && !brief_written_this_session(&claude_home, &stats.last_cwd, session_start)
    {
        blockers.push("write a current working brief with acceptance criteria");
    }
    let comp_counter = completeness_gate_blocks_path(&claude_home, session_id);
    let comp_blocks = read_counter_value(&comp_counter);
    if stop_gate_is_enforcing(
        completeness_gate_mode(),
        completeness_gate_max_blocks(),
        comp_blocks,
    ) && !completeness_marker_ms(&claude_home, &stats.last_cwd)
        .map(|marker_ms| marker_ms >= stats.last_edit_ms)
        .unwrap_or(false)
    {
        blockers.push("run the sibling scan after the latest edit");
    }
    let rev_counter = review_gate_blocks_path(&claude_home, session_id);
    let rev_blocks = read_counter_value(&rev_counter);
    if stop_gate_is_enforcing(review_gate_mode(), review_gate_max_blocks(), rev_blocks)
        && !review_marker_ms(&claude_home, &stats.last_cwd)
            .map(|marker_ms| marker_ms >= stats.last_edit_ms)
            .unwrap_or(false)
    {
        blockers.push("run a reviewer pass after the latest edit");
    }
    let mem_counter = memory_gate_blocks_path(&claude_home, session_id);
    let mem_blocks = read_counter_value(&mem_counter);
    if stop_gate_is_enforcing(memory_gate_mode(), memory_gate_max_blocks(), mem_blocks)
        && !memory_written_this_session(&claude_home, session_start)
    {
        blockers.push("save the non-trivial result to memory");
    }
    let res_counter = research_gate_blocks_path(&claude_home, session_id);
    let res_blocks = read_counter_value(&res_counter);
    if stop_gate_is_enforcing(research_gate_mode(), research_gate_max_blocks(), res_blocks)
        && !session_has_research_tool(&claude_home, session_id)
    {
        blockers.push("record research evidence for the implementation");
    }
    if blockers.is_empty() {
        return 0;
    }

    let _ = increment_counter_file(&stop_counter);
    let reason = format!(
        "Keel closeout is incomplete: {}. Complete every item, then stop again.",
        blockers.join("; ")
    );
    let payload = serde_json::json!({"decision": "block", "reason": reason});
    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(standard_error, "Unable to render Stop gate output: {error}");
            1
        }
    }
}

/// Decide whether the learned-skill reminder fires this turn, returning the
/// `(decision, message, counter_path)` when it does or `None` to fall through.
/// Independent of edit count (passes 1 to `decide_gate`): applicability is the
/// existence of a pending template-state learned skill, like the closeout gate.
/// Fail-open: an empty brief set (nothing pending) yields `None`.
pub(super) fn evaluate_learned_skill_gate(
    claude_home: &Path,
    session_id: &str,
    mode: GateMode,
) -> Option<(GateDecision, String, PathBuf)> {
    let briefs = crate::runner::learning::collect_synthesis_briefs(claude_home);
    if briefs.is_empty() {
        return None;
    }
    let blocks_path = learned_skill_gate_blocks_path(claude_home, session_id);
    let blocks_issued = read_counter_value(&blocks_path);
    let decision = decide_gate(
        mode,
        learned_skill_gate_max_blocks(),
        blocks_issued,
        1,
        false,
    );
    if decision == GateDecision::Advisory {
        return None;
    }
    Some((
        decision,
        learned_skill_gate_message(decision, &briefs),
        blocks_path,
    ))
}

/// Route a fired gate's [`GateDecision`] to the matching emitter: `Nudge` →
/// non-blocking `additionalContext`, `Block` → an imperative `additionalContext`
/// reminder (never `decision: "block"` — no host halt). `Advisory` should never
/// reach here (the caller only emits on a fired gate) but maps to the generic
/// advisory so the function is total and fails safe.
pub(super) fn emit_gate_decision(
    decision: GateDecision,
    message: String,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    match decision {
        GateDecision::Nudge => emit_post_tool_batch_nudge(message, standard_output, standard_error),
        GateDecision::Block => emit_post_tool_batch_block(message, standard_output, standard_error),
        GateDecision::Advisory => emit_post_tool_batch_advisory(standard_output, standard_error),
    }
}
