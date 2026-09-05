//! Hook lifecycle prompt_submit responsibility split.

use super::*;

pub(super) fn user_prompt_submit_core() -> String {
    "Understand before building: research what is needed and avoid building the wrong thing. \
No assumptions: suspicion is a hypothesis, not a finding; trace the symptom and root cause before patching, never jump from \"this may be the case\".\n\
Skill tool: invoke a relevant skill before responding; use Anvil for delivery.\n\
Memory & learning: Recall first. Use Memory-first navigation, write a Working brief for non-trivial work, Save durable learnings, run the Learn loop, then completion-gate and reviewer before close.\n\
Request fidelity: only the requested scope. Ask when unclear. Never trust knowledge-base alone. Preserve existing data. Code comments are contracts only.\n\
Implementation discipline: Think Before Coding, Simplicity First, Surgical Changes, Goal-Driven Execution."
        .to_string()
}

/// Per-prompt action strip: what is enforced this turn (not optional prose).
pub(super) const USER_PROMPT_ENFORCEMENT_STRIP: &str = "\
ENFORCED THIS TURN (mandatory on every turn): FOLLOW THE IRON LAW. USE KEEL.\n\
Research-first: trust the codebase. Read SYSTEM_MAP; use system_map, recall, context_brief, run_command, skill_route/skill_get, and code_search instead of guessing.\n\
Never assume, never guess, never skip required tests, review, or sibling scans.\n\
PreToolUse DENIES Edit/Write and shell work until a keel research tool runs. After edits, run code_search siblings.";

pub(crate) fn user_prompt_submit_context(prompt_text: &str) -> String {
    // Build optional mid-body pointers first, then force the enforcement strip
    // as the absolute first lines of additionalContext so models cannot miss it.
    let mut body = user_prompt_submit_core();
    let claude_home = resolve_claude_home("").ok();

    // Name one matched skill; its body remains on-demand instead of becoming
    // recurring per-prompt context.
    if let (false, Some(home)) = (prompt_text.trim().is_empty(), claude_home.as_ref()) {
        if let Some(matched) =
            crate::utility::skill_match::match_skill_for_prompt(home, prompt_text)
        {
            let pointer = skill_pointer_fallback(&matched.name);
            body = format!("{pointer}\n\n{body}");
        }
    }

    // Point repo/structure and memory questions at the MCP tools.
    if !prompt_text.trim().is_empty() {
        if let Some(pointer) = mcp_tool_pointer_for_prompt(prompt_text) {
            body = format!("{pointer}\n\n{body}");
        }
    }

    // Point code-CHANGE prompts at the read-map/recall front.
    if !prompt_text.trim().is_empty() {
        if let Some(pointer) = work_intent_pointer_for_prompt(prompt_text) {
            body = format!("{pointer}\n\n{body}");
        }
    }

    // Absolute lead: hard enforcement strip (must be first bytes of context).
    format!("{USER_PROMPT_ENFORCEMENT_STRIP}\n\n{body}")
}

/// Fallback per-prompt skill pointer used when the matched skill's body cannot
/// be read for inlining. Names the skill and the exact `Skill("<name>")` call so
/// the model still gets an actionable instruction, even though the brief itself
/// is unavailable this turn.
pub(super) fn skill_pointer_fallback(skill_name: &str) -> String {
    format!(
        "Skill match: this prompt strongly matches the `{skill_name}` skill. Invoke it now with Skill(\"{skill_name}\") BEFORE writing code or giving a final answer. If, after reading it, the skill turns out not to apply, say so and proceed — but do not skip the check."
    )
}

/// Per-prompt pointer at the keel MCP tools for prompts that ask about
/// the repository layout or the agent's own memory.
///
/// The deterministic skill matcher (`utility::skill_match`) stays silent on
/// these prompts by design: "what is this project about", "how is the repo
/// structured", "what do you remember" carry no *distinctive domain token*, so
/// they clear no skill's score floor. That silence is correct for skill
/// routing, but it left a real gap — exactly these prompts are the ones the
/// model should answer by calling `system_map` (structure) or `recall`
/// (memory) instead of guessing or reading files conversationally. This pointer
/// fills that gap with a targeted reminder, fired only when the prompt matches
/// one of the two question shapes below. Returns `None` (no injection) for
/// everything else so the generic per-prompt context is unchanged.
pub(super) fn mcp_tool_pointer_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();

    // Memory questions: the model should search its durable memory rather than
    const MEMORY_CUES: &[&str] = &[
        "what do you remember",
        "what did you learn",
        "what have you learned",
        "do you remember",
        "from memory",
        "your memory",
        "recall what",
        "what's in memory",
        "what is in memory",
    ];
    if MEMORY_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(
            "This prompt asks about your durable memory. If you have not already this turn, call the keel MCP `recall` tool (full-text search over your saved memories and working briefs) before answering — do not claim what you remember from conversation alone. If you already called `recall` this turn, reuse that result; only call again if you wrote new memory since. Use `recall_status` if you need index health.",
        );
    }

    // Repo/structure questions: the model should consult the workspace map
    const REPO_CUES: &[&str] = &[
        "what is this project",
        "what's this project",
        "what is this repo",
        "what's this repo",
        "what is this codebase",
        "what does this project",
        "what does this repo",
        "what does this codebase",
        "about this project",
        "about this repo",
        "about this codebase",
        "project overview",
        "repo structure",
        "repository structure",
        "project structure",
        "codebase structure",
        "how is this repo",
        "how is the repo",
        "how is this project",
        "how is the project",
        "how is this codebase",
        "explain the architecture",
        "explain this project",
        "explain the project",
        "explain the codebase",
    ];
    if REPO_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(
            "This prompt asks about the repository's structure or purpose. If you have not already this turn, call the keel MCP `system_map` tool to get the authoritative workspace structural map before answering — do not describe the repo layout from memory or guesswork. If you already called `system_map` this turn, reuse that result; only call again if you have since created, moved, or deleted files. Read the owning files only after the map points you at them.",
        );
    }

    None
}

