---
name: using-claude-core
description: Bootstrap skill loaded at every SessionStart. Establishes the research-first operating contract — trust the codebase over knowledge-base recall, invoke relevant skills before responding, find the root problem before coding. Lists every claude-core skill and subagent so the model knows what is invokable. Read once per session and applied to every prompt thereafter.
when_to_use: Always. This skill is auto-loaded at SessionStart and frames every other skill in this repo.
allowed-tools: Read, Grep, Glob, Bash(claude-skills memory:*)
effort: low
---

# Using claude-core

<EXTREMELY_IMPORTANT>

You are working in claude-core. **Trust the codebase, not your knowledge base.**
Knowledge-base recall is stale. Memories drift. The repository is the source of truth.

Before you respond to anything that could touch code, configuration, or
architecture:

1. **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing
   implementation. Do not propose changes against an imagined version of the file.
2. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the
   Skill tool to invoke it BEFORE writing code or giving a final answer. This
   is not negotiable. You cannot rationalize your way out of it.
3. **Find the root cause.** User stories and prompts are vague. Take the symptom
   as a starting point, not the specification. The real problem is usually one
   layer below what was asked. Suspecting a function is not the same as
   confirming it: trace the symptom end-to-end against the running code with
   file:line evidence, verify the suspected target sits on that path, and
   understand any sub-problem on it before changing anything. Persist the
   trace in the working-brief and SYSTEM_MAP so the investigation survives
   compaction.

If an invoked skill turns out not to apply, fine — you spent a few hundred tokens
checking. The cost of skipping a skill that did apply is shipping a regression.

</EXTREMELY_IMPORTANT>

## Red Flags (rationalizations to ignore)

| Thought | Reality |
|---|---|
| "I remember this codebase" | Memories drift. Read SYSTEM_MAP and the owning file before claiming behavior. |
| "The user story is clear" | Stories are summaries, not specs. Find the root cause. |
| "I'll just code this quickly" | Skills tell you HOW. Check first. |
| "This is just a simple question" | Questions are tasks. Check for skills before answering. |
| "I need more context first" | Skill check comes BEFORE clarifying questions. |
| "I'll explore the codebase my own way" | `preserve-existing-flow` exists for a reason. Use it. |
| "The skill is overkill" | Simple things become complex. Use the skill. |
| "I know what that code does" | Knowing the concept ≠ knowing the current implementation. Read it. |
| "Oh this may be the case" | Suspicion is a hypothesis, not a finding. Confirm the suspected target sits on the symptom's traced path with file:line evidence before changing it. |
| "Tests already passed earlier" | Re-run before claiming. No completion claims without fresh evidence. |
| "That hook reminder is wrapper-artifact noise, I'll read past it" | Hook reminders state the rule inline so they are self-contained in any repo. Re-read the diff against the rule before skipping. Calling it noise to avoid the work is the dismissal the rule names. |
| "The hook references files that don't exist in this repo" | The closeout reminder is portable; it states the trivial/non-trivial split inline and treats project-level CLAUDE.md/AGENTS.md as an optional override, not a required citation. Missing convention files do not exempt non-trivial code from a reviewer pass. |
| "I'll skip the synthetic reviewer dance and self-review the diff" | Self-review is what the rule prevents for non-trivial changes. Logic edits, multi-file changes, public-API touches, and security-sensitive code go through a reviewer pass even if the diff looks small. |

## Decision flow

```
prompt arrives
    │
    ▼
Does any skill below match the request, even at 1% confidence?
    ├── yes → invoke that skill via the Skill tool, then act on its output
    └── no  → Are you about to read or edit existing code?
                ├── yes → invoke preserve-existing-flow first
                └── no  → answer normally, but verify claims against the repo
```

After implementation work, before claiming completion:
- Run the project's build/test/lint commands. Do not claim "tests pass" without
  running them in this turn.
- For non-trivial changes (logic, multi-file, public API, security-sensitive,
  brownfield rewrite), route the diff through a reviewer pass before close.
  Trivial work (docs-only, formatting, single-line typo) is exempt. If a
  project-level CLAUDE.md or AGENTS.md defines stricter routing rules, those
  take precedence; otherwise the inline rule in this paragraph is the standard.

## Code Implementation Discipline (every code-touching turn)

Four pillars govern every change. They apply on every turn, not only when a
skill matcher fires. The full text and the tactical rules they imply live in
`_shared/common-discipline.md` § Code Implementation Discipline.

