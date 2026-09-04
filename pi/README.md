# keel Pi Agent Bridge

Bridges Pi coding agent lifecycle events to the `keel` Rust CLI for context injection, Iron Law enforcement, compaction rerouting, observation recording, learning checkpoints, and session management.

## What This Does

Pi coding agent (https://pi.dev) loads `AGENTS.md` from the project root (or `~/.pi/agent/AGENTS.md` globally) as project instructions at startup, and runs TypeScript extensions that subscribe to a rich event set (https://pi.dev/docs/latest/extensions). This bridge has **three layers**, mirroring the OpenCode and Codex adapters:

1. **`AGENTS.md`** — the persistent keel iron law, skill catalog, workflow commands, and branch/commit rules, loaded into the system prompt at startup so the model has keel discipline from the first prompt.
2. **`keel-pi.ts`** — a TypeScript extension that subscribes to Pi's `session_start`, `before_agent_start`, `tool_call`, `tool_execution_end`, `session_before_compact`, `session_compact`, and `session_shutdown` events, wiring each to a host-neutral `keel bridge` subcommand. `before_agent_start` appends the cached session bootstrap and current per-prompt pointer to the turn's system prompt, so the bridge output reaches the model without accumulating persistent messages. The extension also enforces the Iron Law edit gate, reroutes noisy shell commands, records observations, and runs the compaction/session-end learning cycle.
3. **`.mcp.json`** — registers keel's MCP server so its tools (recall, system_map, skill_route, anvil, etc.; full surface asserted by `tests/doc_parity_test.rs`) are directly callable by the model without spawning the keel binary per invocation.

## Prerequisites

1. Pi coding agent installed (`npm install -g @earendil-works/pi-coding-agent`, or `pi install`).
2. The `keel` binary installed at `~/.keel/keel` (unix) or `~/.keel/keel.exe` (win32), or on `PATH`.
3. `tsx` or `node` available for TypeScript extension execution (Pi handles this natively for `*.ts` extensions in its discovery paths).

## Install

`keel install` auto-detects Pi coding agent (via `~/.pi/agent/` dir, `PI_CODING_AGENT_DIR` env var, or `pi` binary on PATH) and wires this bridge automatically. Use `--without pi` to skip, `--with pi` to force.

Manual install options:

### Option A: Global (recommended — applies keel to every project)

Pi loads global agent state from `~/.pi/agent/`. Copy the three layers there:

```bash
mkdir -p ~/.pi/agent/extensions
cp pi/AGENTS.md         ~/.pi/agent/AGENTS.md
cp pi/.mcp.json         ~/.pi/agent/mcp.json      # NOTE: Pi expects mcp.json, not .mcp.json
cp pi/keel-pi.ts        ~/.pi/agent/extensions/keel-pi.ts
```

On Windows PowerShell:

```powershell
New-Item -ItemType Directory -Path "$env:USERPROFILE\.pi\agent\extensions" -Force
Copy-Item pi\AGENTS.md         "$env:USERPROFILE\.pi\agent\AGENTS.md"
Copy-Item pi\.mcp.json         "$env:USERPROFILE\.pi\agent\mcp.json"
Copy-Item pi\keel-pi.ts        "$env:USERPROFILE\.pi\agent\extensions\keel-pi.ts"
```

Pi auto-discovers extensions from `~/.pi/agent/extensions/*.ts`, loads `AGENTS.md` into the system prompt, and reads `mcp.json` for MCP servers — all at startup. No `settings.json` `extensions` array entry is required for auto-discovered files, but you may add one explicitly if you place the file elsewhere (see the Note below).

### Option B: Project-scoped

Copy the three layers into your project root:

```bash
mkdir -p .pi/extensions
cp pi/AGENTS.md         ./AGENTS.md
cp pi/.mcp.json         ./.pi/mcp.json            # project-local MCP config lives in .pi/mcp.json
cp pi/keel-pi.ts        ./.pi/extensions/keel-pi.ts
```

On Windows PowerShell:

```powershell
New-Item -ItemType Directory -Path ".\.pi\extensions" -Force
Copy-Item pi\AGENTS.md         ".\AGENTS.md"
Copy-Item pi\.mcp.json         ".\.pi\mcp.json"
Copy-Item pi\keel-pi.ts        ".\.pi\extensions\keel-pi.ts"
```

Project-local settings override global ones. `AGENTS.md` at the repo root is discovered natively; the extension and MCP config are discovered from `.pi/`.

### Option C: Pi rules extension (optional, for glob-based scoping)

If you use the `pi-rules` package, you can place keel rules under `.pi/rules/` for file-type-scoped loading instead of (or in addition to) a root `AGENTS.md`:

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

### Note on extension discovery

Pi auto-discovers `*.ts` and `*/index.ts` from `~/.pi/agent/extensions/` (global) and `.pi/extensions/` (project-local). If you place `keel-pi.ts` elsewhere, reference it explicitly in `settings.json`:

```json
{
  "extensions": ["/absolute/path/to/keel-pi.ts"]
}
```

`settings.json` lives at `~/.pi/agent/settings.json` (global) or `.pi/settings.json` (project). Paths in the global file resolve relative to `~/.pi/agent`; paths in the project file resolve relative to `.pi`.

### Verify the install

After copying the files, confirm Pi loads them:

```bash
# Start Pi and check that AGENTS.md content is present in the system prompt
pi

# Check MCP server connectivity (keel's tools should appear)
/mcp
```

On the first edit-class tool call in a fresh session, the Iron Law gate will block until you have used a reading tool (Read/Glob/Grep) or a keel reading command — this is intended behavior.

## What the Rules Include

### Iron Law (4 rules)

0. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.
1. **Understand before building.** Restate what the request asks and research what is genuinely needed before writing code. No guessing, no building against an imagined spec.
2. **Invoke relevant skills.** If there is even a 1% chance a skill applies, use the Skill tool before writing code or giving a final answer.
3. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything.

### Key Commands

| Command | Use |
|---|---|
| `keel review pre-pr --base-ref origin/feat` | Review before PR |
| `keel memory scope resolve --create-missing --refresh-system-map` | Refresh memory |
| `keel code-search search --workspace-root "$PWD" --query "..."` | Search code |
| `keel code-search siblings` | Completeness scan after a fix or implement |

### Branch and Commit Rules

- Branch model: `main` <- `dev` <- `feat` <- `task/<task>` [<- flat `task/<task>-<subtask>`]
- Commit format: `[category]: [feature_category]: short info` (categories: Add, Config, Refactor, Wip, Fix, Docs; feature_category uppercase)
- Never delete a branch after push or merge

### MCP Server

The included `.mcp.json` (installed as `mcp.json`) registers keel's MCP server, exposing the full keel MCP tool surface (count asserted by `tests/doc_parity_test.rs`):

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
| `anvil` | Drive the Anvil delivery loop (compile/cast/sieve/stamp/run --dry-run in-process; loop and live run background via command_output) |

The MCP config uses Pi's documented structure (`{"settings": {...}, "mcpServers": {...}}`), with `idleTimeout` under `settings` and per-server options `command`/`args`/`env`/`url`/`lifecycle` (`lazy`|`eager`|`keep-alive`)/`idleTimeout`/`directTools`/`debug`. `directTools: true` exposes each keel tool as a top-level tool instead of under an `mcp_` prefix. Adjust the `command` field to `keel.exe` on Windows if installing manually.

### Skill Catalog (specialist skills, roster asserted by tests/doc_parity_test.rs)

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
- **ui-design-systems-and-responsive-interfaces** -- Design-system tokens, responsive layouts, accessibility (WCAG 2.2 AA), visual hierarchy.
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

#### Delivery & Git
- **finishing-a-development-branch** -- Verify, review, then present merge/PR options. Never force-push, never merge to main unilaterally.
- **git-expert** -- Safe Git workflows: branching, commits, PRs, merges, conflict resolution, history repair.
- **using-git-worktrees** -- Isolate feature work in its own checkout to prevent collisions with parallel work.
- **running-anvil** -- Single delivery loop (compile → cast → sieve → stamp → loop).
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

## Event → bridge wiring (keel-pi.ts)

| Pi event | keel bridge subcommand | Behavior |
|---|---|---|
| `session_start` | `session-start` | Bootstrap + workspace digest + MCP self-heal, cached once for the first `before_agent_start` |
| `before_agent_start` | `user-prompt` | Appends the per-prompt routing pointer and any cached lifecycle context to this turn's system prompt |
| `tool_call` (reading tool) | — | Marks Iron Law satisfied, allows |
| `tool_call` (edit-class) | `pre-tool-use` | Blocks with `{block:true,reason}` until Iron Law satisfied; then records gate state |
| `tool_call` (bash/shell) | `rewrite` | Mutates `event.input.command` in place to reroute noisy commands through `keel run --` |
| `tool_execution_end` | `observe` | Records tool observation (fire-and-forget) |
| `session_before_compact` | `pre-compact` | Learning checkpoint before the window rewrite |
| `session_compact` | `post-compact` | Idempotent learning upsert; cache recovery context for the next `before_agent_start` |
| `session_shutdown` | `session-end` | Learning cycle + session summary capture + marker cleanup |

Every bridge call is capped at a 500ms timeout and fails open to "no context / no block" on any error, so the extension never hangs a turn.

## Differences from Other Adapters

| Aspect | OpenCode Adapter | Codex Adapter | Cursor Adapter | Pi Agent Adapter |
|---|---|---|---|---|
| Mechanism | TypeScript plugin with lifecycle hooks | Codex plugin with hooks.json + script | Static .cursorrules file | AGENTS.md + TypeScript extension + mcp.json |
| Runtime bridge | Yes — bridge subcommands per event | Yes — bridge subcommands per event | No — rules only, manual keel CLI | Yes — bridge subcommands per event (via extension) |
| Iron Law enforcement | Yes — throws on `tool.execute.before` | Yes — PreToolUse hook | No (static rules) | Yes — `tool_call` returns `{block:true}` |
| Context injection | Automatic per session/prompt | Automatic per session/prompt | Via Cursor's rules injection | Automatic per session/prompt + AGENTS.md |
| Observation recording | Automatic on tool events | Automatic on tool events | Manual via keel CLI | Automatic on `tool_execution_end` |
| Learning checkpoints | Automatic on compaction | Automatic on compaction | Manual via keel CLI | Automatic on `session_compact` / `session_shutdown` |
| MCP server | N/A (native MCP in OpenCode) | N/A (Codex plugin model) | Via cursor mcp.json | Via `mcp.json` (Pi native MCP) |
| Marker dir | `opencode-session-started` | `codex-session-started` | N/A | `pi-session-started` |

The Pi Agent adapter now matches the OpenCode and Codex adapters in runtime coverage. The `AGENTS.md` file carries the persistent iron law in the system prompt (the part Pi can inject into the model turn directly), and the `keel-pi.ts` extension delivers the automatic, per-event behavior (Iron Law block, compaction reroute, observation, learning) that the static rules alone cannot.

## Hooks Reference

Pi coding agent's core does not expose a JSON hook registry like Claude Code or Codex; instead, hook-equivalent behavior is delivered by the `keel-pi.ts` extension (above) subscribing to Pi's ExtensionAPI events. `hooks.json` in this directory is a **reference mapping** that documents each keel hook, which Pi event delivers it, and the manual `keel` CLI / MCP fallback for users who choose not to install the extension.

For users who want additional hook-like behavior, these community packages complement (but are not required by) the keel bridge:

- **pi-autohooks** (`pi install npm:pi-autohooks`) -- Script-based hooks at `pre-tool-use`, `post-tool-use`, and `agent-stop` stages, using the Claude Code-compatible JSON stdin/stdout protocol.
- **pi-shepherd** -- Rule-based hooks that block, notify, or rewrite tool calls dynamically via JSON config.
