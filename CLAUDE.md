# CLAUDE.md — claude-core Project Guide

## Project Overview

This is the claude-core project — native delivery rails for Claude Code. It provides:
- 1 bootstrap **skill** (`using-claude-core/SKILL.md`) injected verbatim at every `SessionStart` to establish the research-first iron law and list every other skill
- 24 specialist Claude Code **skills** for software delivery (`<name>/SKILL.md`)
- 16 technique/process **skills** (`brainstorming`, `test-driven-development`, `systematic-debugging`, `writing-plans`, `executing-plans`, `subagent-driven-development`, `dispatching-parallel-agents`, `using-git-worktrees`, `finishing-a-development-branch`, `receiving-code-review`, `writing-skills`, `designing-agent-teams`, `compounding-knowledge`, `adversarial-security-review`, plus the token-discipline pair `compression-discipline` and `output-economy`) — main-thread skills with no subagent or managed profile. This makes 41 `SKILL.md` directories total (24 specialists + 16 technique + 1 bootstrap); 40 are matcher-invokable (all but the bootstrap, which loads automatically at SessionStart). `requesting-code-review` is an alias pointer to `reviewer`, not a directory.
- 24 matching Claude Code **subagents** for token-efficient delegation (`.claude/agents/<name>.md`)
- 24 internal **managed profiles** consumed by the CLI (`<name>/agents/claude.yaml`)
- Workflow routing and escalation rules
- Review gates (pre-commit, pre-PR)
- Professional text templates
- Hooks wired into Claude Code's `settings.json` for transparent command rewriting and auto-routing
- A Rust CLI (`claude-skills`) for workflow, memory, command compaction, and hook installation

### Terminology

These three terms are **not** interchangeable:

| Term | What it is | Where it lives | Schema |
|---|---|---|---|
| **Skill** | Claude Code knowledge unit loaded into the main conversation when it matches a request | Source: `<name>/SKILL.md`. Installed: `~/.claude/skills/<name>/SKILL.md` | YAML frontmatter with `name`, `description`, `when_to_use`, `allowed-tools` |
| **Subagent** | Claude Code delegation target that runs in an isolated context window | `.claude/agents/<name>.md` (project) or `~/.claude/agents/<name>.md` (user) | YAML frontmatter with `name`, `description`, `tools`, `model` |
| **Managed profile** | Internal CLI configuration that wires reasoning effort, default prompts, and policy for the `claude-skills` runtime — **not** seen by Claude Code | `<name>/agents/claude.yaml` | Custom YAML consumed by the Rust CLI |

A "skill" runs in the main thread (instructions inline, costs ongoing tokens). A "subagent" runs in its own context window (saves main-thread tokens but adds delegation overhead). The "managed profile" is invisible to Claude Code itself — it only configures how `claude-skills` orchestrates work.

## Key Files

- `00-skill-routing-and-escalation.md` — Read this first. Defines skill routing and escalation.
- `AGENTS.md` — Agent operating doctrine (uses "agent" in the broad sense — covers skills, subagents, and managed profiles).
- `WORKFLOW.md` — Branch and completion rules.
- `templates/` — Professional text templates (commit, PR, final response, review).
- `.claude/review.json` — Review policy configuration.
- `.claude/hooks.json` — Claude Code hook wiring rendered by `claude-skills hook install`.
- `.claude-plugin/plugin.json` — Plugin manifest for Claude Code's plugin system.
- `commands/` — Custom slash commands (`/claude-core:<name>`) wrapping the implemented CLI surfaces: `workflow`, `review`, `recall`, `gain`. Registered via the `commands` key in the plugin manifest. The native `claude-skills install` also syncs them to `~/.claude/commands/` via `sync_commands`, so they work whether installed through the plugin path or the native installer.

## Specialist Layout

Each specialist contains three artifacts, plus an optional reference library:
- `<name>/SKILL.md` — Skill definition (loaded by Claude Code when relevant)
- `.claude/agents/<name>.md` — Subagent definition (delegation target with isolated context)
- `<name>/agents/claude.yaml` — Managed profile (CLI runtime configuration)
- `<name>/references/` — Deep knowledge files referenced by SKILL.md (most specialists; the narrow specialists `api-contract-design`, `postgres-migration-safety`, `react-performance-audit`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `data-and-ml-engineering`, `authentication-and-identity`, `cloud-cost-and-finops`, and `internationalization-and-localization` ship a self-contained SKILL.md with no reference library)

