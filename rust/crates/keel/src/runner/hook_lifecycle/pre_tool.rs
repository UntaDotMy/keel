//! Hook lifecycle pre_tool responsibility split.

use super::*;

pub(super) const IRON_LAW_GATE_DENIAL_STRICT: &str =
    "[keel] Iron Law gate (STRICT): Edit/Write/Bash (non-keel) and Agent/Task are \
        BLOCKED until this session used a keel research tool. Text reminders are not \
        enough — this is a hard deny.\n\
        Do ONE of these, then retry:\n\
        1. MCP `context_brief` or `system_map` (or `keel memory system-map` / `keel doctor`)\n\
        2. MCP `recall` or `skill_route` / `skill_get` (or `keel memory recall`)\n\
        3. MCP `code_search` (or `keel code-search search ...`)\n\
        Mounted tools: Write path=xd://mcp__keel_system_map content={} is the same research call. \
        Hosts must forward the actual path and successful tool observation; an ordinary Write does not qualify.\n\
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
        4. `keel memory research-cache record|reward` when a fresh matching finding \
        already answers the problem (reuse instead of repeat browsing)\n\
        Recall/memory/keel reads do NOT clear VERIFIED — they are internal state, \
        not verification. This is the default law; set KEEL_IRON_LAW_GATE=strict, \
        =balanced, or =off to relax.";

/// Iron-law edit-gate mode. Default is **Verified**: a fresh external web lookup
/// is required before editing, and a fresh research-cache entry counts as reuse.
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
        _ => IronLawGateMode::Verified,
    }
}

/// Jev-style typed gate decision with explicit confidence.
/// Replaces `Option<&'static str>` from the old marker-based gate.
///
/// Confidence is a calibrated probability (0.0-1.0). A confidence of 0.85 means
/// the decision is correct 85% of the time. Decisions below 0.6 confidence
/// should be escalated to a human reviewer.
///
/// The `escalate` flag indicates whether this decision should be routed to a human
/// when combined with low confidence. The `needs_escalation()` accessor returns
/// true when escalation is flagged AND confidence < ESCALATION_CONFIDENCE_THRESHOLD.
#[derive(Debug, Clone, PartialEq)]
pub enum PreToolGateDecision {
    /// Tool call is allowed without restriction.
    Allow,
    /// Tool call is denied with an explicit reason and calibrated confidence.
    Deny {
        /// Why the tool call was denied. Static string for zero-allocation output.
        reason: &'static str,
        /// Calibrated confidence (0.0-1.0) that this denial is correct.
        /// Higher = more reliable denial.
        confidence: f64,
        /// Route to human reviewer when combined with low confidence.
        escalate: bool,
        /// Which gate produced this decision.
        gate_name: &'static str,
    },
    /// Tool call may proceed but with a warning message.
    Warn {
        /// Warning message to display.
        message: &'static str,
        /// Calibrated confidence in the warning.
        confidence: f64,
        /// Whether the tool call can proceed despite the warning.
        continue_anyway: bool,
    },
}
// Production reads decisions through `deny_with_confidence`, `warn`, and
// `as_option`; the items below are test-facing and carry their own narrow allow.
impl PreToolGateDecision {
    #[allow(dead_code)]
    pub(crate) const DEFAULT_CONFIDENCE: f64 = 0.7;

    /// Escalation threshold: denials below it escalate (J07 governs over the J01 sketch).
    pub const ESCALATION_CONFIDENCE_THRESHOLD: f64 = 0.6;

    /// Returns the confidence value, defaulting to 0.7 when Allow has no explicit value.
    #[allow(dead_code)]
    pub(crate) fn confidence(&self) -> f64 {
        match self {
            PreToolGateDecision::Allow => Self::DEFAULT_CONFIDENCE,
            PreToolGateDecision::Deny { confidence, .. } => *confidence,
            PreToolGateDecision::Warn { confidence, .. } => *confidence,
        }
    }

    /// Whether this denial should be escalated to a human reviewer.
    /// True when escalation is flagged AND confidence is below threshold.
    #[allow(dead_code)]
    pub(crate) fn needs_escalation(&self) -> bool {
        match self {
            PreToolGateDecision::Deny {
                escalate,
                confidence,
                ..
            } => *escalate && *confidence < Self::ESCALATION_CONFIDENCE_THRESHOLD,
            _ => false,
        }
    }

    /// Whether the tool call is allowed (no denial or explicit allow with warning).
    #[allow(dead_code)]
    pub(crate) fn is_allowed(&self) -> bool {
        match self {
            PreToolGateDecision::Allow => true,
            PreToolGateDecision::Warn {
                continue_anyway, ..
            } => *continue_anyway,
            PreToolGateDecision::Deny { .. } => false,
        }
    }

