# CLAUDE.md — claude-core Project Guide

## Project Overview

This is the claude-core project — native delivery rails for Claude Code. It provides:
- 1 bootstrap **skill** (`using-claude-core/SKILL.md`) whose compact operating contract (the research-first iron law, MCP tools, and a skill-catalog pointer) is injected at every `SessionStart`; the full skill is too large for the `additionalContext` cap (Claude Code truncates above ~10KB), so the complete catalog and routing rules ship to disk and load on demand via `Skill("using-claude-core")`
- 24 specialist Claude Code **skills** for software delivery (`<name>/SKILL.md`)
- 18 technique/process **skills** (`brainstorming`, `writing-user-stories`, `running-a-sprint`, `test-driven-development`, `systematic-debugging`, `writing-plans`, `executing-plans`, `subagent-driven-development`, `dispatching-parallel-agents`, `using-git-worktrees`, `finishing-a-development-branch`, `receiving-code-review`, `writing-skills`, `designing-agent-teams`, `compounding-knowledge`, `adversarial-security-review`, plus the token-discipline pair `compression-discipline` and `output-economy`) — main-thread skills with no subagent or managed profile. This makes 44 first-party `SKILL.md` directories (24 specialists + 18 technique + 1 `requesting-code-review` alias + 1 bootstrap); 43 are matcher-invokable and listed in `.claude-plugin/plugin.json` (all but the bootstrap, which loads automatically at SessionStart). `requesting-code-review` is a real directory holding a thin alias skill that routes to `reviewer` (not a separate behavior). A 45th `SKILL.md` exists on disk under `karpathy-skills-cmp/` — a vendored benchmark artifact, not a claude-core skill and not in the plugin manifest. The `tests/doc_parity_test.rs` integration test enforces this manifest⇆disk correspondence so these counts cannot silently drift.
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
- `commands/` — Custom slash commands (`/claude-core:<name>`) wrapping the implemented CLI surfaces: `workflow`, `review`, `recall`, `gain`, `sprint`, `user-story`. Registered via the `commands` key in the plugin manifest. The native `claude-skills install` also syncs them to `~/.claude/commands/` via `sync_commands`, so they work whether installed through the plugin path or the native installer.

## Specialist Layout

Each specialist contains three artifacts, plus an optional reference library:
- `<name>/SKILL.md` — Skill definition (loaded by Claude Code when relevant)
- `.claude/agents/<name>.md` — Subagent definition (delegation target with isolated context)
- `<name>/agents/claude.yaml` — Managed profile (CLI runtime configuration)
- `<name>/references/` — Deep knowledge files referenced by SKILL.md (most specialists; the narrow specialists `api-contract-design`, `postgres-migration-safety`, `react-performance-audit`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `data-and-ml-engineering`, `authentication-and-identity`, `cloud-cost-and-finops`, and `internationalization-and-localization` ship a self-contained SKILL.md with no reference library)

