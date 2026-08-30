//! Hook lifecycle prompt_submit responsibility split.

use super::*;

pub(super) fn user_prompt_submit_core() -> String {
    format!(
        "FOLLOW THE IRON LAW. USE KEEL. These are mandatory on every turn — not optional reminders.\n\
         \n\
         Iron Law (every turn):\n\
         1. Research-first: trust the codebase, not your knowledge base. Read SYSTEM_MAP and the owning module before claiming behavior.\n\
         2. Use keel before guessing: native keel MCP tools are always available — prefer them over ad-hoc shell or invented paths: `system_map` (workspace layout — call once per turn when you lack the map, then reuse; call again only if you created/moved/deleted files), `recall` (prior decisions/learnings — call once when you need memory, then reuse; call again only if you wrote new memory this turn), `context_brief` (iron law + skill catalog + memory health + newest brief — call first when starting a task), `skill_route`/`skill_get` (pick and load skills), `run_command` (noisy shell through compaction), `code_search` (live tree search). CLI forms (`keel memory …`, `keel doctor`, `keel code-search …`) count the same. No tool-call loops: re-calling system_map/recall with no intervening change is a loop — re-read context.\n\
         3. Invoke any relevant skill via the Skill tool BEFORE responding — even a 1% chance it applies means use it. Delivery is Anvil only (`anvil` MCP: compile/cast/sieve/stamp/run --dry-run in-process; loop and live run start in the background — poll command_output).\n\
         4. Understand before building: restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code — no guessing, no assuming, no building against an imagined spec. Researching first is what stops you building the wrong thing.\n\
         5. Find the root cause, not just the surface symptom: suspicion is a hypothesis, not a finding — trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing it. Then scan the class: `keel code-search siblings` (or MCP code_search action=siblings) and handle every similar, related, and leftover copy in this turn. A one-site fix is unfinished. No assumptions. No jumping from \"this may be the case\" to a patch.\n\
         6. Edit gate (STRICT): code edits are blocked until this session used a keel research tool (system_map/recall/context_brief/skill_*/code_search or matching keel CLI). Plain Read alone does not clear it.\n\
         \n\
         Memory & learning (part of the Iron Law — do not skip):\n\
         - Recall first: before claiming what you remember, decided earlier, or how this project works, call `recall` (or `keel memory recall`). Memory-first navigation: if SYSTEM_MAP, recall, or a working brief already names the file or module, go there — do not `ls`/list the whole tree or broad-scan the repo to rediscover known paths.\n\
         - Working brief: on non-trivial work, write or update a brief BEFORE coding (`brief_create` / `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"`) so completion can be reconciled.\n\
         - Save durable learnings: when you discover a decision, root cause, convention, or fix worth keeping across sessions, write it now — do not wait for \"later\" or SessionEnd. Use `keel memory research-cache`, working-brief updates, or the project's memory write path. Compaction wipes chat; disk does not.\n\
         - Learn loop: after non-trivial solved problems, capture with compounding-knowledge / memory-consolidation patterns; instincts and learned skills promote at session end — feed them by recording observations (hooks do this on tool use) and by writing explicit notes when something should stick.\n\
         - Before close: `keel memory completion-gate check` when claiming done; run reviewer / `keel review pre-pr` for non-trivial code.\n\
         \n\
         Request fidelity: implement only what the user asked; do not invent features, refactors, files, APIs, or \"improvements\" outside the request. Ask when unclear: if the request is unclear, conflicting, incomplete, or you fear drift into inventing scope, stop and ask the user a concrete question before coding — never decide silently and never \"just pick one and go.\" Never trust knowledge-base alone: training data is not this project's structure, stories, or implementation path; read SYSTEM_MAP, owning files, and the user's stories here. Code comments: never summarize what the code does; write contracts only (`@param`/`@returns`/`# Errors`/`// why:`) or omit. Preserve existing data: never remove or replace a field, column, output, or record to fit a new format — ADD alongside and ASK before dropping anything the user did not name. Implementation discipline applies on every code-touching turn — Think Before Coding, Simplicity First, Surgical Changes, Goal-Driven Execution. Parallel fan-out: only batch agents in the same message when all four hold — no shared inputs, no shared file or git-index writes, no need to cancel/steer one based on another's interim result, and the work fits the current task scope. If any check fails, dispatch sequentially. {}",
        memory_scope_summary()
    )
}

/// Per-prompt action strip: what is enforced this turn (not optional prose).
pub(super) const USER_PROMPT_ENFORCEMENT_STRIP: &str = "\
ENFORCED THIS TURN (not optional):\n\
• Follow the Iron Law. Use keel tools — do not guess from training data.\n\
• PreToolUse DENIES Edit/Write, non-keel Bash, and Agent/Task until a keel research \
tool runs this session (context_brief / system_map / recall / skill_route / skill_get / \
code_search, or matching `keel …` CLI).\n\
• Read/Grep/Glob stay allowed; they do not clear STRICT mode by themselves.\n\
• Memory: recall before claiming prior work; write a working brief before non-trivial \
coding; save durable learnings to disk when you learn something worth keeping.\n\
• After edits: `keel code-search siblings` (MCP code_search action=siblings). Fix every \
copy of the same shape. A one-site change is unfinished.";