24 specialists: `software-development-life-cycle`, `web-development-life-cycle`, `mobile-development-life-cycle`, `backend-and-data-architecture`, `cloud-and-devops-expert`, `qa-and-automation-engineer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `reviewer`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `memory-status-reporter`, `api-contract-design`, `react-performance-audit`, `postgres-migration-safety`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `data-and-ml-engineering`, `authentication-and-identity`, `cloud-cost-and-finops`, `internationalization-and-localization`.

## Schema Compliance Notes

**SKILL.md frontmatter** follows the official Claude Code skill spec. Per the docs, all SKILL.md frontmatter fields are technically optional, but `name` and `description` are **strongly recommended** because the skill matcher uses them to decide when to load the skill. The combined `description` + `when_to_use` text is capped at 1,536 characters. The fields used in this project are all documented Claude Code fields: `name`, `description`, `when_to_use`, `allowed-tools`, `effort`, and `paths`. Reference: https://code.claude.com/docs/en/skills.

Other official optional fields not currently used here include `disable-model-invocation`, `user-invocable`, `disallowed-tools`, `argument-hint`, `arguments`, `model`, `context`, `agent`, `hooks`, and `shell`. Add them deliberately when a skill needs that capability.

**Subagent frontmatter** (`.claude/agents/<name>.md`) follows the official spec: `name` and `description` are required; `tools` (comma-separated bare tool names), `model` (`opus`, `sonnet`, `haiku`, or `inherit`), `color`, and `skills` (a YAML list of bare skill names to preload at startup) are optional. Note: scoped tool patterns like `Bash(git diff:*)` work in SKILL.md `allowed-tools` but not in subagent `tools` — subagents use bare tool names. A consequence: the six read-only review subagents (`reviewer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `ux-research-and-experience-strategy`, `memory-status-reporter`) correctly omit `Edit`/`Write` but still carry an unscoped `Bash` grant, so their read-only contract is enforced by instruction (the `_shared/subagent-iron-law.md` "respect their intent" rule), not by the tool grant — a determined shell command could still mutate the tree. Each managed subagent preloads its same-named skill via `skills:` so the full skill content is in context from startup rather than loaded on demand; `skills` is supported for plugin subagents and a missing/disabled skill is skipped with a debug-log warning. Reference: https://code.claude.com/docs/en/sub-agents.

**Hook events** (`.claude/hooks.json`) are wired through `claude-skills hook <event>` for every Claude Code lifecycle event the manager observes. The `HOOK_EVENTS` table in `rust/crates/claude-skills/src/hooks/claude.rs` defines **30 events**, of which 28 install into `settings.json`. Two rows carry `installs_in_settings: false`: `FileChanged` (its matcher doubles as a per-repo watch list, so an empty matcher would ship dead config) and `MessageDisplay` (no matcher, fires on every assistant message, emits `hookSpecificOutput.displayContent` — auto-installing would either be a no-op or silently rewrite on-screen text, so it stays opt-in). Both still dispatch for ad-hoc invocations (`claude-skills hook file-changed`, `claude-skills hook message-display`). Events the runtime does not actively emit are stubbed for forward-compatibility — the dispatcher no-ops until behavior is needed. When Anthropic adds or renames events, update both `HOOK_EVENTS` and the generated `.claude/hooks.json`. Reference: https://code.claude.com/docs/en/hooks.

**Output styles**: Claude Code ships four built-in output styles — `Default`, `Proactive`, `Explanatory`, and `Learning`. The active style for this project is set in `.claude/settings.local.json`. Reference: https://code.claude.com/docs/en/output-styles.

**Plugin manifest** (`.claude-plugin/plugin.json`) follows the official plugin schema. Only `name` is required; `displayName`, `version`, `description`, `skills`, `agents`, `hooks`, `commands`, `mcpServers`, `outputStyles`, `lspServers`, `experimental.themes`, `experimental.monitors`, `userConfig`, `channels`, and `dependencies` are optional. This project uses `skills`, `agents`, `commands` (set to `["./commands/"]`), `hooks`, and `mcpServers`. Per the official reference, listing `commands` **replaces** the default `commands/` scan, so the explicit `["./commands/"]` keeps the default directory. Command `.md` files live at the plugin root `commands/` (not inside `.claude-plugin/`). Reference: https://code.claude.com/docs/en/plugins-reference.

**Token-saving proxy**: command-output compaction lives in `rust/crates/claude-skills/src/proxy/`. The native `claude-skills run -- <command>`, `claude-skills rewrite`, and `claude-skills gain` surfaces own this work. When Claude Code introduces native compaction primitives, prefer them and keep this layer thin.