    /// Whether the tool call was explicitly denied (not allowed, not warned-through).
    pub(crate) fn is_denied(&self) -> bool {
        matches!(self, PreToolGateDecision::Deny { .. })
    }

    /// Returns the denial reason if this is a denial, else None.
    pub(crate) fn denial_reason(&self) -> Option<&'static str> {
        match self {
            PreToolGateDecision::Deny { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// Creates an Allow decision.
    pub(crate) fn allow() -> Self {
        PreToolGateDecision::Allow
    }

    /// Creates a Deny decision with default confidence and no escalation.
    #[allow(dead_code)]
    pub(crate) fn deny(reason: &'static str, gate_name: &'static str) -> Self {
        PreToolGateDecision::Deny {
            reason,
            confidence: Self::DEFAULT_CONFIDENCE,
            escalate: false,
            gate_name,
        }
    }

    /// Creates a Deny decision with explicit confidence and escalation flag.
    pub(crate) fn deny_with_confidence(
        reason: &'static str,
        confidence: f64,
        escalate: bool,
        gate_name: &'static str,
    ) -> Self {
        PreToolGateDecision::Deny {
            reason,
            confidence: confidence.clamp(0.0, 1.0),
            escalate,
            gate_name,
        }
    }

    /// Creates a Warn decision.
    pub(crate) fn warn(message: &'static str, confidence: f64, continue_anyway: bool) -> Self {
        PreToolGateDecision::Warn {
            message,
            confidence: confidence.clamp(0.0, 1.0),
            continue_anyway,
        }
    }

    /// Backward-compat: converts to Option<&'static str> (None = allow, Some = deny).
    /// Preserves existing code that pattern-matches on Option<&str>.
    pub(crate) fn as_option(&self) -> Option<&'static str> {
        match self {
            PreToolGateDecision::Allow => None,
            PreToolGateDecision::Deny { reason, .. } => Some(reason),
            PreToolGateDecision::Warn { .. } => None,
        }
    }
}

// ============================================================================
// Decision Cache — Jev-inspired TTL-based caching for repeated gate decisions
// ============================================================================

/// TTL for Iron Law gate decisions (2 minutes).
const IRON_LAW_CACHE_TTL_SECS: u64 = 120;

/// TTL for Plan gate decisions (60 minutes).
const PLAN_CACHE_TTL_SECS: u64 = 3600;

/// TTL for Anvil gate decisions (30 minutes).
const ANVIL_CACHE_TTL_SECS: u64 = 1800;

/// Cache entry with expiration time.
struct CacheEntry {
    decision: PreToolGateDecision,
    expires_at_ms: u64,
}

impl CacheEntry {
    fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }
}

/// Thread-safe decision cache with TTL expiration.
///
/// Jev insight: repeated gate decisions (same tool, same session context) can be
/// cached to avoid redundant file-system checks. The cache is session-scoped and
/// automatically expires to prevent stale decisions.
struct DecisionCache {
    /// Gate name -> (session_id, tool_name) -> CacheEntry
    entries:
        std::collections::HashMap<String, std::collections::HashMap<(String, String), CacheEntry>>,
}

impl DecisionCache {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }

    /// Get a cached decision if present and not expired.
    fn get(
        &self,
        gate_name: &str,
        tool_name: &str,
        session_id: &str,
        now_ms: u64,
    ) -> Option<PreToolGateDecision> {
        let gate_entries = self.entries.get(gate_name)?;
        let entry = gate_entries.get(&(session_id.to_string(), tool_name.to_string()))?;
        if entry.is_expired(now_ms) {
            return None;
        }
        Some(entry.decision.clone())
    }

    /// Store a decision in the cache with the given TTL.
    fn put(
        &mut self,
        gate_name: &str,
        tool_name: &str,
        session_id: &str,
        decision: PreToolGateDecision,
        ttl_secs: u64,
        now_ms: u64,
    ) {
        let key = (session_id.to_string(), tool_name.to_string());
        let entry = CacheEntry {
            decision,
            expires_at_ms: now_ms + (ttl_secs * 1000),
        };
        self.entries
            .entry(gate_name.to_string())
            .or_default()
            .insert(key, entry);
    }

    /// Prune expired entries.
    fn prune(&mut self, now_ms: u64) {
        for entries in self.entries.values_mut() {
            entries.retain(|_: &(String, String), entry: &mut CacheEntry| -> bool {
                !entry.is_expired(now_ms)
            });
        }
    }
}

/// Global decision cache instance - lazily initialized.
static DECISION_CACHE: std::sync::LazyLock<
    std::sync::Mutex<DecisionCache>,
    fn() -> std::sync::Mutex<DecisionCache>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(DecisionCache::new()));

