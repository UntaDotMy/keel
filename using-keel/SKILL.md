---
name: using-keel
description: Bootstrap skill loaded at every SessionStart. Establishes the research-first operating contract — trust the codebase over knowledge-base recall, invoke relevant skills before responding, find the root problem before coding. Lists every keel skill and subagent so the model knows what is invokable. Read once per session and applied to every prompt thereafter.
when_to_use: Always. This skill is auto-loaded at SessionStart and frames every other skill in this repo.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*)
effort: low
---

# Using keel

<EXTREMELY_IMPORTANT>

This contract governs **every project you work in**, not just keel itself.
**Trust the codebase, not your knowledge base.**
Knowledge-base recall is stale. Memories drift. The repository in front of you is
the source of truth.

Before you respond to anything that could touch code, configuration, or
architecture:

1. **Read first.** Read SYSTEM_MAP, CLAUDE.md, the owning module, and the existing
   implementation. Do not propose changes against an imagined version of the file.
2. **Understand before building.** Before you write any code, restate what the
   request actually asks, confirm the user story, and research what is genuinely
   needed. Do not guess. Do not assume. Do not blindly start building against an
   imagined spec. The vast majority of wasted work is not buggy code — it is
   correct code that solved the wrong problem. An hour of research is always
   cheaper than shipping the wrong thing and rebuilding it. If the request is
   ambiguous in a way that changes what you build, ask before building, not after.
3. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the
   Skill tool to invoke it BEFORE writing code or giving a final answer. This
   is not negotiable. You cannot rationalize your way out of it.
4. **Find the root cause.** User stories and prompts are vague. Take the symptom
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
| "I already called `system_map`/`recall` but I'll call it again to be safe" | That is the loop. These tools return the same data within a turn unless *you* changed something. Re-read the result already in your context; call again only after you have created, moved, deleted files or written new memory this turn. |
| "The user story is clear" | Stories are summaries, not specs. Find the root cause. |
| "I get the gist, I'll start building" | The gist is not the spec. Restate the request, confirm the user story, research what's needed. Building on a guess is how you ship the wrong thing. |
| "I'll assume they meant X and proceed" | Assuming is guessing with confidence. If the assumption changes what you build, confirm it first — do not build then apologize. |
| "Research is slower than just coding it" | Research is slower than starting; it is far faster than finishing the wrong thing twice. The hour you save guessing, you pay back with interest. |
| "I'll just code this quickly" | Skills tell you HOW. Check first. |
| "This is just a simple question" | Questions are tasks. Check for skills before answering. |
| "I need more context first" | Skill check comes BEFORE clarifying questions. |
| "I'll explore the codebase my own way" | `preserve-existing-flow` exists for a reason. Use it. |
| "The skill is overkill" | Simple things become complex. Use the skill. |
| "I know what that code does" | Knowing the concept ≠ knowing the current implementation. Read it. |
| "Oh this may be the case" | Suspicion is a hypothesis, not a finding. Confirm the suspected target sits on the symptom's traced path with file:line evidence before changing it. |
| "Tests already passed earlier" | Re-run before claiming. No completion claims without fresh evidence. |
| "I noticed a second bug but it's out of scope/pre-existing/cosmetic, I'll leave it" | Surfacing ≠ touching. A defect you found is information the user doesn't have (information asymmetry — you see the code, they don't). Name it with file:line evidence and **ask** whether to fix it now, in a separate change, or defer. "Explicitly defer" is a valid *user* choice; "silently skip" is not a valid *agent* choice. The cosmetic-looking double-label was a 14-site class bug. |
| "That hook reminder is wrapper-artifact noise, I'll read past it" | Hook reminders state the rule inline so they are self-contained in any repo. Re-read the diff against the rule before skipping. Calling it noise to avoid the work is the dismissal the rule names. |
| "The hook references files that don't exist in this repo" | The closeout reminder is portable; it states the trivial/non-trivial split inline and treats project-level CLAUDE.md/AGENTS.md as an optional override, not a required citation. Missing convention files do not exempt non-trivial code from a reviewer pass. |
| "I'll skip the synthetic reviewer dance and self-review the diff" | Self-review is what the rule prevents for non-trivial changes. Logic edits, multi-file changes, public-API touches, and security-sensitive code go through a reviewer pass even if the diff looks small. |
| "I'll fan out three agents at once to look fast" | Parallel fan-out is for genuinely independent work — different domains on the same artifact, disjoint read-only sweeps. If two agents could touch the same file, depend on each other's output, or one finding could cancel another's work, dispatch them sequentially. See AGENTS/references/30-execution-strategy.md § 0.6. |