/// Per-prompt reminder for code-CHANGE prompts: read the map, recall prior work,
/// and write a working brief BEFORE editing existing code.
///
/// This closes the gap that let the front of the Iron Law go unenforced in
/// practice. `mcp_tool_pointer_for_prompt` above fires only on *question*-shaped
/// prompts ("what is this project", "what do you remember"). A *work* prompt
/// ("rework the X", "fix the Y", "add Z") carries a domain token, so the skill
/// matcher may fire — but nothing reminded the model to read SYSTEM_MAP, run
/// `recall`, or write a working brief first. Those are exactly the steps most
/// easily rationalized away under time pressure, and skipping them is what ships
/// the wrong thing.
///
/// Returns `Some(text)` when the prompt looks like a request to change the
/// codebase (edit/build/refactor/fix intent) and `None` otherwise, so it never
/// fires on pure questions, chit-chat, or read-only asks. Deliberately
/// conservative: a missed work prompt just loses a reminder (the default-on
/// brief gate is the hard backstop), while a false positive would add noise to
/// an ordinary question.
pub(super) fn work_intent_pointer_for_prompt(prompt: &str) -> Option<&'static str> {
    let lowered = prompt.to_ascii_lowercase();

    // Unambiguous change-intent cues ; safe to match as substrings because they
    const STRONG_CUES: &[&str] = &[
        "implement",
        "refactor",
        "rework",
        "rewrite",
        "add a ",
        "add an ",
        "add support",
        "change the",
        "update the",
        "modify",
        "migrate",
        "wire up",
        "integrate",
        "create a",
        "create an",
        "delete the",
        "remove the",
        "rename",
        "optimize",
        "extend the",
    ];
    if STRONG_CUES.iter().any(|cue| lowered.contains(cue)) {
        return Some(WORK_INTENT_REMINDER);
    }

    // Verbs that ALSO read as nouns ("the build", "a fix", "the patch"). Treat
    const VERB_OR_NOUN_CUES: &[&str] = &["build ", "fix ", "fixes ", "patch "];
    if VERB_OR_NOUN_CUES
        .iter()
        .any(|cue| cue_used_as_verb(&lowered, cue))
    {
        return Some(WORK_INTENT_REMINDER);
    }

    None
}

/// The read-map / recall / write-brief / preserve-flow reminder injected for
/// code-change prompts. A `const` so both match arms above return the exact same
/// text and the test asserting its content has a single source of truth.
pub(super) const WORK_INTENT_REMINDER: &str = "Code change: read SYSTEM_MAP and the owner; Memory-first recall; write a working brief; use preserve-existing-flow. Keep Request fidelity. Ask when unclear. Never trust knowledge-base alone. Comments: contracts only. After edits, run code_search siblings.";

/// True when `cue` (e.g. `"fix "`) appears in `lowered` used as a verb rather
/// than a noun — that is, at least one occurrence is NOT immediately preceded by
/// a determiner ("the", "a", "an", "this", "that"). This is what separates the
/// change request "fix the bug" from the question "is the fix ready". Whole-word
/// determiner matching avoids treating "breathe " (ends in "the") as an article.
pub(super) fn cue_used_as_verb(lowered: &str, cue: &str) -> bool {
    const ARTICLES: &[&str] = &["the", "a", "an", "this", "that"];
    let mut search_start = 0;
    while let Some(relative) = lowered[search_start..].find(cue) {
        let index = search_start + relative;
        let preceding = lowered[..index].trim_end();
        let after_article = ARTICLES.iter().any(|article| {
            if !preceding.ends_with(article) {
                return false;
            }
            // Whole-word check: the char before the article must be a boundary,
            // so "breathe" (ends with "the") is not mistaken for the article.
            let article_start = preceding.len() - article.len();
            article_start == 0 || preceding.as_bytes()[article_start - 1] == b' '
        });
        if !after_article {
            return true;
        }
        search_start = index + cue.len();
    }
    false
}