/// J02 observability: every gate-cache lookup records one hit or miss.
static GATE_CACHE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GATE_CACHE_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Observable gate-cache counters for `keel decision cache-stats`.
pub(crate) struct GateCacheStats {
    pub hits: u64,
    pub misses: u64,
}

pub(crate) fn gate_cache_stats() -> GateCacheStats {
    use std::sync::atomic::Ordering;
    GateCacheStats {
        hits: GATE_CACHE_HITS.load(Ordering::Relaxed),
        misses: GATE_CACHE_MISSES.load(Ordering::Relaxed),
    }
}

/// Get a cached gate decision if available and not expired.
pub(crate) fn cached_gate_decision(
    gate_name: &str,
    tool_name: &str,
    session_id: &str,
) -> Option<PreToolGateDecision> {
    let cache = DECISION_CACHE.lock().ok()?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let hit = cache.get(gate_name, tool_name, session_id, now_ms);
    use std::sync::atomic::Ordering;
    if hit.is_some() {
        GATE_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
    } else {
        GATE_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    }
    hit
}

/// Cache a gate decision with the appropriate TTL.
pub(crate) fn cache_gate_decision(
    gate_name: &str,
    tool_name: &str,
    session_id: &str,
    decision: &PreToolGateDecision,
) {
    let mut cache = match DECISION_CACHE.lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let ttl = match gate_name {
        "iron_law" => IRON_LAW_CACHE_TTL_SECS,
        "plan" => PLAN_CACHE_TTL_SECS,
        "anvil" => ANVIL_CACHE_TTL_SECS,
        _ => IRON_LAW_CACHE_TTL_SECS,
    };
    cache.put(
        gate_name,
        tool_name,
        session_id,
        decision.clone(),
        ttl,
        now_ms,
    );
}

