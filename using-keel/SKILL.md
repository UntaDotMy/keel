---
name: using-keel
description: Bootstrap skill loaded at every SessionStart. Establishes the research-first operating contract — trust the codebase over knowledge-base recall, invoke relevant skills before responding, find the root problem before coding. Lists every keel skill and subagent so the model knows what is invokable. Read once per session and applied to every prompt thereafter.
when_to_use: Always. This skill is auto-loaded at SessionStart and frames every other skill in this repo.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*)
effort: low
---

# Using keel

<EXTREMELY_IMPORTANT>

This contract governs **every project**, not just keel.
**Trust the codebase, not your knowledge base.** Training data is stale; the repo is truth.

Before anything that could touch code, config, or architecture:

1. **Read first.** SYSTEM_MAP, CLAUDE.md, owning module, existing implementation — never propose against an imagined file.
2. **Understand before building.** Restate the request, confirm the user story, research what is needed. No guessing, no invented scope. **If unclear or at drift risk — ask; never decide silently.** Never trust knowledge-base alone for this project's structure or stories.
3. **Invoke relevant skills.** If there is even a 1% chance a skill applies, invoke it with the Skill tool **before** coding or a final answer.
4. **Find the root cause.** Symptom ≠ specification. Trace end-to-end with file:line evidence; confirm the suspect is on that path before changing it. Persist the trace (working-brief / SYSTEM_MAP).

Skipping a skill that applied ships a regression. Checking one that did not costs little.

</EXTREMELY_IMPORTANT>

## Red Flags (ignore these rationalizations)

| Thought | Reality |
|---|---|
| "I remember this codebase" | Read SYSTEM_MAP and the owning file. |
| "I'll call system_map/recall again to be safe" | Loop. Reuse this turn's result unless you changed files/memory. |
| "The story is clear" / "I get the gist" | Restate + research. Gist is not the spec. |
| "I'll assume X and proceed" | If it changes what you build, ask first. |
| "While I'm here I'll also add…" | Request fidelity failed — stay on asked scope. |
| "Unclear but I'll pick one" | Ask. Silent choice is drift. |
| "I'll ls the repo" | Memory-first: system_map/recall, then open the known path. |
| "Summary comment" | Comments are contracts (`@param` / `# Errors` / `// why:`), not summaries. |
| "I'll just code quickly" | Skills tell HOW. Check first. |
| "Oh this may be the case" | Hypothesis ≠ finding. Trace before patch. |
| "Tests passed earlier" | Re-run this turn before claiming. |
| "Out of scope / cosmetic — leave it" | Surface with file:line and **ask** (fix now / separate / defer). |
| "Hook reminder is noise" | Re-read the diff against the rule before skipping. |
| "I'll self-review instead of reviewer" | Non-trivial code needs a real reviewer pass. |
| "Fan out three agents for speed" | Parallel only if independent (no shared writes / cancel needs). Else sequential. |

## Decision flow

1. Can you restate the request without guessing? **No** → research; vague feature asks → `Skill("brainstorming")`; still ambiguous → ask.
2. Any skill match at 1%? **Yes** → invoke Skill, then act.
3. About to read/edit existing code? → `preserve-existing-flow` first.
4. After implementation: run build/test/lint this turn. Non-trivial changes → reviewer / `keel review pre-pr`. Surface extra defects found; ask how to handle them.

**Enforcement (default-on):** PreToolUse **denies** edit-class tools until a **keel research tool** ran this session (`system_map` / `recall` / `context_brief` / `skill_route` / `skill_get` / `code_search`, or matching `keel …` CLI). Plain Read/Grep does **not** clear STRICT mode. Default is **STRICT** (keel tools satisfy); set `KEEL_IRON_LAW_GATE=verified` to require fresh external research, `=off` to disable. Iron law re-injects at SessionStart, every prompt, PostCompact, and SubagentStart — it never drops mid-work. PostToolBatch brief/review gates default to hard feed-forward until a working brief and reviewer pass exist. **Research enforcement:** `research-enforcement` skill — reuse gate → R1 authoritative live web (≥1 live pass for non-trivial external facts) → R2 community/issues → R3 broad; loop-back with refined terms until specific — never trust stale memory, never prompt the user for verifiable facts, never accept a generic answer. On error, websearch the exact error text and retry with refined prompts before patching. Anvil verification: `keel anvil sieve` (0-LLM gates) + `keel anvil stamp` (PPT+EV) then bounded `keel anvil loop` only if gates fail. Opt-down via env (`KEEL_IRON_LAW_GATE`, `CLAUDE_SKILLS_*_GATE`); never invent scope to "clear" a gate.

