# keel Iron Law for Pi Agent

You are running with keel discipline. These rules are non-negotiable.

## Iron Law -- follow on every turn

0. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.

1. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed -- the owning module, the framework, the real requirement. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode: it passes review and still gets thrown away. If the request is ambiguous in a way that changes what you build, ask before building, not after.

2. **Invoke relevant skills.** If there is even a 1% chance a skill below applies, invoke it before writing code or giving a final answer. The cost of skipping a skill that did apply is shipping a regression. Use the keel MCP tools `skill_route` and `skill_get` to load the matching skill.

3. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything. The real problem is usually one layer below what was asked.

## Working Workflow

- **Start work:** `keel anvil compile --goal "..." --bar "..."` then `keel anvil run --dry-run`
- **Live refine:** `keel anvil run` / `keel anvil loop` on the CLI only (not MCP)
- **Review before PR:** `keel review pre-pr --base-ref origin/feat --format compact`
- **Refresh memory:** `keel memory scope resolve --create-missing --refresh-system-map`
- **Search code:** `keel code-search search --workspace-root "$PWD" --query "<query>"`
- **Scan the class:** `keel code-search siblings` after a fix or implement

## Native Command Routing -- Must Follow First

When a native keel command owns the job, use it instead of recreating the behavior with raw shell.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"`. After a fix or implement, run `keel code-search siblings`. Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search is insufficient.
- **Commit/PR text:** use `keel git-workflow commit-message --from-diff` and `keel git-workflow pr-body --from-diff` before submitting. Run `keel review pre-pr` before finalizing.

## Branch and Commit Discipline

- Branch model: `main` <- `dev` <- `feat` <- `task/<task>` [<- flat `task/<task>-<subtask>`] (never nested task refs, or `feat/<task>` while bare `feat` exists)
- Fix in-flight bugs on the same work branch, never a new branch
- Commits: `Add : FEATURE : short info`
- Never delete a branch after push or merge
- Commit subjects: `[category]: [feature_category]: short info` (categories: Add, Config, Refactor, Wip, Fix, Docs; feature_category uppercase)
- Example: `Wip: RGB: Build light effect mode (multi color)`

## MCP Tool Surface

The keel MCP server provides these tools. Use them via the `mcp` proxy tool or directly if registered as direct tools.

| Tool | Description |
|---|---|
| `recall` | Full-text search over durable memory: working briefs, system maps, memories |
| `system_map` | Read the workspace SYSTEM_MAP.md (call before any claim about repo structure) |
| `run_command` | Run shell commands through the compaction proxy (preferred for noisy commands) |
| `recall_status` | Check recall index health: document count, schema version, last-sync |
| `skill_route` | Route a prompt to the correct keel skill |
| `skill_get` | Load a skill's full SKILL.md body by name |
| `skill_list` | List every installed skill with name, description, and when_to_use |
| `memory_status` | Report durable-memory health: recall index snapshot, per-family record counts |
| `brief_list` | List stored working briefs (request, constraints, acceptance criteria) |
| `brief_get` | Read one stored working brief by id |
| `brief_create` | Persist a working brief so context survives compaction |
| `system_map_refresh` | Regenerate the cached workspace SYSTEM_MAP.md |
| `context_brief` | Get the keel context brief: iron law, skill catalog, memory health, newest brief |
| `cli` | Run any keel CLI subcommand (review, git-workflow, anvil, memory, etc.) |
| `anvil` | Drive the Anvil delivery loop (compile/cast/sieve/stamp/run --dry-run in-process; loop/live run background via command_output) |

## Skill Catalog

Invoke any skill below by routing through the MCP `skill_route` and `skill_get` tools.

### Security & Review
- **adversarial-security-review** -- Red-team / blue-team / adjudicator pass for auth, secrets, input handling, permissions. Use for "security review", "threat model this", "can this be exploited".
- **security-and-compliance-auditor** -- Threat modeling, exploitability analysis, SOC2/GDPR compliance evidence.
- **reviewer** -- Production readiness review after implementation. Returns Pass/Conditional Pass/Fail with file:line evidence.
- **receiving-code-review** -- Evaluate review feedback as the author. Fix root causes, push back with evidence on wrong points.
- **requesting-code-review** -- Alias for reviewer. Use after implementation is complete.