## Decision flow

```
prompt arrives
    │
    ▼
Do you actually understand what is being asked? Can you restate the request and
the user story without guessing or assuming?
    ├── no  → research first: read the request, the codebase, and what's needed.
    │         For a vague or directive feature ask ("add X", "build Y", "make Z
    │         work") whose user story is not confirmed, invoke Skill("brainstorming")
    │         to restate and confirm before building. If still ambiguous in a way
    │         that changes what you build, ask.
    └── yes → Does any skill below match the request, even at 1% confidence?
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
- **Surface defects you found during the work** — including cosmetic or
  pre-existing ones your change did not cause. Do not silently defer them with
  "out of scope, I'll leave it." Name the finding (file:line) and ask the user
  whether to fix it now, in a separate change, or explicitly defer. The user
  cannot direct work on a defect they never learn exists. (See the decision note
  `decision-surface-out-of-scope-defects.md` in the project memory lane.)

> **Default-on enforcement gates.** Two PostToolBatch gates are **on by
> default** — they are the only model-independent backstop for the Iron Law,
> since hooks cannot force a tool/Skill call but can inject a reminder (and,
> opt-in, refuse to let a turn close) when a required artifact is missing. By
> default each fires as a **non-blocking nudge**: the reminder is injected via
> `hookSpecificOutput.additionalContext`, so you are told to act but the turn is
> **not** halted.
> - **Working-brief gate** (front of the law) — if a session changes code but no
>   working brief was written this session, the hook nudges once to write one
>   (`keel memory working-brief write --request "..." --acceptance-criteria "..."`,
>   which clears the gate). Set `CLAUDE_SKILLS_BRIEF_GATE=block` for the opt-in
>   hard stop, or `=off` to disable.
> - **Review gate** (back of the law) — if a session changed code but recorded no
>   reviewer pass since the last edit, the hook nudges once to run a review
>   (invoke the `reviewer` skill, or run `keel review pre-pr`, which
>   clears the gate). Set `CLAUDE_SKILLS_REVIEW_GATE=block` for the opt-in hard
>   stop, or `=off` to disable.
>
> Each gate fires **at most `…_MAX_BLOCKS` time(s) per session (default 1)** via
> a strictly-increasing counter — whether nudging or blocking — and then
> permanently falls through to the generic advisory, so the pair can neither loop
> nor spam. Both fail open (no session id, unreadable telemetry, or a render
> error degrade to the advisory reminder, never a block) and both are
> instantly disablable with their env var or `…_MAX_BLOCKS=0`. A determined model
> can still write a token brief to clear the front gate — that is the
> acknowledged ceiling of artifact-existence enforcement, which is why the
> per-prompt reminders and this contract still matter.

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

**Writing Discipline** governs every word you write — docs, code comments, commit/PR text, review notes, and chat replies. Write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.

## Skill catalog (every matcher-invocable skill, installed under ~/.claude/skills/)
<!-- The count is deliberately not stated; it drifts. Every skill in .claude-plugin/plugin.json
     is listed below; this bootstrap (`using-keel`) is the only first-party skill NOT in the
     manifest (it loads at SessionStart). The structural invariant (disk == manifest + 1) is
     asserted by tests/doc_parity_test.rs. Run `keel skill-lint` for the live verified count. -->


Source: each `<name>/SKILL.md` in this repo. Use the Skill tool with the bare
name (e.g. `Skill("reviewer")`). The count excludes this bootstrap skill itself
(`using-keel`), which is always loaded at SessionStart rather than
invoked on demand. `requesting-code-review` below is an alias pointer, not a
separate skill directory — it routes to `reviewer`.

- `software-development-life-cycle` — Cross-domain planning, architecture framing, multi-phase delivery sequencing.
- `web-development-life-cycle` — Web architecture, quality, and production delivery (Core Web Vitals, SEO, accessibility).
- `mobile-development-life-cycle` — Mobile architecture, quality, and release (Android/iOS lifecycle, store submission).
- `dart-and-flutter-expert` — Dart & Flutter: widget architecture (pure `build`, `const` constructors), state management (Provider/Riverpod/Bloc), jank diagnosis (`ListView.builder`, `RepaintBoundary`), isolates for >16ms work, null-safety without `!`, pubspec hygiene, platform channels, Flutter web/desktop.
- `backend-and-data-architecture` — Backend systems, API design, and data engineering (schemas, messaging, microservice boundaries).
- `cloud-and-devops-expert` — Cloud infrastructure, CI/CD, and DevOps (IaC, container orchestration, progressive delivery).
- `qa-and-automation-engineer` — QA, automated testing, and release reliability (Smoke → Functional → Integration → UI → Load → Stress → Security ladder).
- `security-and-compliance-auditor` — Security reviews, threat modeling, compliance (SOC2, GDPR), remediation quality.
- `git-expert` — Safe Git workflow and version control (branching, conflict resolution, history repair, secret cleanup).
- `preserve-existing-flow` — Pre-edit ownership trace before changing existing behavior in a brownfield codebase.
- `reviewer` — Production-readiness review and quality gate after implementation. Returns Pass / Conditional Pass / Fail.
- `brainstorming` — Socratic design exploration before implementation: refine an open-ended idea into a concrete, agreed design with trade-offs, captured in the working brief before any code. The generative front half of Think-Before-Coding.
- `writing-user-stories` — Convert a requirement-bearing prompt completely into strict Agile/Jira user stories (Connextra "As a/I want/so that" + Gherkin Given/When/Then, validated against INVEST), confirm them with the user via AskUserQuestion, and capture them in the working brief as the anti-drift spec. Runs first on any feature/change/fix ask, before brainstorming or coding. Validate format with `keel user-story lint`.
- `running-a-sprint` — Run the confirmed user stories as a Scrum-style sprint loop: backlog → per-story implement→verify-against-Gherkin→review → LOOP until every story meets Definition of Done → increment + retro. Use for multi-story or multi-step builds that must finish completely, not partially. Backed by `keel sprint` (durable per-story state, fail-closed review gate). The orchestration layer above writing-user-stories and the implementation skills.
- `test-driven-development` — The tight RED-GREEN-REFACTOR loop: write the failing test first, make it pass with the minimum change, refactor under green. The per-change companion to qa-and-automation-engineer's coverage strategy.
- `systematic-debugging` — Root-cause-first defect work: reproduce the symptom, trace it end-to-end with file:line evidence, fix the source of truth, prove it with a regression test. Use instead of patching the first suspicious line.
- `writing-plans` — Turn an agreed design into an ordered, per-step-verifiable implementation plan (each step names its files and its check), captured in the working brief. The front half of execution.
- `executing-plans` — Drive a captured plan to done one step at a time, running each step's verification check before advancing and stopping on a failed check. The back half of planning.
- `subagent-driven-development` — Delegate self-contained plan tasks to fresh-context subagents to preserve the controller's window, then integrate and re-verify in the main thread.
- `dispatching-parallel-agents` — Fan out genuinely independent work concurrently (the four-condition independence test), and sequence work that fails the test instead of colliding.
- `using-git-worktrees` — Isolate feature or experimental work in its own checkout (prefer native harness isolation, fall back to a git worktree) so parallel work and the main tree never collide; clean up on merge or abandon.
- `finishing-a-development-branch` — Close out a completed branch: verify the full suite, confirm the completion gate, review non-trivial work, then present merge/PR/cleanup options rather than acting unilaterally.
- `receiving-code-review` — Act on review feedback as the author: judge each point on merit, fix valid ones at the root cause with evidence, push back on wrong ones with evidence, re-verify before claiming addressed.
- `requesting-code-review` — see `reviewer`; route a non-trivial diff through the fail-closed review gate.
- `writing-skills` — Author and revise skills with evidence the prose changes behavior: RED-GREEN-REFACTOR on the instructions themselves, pressure-testing a fresh subagent without the skill, then with it. The behavioral gate above skill-lint's structural gate.
- `designing-agent-teams` — Decompose a domain or oversized task into a coordinated multi-agent team: pick an architecture pattern (pipeline, fan-out/fan-in, expert pool, producer-reviewer, supervisor, hierarchical), define each agent's role/inputs/output/verification, and wire orchestration. Hands execution to dispatching-parallel-agents and subagent-driven-development.
- `compounding-knowledge` — Capture each solved problem as a durable, deduped, discoverable solution note (problem/root-cause/solution/evidence) wired into the project's CLAUDE.md/AGENTS.md pointers so future work starts ahead. The deliberate, human-readable counterpart to the automatic learn loop.
- `adversarial-security-review` — Red-team / blue-team / adjudicator pass that chains static findings into concrete attacker scenarios and adjudicates each to confirmed/refuted/needs-proof with evidence. The reasoning layer above keel config-audit's deterministic scan.
- `ui-design-systems-and-responsive-interfaces` — UI systems, responsive design, accessibility (WCAG 2.1 AA).
- `component-driven-development` — Component-Driven Development (CDD): build UI component-first (atomic → composite → page, each proven in isolation) instead of page-first. Use when scaffolding/restructuring a UI, building a component library, or setting up Storybook/widgetbook.
- `ux-research-and-experience-strategy` — UX research and evidence-based experience design (journeys, funnels, usability).
- `memory-status-reporter` — Human-style memory health and learning reports.
- `api-contract-design` — REST, GraphQL, and gRPC contract evolution; breaking-change classification, error taxonomy, idempotency, pagination, and SDK migration windows.
- `react-performance-audit` — React render-cost tracing, memoization, bundle-size analysis, list virtualization, Core Web Vitals on React routes.
- `postgres-migration-safety` — Live-traffic Postgres schema changes, lock-level analysis, expand-and-contract sequencing, bounded backfills, rollback paths.
- `stripe-integration` — Stripe Checkout, Payment Intents, Subscriptions, Connect, Webhooks, refunds, disputes, idempotency, and 3DS/SCA.
- `websocket-realtime-design` — WebSocket, SSE, fan-out, reconnect/resume, backpressure, ordering and dedup, auth lifecycle on long-lived connections.
- `observability-and-incident-response` — Metrics/logs/traces via OpenTelemetry, golden signals, SLO/SLI and error-budget math, alerting and burn-rate paging linked to runbooks, on-call ergonomics, and blameless postmortems.
- `dependency-and-supply-chain` — Dependency upgrades, lockfile hygiene and dedup, semver risk tiering, major-version migration planning, transitive triage, Renovate/Dependabot, SBOM, and provenance/signing across npm/cargo/pip/go. The action counterpart to security-and-compliance-auditor's scanning.
- `data-and-ml-engineering` — Data pipelines (ETL/ELT), batch/streaming ingestion, warehouse/lakehouse modeling (dbt), data quality and contracts, orchestration (Airflow/Dagster), and the ML lifecycle (feature engineering, training, serving, evaluation, drift). The analytical/ML-flow counterpart to backend-and-data-architecture's OLTP focus.
- `authentication-and-identity` — Builds login, session, token, and SSO flows: OAuth2/OIDC (authorization-code + PKCE), JWT/opaque token issuance and validation, refresh-token rotation with reuse detection, SAML/SSO, MFA/passkeys/WebAuthn, and argon2/bcrypt password storage. The build counterpart to security-and-compliance-auditor's read-only auditing.
- `cloud-cost-and-finops` — Cloud cost engineering and FinOps: cost estimation before deploy, rightsizing, commitment planning (reserved/savings/CUD), autoscaling and spot strategy, cost allocation and tagging, budget guardrails and anomaly alerts, and unit economics. Owns the spend dimension that cloud-and-devops-expert (mechanics) and observability-and-incident-response (SLOs) do not.
- `internationalization-and-localization` — i18n/l10n: message-catalog design and extraction, ICU MessageFormat, pluralization, locale-aware number/date/currency formatting, RTL/bidi, translation workflows and fallback chains, pseudo-localization, and Unicode correctness. The message/locale layer beneath ui-design-systems-and-responsive-interfaces.
- `compression-discipline` — Per-turn output-compression playbook (narrower line ranges, search before reading, summarize logs). Auto-loaded by the UserPromptSubmit hint when a session crosses the per-day tool-call threshold.
- `output-economy` — Per-response output-token economy: cut reply verbosity (no preamble, no re-narration of tool output, length tracks the task) without dropping technical signal. The output-side counterpart to compression-discipline's input-side rules.

## Subagent catalog (delegation targets in .claude/agents/, roster asserted by tests/doc_parity_test.rs)

Use these via the Agent tool when the work benefits from an isolated context
window. Same names as the skills — pick the subagent when token-saving delegation
matters, pick the skill when the work belongs in the main thread.

**Subagents cannot spawn subagents.** If a subagent needs to delegate, route back
to the main thread via `Skill` tool or a documented workflow step instead of spawning
nested agents.

**Agent teams:** Teammates communicate via the `SendMessage` tool with the agent's ID
as the `to` field. Resumed subagents retain full conversation history and auto-resume
in the background when they receive a `SendMessage`. The `SubagentStop` event fires
when a subagent finishes; `TeammateIdle` fires when a teammate is about to go idle —
both support matchers to target specific agent types. Set `CLAUDE_CODE_FORK_SUBAGENT=1`
to make every subagent spawn a fork that inherits the full conversation history.

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
  `websocket-realtime-design`, `observability-and-incident-response`,
  `dependency-and-supply-chain`, `data-and-ml-engineering`,
  `authentication-and-identity`, `cloud-cost-and-finops`,
  `internationalization-and-localization`.

## Workspace pointers

The one pointer that exists on **every** project is the workspace map:

- Workspace `SYSTEM_MAP.md` lives at `~/.claude/memories/workspaces/<workspace-key>/reference/SYSTEM_MAP.md` and is auto-refreshed by `keel memory scope resolve --refresh-system-map` at session start, pre-compact, and session end. Read it before making structural claims about the current repo. This is keyed to whatever project you are in, so it is always present.

The files below ship **only inside the keel repository** and are synced to disk only when you are working in that repo. On any other project they do not exist — read the current project's own `CLAUDE.md`/`AGENTS.md`/`README` instead, and fall back to the SYSTEM_MAP above:

- `CLAUDE.md` (keel repo root) — project guide, terminology, schema notes, routing rules.
- `AGENTS.md` (keel repo root) — operating doctrine, section-to-reference map.
- `WORKFLOW.md` (keel repo root) — branch naming, commit format, completion rules.
- `00-skill-routing-and-escalation.md` (keel repo root) — read first for routing when in this repo.

## Slash commands (in `commands/`, namespaced `/keel:<name>`)

Thin, discoverable wrappers over the implemented `keel` CLI surfaces.
Each command file maps only to commands that actually ship in the Rust runtime.

- `/keel:workflow [route|start|cockpit|finish] <args>` — drive a proof-first workstream over the JSONL ledger.
- `/keel:review [pre-commit|pre-pr|gates] [base-ref]` — run the native review gates on the current diff.
- `/keel:recall <terms>` — FTS5 search over durable memory (working briefs, system maps, memoriesv2).
- `/keel:gain [since]` — report command-output compaction token savings.
- `/keel:sprint [plan|status|advance|review|list] [story-id]` — drive a Scrum-style sprint loop over confirmed user stories (fail-closed: loops until every story is Done).
- `/keel:user-story [lint] [file-path]` — validate user stories against strict Agile/Jira format (Connextra + Gherkin + INVEST).

These exist so the surface is reachable from the `/` menu, not only by the skill
matcher or raw CLI. They never invoke planned-but-unimplemented commands.

## MCP server (`keel mcp serve`)

`.claude-plugin/plugin.json` registers `mcpServers.keel` at user scope, so the harness auto-discovers the server on **every** project — you do not need to start it. These tools are always available; **prefer them over guessing or ad-hoc file reading**:

- Tool `context_brief` (full name `mcp__keel__context_brief`) — **call this first when you start a session or task.** One call returns the iron law, the full installed skill catalog (name + when_to_use), durable-memory health, and the newest working brief. This is how you become aware of what the toolkit offers when no skill loaded automatically. After reading it, route with `skill_route`, load with `skill_get`, and reach anything else through `cli`.
- Tool `system_map` (full name `mcp__keel__system_map`) — **call this before any claim about the current repo's structure or layout** ("what is this project", "how is this organized", "where does X live") instead of guessing or spelunking files blind. Returns the workspace SYSTEM_MAP.md (auto-refreshed copy preferred, freshly rendered fallback).
- Tool `recall` (full name `mcp__keel__recall`) — **call this before claiming what you remember or previously learned.** Full-text search over `~/.claude/memories`, `memoriesv2`, `working-briefs` via the FTS5 index. Same code path as `keel memory recall`.
- Tool `run_command` (full name `mcp__keel__run_command`) — run a noisy shell command through the proxy capture+compaction pipeline so the compacted output lands in context instead of the raw stream. Prefer it for test/build/lint/log/search commands.
- Tool `recall_status` (full name `mcp__keel__recall_status`) — recall index health snapshot (document count, schema version, last-sync timestamp).
- Tool `skill_route` (full name `mcp__keel__skill_route`) — **the on-demand equivalent of the per-prompt skill router.** Pass a prompt; get the single distinctive skill match plus a bounded inline brief of its guidance. Use this when the lifecycle hook that normally injects the brief did not fire (it is unreliable on some platforms) or whenever you are unsure which skill applies.
- Tool `skill_get` (full name `mcp__keel__skill_get`) — load the full SKILL.md body for an installed skill by name. The full-body upgrade after `skill_route`.
- Tool `skill_list` (full name `mcp__keel__skill_list`) — list every installed skill with its name, description, and when_to_use. Discover what skills exist before routing.
- Tool `memory_status` (full name `mcp__keel__memory_status`) — durable-memory health: the recall index snapshot plus per-family record counts. Read-only.
- Tool `brief_list` / `brief_get` / `brief_create` (full names `mcp__keel__brief_*`) — read and write working briefs under `~/.claude/working-briefs`. `brief_create` records the restated request, constraints, acceptance criteria, and assumptions so they survive compaction; the read pair retrieves them.
- Tool `system_map_refresh` (full name `mcp__keel__system_map_refresh`) — regenerate the cached SYSTEM_MAP.md (`system_map` only reads it). Call after creating, deleting, moving, or renaming files.
- Tool `cli` (full name `mcp__keel__cli`) — run any other keel subcommand (`review`, `git-workflow`, `workflow`, `memory`, `orchestration`, `flow`, `code-search`, `config-audit`, `skill-lint`, `checkpoint`, `gain`, `telemetry`, `status`, `doctor`, ...) and get its compacted output. Pass `args` as a string array. Destructive/management subcommands (`install`, `update`, `repair`, `uninstall`, `validate`, `all`, `__self-replace`, `checkpoint restore`) require `confirm: true`; `mcp` is refused. Prefer a dedicated tool when one fits; use `cli` for the rest.
- Resource `keel://system-map` (`text/markdown`) and `keel://recall/status` (`application/json`).

