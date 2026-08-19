<!--
Purpose: Compact entry point for skill routing rules and the specialist roster. Detailed doctrine lives under AGENTS/references/.
Caller: Synced the harness guidance files and contributors needing the routing summary.
Dependencies: AGENTS.md, AGENTS/references/20-skill-routing.md, the specialist SKILL.md files (roster asserted by tests/doc_parity_test.rs).
Main Functions: Provide the short routing contract, ownership map, and pointers to depth references.
Side Effects: Changes to this file affect every harness session; keep it tight.
-->
# Skill Routing and Escalation (the harness CLI)

This is the short pointer file for skill routing. The detailed doctrine lives in [AGENTS/references/20-skill-routing.md](AGENTS/references/20-skill-routing.md). When this file and a reference disagree, this file wins; open a follow-up to reconcile.

## Native Command Routing — Must Follow First

Token-saving rule: prevent noisy raw command output from entering the harness context. Route through `keel run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands**: prefer `keel run -- <command>` for test, build, lint, log, status, search, container, package-manager, and CI commands. Use `keel rewrite "<command>"` to inspect.
- **Hook block-and-rerun**: if the managed `PreToolUse` hook returns `Rerun that as: <command>`, run that exact command once and continue from the compacted output. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search**: prefer `keel code-search search --workspace-root "$PWD" --query "<query>"` before raw `rg`/`grep`/`find`/`git grep`. After a fix or implement, run `keel code-search siblings` and handle every hit — a one-site change is unfinished.
- **Existing-source edits**: validate Preserve Existing Flow evidence with `keel flow start|check|finish` and record the owner path in the global flow-check artifact before patching.
- **Commit/PR/final-response text**: use `keel git-workflow commit-message|pr-body|lint-message`, then `keel git-workflow preflight` and `keel review pre-pr` before merge.

## Routing Contract (the ten rules)

0. **Understand before building** — before writing any code, restate what the request actually asks, confirm the requirement, and research what is genuinely needed. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem still gets thrown away, so this gates every rule below: there is no point routing a skill or refreshing memory for the wrong task. If the request is ambiguous in a way that changes what you build, ask before building, not after. Capture the restated request, constraints, and acceptance criteria in a working brief (`keel memory working-brief write` or `brief_create`). That brief is the anti-drift spec `reviewer` Stage 1 reconciles the diff against. Pure questions, lookups, and already-confirmed trivial edits are exempt.
1. **Skills first** — route domain work through the matching `~/.claude/skills/<name>/SKILL.md`. On a requirement-bearing prompt, write the working brief first. Run `preserve-existing-flow` before editing existing source. Run `reviewer` before closing **non-trivial** work (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield behavior changes, release-impacting work). Skip `reviewer` for trivial work: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work.
2. **Native commands first** — prefer `keel` surfaces over raw shell when they own the job.
3. **Memory first** — resolve scoped memory and read `SYSTEM_MAP.md` before broad analysis: `keel memory scope resolve --create-missing --refresh-system-map`.
4. **Iterative loop** — ALIGN → RESEARCH → PLAN → IMPLEMENT → TEST → FIX → VERIFY → REVIEW → RECONCILE. For multi-piece or multi-step builds, run this as **Anvil** (`running-anvil` / `keel anvil`): compile a named bar into lock+prefix+gates, cast isolated workspaces, sieve with 0-LLM gates, stamp the winner, and bounded-loop only if gates still fail. MCP uses `compile` then `run --dry-run`; live `run`/`loop` stay on the CLI. Nothing half-built is presented as complete.
5. **Branch model + commit format** — `main` ← `dev` ← `feat` ← `task/<task>` [← `task/<task>/<subtask>`]. Never use `feat/<task>` while bare `feat` exists (Git ref collision). Fixes stay on the same work branch. Never delete branches after push/merge. Commits: `Add : FEATURE : short info` (capitalized category, uppercase FEATURE, spaces around colons). Legacy `add/`/`feature/` branches may continue with a warning.
6. **Release ladder is fail-closed** — Smoke → Functional → Integration → UI → Load → Stress → Security. Mark not-applicable only with explicit, evidence-backed reasoning.
7. **Completion reconciliation** — re-read the working brief and impacted surface before final answer. Every explicit user requirement **must** map to evidence or a verified blocker. No partial-as-complete.
8. **Writing Discipline** — all written output (docs, code comments, commit/PR text, review notes, chat) **must** follow: write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.
9. **Agent teams** — use `designing-agent-teams` when a task needs coordinated multi-agent decomposition. Subagents **must not** spawn subagents — route delegation back to the main thread. Teammates **must** communicate via `SendMessage(to: <agent-id>)`. Resumed subagents retain full history and auto-resume in background on `SendMessage`; `SubagentStop` and `TeammateIdle` events fire on lifecycle. Set `CLAUDE_CODE_FORK_SUBAGENT=1` to fork conversation history into every subagent.

## Skill Ownership Map (the harness CLI)

```
┌──────────────────────────────────────┐
│  SOFTWARE-DEVELOPMENT-LIFE-CYCLE     │
│  (Cross-domain manager when needed)  │
└──────────────────────────────────────┘
                │
                ├─────┬──────┬───────┬───────┬──────┬─────────┬───────┐
                ▼     ▼      ▼       ▼       ▼      ▼         ▼       ▼
            PRESERVE  WEB   MOBILE  BACKEND DEVOPS  QA      SECURITY  GIT
              FLOW   LIFE   LIFE   & DATA  & CLOUD AUTO    & COMPL   EXPERT

            ┌──────┐ ┌──────┐
            │  UI  │ │  UX  │
            └──────┘ └──────┘

            ┌────────────────────────────────────┐
            │ MEMORY STATUS REPORTER (memory)    │
            │ REVIEWER (final quality gate)      │
            └────────────────────────────────────┘