**Native Auto memory**: recent Claude Code ships *Auto memory* — notes the model writes itself to `~/.claude/projects/<project>/memory/MEMORY.md` from your corrections, loaded automatically each session (docs: https://code.claude.com/docs/en/memory). This is complementary to claude-core's memory surfaces, not redundant: native Auto memory is passive and machine-local for incidental learnings; claude-core's `SYSTEM_MAP`, working briefs, completion gate, FTS5 recall, and `memoriesv2` families are explicit, structured, reconcilable artifacts. Prefer native Auto memory for incidental notes and the structured commands when an artifact must survive compaction or be reconciled against the request; do not duplicate the same fact into both.

**MCP server**: `claude-skills mcp serve` runs a JSON-RPC 2.0 stdio server registered through `.claude-plugin/plugin.json` `mcpServers.claude_core`, pinned with `alwaysLoad: true` so Claude Code keeps the server's tools in context instead of deferring them (`alwaysLoad` is per-server, so all tools are pinned together). Claude Code auto-discovers it and gets 14 tools plus two resources (`claude_core://system-map`, `claude_core://recall/status`). The tools fall into five groups: **awareness** (`context_brief` — one call returning the iron law, the full skill catalog, memory health, and the newest working brief, so the agent knows what exists even when no skill auto-loaded), **search/compaction** (`recall`, `run_command`, `recall_status`), **skills** (`skill_route`, `skill_get`, `skill_list`), **memory** (`memory_status`, `brief_list`, `brief_get`, `brief_create`), **workspace map** (`system_map`, `system_map_refresh`), and a **generic passthrough** (`cli`) that runs any remaining claude-skills subcommand so the MCP surface matches the full CLI surface. `cli` gates destructive/management subcommands (`install`, `update`, `repair`, `uninstall`, `validate`, `all`, `__self-replace`, and `checkpoint restore`) behind an explicit `confirm: true`, and refuses `mcp` outright. The `run_command` and `cli` tools run through the same proxy capture+compaction pipeline as `claude-skills run --`, so command-output compaction applies on the MCP surface too. The skill/memory/brief tools mirror capabilities the lifecycle hooks otherwise deliver (e.g. `skill_route` is the on-demand equivalent of the per-prompt skill-brief injection) so they stay reachable on platforms where hooks are unreliable; each is a thin wrapper over the same function that backs the corresponding CLI surface, so MCP and CLI never drift. The `initialize` handshake echoes the client's requested `protocolVersion` (falling back to the server default) so the server stays compatible as the MCP spec revises. If the server's tools ever go missing or stop loading, `claude-skills doctor` reports the registration and its `alwaysLoad` state and `claude-skills repair` re-pins the `~/.claude.json` entry. Tool dispatch lives in `mcp/tools.rs`; `mcp/mod.rs` keeps JSON-RPC framing, the serve loop, and the resource surface.

## Routing Rules

0. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed — the owning module, the framework, the real requirement. Do not guess, do not assume, do not build against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode here: it passes review and still gets thrown away. Researching first is what saves you from building the wrong thing. If the request is ambiguous in a way that changes what you build, ask before building, not after. This is rule zero because it gates every rule below — there is no point routing a skill or refreshing memory for the wrong task.

1. Routing is driven by Claude Code's native skill matcher against the installed `~/.claude/skills/<name>/SKILL.md` files — each skill's frontmatter (`description`, `when_to_use`) is what triggers selection. The bootstrap skill `using-claude-core/SKILL.md` is injected verbatim into `SessionStart` `hookSpecificOutput.additionalContext` per the official Claude Code hooks schema, so the iron law (understand before building, research first, invoke skills before responding, find the root cause) and the full skill catalog land in model context once at session start. `UserPromptSubmit` then restates the iron law in compact form on every turn.
2. Run `preserve-existing-flow` before editing any existing source file.
3. Run `reviewer` before closing **non-trivial** work. Trivial exemptions: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work the user asked for. Everything else (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield rewrites) goes through `reviewer` before close.
4. Delegate to the matching `.claude/agents/<name>.md` subagent for heavy work that benefits from an isolated context window (saves main-thread tokens).
5. Use `templates/` for commit bodies, PR bodies, final responses, and review summaries.
6. Read `WORKFLOW.md` for branch naming, commit format, and completion rules.
7. **Writing Discipline** applies to all written output — docs, code comments, commit/PR text, review notes, chat replies: write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.

### Enforcement Gates (PostToolBatch)

Two default-on PostToolBatch gates are the only model-independent backstop for the Iron Law — hooks cannot force a tool/Skill call, but they can inject a reminder when a required artifact is missing. Both live in `rust/crates/claude-skills/src/runner/hook_lifecycle.rs`:

- **Working-brief gate** (`CLAUDE_SKILLS_BRIEF_GATE`, front of the law) — fires once when a session edits code but no working brief was written this session. Clear it by writing one: `claude-skills memory working-brief write --request "..." --acceptance-criteria "..."`, or the `brief_create` MCP tool.
- **Review gate** (`CLAUDE_SKILLS_REVIEW_GATE`, back of the law) — fires once when a session edits code but records no reviewer pass since the last edit. Clear it by invoking the `reviewer` skill or running `claude-skills review pre-pr`.

Each env var takes three values: **unset / anything else → non-blocking nudge** (the default — the reminder is injected via `hookSpecificOutput.additionalContext`, so the agent is told to act but the turn is *not* halted); **`block` → opt-in hard stop** (`decision: "block"`, refuses closeout until the artifact exists); **`off` (or `…_MAX_BLOCKS=0`) → disabled**. Whichever mode is set, each gate fires at most `…_MAX_BLOCKS` time(s) per session (default 1) via a monotonic counter, then falls through to the generic advisory — so it can neither loop nor spam — and fails open to the advisory on any telemetry error. Pure-research and question turns (no code edits) never fire a gate.

## Commands

- `claude-skills workflow route --request "..."` — Route a request
- `claude-skills workflow start --preset <preset> --request "..."` — Start work
- `claude-skills workflow cockpit` — Watch state
- `claude-skills review pre-commit` — Pre-commit review
- `claude-skills review pre-pr` — Pre-PR review
- `claude-skills workflow finish` — Finish branch with proof
- `claude-skills run -- <command>` — Run with output compaction
- `claude-skills memory scope resolve --create-missing --refresh-system-map` — Refresh memory
- `claude-skills hook install` — Wire hooks into Claude Code's `settings.json`
- `claude-skills doctor` — Report MCP registration health (including `alwaysLoad` state)
- `claude-skills repair` — Re-pin the MCP server entry in `~/.claude.json`

### Declarative Filter Registry

`claude-skills run` supports project-specific TOML filter files that compact command output without writing Rust code.

Place a filter file at either:
- `.claude-skills/filters.toml`
- `claude-skills.filters.toml` (project root)

Example:
```toml
[[filter]]
name = "cargo-test"
command = "cargo test"
match_mode = "starts_with"  # starts_with | exact | contains | regex
keep = ["FAILED", "error", "test result"]
remove = ["running", "Doc-tests"]
max_lines = 50
```

| Field | Required | Default | Description |
|---|---|---|---|
| `name` | yes | — | Filter identifier |
| `command` | yes | — | Command string to match |
| `match_mode` | no | `starts_with` | How to match: `starts_with`, `exact`, `contains`, `regex` |
| `exit_code` | no | any | Only apply when exit code matches |
| `keep` | no | `[]` | Line substrings to retain (empty = keep all non-removed) |
| `remove` | no | `[]` | Line substrings to discard before keep |
| `max_lines` | no | `40` | Max lines to retain |
| `enabled` | no | `true` | Toggle filter on/off |

## Build & Test

```bash
cargo build
cargo test
cargo fmt --all -- --check
cargo clippy -- -D warnings
```

## Skill Lint

`claude-skills skill-lint --repo-root .` validates that every `<name>/SKILL.md`
has the structural properties the Claude Code matcher needs to *trigger* the
skill — non-empty `description`, `name` matching the directory, combined
`description` + `when_to_use` within the 1536-char matcher budget, scoped
`allowed-tools` (warn), and no dangling `references/*.md` links. It is the
superpowers-style "test that skills trigger" gate, expressed as deterministic
checks. Add `--json` for machine output; exits non-zero when any skill fails.

## Config Audit

`claude-skills config-audit --repo-root .` is an AgentShield-style security audit
of claude-core's *own* config surface (not user code): `.claude/hooks.json`,
`.claude/settings.json` permissions, and `.claude-plugin/plugin.json`. It flags
shell-metacharacter injection and network fetches in hook commands,
`bypassPermissions` mode, unscoped `Bash` allow rules, and committed secret
literals in MCP env. Exits 2 on any high-severity finding. Add `--json` for
machine output. Distinct from the `security-and-compliance-auditor` skill, which
audits the user's application code.

## Code Checkpoints

`claude-skills checkpoint create|list|show|restore` is a git-backed working-tree
snapshot surface — the durable, cross-session analog to native `/rewind` for the
*code* axis. `create` snapshots tracked changes via `git stash create` pinned
under `refs/claude-checkpoints/<id>` (non-destructive); `restore --id <id>
--confirm` reapplies one, taking an automatic pre-restore safety snapshot first so
the restore is itself reversible. It does not capture conversation state (only
Claude Code's `/rewind` can), so the two are complementary.