The same 1% rule that governs skills applies here: if a tool could answer the question more authoritatively than your own recall, use it before responding.

**If these tools seem absent — they are almost never actually missing.** MCP tools are namespaced `mcp__keel__<tool>` and may be *deferred* behind `ToolSearch` (the harness forces deferral whenever tool search is on or `ANTHROPIC_BASE_URL` points at a non-first-party gateway). Two traps:

1. **Searching by bare name fails.** `ToolSearch("select:recall")` does an *exact* match on the full name and returns nothing, because the real name is `mcp__keel__recall`. Do **not** conclude "MCP isn't registered" from an empty `select:` result. Search by keyword (`ToolSearch("recall system map run command")`) or select the full namespaced name (`select:mcp__keel__recall`).
2. **Deferral is the bug, not absence.** The fix is `alwaysLoad: true` on the `~/.claude.json` entry, which pins the server's tools into context so they are never deferred. `alwaysLoad` is per-server, so every tool the server publishes is pinned together. Verify with `keel doctor` (it now reports the entry and `alwaysLoad`); repair with `keel repair`, then restart the harness.


## Memory writes (when you learn something durable)

Your working memory only lives in the current context window. Anything you want to survive compaction or the next session has to land on disk. Four memory subcommands actually write — call them when the trigger fires, do not wait for "later":

