<!--
Purpose: Compact entry point for skill routing rules and the specialist roster. Detailed doctrine lives under AGENTS/references/.
Caller: Synced Claude Code guidance files and contributors needing the routing summary.
Dependencies: AGENTS.md, AGENTS/references/20-skill-routing.md, the 24 specialist SKILL.md files.
Main Functions: Provide the short routing contract, ownership map, and pointers to depth references.
Side Effects: Changes to this file affect every Claude Code session; keep it tight.
-->
# Skill Routing and Escalation (Claude Code CLI)

This is the short pointer file for skill routing. The detailed doctrine lives in [AGENTS/references/20-skill-routing.md](AGENTS/references/20-skill-routing.md). When this file and a reference disagree, this file wins; open a follow-up to reconcile.

## Native Command Routing — Must Follow First

Token-saving rule: prevent noisy raw command output from entering Claude Code context. Route through `claude-skills run -- <command>` or the hook-provided `Rerun that as:` wrapper before noisy output is produced.

- **Noisy shell commands**: prefer `claude-skills run -- <command>` for test, build, lint, log, status, search, container, package-manager, and CI commands. Use `claude-skills rewrite "<command>"` to inspect.
- **Hook block-and-rerun**: if the managed `PreToolUse` hook returns `Rerun that as: <command>`, run that exact command once and continue from the compacted output. Do not ask the user, do not treat the hook block as a task failure, and do not repeat the raw command first.
- **Repository search**: prefer `claude-skills code-search search --workspace-root "$PWD" --query "<query>"` before raw `rg`/`grep`/`find`/`git grep`.
- **Existing-source edits**: validate Preserve Existing Flow evidence with `claude-skills flow start|check|finish` and record the owner path in the global flow-check artifact before patching.
- **Commit/PR/final-response text**: use `claude-skills git-workflow commit-message|pr-body|lint-message`, then `claude-skills git-workflow preflight` and `claude-skills review pre-pr` before merge.

## Routing Contract (the eight rules)

0. **Understand before building** — before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem still gets thrown away, so this gates every rule below: there is no point routing a skill or refreshing memory for the wrong task. If the request is ambiguous in a way that changes what you build, ask before building, not after.
1. **Skills first** — route domain work through the matching `~/.claude/skills/<name>/SKILL.md`. Run `preserve-existing-flow` before editing existing source. Run `reviewer` before closing **non-trivial** work (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield behavior changes, release-impacting work). Skip `reviewer` for trivial work: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work.
2. **Native commands first** — prefer `claude-skills` surfaces over raw shell when they own the job.
3. **Memory first** — resolve scoped memory and read `SYSTEM_MAP.md` before broad analysis: `claude-skills memory scope resolve --create-missing --refresh-system-map`.
4. **Iterative loop** — ALIGN → RESEARCH → PLAN → IMPLEMENT → TEST → FIX → VERIFY → REVIEW → RECONCILE.
5. **One feature per branch** — `feat/<name>` / `fix/<name>` / `improve/<name>` / `add/<name>`. Professional commit and PR text.
6. **Release ladder is fail-closed** — Smoke → Functional → Integration → UI → Load → Stress → Security. Mark not-applicable only with explicit, evidence-backed reasoning.
7. **Completion reconciliation** — re-read the working brief and impacted surface before final answer. Every explicit user requirement maps to evidence or a verified blocker. No partial-as-complete.

## Skill Ownership Map (Claude Code CLI)

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
