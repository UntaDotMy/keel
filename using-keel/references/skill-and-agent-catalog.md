# Skill and agent catalog (on-demand)

Load when routing or discovering skills. Always-on SessionStart does **not** embed this list.

## Skill catalog (every matcher-invocable skill, installed under ~/.claude/skills/)
<!-- The count is deliberately not stated; it drifts. Every skill in .claude-plugin/plugin.json
     is listed below; this bootstrap (`using-keel`) is the only first-party skill NOT in the
     manifest (it loads at SessionStart). The structural invariant (disk == manifest + 1) is
     asserted by tests/doc_parity_test.rs. Run `keel skill-lint` for the live verified count. -->


Source: each `<name>/SKILL.md` in this repo. Use the Skill tool with the bare
name (e.g. `Skill("reviewer")`). The count excludes this bootstrap skill itself
(`using-keel`), which is always loaded at SessionStart rather than
invoked on demand. `requesting-code-review` is a real thin-alias directory that
routes to `reviewer` (not a separate review behavior).

- `software-development-life-cycle` — Cross-domain planning, architecture framing, multi-phase delivery sequencing.
- `web-development-life-cycle` — Web architecture, quality, and production delivery (Core Web Vitals, SEO, accessibility).
- `mobile-development-life-cycle` — Mobile architecture, quality, and release (Android/iOS lifecycle, store submission).
- `dart-and-flutter-expert` — Dart & Flutter: widget architecture (pure `build`, `const` constructors), state management (Provider/Riverpod/Bloc), jank diagnosis (`ListView.builder`, `RepaintBoundary`), isolates for >16ms work, null-safety without `!`, pubspec hygiene, platform channels, Flutter web/desktop.
- `backend-and-data-architecture` — Backend systems, API design, and data engineering (schemas, messaging, microservice boundaries).
- `domain-driven-design` — Domain-Driven Design (DDD): ubiquitous language, bounded contexts, aggregates, entities/value objects, domain events, context maps, strategic vs tactical design, optional CQRS/event sourcing. Use for complex domain models and service boundaries.
- `behavior-driven-development` — Behavior-Driven Development (BDD): shared Gherkin examples, outside-in delivery, living documentation. Bridges product language to automated acceptance checks (pairs with writing-user-stories + TDD).
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
- `component-driven-development` — Component-Driven Development (CDD) + Atomic Design: build UI component-first (atom → molecule → organism → page, each proven in isolation / Storybook visual TDD) instead of page-first.
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
- `critic` — In-flight critique during/before implementation: catch blind code, missing tests, missing memory capture, and skipped workflow early. Distinct from `reviewer` (post-implementation gate).
- `deliberation` — Structured disagreement when experts or subagents conflict: surface consensus, contradictions, unique insights, and blind spots before architecture or review decisions.
- `memory-consolidation` — Distill recent observations into durable memory notes (patterns, decisions, solutions) at session-end, compaction, or on "what did we learn".
- `research-enforcement` — Require fresh research (web/docs/recall) before implementing against external libraries, APIs, or frameworks so training data is not treated as current fact.

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
  `mobile-development-life-cycle`, `dart-and-flutter-expert`,
  `backend-and-data-architecture`, `domain-driven-design`,
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
- `/keel:recall <terms>`: FTS5 search over durable memory (working briefs, system maps, memories).
- `/keel:gain [since]` — report command-output compaction token savings.
- `/keel:sprint [plan|status|advance|review|list] [story-id]` — drive a Scrum-style sprint loop over confirmed user stories (fail-closed: loops until every story is Done).
- `/keel:user-story [lint] [file-path]` — validate user stories against strict Agile/Jira format (Connextra + Gherkin + INVEST).

These exist so the surface is reachable from the `/` menu, not only by the skill
matcher or raw CLI. They never invoke planned-but-unimplemented commands.