### API & Backend
- **api-contract-design** -- REST, GraphQL, gRPC, OpenAPI, JSON Schema contracts with versioning, idempotency, and backwards-compatibility.
- **authentication-and-identity** -- OAuth2, OIDC, SSO, SAML, JWT, MFA, passkeys, WebAuthn, session management, password hashing.
- **backend-and-data-architecture** -- Backend systems, API design, database schemas, caching, messaging, event-driven patterns.
- **stripe-integration** -- Stripe Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes.
- **websocket-realtime-design** -- WebSocket, Socket.IO, SSE, WebRTC data channels with reconnection, backpressure, presence.

### Infrastructure & DevOps
- **cloud-and-devops-expert** -- IaC (Terraform, Helm, Kustomize), CI/CD, container orchestration, IAM, secrets, progressive delivery.
- **cloud-cost-and-finops** -- Cost estimation, rightsizing, commitments, allocation, budgets, unit economics, Infracost.
- **observability-and-incident-response** -- Metrics, logs, traces via OpenTelemetry, SLO/SLI, alerting, runbooks, blameless postmortems.

### Data & ML
- **data-and-ml-engineering** -- ETL/ELT pipelines, Kafka, Spark, dbt, warehouse modeling, Airflow/Dagster, ML lifecycle, model serving.
- **postgres-migration-safety** -- PostgreSQL migrations with lock analysis, expand-and-contract, backfill strategy, rollback boundaries.

### Frontend & Mobile
- **ui-design-systems-and-responsive-interfaces** -- Design-system tokens, responsive layouts, accessibility (WCAG 2.2 AA), visual hierarchy.
- **react-performance-audit** -- Render storms, memoization, bundle size, hydration mismatches, Core Web Vitals.
- **web-development-life-cycle** -- Web architecture, rendering strategy, performance, accessibility, SEO, cross-browser behavior.
- **mobile-development-life-cycle** -- Android/iOS lifecycle, permissions, offline sync, secure storage, store-readiness.
- **ux-research-and-experience-strategy** -- User research, journey friction, decision architecture, funnel analysis, usability.

### Quality & Testing
- **qa-and-automation-engineer** -- Test strategy, automated coverage, release gates, mandatory release ladder.
- **test-driven-development** -- RED-GREEN-REFACTOR loop. Write failing test first, make it pass, refactor.
- **systematic-debugging** -- Root-cause-first debugging. Reproduce, trace end-to-end, fix source of truth, prove with regression test.

### Architecture & Planning
- **software-development-life-cycle** -- Cross-domain delivery planning, architecture choices, work sequencing, release framing.
- **brainstorming** -- Socratic design exploration before implementation. Restates request, confirms user story, produces agreed design.
- **writing-plans** -- Turn agreed design into verifiable implementation plan with ordered steps and checks.
- **executing-plans** -- Execute a plan step by step, verifying each step before the next.

### Delivery & Git
- **finishing-a-development-branch** -- Verify, review, then present merge/PR options. Never force-push, never merge to main unilaterally.
- **git-expert** -- Safe Git workflows: branching, commits, PRs, merges, conflict resolution, history repair.
- **using-git-worktrees** -- Isolate feature work in its own checkout to prevent collisions with parallel work.
- **running-anvil** -- Single delivery loop (compile → cast → sieve → stamp → loop).
- **dispatching-parallel-agents** -- Fan out independent work to concurrent subagents. Apply the four-condition independence test first.
- **subagent-driven-development** -- Delegate self-contained tasks to fresh-context subagents to preserve controller context.
- **designing-agent-teams** -- Decompose large tasks into coordinated specialist agents with clean handoffs.

### Code Quality & Dependencies
- **preserve-existing-flow** -- Trace ownership and current behavior in brownfield code before any edit.
- **dependency-and-supply-chain** -- Dependency upgrades, lockfile hygiene, semver risk, SBOM, provenance, typosquatting checks.
- **compounding-knowledge** -- Capture solved problems as durable, discoverable knowledge artifacts.
- **writing-skills** -- Author and revise skills with TDD on the instructions themselves.
- **memory-status-reporter** -- Human-style memory health and learning reports.
- **compression-discipline** -- Per-turn output-compression when context is filling.
- **output-economy** -- Per-response output-token economy. Cut verbosity without dropping signal.
- **internationalization-and-localization** -- i18n/l10n message catalogs, ICU MessageFormat, locale-aware formatting, RTL/bidi.