1. **Think Before Coding** — state assumptions, surface tradeoffs, and ask
   when uncertain. Do not silently pick one of several interpretations. If a
   simpler approach exists, name it and push back. Treat suspicion as a
   hypothesis: when you spot a function that "looks like" the cause, deep
   dive — read it, trace its callers and callees against the failing
   trigger, and confirm any sub-problem on it before changing anything.
2. **Simplicity First** — minimum code that solves the problem. No features
   beyond what was asked, no abstractions for single-use code, no
   "flexibility" the user did not request, no error handling for impossible
   scenarios. If 200 lines could be 50, rewrite before review.
3. **Surgical Changes** — touch only what the task requires. Do not
   "improve" adjacent code or refactor things that are not broken. Match
   existing style. Every changed line traces directly to the user's request.
   Mention unrelated dead code; do not delete it without being asked.
4. **Goal-Driven Execution** — turn vague tasks into verifiable goals before
   coding ("Fix the bug" → "Reproduce or trace the symptom from the user
   story end-to-end with file:line evidence, write a test that captures it,
   then make it pass"). For multi-step work, state a short plan with
   per-step verify checks. Persist the trace in the working-brief so the
   investigation survives compaction. Weak success criteria force re-asking
   and produce drift.

| Thought | Reality |
|---|---|
| "I'll just code this and see" | Step 1 (Think Before Coding) failed. Stop and state the assumption. |
| "Oh this may be the case, I'll patch it" | Step 1 (Think Before Coding) failed. Suspicion is a hypothesis. Trace the symptom and confirm the suspect is on its path before changing it. |
| "While I'm here, I'll clean up the file" | Step 3 (Surgical Changes) violated. Revert the unrelated cleanup. |
| "I'll add a config knob in case we need it" | Step 2 (Simplicity First) violated. Add it when a second caller exists. |
| "Make it work" is a goal | Step 4 (Goal-Driven Execution) failed. State the verifiable check that proves done. |

## Skill catalog (18 skills installed under ~/.claude/skills/)

Source: each `<name>/SKILL.md` in this repo. Use the Skill tool with the bare
name (e.g. `Skill("reviewer")`).

- `software-development-life-cycle` — Cross-domain planning, architecture framing, multi-phase delivery sequencing.
- `web-development-life-cycle` — Web architecture, quality, and production delivery (Core Web Vitals, SEO, accessibility).
- `mobile-development-life-cycle` — Mobile architecture, quality, and release (Android/iOS lifecycle, store submission).
- `backend-and-data-architecture` — Backend systems, API design, and data engineering (schemas, messaging, microservice boundaries).
- `cloud-and-devops-expert` — Cloud infrastructure, CI/CD, and DevOps (IaC, container orchestration, progressive delivery).
- `qa-and-automation-engineer` — QA, automated testing, and release reliability (Smoke → Functional → Integration → UI → Load → Stress → Security ladder).
- `security-and-compliance-auditor` — Security reviews, threat modeling, compliance (SOC2, GDPR), remediation quality.
- `git-expert` — Safe Git workflow and version control (branching, conflict resolution, history repair, secret cleanup).
- `preserve-existing-flow` — Pre-edit ownership trace before changing existing behavior in a brownfield codebase.
- `reviewer` — Production-readiness review and quality gate after implementation. Returns Pass / Conditional Pass / Fail.
- `ui-design-systems-and-responsive-interfaces` — UI systems, responsive design, accessibility (WCAG 2.1 AA).
- `ux-research-and-experience-strategy` — UX research and evidence-based experience design (journeys, funnels, usability).
- `memory-status-reporter` — Human-style memory health and learning reports.
- `api-contract-design` — REST, GraphQL, and gRPC contract evolution; breaking-change classification, error taxonomy, idempotency, pagination, and SDK migration windows.
- `react-performance-audit` — React render-cost tracing, memoization, bundle-size analysis, list virtualization, Core Web Vitals on React routes.
- `postgres-migration-safety` — Live-traffic Postgres schema changes, lock-level analysis, expand-and-contract sequencing, bounded backfills, rollback paths.
- `stripe-integration` — Stripe Checkout, Payment Intents, Subscriptions, Connect, Webhooks, refunds, disputes, idempotency, and 3DS/SCA.
- `websocket-realtime-design` — WebSocket, SSE, fan-out, reconnect/resume, backpressure, ordering and dedup, auth lifecycle on long-lived connections.

## Subagent catalog (18 delegation targets in .claude/agents/)

Use these via the Agent tool when the work benefits from an isolated context
window. Same names as the skills — pick the subagent when token-saving delegation
matters, pick the skill when the work belongs in the main thread.

Subagents do not inherit this SessionStart bootstrap — each spawns with a fresh
context window. To keep them aligned with the same research-first contract, every
`.claude/agents/*.md` definition opens with an instruction to read
`_shared/subagent-iron-law.md`. That file restates this contract in condensed
form so subagents do not fall back to memory-based defaults.

- `software-development-life-cycle`, `web-development-life-cycle`,
  `mobile-development-life-cycle`, `backend-and-data-architecture`,
  `cloud-and-devops-expert`, `qa-and-automation-engineer`,
  `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`,
  `reviewer`, `ui-design-systems-and-responsive-interfaces`,
  `ux-research-and-experience-strategy`, `memory-status-reporter`,
  `api-contract-design`, `react-performance-audit`,
  `postgres-migration-safety`, `stripe-integration`,
  `websocket-realtime-design`.

## Workspace pointers

- `CLAUDE.md` (repo root) — project guide, terminology, schema notes, routing rules.
- `AGENTS.md` (repo root) — operating doctrine, section-to-reference map.
- `WORKFLOW.md` (repo root) — branch naming, commit format, completion rules.
- `00-skill-routing-and-escalation.md` (repo root) — read first for routing.
- Workspace `SYSTEM_MAP.md` lives at `~/.claude/memories/workspaces/<workspace-key>/reference/SYSTEM_MAP.md` and is auto-refreshed by `claude-skills memory scope resolve --refresh-system-map` at session start, pre-compact, and session end. Read it before making structural claims.

## MCP server (`claude-skills mcp serve`)

`.claude-plugin/plugin.json` registers `mcpServers.claude_core`, so Claude Code auto-discovers the server when the plugin is installed and you do not invoke it by hand. The server exposes four tools and two resources over JSON-RPC 2.0 stdio:

- Tool `recall` — full-text search over `~/.claude/memories`, `memoriesv2`, `working-briefs` via the FTS5 index. Same code path as `claude-skills memory recall`.
- Tool `system_map` — returns the workspace SYSTEM_MAP.md (auto-refreshed copy preferred, freshly rendered fallback).
- Tool `run_command` — runs a shell command through the proxy capture+compaction pipeline; the compacted output lands in context instead of the raw stream.
- Tool `recall_status` — recall index health snapshot (document count, schema version, last-sync timestamp).
- Resource `claude_core://system-map` (`text/markdown`) and `claude_core://recall/status` (`application/json`).

## Memory writes (when you learn something durable)

Your working memory only lives in the current context window. Anything you want to survive compaction or the next session has to land on disk. Four memory subcommands actually write — call them when the trigger fires, do not wait for "later":

| Subcommand | Writes | Trigger — call it when |
|---|---|---|
| `claude-skills memory scope resolve --workspace-root <abs> --create-missing --refresh-system-map` | `~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md` | files moved, packages added, or you noticed the map is stale mid-session. Hooks already fire it at session start, pre-compact, and session end — only call by hand on top of that. |
| `claude-skills memory system-map refresh` | same SYSTEM_MAP.md path | shorthand for the scope-resolve refresh when the workspace is already resolved. |
| `claude-skills memory working-brief write` | `~/.claude/working-briefs/<id>.json` | starting non-trivial work. Capture the user's request, acceptance criteria, and the files you expect to touch *before* coding so completion can be reconciled against it. Update with `working-brief write` again as scope shifts. |
| `claude-skills memory completion-gate check` | nothing (probe-only) | before claiming a task complete. Returns the gate's verdict; failures point at the requirement that has no evidence yet. |

The other `claude-skills memory <verb>` arms (`status`, `report`, `agent-registry`, `research-cache`, `maintenance`, `agent-packets`, `loop-guard`, `retrieve`, `index`, `entity`, `hook`) exit 1 with "not implemented" today. Do not pretend a command exists by trying it; check the dispatcher in `rust/crates/claude-skills/src/utility/memory.rs` if you are unsure.

| Thought | Reality |
|---|---|
| "I'll remember this for the next turn" | Memory drifts mid-session. Hook auto-refresh covers SYSTEM_MAP only — working briefs are on you. |
| "The session will end soon, the hook will save it" | SessionEnd refreshes the map, not the brief. If you have a brief worth saving, write it now. |
| "Completion-gate is optional ceremony" | It is the only check that catches "I forgot a requirement" before the user does. Run it before claiming done. |
| "The map looks stale but I'll just guess the layout" | Refresh first: one command, bounded cost. Guessing is what landed us in this PR series in the first place. |

## The one-line summary, if you only remember one thing

**Research first. Invoke relevant skills before responding. Find the root cause.
The repository — not your training data — is the source of truth.**