24 specialists: `software-development-life-cycle`, `web-development-life-cycle`, `mobile-development-life-cycle`, `backend-and-data-architecture`, `cloud-and-devops-expert`, `qa-and-automation-engineer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `reviewer`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `memory-status-reporter`, `api-contract-design`, `react-performance-audit`, `postgres-migration-safety`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `data-and-ml-engineering`, `authentication-and-identity`, `cloud-cost-and-finops`, `internationalization-and-localization`.

## Schema Compliance Notes

**SKILL.md frontmatter** follows the official Claude Code skill spec. Per the docs, all SKILL.md frontmatter fields are technically optional, but `name` and `description` are **strongly recommended** because the skill matcher uses them to decide when to load the skill. The combined `description` + `when_to_use` text is capped at 1,536 characters. The fields currently used by claude-core's own skills are: `name`, `description`, `when_to_use`, `allowed-tools`, `effort`, and `paths`. All other official fields are supported and documented here for completeness:

| Field | Purpose |
|---|---|
| `name` | Display name; defaults to directory name. Controls invocation name only for plugin-root `SKILL.md` |
| `description` | Strongly recommended. Truncated at 1,536 chars combined with `when_to_use` in skill listings |
| `when_to_use` | Additional trigger context, appended to `description` in listings, counts toward 1,536-char cap |
| `disable-model-invocation` | Set `true` to prevent auto-loading — only manual `/name` invocation. Default: `false` |
| `user-invocable` | Set `false` to hide from `/` menu. Default: `true` |
| `allowed-tools` | Grant permission for listed tools while skill is active (scoped patterns like `Bash(git diff:*)` work here) |
| `disallowed-tools` | Remove tools from the pool while skill is active; clears on next message |
| `model` | Override model for the rest of the current turn: `sonnet`, `opus`, `haiku`, `fable`, full ID, or `inherit` |
| `effort` | Override effort level: `low`, `medium`, `high`, `xhigh`, `max` (ultracode = `xhigh`) |
| `context` | Set to `fork` to run the skill in a forked subagent context |
| `agent` | Which subagent type to use when `context: fork` is set (`Explore`, `Plan`, `general-purpose`, or custom) |
| `hooks` | Hooks scoped to this skill's lifecycle (see Hook events table) |
| `paths` | Glob patterns limiting when the skill auto-activates (comma-separated string or YAML list) |
| `shell` | Shell for `` !`command` `` and ` ```! ` blocks: `bash` (default) or `powershell` (requires `CLAUDE_CODE_USE_POWERSHELL_TOOL=1`) |
| `argument-hint` | Autocomplete hint shown for expected arguments, e.g. `[issue-number]` or `[filename] [format]` |
| `arguments` | Named positional arguments for `$name` substitution in skill content; names map to argument positions |

String substitutions available in skill content: `$ARGUMENTS`, `$ARGUMENTS[N]` / `$N`, `$name`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}`. Reference: https://code.claude.com/docs/en/skills.

**Subagent frontmatter** (`.claude/agents/<name>.md`) follows the official spec: `name` and `description` are required. Full field reference:

| Field | Required | Purpose |
|---|---|---|
| `name` | Yes | Lowercase letters and hyphens. Hooks receive this as `agent_type` |
| `description` | Yes | When Claude should delegate to this subagent |
| `tools` | No | Allowlist of tools. Scoped patterns like `Bash(git diff:*)` do NOT work here — subagents use bare tool names only |
| `disallowedTools` | No | Denylist; applied before `tools` allowlist is resolved |
| `model` | No | `sonnet`, `opus`, `haiku`, `fable`, full ID, or `inherit` (default) |
| `permissionMode` | No | `default`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`, `plan` |
| `maxTurns` | No | Maximum agentic turns before subagent stops |
| `skills` | No | Preload skills into subagent context at startup; full content injected, not just description. `disable-model-invocation: true` skills cannot be preloaded |
| `mcpServers` | No | Inline or string-reference MCP server definitions scoped to this subagent |
| `hooks` | No | Lifecycle hooks scoped to this subagent |
| `memory` | No | `user` (`~/.claude/agent-memory/<name>/`), `project` (`.claude/agent-memory/<name>/`), or `local` (`.claude/agent-memory-local/<name>/`) — enables cross-session learning |
| `background` | No | Set `true` to always run as a background task |
| `effort` | No | Override effort level: `low`, `medium`, `high`, `xhigh`, `max` |
| `isolation` | No | Set to `worktree` to run in a temporary git worktree (isolated checkout, auto-cleanup if no changes) |
| `color` | No | Display color in task list: `red`, `blue`, `green`, `yellow`, `purple`, `orange`, `pink`, `cyan` |
| `initialPrompt` | No | Auto-submitted as first user turn when agent runs as main session via `--agent` or `agent` setting |