/// UserPromptSubmit dispatcher that reads stdin and composes the per-prompt
/// `additionalContext`.
///
/// the harness delivers a JSON payload on stdin for this event with at least
/// `session_id`, `transcript_path`, `cwd`, and `prompt`. We use `session_id`
/// to read today's tool-timings JSONL and decide whether enough tool calls
/// have already happened in this session to merit the compression-discipline
/// nudge. Every failure path (no stdin, unparseable stdin, missing session id,
/// missing JSONL, no claude_home) falls back to the unchanged base text so
/// the existing back-compat test keeps passing and a hook misconfiguration
/// can never break the per-prompt injection.
pub(super) fn run_hook_user_prompt_submit(
    standard_input: &mut dyn Read,
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let stdin_payload = read_json_stdin_fail_open(standard_input);

    let session_id = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("session_id"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    let prompt_text = stdin_payload
        .as_ref()
        .and_then(|payload| payload.get("prompt"))
        .and_then(JsonDocument::as_str)
        .unwrap_or_default();

    let claude_home = resolve_claude_home("").ok();

    let base_context = user_prompt_submit_context(prompt_text);

    let final_context = match (session_id.is_empty(), claude_home.as_ref()) {
        (false, Some(home)) => match maybe_compression_hint(home, session_id) {
            Some(hint) => format!("{base_context}\n\n{hint}"),
            None => base_context,
        },
        _ => append_compression_hint_when_forced(base_context),
    };

    let event = match event_by_name("UserPromptSubmit") {
        Some(row) => row,
        None => {
            let _ = writeln!(
                standard_error,
                "UserPromptSubmit row missing from canonical event table"
            );
            return 1;
        }
    };

    let payload = render_lifecycle_payload(event, &final_context);

    match serde_json::to_string_pretty(&payload) {
        Ok(rendered) => {
            let _ = writeln!(standard_output, "{rendered}");
            0
        }
        Err(error) => {
            let _ = writeln!(
                standard_error,
                "Unable to render the harness lifecycle hook output: {error}"
            );
            0
        }
    }
}

/// Per-session compression-discipline nudge.
///
/// Returns `Some(text)` when the heuristic decides this turn would benefit
/// from a reminder to compress tool output, or `None` to leave the per-prompt
/// payload unchanged.
///
/// Heuristic (deterministic):
///   * `CLAUDE_SKILLS_COMPRESSION_HINT=off`  -> always None
///   * `CLAUDE_SKILLS_COMPRESSION_HINT=force` -> always Some
///   * Otherwise: Some when this session has recorded at least
///     `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER` tool-timings rows in today's
///     JSONL (default 40), None below that threshold.
///
/// Telemetry rule: any read failure (no JSONL, unreadable file, malformed
/// rows) returns None silently. A telemetry hiccup must never fail the hook.
pub(super) fn maybe_compression_hint(claude_home: &Path, session_id: &str) -> Option<&'static str> {
    match std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("off") => return None,
        Some("force") => return Some(compression_hint_text()),
        _ => {}
    }

    if session_id.is_empty() {
        return None;
    }

    let threshold = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(COMPRESSION_HINT_DEFAULT_THRESHOLD);
    if threshold == 0 {
        return None;
    }

    let row_count = count_session_tool_timing_rows(claude_home, session_id);
    if row_count >= threshold {
        Some(compression_hint_text())
    } else {
        None
    }
}

/// Honor `CLAUDE_SKILLS_COMPRESSION_HINT=force` even when stdin or
/// claude_home are unavailable so test scaffolding and operators can demand
/// the nudge for diagnostic runs without populating a real JSONL.
pub(super) fn append_compression_hint_when_forced(base_context: String) -> String {
    match std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("force") => format!("{base_context}\n\n{}", compression_hint_text()),
        _ => base_context,
    }
}

/// Default threshold of 40 tool calls is calibrated against the per-day
/// tool-timings JSONL: a heavy investigation session typically logs 60-100
/// rows, so 40 fires the hint roughly halfway through and gives the model
/// real budget headroom for the back half of the work. Operators can tune
/// via `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER`; setting it to 0 disables.
pub(super) const COMPRESSION_HINT_DEFAULT_THRESHOLD: usize = 40;

/// The compression-discipline reminder. Three concrete actions, ~50 tokens.
///
/// Compact by design: this lands per-prompt in addition to the existing
/// research-first iron law text. Token cost matters. Keep it surgical and
/// actionable.
pub(super) fn compression_hint_text() -> &'static str {
    "Output compression is on for this turn — context is heavy. Read narrower line ranges (offset+limit) instead of whole files. Search before reading: use your host's search tool to locate the exact symbol, then read only the relevant window. Summarize logs and command output instead of pasting them in full. Skill: compression-discipline."
}

/// Count tool-timings JSONL rows for `session_id` recorded today. Returns 0
/// for any failure (missing file, unreadable, malformed lines). Each
/// matching row counts once; non-matching rows and parse errors are
/// silently skipped so a single corrupt line cannot poison the count.
pub(super) fn count_session_tool_timing_rows(claude_home: &Path, session_id: &str) -> usize {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let body = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return 0,
    };
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<JsonDocument>(line).ok())
        .filter(|row| {
            row.get("session_id")
                .and_then(JsonDocument::as_str)
                .map(|recorded| recorded == session_id)
                .unwrap_or(false)
        })
        .count()
}