| Subcommand | Writes | Trigger — call it when |
|---|---|---|
| `keel memory scope resolve --workspace-root <abs> --create-missing --refresh-system-map` | `~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md` | files moved, packages added, or you noticed the map is stale mid-session. Hooks already fire it at session start, pre-compact, and session end — only call by hand on top of that. |
| `keel memory system-map refresh` | same SYSTEM_MAP.md path | shorthand for the scope-resolve refresh when the workspace is already resolved. |
| `keel memory working-brief write` | `~/.claude/working-briefs/<id>.json` | starting non-trivial work. Capture the user's request, acceptance criteria, and the files you expect to touch *before* coding so completion can be reconciled against it. Update with `working-brief write` again as scope shifts. |
| `keel memory completion-gate check` | nothing (probe-only) | before claiming a task complete. Returns the gate's verdict; failures point at the requirement that has no evidence yet. |

Beyond the four writers above, these `keel memory <verb>` arms are implemented (under both `memory` and `memoriesv2`): `research-cache`, `maintenance`, `agent-registry`, `agent-packets`, `loop-guard`, `entity`, `graph`, `retrieve`, `instincts`, and `status`. `report` is an alias for `status`, and `index` rebuilds the FTS5 recall index — both work. The `orchestration` group adds `task begin|progress|complete|list` and `checkpoint`. The only `memory` verb that does not run is `hook`: it exits with a pointer to `keel hook install|list|instructions|diagnose`, which owns the harness lifecycle hooks. Do not pretend a command exists by trying it; check the dispatcher in `rust/crates/keel/src/utility/memory.rs` (and `memory_families.rs`) if you are unsure.