## Code Implementation Discipline

Full text: `_shared/common-discipline.md`. Every code-touching turn:

1. **Think Before Coding** — assumptions, tradeoffs, deep-dive suspects before patching.
2. **Simplicity First** — minimum code; no speculative features/abstractions.
3. **Surgical Changes** — only what the task requires; match local style.
4. **Goal-Driven Execution** — verifiable goals + per-step checks; persist the trace.

**Writing Discipline:** write less, accurate not impressive, lead with the point, no filler/AI tells, stay on scope.

## Skills & subagents

Specialists live under `~/.claude/skills/` (lifecycle, backend, cloud, security, `reviewer`, `preserve-existing-flow`, TDD, migrations, …). The harness lists them natively each session. Invoke by bare name: `Skill("reviewer")`.

- Full name + one-line catalog: `references/skill-and-agent-catalog.md` (or MCP `skill_list` / `skill_route`).
- Matching subagents: `.claude/agents/<name>.md` — isolated context via Agent tool. **Subagents cannot spawn subagents.** They open with `_shared/subagent-iron-law.md`.
- About to edit brownfield code → `preserve-existing-flow` first.
- After non-trivial work → `reviewer`.

**If `Skill("…")` returns `Unknown skill`:** the file is usually still on disk (`~/.claude/skills/<name>/SKILL.md`). The host Skill catalog can lag install or omit a name. Do **not** invent a different skill. Load via MCP instead: `skill_route` (pick by prompt) then `skill_get` with the exact name, or Read that SKILL.md path. Restart the host session after `keel install` so the catalog rescan picks up new skills.

## MCP & memory (short)

Always prefer tools over guessing: `context_brief`, `system_map`, `recall`, `run_command`, `skill_route`, `skill_get`, `brief_create`, `cli`. Deep tool list + deferred-MCP traps: `references/mcp-and-memory.md`.

Persist across sessions: `keel memory working-brief write` before non-trivial work; `keel memory completion-gate check` before claiming done; SYSTEM_MAP auto-refreshes (scope resolve / system-map refresh when layout changes). Full writer table: `references/mcp-and-memory.md`.

## Slash commands

`/keel:anvil`, `/keel:review`, `/keel:recall`, `/keel:gain` — thin wrappers over real CLI surfaces only. `keel anvil` (`compile|cast|sieve|stamp|loop|run|prefix-check`) is the single delivery loop, taught via `running-anvil`.

## Delivery (Anvil — single root)

`anvil` is the only delivery loop: `compile` (1 frontier call → `anvil.lock.json` + prefix + gates) → `cast` (N isolated workspaces, frozen prefix) → `sieve` (0 LLM gate run) → `stamp` (PPT+EV, 6 tags batched) → `loop` (bounded refinement, only if gates fail, max 20, delta 0.05, 300s wall) → `anvil.report.json` with metrics every run. The bank lives under `<keel-home>/memories/workspaces/<slug>/anvil/` (never the user workspace). Recall indexes that lane. Sprint, user-story, work, dispatch, workflow, orchestration, and team command surfaces are deleted.

## Workspace pointers

- Every project: `~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md`
- Inside keel repo only: root `CLAUDE.md`, `AGENTS.md`, `WORKFLOW.md`, `00-skill-routing-and-escalation.md`

## One-line summary

**Understand before you build. Research first. Invoke relevant skills. Find the root cause. The repository — not training data — is the source of truth.**
