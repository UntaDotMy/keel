# keel Pi Agent Bridge

Injects keel's iron law, skill catalog, and operating instructions into Pi Agent via the `AGENTS.md` file and standard MCP server configuration.

## What This Does

Pi Agent reads `AGENTS.md` from the project root (or `~/.pi/agent/AGENTS.md` globally) and loads the contents as project instructions at startup. This adapter puts keel's discipline -- the four iron law rules, the full 24-skill catalog, workflow commands, branch/commit rules, and MCP tool surface -- into that instruction surface so Pi Agent follows the keel operating contract automatically.

Unlike the Codex adapter, this is a **static rules file plus MCP configuration**, not a hook-based bridge. Pi Agent's core does not expose a hook lifecycle comparable to Codex or Claude Code, but it supports hook-like behavior through extensions (`pi-autohooks`, `pi-shepherd`). The `AGENTS.md` file ensures the model operating in Pi Agent has the full keel discipline available from the first prompt, and the `.mcp.json` file registers keel's MCP server so its tools are discoverable.

## Prerequisites

1. Pi Agent installed (`npm install -g pi` or via `pi install`).
2. The `keel` binary installed at `~/.claude/keel` (unix) or `~/.claude/keel.exe` (win32), or on `PATH`.
3. The `pi-mcp` adapter extension installed for MCP server support:
   ```bash
   pi install npm:@spences10/pi-mcp
   ```

## Install

`keel install` auto-detects Pi Agent (via `~/.pi/agent/` dir, `PI_CODING_AGENT_DIR` env var, or `pi` binary on PATH) and wires this adapter automatically. Use `--without pi` to skip, `--with pi` to force.

Manual install options:

### Option A: Project-scoped (recommended)

Copy the instruction file into your project root:

```bash
cp pi/AGENTS.md /path/to/your/project/AGENTS.md
```

On Windows:

```powershell
Copy-Item pi\AGENTS.md "C:\path\to\your\project\AGENTS.md"
```

Copy the MCP configuration into your project root:

```bash
cp pi/.mcp.json /path/to/your/project/.mcp.json
```

On Windows:

```powershell
Copy-Item pi\.mcp.json "C:\path\to\your\project\.mcp.json"
```

Pi Agent loads both files automatically when you open the project.

### Option B: Global

Copy to your Pi agent directory:

```bash
cp pi/AGENTS.md ~/.pi/agent/AGENTS.md
cp pi/.mcp.json ~/.config/mcp/mcp.json
```

On Windows:

```powershell
Copy-Item pi\AGENTS.md "$env:USERPROFILE\.pi\agent\AGENTS.md"
Copy-Item pi\.mcp.json "$env:USERPROFILE\.config\mcp\mcp.json"
```

This applies keel discipline to every project in Pi Agent.

### Option C: Pi rules extension (optional, for glob-based scoping)

If you use the `pi-rules` extension, you can place keel rules under `.pi/rules/` for project-scoped loading:

```bash
mkdir -p .pi/rules
cp pi/AGENTS.md .pi/rules/keel.md
```

Add frontmatter to scope the rules to specific file types:

```yaml
---
description: keel iron law and skill catalog
alwaysApply: true
---
```

Pi Agent also discovers `AGENTS.md` natively, so Option A or B is sufficient without the rules extension.

### Verify the install

After copying the files, confirm Pi Agent loads them:

```bash
# Start Pi and check that AGENTS.md content is present in the system prompt
pi

# Check MCP server connectivity (if pi-mcp adapter is installed)
/mcp
```

## What the Rules Include

### Iron Law (4 rules)

0. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.

1. **Understand before building.** Restate what the request asks and research what is genuinely needed before writing code. No guessing, no building against an imagined spec.

2. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool before writing code or giving a final answer.

3. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything.

### Workflow Commands

| Command | Use |
|---|---|
| `keel workflow route --request "..."` | Route a broad request to a preset |
| `keel workflow start --preset <preset> --request "..."` | Start work |
| `keel workflow cockpit` | View live state |
| `keel workflow finish --id <id> --proof "..."` | Finish a workstream |
| `keel review pre-pr --base-ref origin/feat` | Review before PR |
| `keel memory scope resolve --create-missing --refresh-system-map` | Refresh memory |
| `keel code-search search --workspace-root "$PWD" --query "..."` | Search code |

### Branch and Commit Rules

