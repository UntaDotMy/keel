//! Hook lifecycle session_start responsibility split.

use super::*;

pub(super) fn run_hook_lifecycle(
    subcommand: &str,

    standard_output: &mut dyn Write,

    standard_error: &mut dyn Write,
) -> u8 {
    // Look up the canonical row once; every behaviour below comes from that row.
    let event = match event_by_slug(subcommand) {
        Some(row) => row,
        // Unknown slugs fall back to the canonical SessionStart event.
        None => event_by_name("SessionStart").expect("SessionStart row missing"),
    };

    // Refresh the workspace system map at the three natural transition
    if should_refresh_system_map(event.name) {
        let _ = refresh_memory_scope_for_current_directory(standard_error);
    }

    if event.name == "SessionEnd" {
        prune_raw_output_store(standard_error);
        prune_tool_timings_store(standard_error);
        prune_observations_store(standard_error);
        prune_state_marker_stores(standard_error);
        run_session_end_learning(standard_error);
    }

    // PreCompact is the OTHER point the learning cycle must run. Working memory is
    if event.name == "PreCompact" {
        run_session_end_learning(standard_error);
    }

    let context = lifecycle_additional_context(event.slug);

    if context.trim().is_empty() {
        return 0;
    }

    // Whether this event accepts `hookSpecificOutput.additionalContext`
    let payload = render_lifecycle_payload(event, &context);

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

/// Wrap `context` in the JSON payload the harness expects for `event`.
///
/// Events whose schema accepts `hookSpecificOutput.additionalContext` get
/// the per-event shape; everything else falls back to top-level
/// `systemMessage`. The split lives on the event row so adding a new event
/// to `HOOK_EVENTS` automatically picks up the right schema.
///
/// Pulled out of `run_hook_lifecycle` so tests can exercise both branches
/// without setting up the surrounding side-effects (system map refresh,
/// raw-output prune). The previous in-line shape was effectively
/// untestable: the only events whose `lifecycle_additional_context`
/// returned non-empty all had `supports_hook_specific_output: true`, so
/// the `systemMessage` branch was dead code in tests and a regression
/// could have shipped silently.
pub(crate) fn render_lifecycle_payload(event: &HookEvent, context: &str) -> JsonDocument {
    if event.supports_hook_specific_output {
        let mut hook_output = serde_json::json!({
            "hookEventName": event.name,
            "additionalContext": context,
        });

        // SessionStart: add watchPaths for key files so FileChanged fires
        // when CLAUDE.md, Cargo.toml, or settings change during the session.
        if event.name == "SessionStart" {
            if let Ok(cwd) = std::env::current_dir() {
                let watch_files: Vec<String> = [
                    "CLAUDE.md",
                    "Cargo.toml",
                    "package.json",
                    ".claude/settings.json",
                ]
                .iter()
                .map(|f| display_path(&cwd.join(f)))
                .collect();
                hook_output["watchPaths"] = serde_json::json!(watch_files);
            }
        }

        serde_json::json!({
            "hookSpecificOutput": hook_output,
            "suppressOutput": true,
        })
    } else {
        serde_json::json!({
            "systemMessage": context,
            "suppressOutput": true,
        })
    }
}

/// Look up the PascalCase event name for a kebab slug. Used by callers that have a
/// slug in hand but need to reason in the harness's PascalCase vocabulary.
pub(super) fn lifecycle_additional_context(subcommand: &str) -> String {
    match subcommand {
        "session-start" => session_start_context(),

        "pre-compact" => pre_compact_context(),

        "post-compact" => post_compact_context(),

        // UserPromptSubmit is intercepted before this match in `run_hook_command`

        // PostToolBatch fires after a batch of parallel tools resolves, just
        "post-tool-batch" => post_tool_batch_context(),

        // SubagentStart: inject a compact iron law reminder so spawned
        // subagents start with the core operating contract.
        "subagent-start" => subagent_start_context(),

        // Silenced events. Stop / SubagentStop are silenced because emitting
        "stop" | "subagent-stop" | "session-end" | "post-tool-use" | "post-tool-use-failure" => {
            String::new()
        }

        _ => String::new(),
    }
}

/// Compact SessionStart bootstrap contract.
///
/// This must land in the model's context *in full*. The harness truncates hook
/// `hookSpecificOutput.additionalContext` once it crosses ~10KB: the full text
/// is persisted to `<project>/tool-results/hook-…-additionalContext.txt` and the
/// model receives only a ~2KB preview plus a file pointer it never reads back.
/// The previous implementation injected the entire 27KB `using-keel/
/// SKILL.md` here, so in every project the bootstrap was silently truncated to
/// its first ~2KB — the model never saw the iron law's later rules, the MCP tool
/// list, the discipline pillars, or the skill catalog. Verified against live
/// session transcripts: the 27.6KB SessionStart additionalContext was replaced
/// by a 2KB preview while a 5.9KB UserPromptSubmit context landed intact.
///
/// The fix is to keep this block small enough to survive the cap. We drop the
/// ~8.5KB skill catalog enumeration (the harness already injects its own native
/// skill listing every session, so it was pure duplication) and the verbose
/// prose, keeping the operative contract: the iron law, the rationalization Red
/// Flags, the four discipline pillars, the always-on MCP tools, and the memory
/// writers. The full body still ships to disk as
/// `~/.claude/skills/using-keel/SKILL.md` (synced by `sync_skills`) and is
/// loadable on demand via `Skill("using-keel")` when the model wants the
/// complete catalog and routing rules. `~/.claude/CLAUDE.md` carries the same
/// compact contract through the hook-independent user-memory channel.
pub(super) const COMPACT_BOOTSTRAP: &str = r#"# keel operating contract (loaded at SessionStart)

<EXTREMELY_IMPORTANT>
This contract governs **every project you work in**, not just keel itself.
**Trust the codebase, not your knowledge base.** Knowledge-base recall is stale. Memories drift. The repository in front of you is the source of truth.

## The Iron Law — before you respond to anything that could touch code, configuration, or architecture
1. **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing implementation before claiming behavior. Never propose changes against an imagined version of the file.
2. **Understand before building.** Restate what the request actually asks, confirm the user story, and research what is genuinely needed before writing code. Do not guess, do not assume, do not build against an imagined spec. The most expensive waste is not buggy code — it is correct code that solved the wrong problem. If the request is ambiguous in a way that changes what you build, ask before building, not after.
3. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool to invoke it BEFORE writing code or giving a final answer. This is not negotiable. You cannot rationalize your way out of it.
4. **Find the root cause.** Suspicion is a hypothesis, not a finding. Take the symptom as a starting point, trace it end-to-end against the running code with file:line evidence, and confirm the suspected target sits on that path before changing anything.
5. **Preserve existing data.** Never remove or replace an existing field, column, output, or record to fit a new format — ADD alongside, and ASK before dropping anything the user did not name. Data loss in an edit is destructive like `DROP TABLE`. Autonomy covers reversible choices, never data deletion or a changed data contract; when a request could mean "add" or "replace", ask before acting.

This is the **Iron Law** of keel. It is loaded into your context at SessionStart and applies to every prompt thereafter — if asked whether the Iron Law is in your context, the answer is yes: it is the rules above.

**Hard enforcement:** PreToolUse **denies** edit-class tools until this session has used a **keel research tool** (`system_map` / `recall` / `context_brief` / `skill_route` / `skill_get` / `code_search`, or matching `keel …` CLI). Plain Read alone does not clear the gate (`KEEL_IRON_LAW_GATE=strict` default). Working-brief and review closeout gates default to hard feed-forward until satisfied.
</EXTREMELY_IMPORTANT>

## Red Flags (rationalizations to ignore)
- "I remember this codebase" → Memories drift. Read SYSTEM_MAP and the owning file before claiming behavior.
- "The user story is clear" → Stories are summaries, not specs. Find the root cause.
- "I get the gist, I'll start building" → The gist is not the spec. Restate the request and research what's needed; building on a guess ships the wrong thing.
- "I'll just code this quickly" → Skills tell you HOW. Check first.
- "Oh this may be the case" → Suspicion is a hypothesis, not a finding. Confirm the suspect sits on the symptom's traced path with file:line evidence before changing it.
- "Tests already passed earlier" → Re-run before claiming. No completion claims without fresh evidence.
- "I'll just remove this field to match the format" → ADD alongside; format copies style, not omissions. If you would note the removal after, ask before instead.
- "That hook reminder is wrapper noise" → It states the rule inline so it is self-contained in any repo. Re-read the diff against the rule before skipping.

## Code Implementation Discipline (every code-touching turn)
1. **Think Before Coding** — state assumptions, surface tradeoffs, and deep-dive any suspected target (read it, trace callers/callees against the failing trigger) before changing it.
2. **Simplicity First** — the minimum code that solves the problem. No speculative features, no abstractions for single-use code, no error handling for impossible scenarios.
3. **Surgical Changes** — touch only what the task requires. Match existing style. Every changed line traces directly to the request. Do not refactor unrelated code.
4. **Goal-Driven Execution** — turn vague tasks into verifiable goals before coding. Reproduce or trace the symptom from the user story end-to-end before naming a root cause.
5. **Short Comments** — one line is the default; comments say *why*, never *what*. No multi-paragraph narrative blocks or design history in the code body — that belongs in the brief or commit. A comment that takes longer to read than the code it describes gets cut.

## keel MCP tools — always available, prefer over guessing
- `system_map` — call before any claim about a repository's structure or layout ("what is this project", "where does X live") instead of reading files blind.
- `recall` — call before claiming what you remember or previously learned; full-text search over your durable memory and working briefs.
- `run_command` — run noisy shell commands (test, build, lint, logs, search) through it so compacted output enters context instead of the raw stream.

## Skills & subagents
Specialist skills are installed under `~/.claude/skills/` (lifecycle, backend, cloud, security, `reviewer`, UI/UX, `preserve-existing-flow`, systematic-debugging, TDD, migrations, and more) — the harness lists them natively each session. Invoke by bare name, e.g. `Skill("reviewer")`. For the full catalog and routing rules, call `Skill("using-keel")`. Matching subagents in `.claude/agents/` handle delegated isolated-context work via the Agent tool. About to read or edit existing code? Invoke `preserve-existing-flow` first. Delivery is Anvil only (`anvil` MCP).

## Memory writes (when you learn something durable)
Working memory dies at compaction. To persist across sessions:
- `keel memory working-brief write` — when starting non-trivial work: capture the request, acceptance criteria, and files you expect to touch BEFORE coding so completion can be reconciled against it.
- `keel memory completion-gate check` — before claiming a task complete: returns the gate's verdict and points at any requirement with no evidence yet.
- SYSTEM_MAP auto-refreshes at session start, pre-compact, and session end — read it before repo-structure claims.

## The one thing to remember
**Understand before you build. Research first. Invoke relevant skills before responding. Find the root cause. The repository — not your training data — is the source of truth.**"#;

pub(crate) fn session_start_context() -> String {
    // SessionStart fires once per session and is the documented entry point
    let mut context = format!("{COMPACT_BOOTSTRAP}\n\n{}", memory_scope_summary());
    // PUSH actual workspace memory content (map head + newest brief + most
    let workspace_digest = workspace_memory_digest();
    if !workspace_digest.trim().is_empty() {
        context.push_str("\n\n");
        context.push_str(&workspace_digest);
    }
    if let (Ok(claude_home), Ok(cwd)) = (resolve_claude_home(""), std::env::current_dir()) {
        let cwd = cwd.to_string_lossy();
        // Instinct + synthesis are append-only extras. Cap each so a large
        let instinct_digest = learning::project_instinct_digest(&claude_home, &cwd);
        if !instinct_digest.trim().is_empty() {
            context.push_str("\n\n");
            context.push_str(&truncate_on_line_boundary(
                &instinct_digest,
                INSTINCT_DIGEST_MAX_BYTES,
            ));
        }
        let synthesis = learning::project_synthesis_nudge(&claude_home, &cwd);
        // Synthesis nudge: refine a template-state skill's prose. Gated by
        let enrichment_enabled = !std::env::var("CLAUDE_SKILLS_LEARNED_SKILL_ENRICH")
            .map(|value| value.trim().eq_ignore_ascii_case("off"))
            .unwrap_or(false);
        if enrichment_enabled && !synthesis.trim().is_empty() {
            context.push_str("\n\n");
            context.push_str(&truncate_on_line_boundary(
                &synthesis,
                SYNTHESIS_NUDGE_MAX_BYTES,
            ));
        }
    }
    context
}

pub(super) fn pre_compact_context() -> String {
    "Before compaction, preserve keel continuity: summarize active workflow stage, files changed, validation evidence, unresolved blockers, memory facts to save, and next review gate.".to_string()
}

pub(crate) fn post_compact_context() -> String {
    let mut context = format!(

        "After compaction, resume using keel automatically: reload workspace memory/system map, re-establish workflow proof state, and run review gates before final closeout.\n\n{}",

        memory_scope_summary()

    );
    // Re-PUSH the workspace digest after compaction: the original SessionStart
    let digest = workspace_memory_digest();
    if !digest.trim().is_empty() {
        context.push_str("\n\n");
        context.push_str(&digest);
    }
    context
}

/// Per-prompt research-first iron law.
///
/// Compact by design: every byte lands per prompt as input tokens. The full
/// bootstrap rides SessionStart; this hook **always** restates the mandatory
/// contract so it cannot drop out of the working window:
///   * lead with FOLLOW THE IRON LAW + USE KEEL (not optional prose)
///   * research-first + skill invoke + MCP tools
///   * memory loop: recall → brief → save durable learnings → learn
///   * discipline pillars + parallel fan-out guard
///
/// Kept separate so the bridge `user-prompt` subcommand can compose the full
/// per-prompt context from flat fields without needing stdin parsing.
pub(super) fn post_tool_batch_context() -> String {
    "Closeout check: non-trivial code changes (logic, multi-file, public API, security) need a reviewer pass before closeout. Trivial work (docs, formatting, typos) skips this. The brief/review gates are blunt — any code-changing session may get one bounded, clearable nudge (does not stop the turn); follow it to satisfy or disable (`=off`), or set `=block` for hard-stop. Project-level CLAUDE.md/AGENTS.md rules take precedence.".to_string()
}

/// SubagentStart context — injected into every spawned subagent so it starts
/// with the core operating contract instead of blind. Kept compact to avoid
/// burning subagent context on a wall of text.
pub(super) fn subagent_start_context() -> String {
    "keel iron law for this subagent: (1) Read SYSTEM_MAP and the owning file before claiming behavior. (2) Understand before building — restate the request and research what is needed. (3) Invoke relevant skills if there is even a 1% chance one applies. (4) Find the root cause — trace with file:line evidence before changing anything. Trust the codebase, not your knowledge base. Native MCP tools available: system_map, recall, run_command.".to_string()
}

// The PostToolBatch hook fires after a batch of tool calls resolves, just

pub(super) fn should_refresh_system_map(event_name: &str) -> bool {
    matches!(
        event_name,
        "SessionStart" | "PreCompact" | "SessionEnd" | "CwdChanged"
    )
}

/// Idempotently re-assert the `keel` MCP registration at session start.
///
/// This is the drift self-heal: `register_mcp_server` writes `~/.claude.json`
/// only when the live entry differs from the desired one (which carries
/// `alwaysLoad: true`), so this costs nothing on a healthy config and silently
/// repairs an entry left stale by any binary-swap path that never re-ran
/// install/update/repair. Honors `CLAUDE_TARGET_OVERRIDE` through
/// `resolve_claude_home`, and only writes for a standard `~/.claude` home
/// (`self_heal_registration` guards that), so the suite's throwaway homes are
/// never touched.
///
/// Best-effort: every failure path is swallowed to stderr. The caller must not
/// change its exit code based on this — the SessionStart context render is the
/// load-bearing work; MCP registration is additive.
pub(super) fn maybe_self_heal_mcp_registration(standard_error: &mut dyn Write) {
    if std::env::var(MCP_SELF_HEAL_ENV_VAR)
        .map(|value| value.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let claude_home = match resolve_claude_home("") {
        Ok(path) => path,
        Err(_) => return,
    };
    match crate::manager::mcp_register::self_heal_registration(&claude_home) {
        // Skipped (non-standard home) or already current to nothing to report.
        None | Some(Ok(crate::manager::mcp_register::McpRegistration::AlreadyCurrent)) => {}
        Some(Ok(crate::manager::mcp_register::McpRegistration::Added)) => {
            let _ = writeln!(
                standard_error,
                "keel: registered keel MCP server in ~/.claude.json (alwaysLoad). Restart the harness to load the tools into context."
            );
        }
        Some(Ok(crate::manager::mcp_register::McpRegistration::Updated)) => {
            let _ = writeln!(
                standard_error,
                "keel: repaired drifted keel MCP entry in ~/.claude.json (alwaysLoad). Restart the harness to load the tools into context."
            );
        }
        Some(Err(error)) => {
            let _ = writeln!(standard_error, "keel: MCP self-heal skipped ({error})");
        }
    }
}