```

## Specialist Roster (24)

1. **software-development-life-cycle** — full SDLC, architecture, cross-domain coordination
2. **preserve-existing-flow** — brownfield ownership tracing before existing-source edits
3. **web-development-life-cycle** — web frontend and full-stack frameworks
4. **mobile-development-life-cycle** — mobile development (Android, iOS, cross-platform)
5. **backend-and-data-architecture** — APIs, microservices, databases, message queues
6. **cloud-and-devops-expert** — IaC, CI/CD, container orchestration, rollout strategy
7. **qa-and-automation-engineer** — TDD, E2E frameworks, release ladder
8. **security-and-compliance-auditor** — threat modeling, vulnerability hunting, compliance
9. **ui-design-systems-and-responsive-interfaces** — UI, design systems, accessibility
10. **ux-research-and-experience-strategy** — UX research, journey design, recovery paths
11. **git-expert** — version control, branching strategy, PR/MR hygiene
12. **memory-status-reporter** — memory health, learning recaps, mistake ledgers
13. **reviewer** — production readiness, final quality gate
14. **api-contract-design** — REST, GraphQL, gRPC contract evolution and breaking-change governance
15. **react-performance-audit** — render cost, bundle size, virtualization, Core Web Vitals on React apps
16. **postgres-migration-safety** — live-traffic Postgres schema changes, backfills, indexes, rollback plans
17. **stripe-integration** — Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes
18. **websocket-realtime-design** — WebSocket, SSE, fan-out, reconnect, backpressure, auth lifecycle
19. **observability-and-incident-response** — metrics/logs/traces, SLO/error budgets, alerting, runbooks, postmortems
20. **dependency-and-supply-chain** — dependency upgrades, lockfile hygiene, major-version migration, SBOM, provenance
21. **data-and-ml-engineering** — data pipelines, dbt/warehouse modeling, orchestration, ML lifecycle and drift
22. **authentication-and-identity** — OAuth2/OIDC, SSO/SAML, sessions, tokens, refresh rotation, MFA/passkeys, password storage
23. **cloud-cost-and-finops** — cost estimation, rightsizing, commitments, autoscaling/spot, budgets, unit economics
24. **internationalization-and-localization** — message catalogs, ICU MessageFormat, locale formatting, RTL/bidi, translation workflows

## Pointers to Depth

Open the matching reference when you need the full ruleset:

| Topic | File |
|---|---|
| 53 routing principles, overlap resolution, context-efficiency ladder, planning defaults, honest reporting | [AGENTS/references/20-skill-routing.md](AGENTS/references/20-skill-routing.md) |
| Native command routing depth, hook transparent rewrite, token compaction | [AGENTS/references/10-native-command-routing.md](AGENTS/references/10-native-command-routing.md) |
| Execution strategy, iterative loop, memory protocol | [AGENTS/references/30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) |
| Code quality standards, testing requirements, feature flags | [AGENTS/references/40-code-quality-and-testing.md](AGENTS/references/40-code-quality-and-testing.md) |
| Delivery rules and prohibited shortcuts | [AGENTS/references/50-delivery-and-prohibited-shortcuts.md](AGENTS/references/50-delivery-and-prohibited-shortcuts.md) |
| Environment and cross-platform script portability | [AGENTS/references/60-environment-and-portability.md](AGENTS/references/60-environment-and-portability.md) |
| Review gates, quality policies, reasoning effort, model policy | [AGENTS/references/70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md) |

## Honest Reporting

State what is verified, mark inferences as inferences, and call out blocked, partial, or unvalidated work before claiming completion. Polished wording does not hide missing validation.