- Branch model: `main` (stable) <- `dev` (staging) <- `feat` (integration) <- `<category>/<FEATURE>` work branch (branch off `feat`)
- Commit format: `<category>: <FEATURE>: <short info>` (categories lowercase: add, config, refactor, wip, fix, docs; FEATURE uppercase)
- Never delete a branch after push or merge

### MCP Server

The included `.mcp.json` registers keel's MCP server, exposing 31 tools:

| Tool | Description |
|---|---|
| `recall` | Full-text search over durable memory (working briefs, system maps, memories) |
| `system_map` | Read the workspace SYSTEM_MAP.md |
| `run_command` | Run shell commands through the compaction proxy |
| `recall_status` | Check recall index health |
| `skill_route` | Route a prompt to the correct keel skill |
| `skill_get` | Load a skill's full SKILL.md body |
| `skill_list` | List every installed skill |
| `memory_status` | Report durable-memory health |
| `brief_list` | List stored working briefs |
| `brief_get` | Read one stored working brief |
| `brief_create` | Persist a working brief |
| `system_map_refresh` | Regenerate the cached workspace SYSTEM_MAP |
| `context_brief` | Get the keel context brief (skills, memory, working brief) |
| `cli` | Run any keel CLI subcommand |
| `sprint` | Drive a Scrum-style sprint loop |
| `user_story_lint` | Validate user stories against strict format |

### Skill Catalog (24 specialist skills)

Invoke any skill below by routing through the MCP `skill_route` and `skill_get` tools.

#### Security & Review
- **adversarial-security-review** -- Red-team / blue-team / adjudicator pass for auth, secrets, input handling, permissions. Use for "security review", "threat model this", "can this be exploited".
- **security-and-compliance-auditor** -- Threat modeling, exploitability analysis, SOC2/GDPR compliance evidence.
- **reviewer** -- Production readiness review after implementation. Returns Pass/Conditional Pass/Fail with file:line evidence.
- **receiving-code-review** -- Evaluate review feedback as the author. Fix root causes, push back with evidence on wrong points.
- **requesting-code-review** -- Alias for reviewer. Use after implementation is complete.

#### API & Backend
- **api-contract-design** -- REST, GraphQL, gRPC, OpenAPI, JSON Schema contracts with versioning, idempotency, and backwards-compatibility.
- **authentication-and-identity** -- OAuth2, OIDC, SSO, SAML, JWT, MFA, passkeys, WebAuthn, session management, password hashing.
- **backend-and-data-architecture** -- Backend systems, API design, database schemas, caching, messaging, event-driven patterns.
- **stripe-integration** -- Stripe Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes.
- **websocket-realtime-design** -- WebSocket, Socket.IO, SSE, WebRTC data channels with reconnection, backpressure, presence.

#### Infrastructure & DevOps
- **cloud-and-devops-expert** -- IaC (Terraform, Helm, Kustomize), CI/CD, container orchestration, IAM, secrets, progressive delivery.
- **cloud-cost-and-finops** -- Cost estimation, rightsizing, commitments, allocation, budgets, unit economics, Infracost.
- **observability-and-incident-response** -- Metrics, logs, traces via OpenTelemetry, SLO/SLI, alerting, runbooks, blameless postmortems.

#### Data & ML
- **data-and-ml-engineering** -- ETL/ELT pipelines, Kafka, Spark, dbt, warehouse modeling, Airflow/Dagster, ML lifecycle, model serving.
- **postgres-migration-safety** -- PostgreSQL migrations with lock analysis, expand-and-contract, backfill strategy, rollback boundaries.

#### Frontend & Mobile
- **ui-design-systems-and-responsive-interfaces** -- Design-system tokens, responsive layouts, accessibility (WCAG 2.1 AA), visual hierarchy.
- **react-performance-audit** -- Render storms, memoization, bundle size, hydration mismatches, Core Web Vitals.
- **web-development-life-cycle** -- Web architecture, rendering strategy, performance, accessibility, SEO, cross-browser behavior.
- **mobile-development-life-cycle** -- Android/iOS lifecycle, permissions, offline sync, secure storage, store-readiness.
- **ux-research-and-experience-strategy** -- User research, journey friction, decision architecture, funnel analysis, usability.

#### Quality & Testing
- **qa-and-automation-engineer** -- Test strategy, automated coverage, release gates, mandatory release ladder.
- **test-driven-development** -- RED-GREEN-REFACTOR loop. Write failing test first, make it pass, refactor.
- **systematic-debugging** -- Root-cause-first debugging. Reproduce, trace end-to-end, fix source of truth, prove with regression test.