Note: scoped tool patterns like `Bash(git diff:*)` work in SKILL.md `allowed-tools` but NOT in subagent `tools` — subagents use bare tool names only. A consequence: the six read-only review subagents (`reviewer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `ux-research-and-experience-strategy`, `memory-status-reporter`) correctly omit `Edit`/`Write` but still carry an unscoped `Bash` grant, so their read-only contract is enforced by instruction (the `_shared/subagent-iron-law.md` "respect their intent" rule), not by the tool grant. Each managed subagent preloads its same-named skill via `skills:` so the full skill content is in context from startup rather than loaded on demand; `skills` is supported for plugin subagents and a missing/disabled skill is skipped with a debug-log warning. Reference: https://code.claude.com/docs/en/sub-agents.

**Hook events** (`.claude/hooks.json`) are wired through `claude-skills hook <event>` for every Claude Code lifecycle event the manager observes. The `HOOK_EVENTS` table in `rust/crates/claude-skills/src/hooks/claude.rs` defines **30 events**, of which 28 install into `settings.json`. Two rows carry `installs_in_settings: false`: `FileChanged` (its matcher doubles as a per-repo watch list, so an empty matcher would ship dead config) and `MessageDisplay` (no matcher, fires on every assistant message, emits `hookSpecificOutput.displayContent` — auto-installing would either be a no-op or silently rewrite on-screen text, so it stays opt-in). Both still dispatch for ad-hoc invocations (`claude-skills hook file-changed`, `claude-skills hook message-display`). Actively implemented handlers: `SessionStart` (MCP self-heal + bootstrap context + watchPaths), `UserPromptSubmit` (skill routing + compression nudge), `PreToolUse` (command rewrite + auto-allow rules), `PostToolUse` (timings + observations + system map refresh, runs async), `PostToolUseFailure` (timings + failure observation, runs async), `PostToolBatch` (review/brief/closeout gates), `Stop` (closeout context injection), `SubagentStart` (iron law context injection), `SubagentStop` (lifecycle), `PermissionRequest` (auto-approve claude-skills commands), `PermissionDenied` (retry signaling), `CwdChanged` (system map refresh), `SessionEnd` (auto-capture + learning + prune), `PreCompact`/`PostCompact` (learning + system map), `Notification` (terminal bell). Remaining events dispatch through the lifecycle wildcard for forward-compatibility. Hook types supported: `command` (shell scripts), `http` (POST event JSON to a URL), `mcp_tool` (call an MCP server tool), `prompt` (evaluate a prompt with an LLM using `$ARGUMENTS` for context), and `agent` (run an agentic verifier with tools for complex verification). Elicitation events (`Elicitation`, `ElicitationResult`) handle MCP server user-input requests — the hook can accept/decline/cancel via `hookSpecificOutput.action`. When Anthropic adds or renames events, update both `HOOK_EVENTS` and the generated `.claude/hooks.json`. Reference: https://code.claude.com/docs/en/hooks.

**Output styles**: Claude Code ships four built-in output styles — `Default`, `Proactive`, `Explanatory`, and `Learning`. The active style for this project is set in `.claude/settings.local.json`. Reference: https://code.claude.com/docs/en/output-styles.

**Plugin manifest** (`.claude-plugin/plugin.json`) follows the official plugin schema. Only `name` is required; `displayName`, `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`, `skills`, `agents`, `hooks`, `commands`, `mcpServers`, `outputStyles`, `lspServers`, `experimental.themes`, `experimental.monitors`, `userConfig`, `channels`, and `dependencies` are optional. Notable fields: `displayName` (human-readable name with spaces, shown in `/plugin` picker; `name` is used for namespacing only), `defaultEnabled` (ship plugin in disabled state; users opt in via `/plugin` or `claude plugin enable`), `shell` (for hook scripts; `powershell` on Windows requires `CLAUDE_CODE_USE_POWERSHELL_TOOL=1`). This project uses `skills`, `agents`, `commands` (set to `["./commands/"]`), `hooks`, `mcpServers`, `userConfig` (configurable review strictness, system map refresh interval, and memory retention days), and `experimental.monitors` (a build-watcher that reports Rust compilation errors on review invocation). Per the official reference, listing `commands` **replaces** the default `commands/` scan, so the explicit `["./commands/"]` keeps the default directory. Command `.md` files live at the plugin root `commands/` (not inside `.claude-plugin/`). For `hooks` and `mcpServers`, multiple source paths are merged rather than replaced. Reference: https://code.claude.com/docs/en/plugins-reference.

**Token-saving proxy**: command-output compaction lives in `rust/crates/claude-skills/src/proxy/`. The native `claude-skills run -- <command>`, `claude-skills rewrite`, and `claude-skills gain` surfaces own this work. When Claude Code introduces native compaction primitives, prefer them and keep this layer thin.

**Native Auto memory**: recent Claude Code ships *Auto memory* — notes the model writes itself to `~/.claude/projects/<project>/memory/MEMORY.md` from your corrections, loaded automatically each session (docs: https://code.claude.com/docs/en/memory). This is complementary to claude-core's memory surfaces, not redundant: native Auto memory is passive and machine-local for incidental learnings; claude-core's `SYSTEM_MAP`, working briefs, completion gate, FTS5 recall, and `memoriesv2` families are explicit, structured, reconcilable artifacts. Prefer native Auto memory for incidental notes and the structured commands when an artifact must survive compaction or be reconciled against the request; do not duplicate the same fact into both.

**Memory ownership boundary (no double-write)**: claude-core and native Auto memory write to *disjoint* paths, so there is no collision to guard against. Native Auto memory owns `~/.claude/projects/<project>/memory/MEMORY.md`. claude-core's autonomous learning loop owns `~/.claude/memory/instincts/`, `~/.claude/skills/learned-<project>/`, and `~/.claude/agents/learned-<project>.md`; its explicit surfaces own `~/.claude/memory/`, `~/.claude/memories*/`, and `~/.claude/working-briefs/`. claude-core never writes the native `MEMORY.md`. The two layers are read together at SessionStart (native loads its file, claude-core injects its digest) and never overwrite each other.

**Recall vs code-search boundary**: `recall` (FTS5 over `memory`, `memories`, `memoriesv2`, `working-briefs`) is the *memory* index — durable notes, briefs, and learnings. It deliberately does **not** index project source, so a code question is not answered with a stale memory hit. Searching the working tree is `claude-skills code-search search` (a fresh, gitignore-aware scan of repository files). Use `recall` for "what did I learn / decide / capture", `code-search` for "where in the code is X". Neither is a blind scan: recall is indexed; code-search walks the live tree and skips `target/`, `node_modules/`, `.git/`, and binaries.

**MCP server**: `claude-skills mcp serve` runs a JSON-RPC 2.0 stdio server registered through `.claude-plugin/plugin.json` `mcpServers.claude_core`, pinned with `alwaysLoad: true` so Claude Code keeps the server's tools in context instead of deferring them (`alwaysLoad` is per-server, so all tools are pinned together). Claude Code auto-discovers it and gets 16 tools plus two resources (`claude_core://system-map`, `claude_core://recall/status`). The tools fall into six groups: **awareness** (`context_brief` — one call returning the iron law, the full skill catalog, memory health, and the newest working brief, so the agent knows what exists even when no skill auto-loaded), **search/compaction** (`recall`, `run_command`, `recall_status`), **skills** (`skill_route`, `skill_get`, `skill_list`), **memory** (`memory_status`, `brief_list`, `brief_get`, `brief_create`), **workspace map** (`system_map`, `system_map_refresh`), **workflow** (`sprint`, `user_story_lint` — the fail-closed sprint loop and the strict Connextra+Gherkin+INVEST story lint as dedicated tools so they stay reachable when hooks are unreliable), and a **generic passthrough** (`cli`) that runs any remaining claude-skills subcommand so the MCP surface matches the full CLI surface. The `tests/doc_parity_test.rs` integration test counts the tool definitions in `mcp/tools.rs` and asserts this documented count, so the number cannot silently drift again. `cli` gates destructive/management subcommands (`install`, `update`, `repair`, `uninstall`, `remove`, `validate`, `all`, `__self-replace`, `checkpoint restore`, and `hook install`/`hook uninstall`) behind an explicit `confirm: true`, and refuses `mcp` outright. The `run_command` and `cli` tools run through the same proxy capture+compaction pipeline as `claude-skills run --`, so command-output compaction applies on the MCP surface too. The skill/memory/brief tools mirror capabilities the lifecycle hooks otherwise deliver (e.g. `skill_route` is the on-demand equivalent of the per-prompt skill-brief injection) so they stay reachable on platforms where hooks are unreliable; each is a thin wrapper over the same function that backs the corresponding CLI surface, so MCP and CLI never drift. The `initialize` handshake echoes the client's requested `protocolVersion` (falling back to the server default) so the server stays compatible as the MCP spec revises. If the server's tools ever go missing or stop loading, `claude-skills doctor` reports the registration and its `alwaysLoad` state and `claude-skills repair` re-pins the `~/.claude.json` entry. Tool dispatch lives in `mcp/tools.rs`; `mcp/mod.rs` keeps JSON-RPC framing, the serve loop, and the resource surface.

## Routing Rules

0. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed — the owning module, the framework, the real requirement. Do not guess, do not assume, do not build against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode here: it passes review and still gets thrown away. Researching first is what saves you from building the wrong thing. If the request is ambiguous in a way that changes what you build, ask before building, not after. This is rule zero because it gates every rule below — there is no point routing a skill or refreshing memory for the wrong task.

1. Routing is driven by Claude Code's native skill matcher against the installed `~/.claude/skills/<name>/SKILL.md` files — each skill's frontmatter (`description`, `when_to_use`) is what triggers selection. The bootstrap skill `using-claude-core/SKILL.md` is injected verbatim into `SessionStart` `hookSpecificOutput.additionalContext` per the official Claude Code hooks schema, so the iron law (understand before building, research first, invoke skills before responding, find the root cause) and the full skill catalog land in model context once at session start. `UserPromptSubmit` then restates the iron law in compact form on every turn.
2. Run `preserve-existing-flow` before editing any existing source file.
3. Run `reviewer` before closing **non-trivial** work. Trivial exemptions: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work the user asked for. Everything else (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield rewrites) goes through `reviewer` before close.
4. Delegate to the matching `.claude/agents/<name>.md` subagent for heavy work that benefits from an isolated context window (saves main-thread tokens). Subagents cannot spawn other subagents; use `Skill` tool or chain from the main conversation instead.
5. Use `templates/` for commit bodies, PR bodies, final responses, and review summaries.
6. Read `WORKFLOW.md` for branch naming, commit format, and completion rules.
7. **Agent teams** (agent teams are different from subagents): Teammates communicate via `SendMessage` tool with the agent's ID as the `to` field. Resumed subagents retain full conversation history and auto-resume in the background when they receive a `SendMessage`. The `SubagentStop` event fires when a subagent finishes; `TeammateIdle` fires when a teammate is about to go idle — both support matchers to target specific agent types. Background subagents run concurrently with auto-deny on permission prompts; foreground subagents block until complete. Set `CLAUDE_CODE_FORK_SUBAGENT=1` to make every subagent spawn a fork that inherits the full conversation history. Reference: https://code.claude.com/docs/en/agent-teams.
8. **Writing Discipline** applies to all written output — docs, code comments, commit/PR text, review notes, chat replies: write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.

### Enforcement Gates (PostToolBatch + Stop)

Three default-on gates are the model-independent backstop for the Iron Law — hooks cannot force a tool/Skill call, but they can inject a reminder when a required artifact is missing. The gates fire on PostToolBatch (mid-turn checkpoint) and the Stop event also injects a supplementary closeout reminder. All three gate implementations live in `rust/crates/claude-skills/src/runner/hook_lifecycle.rs`:

- **Working-brief gate** (`CLAUDE_SKILLS_BRIEF_GATE`, front of the law) — fires once when a session edits code but no working brief was written this session. Clear it by writing one: `claude-skills memory working-brief write --request "..." --acceptance-criteria "..."`, or the `brief_create` MCP tool.
- **Review gate** (`CLAUDE_SKILLS_REVIEW_GATE`, back of the law) — fires once when a session edits code but records no reviewer pass since the last edit. Clear it by invoking the `reviewer` skill or running `claude-skills review pre-pr`.
- **Honest-closeout gate** (`CLAUDE_SKILLS_STORY_CLOSEOUT_GATE`, the honesty backstop) — fires when the current workspace has an **active sprint** (`claude-skills sprint` story records exist) that is not COMPLETE, injecting a gap report that names each open/blocked story and tells the agent to state it as a gap and loop back rather than present the work as done. This is why an incomplete sprint cannot be soft-closed: the user-visible "I found these gaps, I'm not done" is enforced at the one end-of-turn event that can inject context. Distinct from the other two in two ways: it is **scoped to user-story work** (silent when there is no sprint, so ordinary and question turns are untouched — the "if based on user stories, else ignore" rule), and it is **not gated on this turn editing code** (an incomplete sprint matters at closeout even on a no-edit summary turn). Clear it by finishing the stories (`claude-skills sprint advance --id <id> --state done` as each passes its acceptance criteria and review) until `claude-skills sprint review` reports COMPLETE.

Why the gates ride PostToolBatch *in addition to* Stop: PostToolBatch fires after every tool batch (more frequently than Stop) and catches issues mid-turn. The Stop event also injects closeout context at the natural "about to stop" moment — per the official Claude Code hooks schema, `Stop` and `SubagentStop` both support `hookSpecificOutput.additionalContext` and `decision: "block"`. Only `SessionEnd` truly cannot inject context (it fires at session termination and carries no hookSpecificOutput). The gates ride PostToolBatch as the *primary* checkpoint because it fires more often, but the Stop handler injects a supplementary closeout reminder at the final moment.

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
- `claude-skills sprint plan|status|advance|review|list` — Drive a Scrum-style sprint loop over confirmed user stories
- `claude-skills user-story lint` — Validate user stories against strict Agile/Jira format (Connextra + Gherkin + INVEST)
- `claude-skills dispatch plan|start|status|complete|merge|abandon|list` — Coordinate git-worktree-isolated parallel workers with a fail-closed merge (only a `complete` worker merges; conflict aborts via `git merge --abort` leaving a clean tree; `merge`/`abandon` are `--confirm`-gated). Owns the worktree lifecycle + durable ledger + merge gate; it does **not** spawn agents — the main thread still drives the subagents.
- `claude-skills eval` — Run the real compaction eval: drives the genuine adapter pipeline over embedded fixtures and reports EXACT o200k_base token deltas, with measured floors asserted in CI. This is the real measurement; the legacy `bench` is only a runtime-provenance/feature-parity marker (see below).
- `claude-skills observe` — Read-only aggregator over recall health + working-brief count + sprint progress; defers the token-savings axis to `gain`/`session` rather than recomputing it.
- `claude-skills hook install` — Wire hooks into Claude Code's `settings.json`
- `claude-skills doctor` — Report MCP registration health (including `alwaysLoad` state)
- `claude-skills repair` — Re-pin the MCP server entry in `~/.claude.json`

**Additional CLI surfaces** not yet wired to `claude-skills` subcommands:
- `claude --agents '<json>'` — Pass inline subagent definitions (JSON) for the current session only; supports the same frontmatter fields as file-based subagents including `prompt`, `tools`, `model`, `maxTurns`, `mcpServers`, `hooks`, `skills`, `memory`, `effort`, `background`, `isolation`, and `color`
- `claude --agent <name>` — Run the entire session as a named subagent; the subagent's system prompt replaces the default, `CLAUDE.md` still loads normally
- `claude --plugin-dir <path>` or `claude --plugin-url <url>` — Load a plugin for the current session without installing it
- `@skills-dir` plugins: any `~/.claude/skills/<name>/` directory containing `.claude-plugin/plugin.json` loads as `<name>@skills-dir` with no install step; also scaffoldable via `claude plugin init <name>` or `claude plugin new`. Project-scope `@skills-dir` plugins load only from the `.claude/skills/` of the directory where Claude Code was launched (not parent directories); launch from the repo root or run `/reload-plugins` after changing directories

### Managed Profile Schema

Managed profiles (`<name>/agents/claude.yaml`) wire the `claude-skills` runtime to specific reasoning effort and tool policy. Supported fields:

| Field | Purpose |
|---|---|
| `agent` | Default subagent to spawn for this profile (e.g. `Explore`, `Plan`, `general-purpose`) |
| `maxTurns` | Maximum agentic turns per session before auto-terminating |
| `effort` | Default effort level: `low`, `medium`, `high`, `xhigh`, `max` (ultracode = `xhigh`) |
| `permissionMode` | Tool permission mode for the managed subagent: `default`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`, `plan` |

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
`allowed-tools` (warn), trigger language in the description (warn — a passive
"X is a Y specialist" description activates less reliably than one that says when
to use it), and no dangling `references/*.md` links. It is the
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

## Code Graph

`claude-skills code-graph build|impact` is a deterministic codebase-understanding
graph — the structural layer the flat `SYSTEM_MAP` and the manual
`preserve-existing-flow` owner trace lacked. `build` scans the workspace and writes
a committable JSON artifact (default `.understand/code-graph.json`) of nodes
(source files with their top-level symbol definitions and import specifiers) and
edges (cross-file `imports` dependencies). Extraction is line-based and
dependency-free (no tree-sitter grammar, no LLM): nodes sort by path, edges by
(from,to,kind), so two runs over the same tree produce byte-identical JSON. Edges
are only emitted when an import resolves to a real in-repo file — relative JS/TS
imports (with `index` resolution), relative Python imports (with `__init__`), and
Rust `mod` declarations; bare/package imports stay as node `imports` strings and
are never invented as edges, so the graph stays honest. `impact --changed a,b,c`
rebuilds the graph and reports the transitive reverse-dependency closure (every
in-repo file that imports the changed files, directly or indirectly), excluding the
changed files themselves — the cheap "what could this edit break" query for review
scoping. Supported languages: Rust, JavaScript/TypeScript, Python, Go (Go records
import specifiers but does not resolve package paths to files, by design).

## User Stories

`claude-skills user-story lint --file <stories.md>` (or `--stdin`) is the
deterministic strict-format validator behind the `writing-user-stories` skill —
the requirements-capture front of the workflow. It parses a markdown story set and
**fails** (exit 1) when a story is missing a Connextra clause (role/goal/benefit,
"As a `<role>`, I want `<goal>`, so that `<benefit>`") or has no Gherkin acceptance
scenario (Given/When/Then), and **warns** on INVEST risks (filler benefit clause,
an "and"-chained goal that should be split). A document with no parseable story
also fails, so an empty or garbage artifact never passes silently. Parsing is
line-based and case-insensitive on the keyword anchors, so two runs over the same
input produce identical findings. It is the structural gate for user stories the
way `skill-lint` is for SKILL.md and `config-audit` is for the config surface:
catch a malformed artifact before it is trusted, without invoking the live model.
The `writing-user-stories` skill runs on every requirement-bearing prompt — it
converts the prompt completely into stories, validates them with this command,
confirms them with the user via `AskUserQuestion`, and captures them in the working
brief as the anti-drift spec that `reviewer` Stage 1 reconciles the diff against.

## Sprint Loop

`claude-skills sprint plan|status|advance|review|list` is the Scrum-style sprint
ledger behind the `running-a-sprint` skill. The confirmed user stories become a
sprint backlog (`plan --story "..."`, each item starts `todo`); `advance --id <id>
--state <todo|in-progress|blocked|done>` moves a story across the board; `status`
shows the board (daily-scrum view); and `review` is the **fail-closed loop gate** —
it exits 0 (COMPLETE) only when there is at least one story and every story is
`done`, and exits non-zero (NOT COMPLETE) while any story is `todo`,
`in-progress`, or `blocked`, naming the open ones. A `blocked` story is explicitly
not done, so it keeps the sprint open rather than being silently counted complete;
an empty sprint is "not complete", not "done". State is durable per workspace
(`<claude_home>/sprint/<workspace-slug>/`, one record per story) so the loop —
"which stories are still open" — survives compaction and a fresh session. The
`running-a-sprint` skill orchestrates the loop: plan the backlog from confirmed
stories, drive each story implement → verify-against-its-Gherkin-criteria →
`reviewer` to Definition of Done, then `sprint review` and loop back on any open
story until COMPLETE, then demo the increment and capture a retrospective to
memory.