**Relationship to the harness's native Auto memory.** Recent the harness ships its own *Auto memory* — notes the model writes itself to `~/.claude/projects/<project>/memory/MEMORY.md` based on your corrections, loaded automatically each session. The two are complementary, not competing: native Auto memory is *passive* (the model decides what to jot, machine-local, per-repo), while keel's surfaces above are *explicit and structured* — a deterministic SYSTEM_MAP, reconcilable working briefs, a completion gate, an FTS5-searchable recall index, and the durable `memoriesv2` families. Use native Auto memory for incidental learnings; use these commands when you need a structured artifact that survives compaction and can be reconciled against the request. Do not duplicate the same fact into both.

| Thought | Reality |
|---|---|
| "I'll remember this for the next turn" | Memory drifts mid-session. Hook auto-refresh covers SYSTEM_MAP only — working briefs are on you. |
| "The session will end soon, the hook will save it" | SessionEnd refreshes the map, not the brief. If you have a brief worth saving, write it now. |
| "Completion-gate is optional ceremony" | It is the only check that catches "I forgot a requirement" before the user does. Run it before claiming done. |
| "The map looks stale but I'll just guess the layout" | Refresh first: one command, bounded cost. Guessing is what landed us in this PR series in the first place. |

## The one-line summary, if you only remember one thing

**Understand before you build. Research first. Invoke relevant skills before
responding. Find the root cause. The repository — not your training data — is
the source of truth. Researching first is what saves you from building the
wrong thing.**