/// Max bytes of workspace digest to push on every UserPromptSubmit (on top of
/// the iron-law text). Keeps per-prompt cost bounded while still *pushing* map
/// and brief content so the agent does not have to choose to call system_map.
pub(super) const USER_PROMPT_DIGEST_MAX_BYTES: usize = 1400;

pub(crate) fn user_prompt_submit_context(prompt_text: &str) -> String {
    // Build optional mid-body pointers first, then force the enforcement strip
    // as the absolute first lines of additionalContext so models cannot miss it.
    let mut body = user_prompt_submit_core();
    let claude_home = resolve_claude_home("").ok();

    // Inline the matched skill's own guidance when the prompt distinctively
    // matches one installed skill.
    if let (false, Some(home)) = (prompt_text.trim().is_empty(), claude_home.as_ref()) {
        if let Some(matched) =
            crate::utility::skill_match::match_skill_for_prompt(home, prompt_text)
        {
            let pointer = match crate::utility::skill_match::skill_inline_brief(home, &matched.name)
            {
                Some(brief) => skill_pointer_text(&matched.name, &brief),
                None => skill_pointer_fallback(&matched.name),
            };
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

    // PUSH workspace map/brief content every prompt (not only SessionStart).
    let digest = workspace_memory_digest();
    if !digest.trim().is_empty() {
        let pushed = truncate_on_line_boundary(&digest, USER_PROMPT_DIGEST_MAX_BYTES);
        body.push_str(
            "\n\n--- keel workspace push (already loaded — use this; do not re-guess the repo) ---\n",
        );
        body.push_str(&pushed);
        body.push_str("\n--- end keel workspace push ---");
    }

    // Absolute lead: hard enforcement strip (must be first bytes of context).
    format!("{USER_PROMPT_ENFORCEMENT_STRIP}\n\n{body}")
}

/// Concrete per-prompt skill guidance. Emitted only when the prompt
/// distinctively matches one installed skill (see
/// `utility::skill_match::match_skill_for_prompt`).
///
/// Two parts, deliberately ordered:
///   1. A one-line header naming the matched skill and the `Skill("<name>")`
///      call that loads its full body.
///   2. The skill's *own* bounded brief (`brief`) — its description plus the
///      opening of its body. This is the model-independence fix: the operative
///      guidance is injected as input context for this turn, so it lands even
///      if the gateway model never makes the `Skill()` call. Earlier this hook
///      only asked the model to call `Skill()`; whether the skill loaded then
///      depended entirely on the model honoring that instruction.
pub(super) fn skill_pointer_text(skill_name: &str, brief: &str) -> String {
    format!(
        "Skill match: this prompt strongly matches the `{skill_name}` skill. Its guidance is inlined below and applies now — follow it before writing code or giving a final answer. For the complete skill, call Skill(\"{skill_name}\"). If, after reading, the skill turns out not to apply, say so and proceed.\n\n--- begin {skill_name} skill brief ---\n{brief}\n--- end {skill_name} skill brief ---"
    )
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
pub(super) const WORK_INTENT_REMINDER: &str = "This prompt asks you to change the codebase. Before editing: (1) read the workspace SYSTEM_MAP and the owning file — if you have not already this turn, call the keel MCP `system_map` tool to get it; never edit against an imagined version (if you already called `system_map` this turn, reuse that result; call again only if you have since created, moved, or deleted files); (2) if you have not already this turn, call `recall` to surface any prior work, decisions, or conventions on this topic (reuse the result if you already called it this turn; call again only if you wrote new memory since); (3) write a working brief with `keel memory working-brief write --request \"...\" --acceptance-criteria \"...\"` capturing what the task actually asks and how completion is judged BEFORE you start (this also clears the default-on working-brief gate); (4) if you are about to edit existing code, invoke the `preserve-existing-flow` skill first. After the first site: run `keel code-search siblings` (or MCP code_search action=siblings) and handle every similar/related hit — other hosts, CLIs, tests, install/update/uninstall. A one-site fix or implement is unfinished. Memory-first: if map/recall/brief already names the path, open that file — do not list the whole tree or broad-scan to rediscover known locations. Request fidelity: implement only the asked work; no invented extras. Ask when unclear: if confused, incomplete, or drift-risk, stop and ask the user before coding — do not invent the answer yourself. Never trust knowledge-base alone as this project's structure or stories — read this repo. Comments: contracts only (`@param`/`# Errors`/`// why:`), never summary restatements of the code. Understand before building — correct code that solved the wrong problem is the most expensive failure.";

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