#### Architecture & Planning
- **software-development-life-cycle** -- Cross-domain delivery planning, architecture choices, work sequencing, release framing.
- **brainstorming** -- Socratic design exploration before implementation. Restates request, confirms user story, produces agreed design.
- **writing-plans** -- Turn agreed design into verifiable implementation plan with ordered steps and checks.
- **executing-plans** -- Execute a plan step by step, verifying each step before the next.
- **writing-user-stories** -- Connextra-format stories with Gherkin acceptance criteria, validated against INVEST.

#### Delivery & Git
- **finishing-a-development-branch** -- Verify, review, then present merge/PR options. Never force-push, never merge to main unilaterally.
- **git-expert** -- Safe Git workflows: branching, commits, PRs, merges, conflict resolution, history repair.
- **using-git-worktrees** -- Isolate feature work in its own checkout to prevent collisions with parallel work.
- **running-a-sprint** -- Scrum-style sprint loop over confirmed user stories until every story is Done.
- **dispatching-parallel-agents** -- Fan out independent work to concurrent subagents. Apply the four-condition independence test first.
- **subagent-driven-development** -- Delegate self-contained tasks to fresh-context subagents to preserve controller context.
- **designing-agent-teams** -- Decompose large tasks into coordinated specialist agents with clean handoffs.

#### Code Quality & Dependencies
- **preserve-existing-flow** -- Trace ownership and current behavior in brownfield code before any edit.
- **dependency-and-supply-chain** -- Dependency upgrades, lockfile hygiene, semver risk, SBOM, provenance, typosquatting checks.
- **compounding-knowledge** -- Capture solved problems as durable, discoverable knowledge artifacts.
- **writing-skills** -- Author and revise skills with TDD on the instructions themselves.
- **memory-status-reporter** -- Human-style memory health and learning reports.
- **compression-discipline** -- Per-turn output-compression when context is filling.
- **output-economy** -- Per-response output-token economy. Cut verbosity without dropping signal.
- **internationalization-and-localization** -- i18n/l10n message catalogs, ICU MessageFormat, locale-aware formatting, RTL/bidi.

## Differences from Other Adapters

| Aspect | OpenCode Adapter | Codex Adapter | Cursor Adapter | Pi Agent Adapter |
|---|---|---|---|---|
| Mechanism | TypeScript plugin with lifecycle hooks | Codex plugin with hooks.json + script | Static .cursorrules file | Static AGENTS.md + .mcp.json |
| Runtime bridge | Yes -- bridge subcommands per event | Yes -- bridge subcommands per event | No -- rules only, manual keel CLI | No -- rules + MCP tools, manual keel CLI |
| Context injection | Automatic per session/prompt | Automatic per session/prompt | Via Cursor's rules injection | Via Pi Agent's AGENTS.md loading |
| Observation recording | Automatic on tool events | Automatic on tool events | Manual via keel CLI | Manual via keel CLI or MCP tools |
| Learning checkpoints | Automatic on compaction | Automatic on compaction | Manual via keel CLI | Manual via keel CLI or MCP tools |
| MCP server | N/A (native MCP in OpenCode) | N/A (Codex plugin model) | Via cursor mcp.json | Via standard .mcp.json |

The Pi Agent adapter is similar to the Cursor adapter in that it uses static instruction files rather than hook-based bridges. The key difference is that Pi Agent natively supports MCP servers, so the adapter includes a `.mcp.json` that registers keel's MCP server for direct tool access without needing the keel binary on every invocation. The 16 MCP tools provide the same capabilities that hooks deliver in Claude Code -- memory search, system map, skill routing, workflow management, and sprint tracking.

## Hooks Reference

Pi Agent's core does not expose a hook lifecycle comparable to Claude Code or Codex, but hook-like behavior is available through extensions:

- **pi-autohooks** (`pi install npm:pi-autohooks`) -- Script-based hooks at `pre-tool-use`, `post-tool-use`, and `agent-stop` stages, using the Claude Code-compatible JSON stdin/stdout protocol.
- **pi-shepherd** -- Rule-based hooks that block, notify, or rewrite tool calls dynamically via JSON config.
- **Extension API** -- Extensions can listen to `tool_call`, `tool_result`, `agent_end`, `session_end`, and `session_shutdown` events directly.

For users who want keel's hook equivalent behavior without extensions, see `hooks.json` in this directory for a reference mapping of what keel hooks provide and how to trigger them manually via the keel CLI or MCP tools.