/// Prune expired entries from the decision cache.
pub(crate) fn prune_decision_cache() {
    if let Ok(mut cache) = DECISION_CACHE.lock() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        cache.prune(now_ms);
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

/// Release the Iron Law marker for a finished session.
///
/// Every bridge adapter clears its own marker at session end, and the native
/// hook path has to do the same. Without it a key that outlives its session
/// stays satisfied, so the next session to reuse that key never sees the gate.
pub(crate) fn release_iron_law_marker(session_id: &str) {
    let Ok(claude_home) = crate::runtime::resolve_claude_home("") else {
        return;
    };
    for path in [
        iron_law_satisfied_path(&claude_home, session_id),
        iron_law_legacy_path(&claude_home, session_id),
    ] {
        let _ = fs::remove_file(path);
    }
}

const KEEL_RESEARCH_TOOL_NAMES: &[&str] = &[
    "brief_get",
    "brief_list",
    "code_graph",
    "code_index",
    "code_search",
    "config_audit",
    "context_brief",
    "doctor",
    "gain",
    "memory_status",
    "observe",
    "recall",
    "recall_status",
    "session",
    "skill_get",
    "skill_lint",
    "skill_list",
    "skill_route",
    "stats",
    "system_map",
    "system_map_refresh",
    "telemetry",
];

fn keel_tool_leaf(tool_name: &str) -> Option<&str> {
    if let Some(leaf) = tool_name.strip_prefix("mcp__keel__") {
        return Some(leaf);
    }
    if let Some(leaf) = tool_name.strip_prefix("mcp__keel_") {
        return (!leaf.contains("__")).then_some(leaf);
    }
    if let Some(leaf) = tool_name.strip_prefix("keel__") {
        return Some(leaf);
    }
    tool_name
        .strip_prefix("keel_")
        .filter(|leaf| !leaf.contains("__"))
}

/// Identify an allowlisted Keel research tool in a canonical or legacy host
/// namespace. Arbitrary Keel tools and lookalike server prefixes do not qualify.
pub(crate) fn is_keel_research_tool_name(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    keel_tool_leaf(&lower).is_some_and(|leaf| KEEL_RESEARCH_TOOL_NAMES.contains(&leaf))
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

/// True when a shell command records or rewards a research-cache entry. That is
/// the agent-attested reuse path VERIFIED accepts in place of a repeat lookup.
pub(super) fn command_is_research_cache_evidence(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("research-cache") && (lower.contains(" record") || lower.contains(" reward"))
}

/// Shell tools keel may rewrite into `keel run --`.
///
/// Deliberately narrower than [`is_host_shell_tool_name`]: the gate reads the
/// canonical vocabulary (every host's shell names) while the rewriter only
/// accepts names whose shell keel knows, so a host tool like Cursor's `Command`
/// stays gated without the rewrite ever guessing its shell.
pub(super) fn is_shell_tool_name(tool_name: &str) -> bool {
    crate::runner::shell_rewrite::is_shell_tool_name(tool_name)
}

pub(crate) fn strip_keel_run_wrapper(command: &str) -> Option<&str> {
    let trimmed = command.trim();
    if let Some(idx) = trimmed.find(" run -- ") {
        let prefix = &trimmed[..idx];
        let stripped_prefix = prefix
            .trim_start_matches('&')
            .trim()
            .trim_matches(|c| c == '\'' || c == '"');
        // why: hosts quote Windows paths with backslashes, and `Path::file_name`
        // only splits the host separator, so basenames split on both.
        let base = match stripped_prefix.rfind(['/', '\\']) {
            Some(idx) => &stripped_prefix[idx + 1..],
            None => stripped_prefix,
        };
        if base.eq_ignore_ascii_case("keel") || base.eq_ignore_ascii_case("keel.exe") {
            return Some(trimmed[idx + " run -- ".len()..].trim());
        }
    }
    None
}

/// Whether one already-tokenized segment is a keel research invocation.
fn segment_is_keel_research(words: &[String]) -> bool {
    if words.is_empty() {
        return false;
    }
    let rendered = words.join(" ").to_ascii_lowercase();
    let body = rendered
        .strip_prefix("keel run -- ")
        .or_else(|| rendered.strip_prefix("keel.exe run -- "))
        .unwrap_or(rendered.as_str());
    let has_keel = body.starts_with("keel ")
        || body.starts_with("keel.exe ")
        || body.contains("\\keel.exe ")
        || body.contains("/keel ")
        || body.contains("\\keel ");
    if !has_keel {
        return false;
    }
    // Research / orientation subcommands that clear the edit gate. Kept in lockstep
    const HITS: &[&str] = crate::runner::tool_names::KEEL_RESEARCH_SUBCOMMANDS;
    HITS.iter().any(|hit| body.contains(hit))
}

/// Whether a shell command is a keel research/read surface (not install/mutate).
///
/// Compound commands clear the gate only when at least one segment is a keel
/// research invocation and every other segment is a safe stream consumer, so
/// `git status && keel recall` stays denied while `keel recall; keel doctor` and
/// `keel system-map | head` are allowed. Substitution, subshells, and
/// redirection always fail closed: their payload is not a segment and cannot be
/// classified.
pub(crate) fn is_keel_research_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let (words, operators) = crate::runner::shell_rewrite::shell_words_and_operators(trimmed);
    if operators
        .iter()
        .any(|operator| matches!(operator.as_str(), "<" | ">" | ">>" | "`" | "(" | ")"))
    {
        return false;
    }
    let mut research_seen = false;
    for segment in crate::runner::shell_rewrite::segments_from_words(&words) {
        if segment_is_keel_research(&segment) {
            research_seen = true;
            continue;
        }
        if !crate::runner::tool_names::is_safe_pipe_consumer(&segment.join(" ")) {
            return false;
        }
    }
    research_seen
}

pub(crate) fn is_host_shell_tool_name(tool_name: &str) -> bool {
    crate::runner::tool_names::is_shell_tool_name(tool_name)
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
    // Verified mode: only a fresh external research tool clears the gate, except
    // that a fresh research-cache record/reward counts as researched reuse. keel
    // research tools, recall, and host reads are internal state, not verification.
    if mode == IronLawGateMode::Verified {
        if is_web_research_tool_name(tool_name) {
            return true;
        }
        return is_host_shell_tool_name(tool_name)
            && command.is_some_and(command_is_research_cache_evidence);
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
    for key in &["command", "CommandLine", "cmd", "script"] {
        if let Some(cmd) = input.get(*key).and_then(JsonDocument::as_str) {
            return Some(cmd);
        }
    }
    let nested = input
        .get("tool_input")
        .or_else(|| input.get("toolInput"))
        .or_else(|| input.get("input"));
    if let Some(nested) = nested {
        for key in &["command", "CommandLine", "cmd", "script"] {
            if let Some(cmd) = nested.get(*key).and_then(JsonDocument::as_str) {
                return Some(cmd);
            }
        }
    }
    None
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

/// Decide whether to deny a gated tool. Returns `GateDecision`.
///
/// Evidence-based: does **not** write a satisfaction marker on deny. The marker
/// is written only when PostToolUse/observe sees a qualifying research tool.
///
/// Returns:
/// - `PreToolGateDecision::Allow` when the session has research evidence
/// - `PreToolGateDecision::Deny { reason, confidence, escalate, gate_name }` when denied
pub(crate) fn iron_law_gate_decision(session_id: &str) -> PreToolGateDecision {
    let mode = iron_law_gate_mode();
    if mode == IronLawGateMode::Off {
        return PreToolGateDecision::allow();
    }

    let claude_home = match crate::runtime::resolve_claude_home("") {
        Ok(home) => home,
        Err(error) => {
            // without the home dir the code cannot read the research marker; surface
            // the fail-open rather than silently disabling the gate.
            eprintln!(
                "[keel] Iron Law gate could not resolve the claude home directory ({error}); allowing this tool call unverified."
            );
            return PreToolGateDecision::allow();
        }
    };

    if iron_law_marker_present(&claude_home, session_id) {
        return PreToolGateDecision::allow();
    }

    // Recover if the marker write failed earlier but timings prove research ran.
    if session_has_iron_law_evidence(&claude_home, session_id, mode) {
        mark_iron_law_satisfied(session_id);
        return PreToolGateDecision::allow();
    }

    // Denial with high confidence — no marker present means definitive evidence of no research.
    // The confidence reflects certainty that the denial is correct (0.95 for marker-based).
    let (reason, escalate) = match mode {
        IronLawGateMode::Strict => (IRON_LAW_GATE_DENIAL_STRICT, false),
        IronLawGateMode::Balanced => (IRON_LAW_GATE_DENIAL_BALANCED, false),
        IronLawGateMode::Verified => (IRON_LAW_GATE_DENIAL_VERIFIED, true),
        IronLawGateMode::Off => return PreToolGateDecision::allow(),
    };

    PreToolGateDecision::deny_with_confidence(reason, 0.95, escalate, "iron_law")
}

/// Canonical path fields emitted by the host adapters. Keep this list narrow:
/// an unrecognized target shape must stay behind the Anvil gate.
const EXPLICIT_TARGET_PATH_KEYS: &[&str] = &["path", "file_path", "filePath"];

fn is_explicit_markdown_path(path: &str) -> bool {
    let path = path.trim();
    path.len() > ".md".len() && path.to_ascii_lowercase().ends_with(".md")
}

fn collect_explicit_target_paths<'a>(
    value: &'a JsonDocument,
    paths: &mut Vec<&'a str>,
    saw_target_field: &mut bool,
    saw_unknown_target: &mut bool,
) {
    match value {
        JsonDocument::Object(object) => {
            for (key, nested) in object {
                if EXPLICIT_TARGET_PATH_KEYS.contains(&key.as_str()) {
                    *saw_target_field = true;
                    match nested {
                        JsonDocument::String(path) if !path.trim().is_empty() => {
                            paths.push(path);
                        }
                        _ => *saw_unknown_target = true,
                    }
                }
                collect_explicit_target_paths(nested, paths, saw_target_field, saw_unknown_target);
            }
        }
        JsonDocument::Array(values) => {
            for nested in values {
                collect_explicit_target_paths(nested, paths, saw_target_field, saw_unknown_target);
            }
        }
        _ => {}
    }
}

/// Full hook payloads can prove a docs-only edit without widening the bridge
/// contract. Unknown or mixed target shapes deliberately return `false`.
pub(super) fn markdown_only_edit_targets(input: &JsonDocument, tool_name: &str) -> bool {
    if !is_edit_class_tool(tool_name) {
        return false;
    }

    let mut paths = Vec::new();
    let mut saw_target_field = false;
    let mut saw_unknown_target = false;
    collect_explicit_target_paths(
        input,
        &mut paths,
        &mut saw_target_field,
        &mut saw_unknown_target,
    );

    saw_target_field
        && !saw_unknown_target
        && !paths.is_empty()
        && paths.iter().all(|path| is_explicit_markdown_path(path))
}

/// The bridge carries one explicit target path instead of the full hook JSON.
/// Keep the same narrow markdown proof for that path; empty or unknown targets
/// remain conservative and stay behind Anvil.
pub(crate) fn markdown_only_edit_path(path: &str, tool_name: &str) -> bool {
    is_edit_class_tool(tool_name) && is_explicit_markdown_path(path)
}
/// Iron Law gate name constant - kept for API completeness even though iron_law
/// gate doesn't use the cache (state can change between calls).
#[allow(dead_code)] // gate identifier retained for API completeness
pub(crate) const GATE_NAME_IRON_LAW: &str = "iron_law";
pub(crate) const GATE_NAME_PLAN: &str = "plan";
pub(crate) const GATE_NAME_ANVIL: &str = "anvil";
/// Decide whether to allow a tool call based on all applicable gates.
/// Returns `GateDecision` with explicit confidence and escalation flags.
///
/// Checks in order:
/// 1. Iron Law gate (marker-based evidence)
/// 2. Plan gate (blocked tasks or not ready)
/// 3. Anvil gate (compile + dry-run required for edits)
pub(crate) fn evaluate_plan_gate(
    session_id: &str,
    tool_name: &str,
    cwd: &str,
) -> PreToolGateDecision {
    if let Ok(plan_id) =
        std::env::var("KEEL_PLAN_ID").or_else(|_| std::env::var("CLAUDE_SKILLS_PLAN"))
    {
        let plan = plan_id.trim();
        if !plan.is_empty() && is_edit_class_tool(tool_name) {
            let plan_cache_key = &format!("{}:{}", tool_name, plan);
            let plan_cached = cached_gate_decision(GATE_NAME_PLAN, plan_cache_key, session_id);
            if let Some(cached) = plan_cached {
                if cached.is_denied() {
                    return cached;
                }
            }

            let blocked = crate::utility::plan::plan_has_blocked_tasks(Path::new(cwd), "", plan)
                .unwrap_or(false);
            if blocked {
                let decision = PreToolGateDecision::deny_with_confidence(
                    PLAN_TASK_BLOCKED_DENIAL,
                    0.95,
                    false,
                    GATE_NAME_PLAN,
                );
                cache_gate_decision(GATE_NAME_PLAN, plan_cache_key, session_id, &decision);
                return decision;
            }
            let ready =
                crate::utility::plan::evaluate_definition_of_ready(Path::new(cwd), "", plan)
                    .is_ok_and(|eval| eval.satisfied);
            if !ready {
                let decision = PreToolGateDecision::deny_with_confidence(
                    PLAN_READY_GATE_DENIAL,
                    0.9,
                    true,
                    GATE_NAME_PLAN,
                );
                cache_gate_decision(GATE_NAME_PLAN, plan_cache_key, session_id, &decision);
                return decision;
            }
        }
    }
    PreToolGateDecision::allow()
}

pub(crate) fn evaluate_anvil_gate(
    session_id: &str,
    tool_name: &str,
    cwd: &str,
) -> PreToolGateDecision {
    if anvil_gate_enabled() && is_edit_class_tool(tool_name) {
        let anvil_cached = cached_gate_decision(GATE_NAME_ANVIL, tool_name, session_id);
        if let Some(cached) = anvil_cached {
            if cached.is_denied() {
                return cached;
            }
        }

        let satisfied = resolve_claude_home("")
            .ok()
            .is_some_and(|home| anvil_satisfied_this_session(&home, session_id, cwd));
        if !satisfied {
            let decision = PreToolGateDecision::deny_with_confidence(
                ANVIL_GATE_DENIAL,
                0.95,
                false,
                GATE_NAME_ANVIL,
            );
            cache_gate_decision(GATE_NAME_ANVIL, tool_name, session_id, &decision);
            return decision;
        }
    }
    PreToolGateDecision::allow()
}

/// Decide whether to allow a tool call based on all applicable gates.
/// Returns `GateDecision` with explicit confidence and escalation flags.
///
/// Checks (evaluated in parallel for edit tools per J10):
/// 1. Iron Law gate (marker-based evidence)
/// 2. Plan gate (blocked tasks or not ready)
/// 3. Anvil gate (compile + dry-run required for edits)
pub(crate) fn pre_tool_gate_decision_with_markdown_context(
    session_id: &str,
    tool_name: &str,
    command: Option<&str>,
    cwd: &str,
    markdown_only_edit: bool,
) -> PreToolGateDecision {
    if !tool_is_iron_law_gated(tool_name, command) {
        return PreToolGateDecision::allow();
    }

    // J10: When multiple gates apply to an edit-class tool, evaluate independent
    // checks in parallel using std::thread::scope
    if is_edit_class_tool(tool_name) && !markdown_only_edit {
        let (iron_law, plan_dec, anvil_dec) = std::thread::scope(|s| {
            let h1 = s.spawn(|| iron_law_gate_decision(session_id));
            let h2 = s.spawn(|| evaluate_plan_gate(session_id, tool_name, cwd));
            let h3 = s.spawn(|| evaluate_anvil_gate(session_id, tool_name, cwd));
            (
                h1.join().unwrap_or_else(|_| PreToolGateDecision::allow()),
                h2.join().unwrap_or_else(|_| PreToolGateDecision::allow()),
                h3.join().unwrap_or_else(|_| PreToolGateDecision::allow()),
            )
        });

        // Precedence fold: Iron Law deny wins, then Plan, then Anvil (J10).
        return fold_gate_decisions(iron_law, plan_dec, anvil_dec);
    }

    // Single-gate path for non-edit tools (e.g. non-keel shell commands or Agent/Task)
    let iron_law = iron_law_gate_decision(session_id);
    if iron_law.is_denied() {
        return iron_law;
    }

    PreToolGateDecision::allow()
}

/// J10 precedence fold: Iron Law deny wins, then Plan, then Anvil.
pub(crate) fn fold_gate_decisions(
    iron_law: PreToolGateDecision,
    plan_dec: PreToolGateDecision,
    anvil_dec: PreToolGateDecision,
) -> PreToolGateDecision {
    if iron_law.is_denied() {
        return iron_law;
    }
    if plan_dec.is_denied() {
        return plan_dec;
    }
    if anvil_dec.is_denied() {
        return anvil_dec;
    }
    PreToolGateDecision::allow()
}
/// Backward-compat wrapper: converts GateDecision to Option<&'static str>.
/// New code should use pre_tool_gate_decision_with_markdown_context directly.
#[allow(dead_code)] // backward-compat wrapper for external callers
pub(crate) fn pre_tool_gate_decision(
    session_id: &str,
    tool_name: &str,
    command: Option<&str>,
    cwd: &str,
) -> Option<&'static str> {
    // Bridge callers do not pass the complete hook payload; keep them
    // conservative and require Anvil for edit calls with unknown targets.
    pre_tool_gate_decision_with_markdown_context(session_id, tool_name, command, cwd, false)
        .as_option()
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
    let markdown_only_edit = markdown_only_edit_targets(&input, tool_name);

    let command = tool_input_command(&input).unwrap_or("");
    let command_opt = if command.is_empty() {
        None
    } else {
        Some(command)
    };
    let session_id = hook_session_id(&input);

    let cwd = hook_str(&input, &["cwd", "workspaceRoot"]);
    let current_directory;
    let cwd = if cwd.is_empty() {
        current_directory = std::env::current_dir()
            .ok()
            .map(|path| display_path(&path))
            .unwrap_or_default();
        current_directory.as_str()
    } else {
        cwd
    };
    let decision = pre_tool_gate_decision_with_markdown_context(
        session_id,
        tool_name,
        command_opt,
        cwd,
        markdown_only_edit,
    );
    if decision.is_denied() {
        emit_pretool_deny(
            decision.denial_reason().unwrap_or("gate denied"),
            standard_output,
            standard_error,
        );
        return 0;
    }

    // Compaction rewrite only applies to shell tools.
    if !is_shell_tool_name(tool_name) {
        return 0;
    }
    // If the command is already wrapped in `keel run -- ...`, evaluate the inner payload
    let effective_command = strip_keel_run_wrapper(command).unwrap_or(command);

    // J06: Shell destructive probability & risk action via Noul evaluation
    let noul = crate::utility::decision::evaluate_shell_command_noul(effective_command);
    match noul.action {
        crate::utility::decision::ShellRiskAction::Block => {
            let reason = format!(
                "[keel] Destructive command blocked (prob: {:.2}): {}",
                noul.probability, noul.reason
            );
            emit_pretool_deny(&reason, standard_output, standard_error);
            return 0;
        }
        crate::utility::decision::ShellRiskAction::Escalate => {
            let escalation = crate::utility::decision::format_gate_escalation(
                "shell_noul",
                &noul.reason,
                noul.confidence,
            );
            emit_pretool_deny(&escalation, standard_output, standard_error);
            return 0;
        }
        _ => {}
    }

    // Inspect EVERY segment of a compound command, not just the first supported
    if let Some(finding) =
        crate::runner::shell_rewrite::detect_destructive_in_command(effective_command)
    {
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
pub(crate) fn emit_pretool_deny(
    reason: &str,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) {
    let clean_reason = if cfg!(windows) {
        reason.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        reason.to_string()
    };
    // Claude reads hookSpecificOutput.permissionDecision. Grok reads top-level
    // decision/reason. Emit both so one payload blocks every host.
    let deny_payload = serde_json::json!({
        "decision": "deny",
        "reason": clean_reason,
        "hookSpecificOutput": {
            "hookEventName": MANAGED_PRE_TOOL_USE_EVENT,
            "permissionDecision": "deny",
            "permissionDecisionReason": clean_reason,
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

#[cfg(test)]
pub(super) const ANVIL_SATISFIED_DIR: &str = "anvil-satisfied";

#[cfg(test)]
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
    let Some(marker) = anvil_workspace_marker_ms(claude_home, workspace_cwd) else {
        return false;
    };
    match session_start_ms(claude_home, session_id) {
        Some(start) => marker.saturating_add(BRIEF_GATE_SESSION_GRACE_MS) >= start,
        // why: fail open like the brief gate: the marker proves the dry-run ran;
        // unknown start (empty session, older host) must not wedge edits forever.
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
This is the only keel delivery loop. MCP: keel__anvil action=compile args=[--goal,...,--bar,...,--files,...] then action=run args=[--dry-run]. \
CLI: keel anvil compile --goal \"...\" --bar \"echo ok\" --files \"src/file.rs\" then keel anvil run --dry-run. \
Set KEEL_ANVIL_GATE=off to disable.";
pub(super) const PLAN_TASK_BLOCKED_DENIAL: &str = "\
Plan gate: the selected plan has blocked task(s) with unresolved prerequisites. \
Resolve or explicitly skip each blocked task before editing source code. \
Run `keel plan status --plan <id>` to inspect blockers.";

pub(super) const PLAN_READY_GATE_DENIAL: &str = "\
Plan gate: the active plan has unresolved prerequisites, ambiguity, or incomplete design. \
All 11 Definition of Ready (DoR) items must pass before editing source code. \
Run `keel plan ready --plan <id>` to evaluate readiness, or resolve the blocking items.";

#[cfg(test)]
mod namespace_tests {
    use super::*;

    #[test]
    fn keel_research_matcher_rejects_foreign_and_lookalike_namespaces() {
        assert!(is_keel_research_tool_name("mcp__keel__system_map"));
        assert!(is_keel_research_tool_name("mcp__keel_system_map"));
        assert!(is_keel_research_tool_name("keel__recall"));
        assert!(!is_keel_research_tool_name("mcp__vendor__keel__system_map"));
        assert!(!is_keel_research_tool_name("mcp__keel_evil__system_map"));
        assert!(!is_keel_research_tool_name("keel_evil__system_map"));
        assert!(!is_keel_research_tool_name("mcp__keel__run_command"));
    }

    #[test]
    fn decision_cache_hit_miss_ttl_and_prune() {
        // J02 checks: TTL expiration, hit/miss, prune on explicit timestamps.
        let mut cache = DecisionCache::new();
        cache.put(
            "iron_law",
            "Edit",
            "sess-ttl",
            PreToolGateDecision::allow(),
            120,
            1_000,
        );
        assert!(cache.get("iron_law", "Edit", "sess-ttl", 1_000).is_some());
        assert!(
            cache
                .get("iron_law", "Edit", "sess-ttl", 1_000 + 120_000)
                .is_none(),
            "entry must expire exactly at now + ttl"
        );
        assert!(
            cache.get("plan", "Edit", "sess-ttl", 1_000).is_none(),
            "unknown gate must miss"
        );
        cache.prune(1_000 + 120_000);
        cache.put(
            "plan",
            "Edit",
            "sess-ttl",
            PreToolGateDecision::allow(),
            3_600,
            2_000,
        );
        assert!(cache.get("plan", "Edit", "sess-ttl", 2_000).is_some());
    }

    fn deny(gate: &'static str) -> PreToolGateDecision {
        PreToolGateDecision::deny_with_confidence("test denial", 0.9, false, gate)
    }

    #[test]
    fn gate_decision_fold_prefers_iron_over_plan_over_anvil() {
        // J10 checks: precedence over every denial combination.
        let allow = PreToolGateDecision::allow();
        assert!(fold_gate_decisions(allow.clone(), allow.clone(), allow.clone()).is_allowed());
        let folded = fold_gate_decisions(deny("iron_law"), deny("plan"), deny("anvil"));
        assert!(folded.is_denied());
        // The winner carries its gate name: iron first, then plan, then anvil.
        for (iron, plan, anvil, winner) in [
            (true, true, true, "iron_law"),
            (false, true, true, "plan"),
            (false, false, true, "anvil"),
            (true, false, false, "iron_law"),
            (false, true, false, "plan"),
        ] {
            let pick = |denied: bool, gate: &'static str| {
                if denied {
                    deny(gate)
                } else {
                    PreToolGateDecision::allow()
                }
            };
            let folded = fold_gate_decisions(
                pick(iron, "iron_law"),
                pick(plan, "plan"),
                pick(anvil, "anvil"),
            );
            match folded {
                PreToolGateDecision::Deny { gate_name, .. } => {
                    assert_eq!(gate_name, winner, "iron={iron} plan={plan} anvil={anvil}")
                }
                other => panic!("expected Deny, got {other:?}"),
            }
        }
    }

    #[test]
    fn gate_decision_fold_is_deterministic() {
        // J10 acceptance: identical inputs always produce identical output,
        // which is what makes parallel evaluation equivalent to sequential.
        let first = fold_gate_decisions(deny("plan"), PreToolGateDecision::allow(), deny("anvil"));
        let second = fold_gate_decisions(deny("plan"), PreToolGateDecision::allow(), deny("anvil"));
        assert_eq!(first, second);
        match first {
            PreToolGateDecision::Deny { gate_name, .. } => assert_eq!(gate_name, "plan"),
            other => panic!("expected plan Deny, got {other:?}"),
        }
    }
}
