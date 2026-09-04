# CLAUDE.md — keel Project Guide

## Project Overview

This is the keel project — native delivery rails for the harness. It provides:
- 1 bootstrap **skill** (`using-keel/SKILL.md`) whose compact operating contract (the research-first iron law, MCP tools, and a skill-catalog pointer) is injected at every `SessionStart`; the full skill is too large for the `additionalContext` cap (the harness truncates above ~10KB), so the complete catalog and routing rules ship to disk and load on demand via `Skill("using-keel")`
- **Specialist skills** for software delivery (`<name>/SKILL.md`) — each paired with a matching subagent (`.claude/agents/<name>.md`) and a managed profile (`<name>/agents/claude.yaml`) for token-efficient delegation.
- **Technique/process skills** (`brainstorming`, `running-anvil`, `test-driven-development`, `behavior-driven-development`, `systematic-debugging`, `writing-plans`, `executing-plans`, `subagent-driven-development`, `dispatching-parallel-agents`, `using-git-worktrees`, `finishing-a-development-branch`, `receiving-code-review`, `writing-skills`, `designing-agent-teams`, `compounding-knowledge`, `adversarial-security-review`, `compression-discipline`, `output-economy`, `critic`, `deliberation`, `research-enforcement`, `memory-consolidation`, `component-driven-development`) — main-thread skills with **no** subagent or managed profile. The distinguishing rule: a specialist ships three artifacts (SKILL.md + subagent + profile); a technique skill ships only SKILL.md.
- The bootstrap (`using-keel`) is the only first-party skill **not** in `.claude-plugin/plugin.json` (it loads at SessionStart, not via the matcher). Every other first-party skill directory is listed there. `requesting-code-review` is a real directory holding a thin alias skill that routes to `reviewer` (not a separate behavior). Historical note: an optional vendored `karpathy-skills-cmp/` tree may appear in some checkouts as a non-skill benchmark artifact; it is not a keel skill, is not in the plugin manifest, and is not required on disk.
- **Live counts, not hardcoded ones**: the docs deliberately do not state skill/subagent/profile counts. Run `keel skill-lint` (CLI) or the `skill_list` MCP tool for the verified skill roster, and see `tests/doc_parity_test.rs` — it asserts the *structural invariant* (the bootstrap is the only on-disk skill not in the manifest; disk = manifest + 1) so adding or removing a skill never requires editing a number in docs or tests.
- Matching harness **subagents** for token-efficient delegation (`.claude/agents/<name>.md`) — one per specialist.
- Internal **managed profiles** consumed by the CLI (`<name>/agents/claude.yaml`) — one per specialist.
- Workflow routing and escalation rules
- Review gates (pre-commit, pre-PR): a comment-style gate (over-length/chatty/em-dash code comments, plus **restating comments**: a comment whose informative words all already appear as identifiers/operations on the very next line adds zero information and is flagged delete-or-rewrite, the `i++ // increment i` archetype), a **prose-style gate** (AI-slop vocabulary, em-dash, hype, first-person, chatty wording in markdown/doc body text), a code-slop detector (dead defensive code, over-commenting, phantom flags), and a **blocking `flow_check` gate** that fails review when the diff modifies established source without a complete flow-check artifact. The flow gate reads the same artifact `keel flow check` validates (`keel-flow::validate_check`), so the two never drift, and it runs on all three surfaces (`pre-commit`, `pre-pr`, `gates check`). The artifact must also **trace one of the modified files**: because it is workspace-global, a single filled artifact would otherwise satisfy the gate forever regardless of what changed next. Renames (`R`) count as brownfield edits and are matched on the destination path. Exempt: added files (greenfield has no prior owner), non-source extensions, and generated/vendored trees (`target/`, `node_modules/`, `vendor/`, `dist/`, `build/`, `generated/`, `__pycache__/`). A diff touching no existing source passes untouched. If the diff range cannot be resolved (unknown `--base-ref`, no git), the gate reports a non-blocking **warn** stating that evidence was not checked — it never reports a silent pass.
- Professional text templates
- Hooks wired into the harness's `settings.json` for transparent command rewriting and auto-routing
- A Rust CLI (`keel`) for workflow, memory, command compaction, and hook installation

### Terminology

These three terms are **not** interchangeable:

| Term | What it is | Where it lives | Schema |
|---|---|---|---|
| **Skill** | the harness knowledge unit loaded into the main conversation when it matches a request | Source: `<name>/SKILL.md`. Installed: `~/.claude/skills/<name>/SKILL.md` | YAML frontmatter with `name`, `description`, `when_to_use`, `allowed-tools` |
| **Subagent** | the harness delegation target that runs in an isolated context window | `.claude/agents/<name>.md` (project) or `~/.claude/agents/<name>.md` (user) | YAML frontmatter with `name`, `description`, `tools`, `model` |
| **Managed profile** | Internal CLI configuration that wires reasoning effort, default prompts, and policy for the `keel` runtime — **not** seen by the harness | `<name>/agents/claude.yaml` | Custom YAML consumed by the Rust CLI |

A "skill" runs in the main thread (instructions inline, costs ongoing tokens). A "subagent" runs in its own context window (saves main-thread tokens but adds delegation overhead). The "managed profile" is invisible to the harness itself — it only configures how `keel` orchestrates work.

## Key Files

- `00-skill-routing-and-escalation.md` — Read this first. Defines skill routing and escalation.
- `AGENTS.md` — Agent operating doctrine (uses "agent" in the broad sense — covers skills, subagents, and managed profiles).
- `WORKFLOW.md` — Branch and completion rules.
- `templates/` — Professional text templates (commit, PR, final response, review).
- `.claude/hooks.json` — the harness hook wiring rendered by `keel hook install`. (Review policy is configured in `.claude-plugin/plugin.json` under `userConfig.review_strictness`, not a separate `review.json`.)
- `.claude-plugin/plugin.json` — Plugin manifest for the harness's plugin system. Carries `skills`, `agents`, `commands` (`["./commands/"]`), `hooks` (`./.claude/hooks.json`), `outputStyles` (`./output-styles/`), `lspServers` (`./.lsp.json`), `mcpServers.keel` (`alwaysLoad: true`), `userConfig` (review strictness, system-map refresh interval, memory retention days), and `experimental.monitors` (a build-watcher). `.claude/settings.json` is **not committed** — it is a runtime install target written by `keel install`/`keel hook install` into `~/.claude/settings.json`.
- `commands/` — Custom slash commands (`/keel:<name>`) wrapping the implemented CLI surfaces: `anvil`, `review`, `recall`, `gain`. Registered via the `commands` key in the plugin manifest. The native `keel install` also syncs them to `~/.claude/commands/` via `sync_commands`, so they work whether installed through the plugin path or the native installer.

## Specialist Layout

Each specialist contains three artifacts, plus an optional reference library:
- `<name>/SKILL.md` — Skill definition (loaded by the harness when relevant)
- `.claude/agents/<name>.md` — Subagent definition (delegation target with isolated context)
- `<name>/agents/claude.yaml` — Managed profile (CLI runtime configuration)
- `<name>/references/` — Deep knowledge files referenced by SKILL.md (most specialists; the narrow specialists `api-contract-design`, `postgres-migration-safety`, `react-performance-audit`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `authentication-and-identity`, `cloud-cost-and-finops`, and `internationalization-and-localization` ship a self-contained SKILL.md with no reference library)

Specialists (each with a paired subagent + managed profile; roster maintained by `tests/doc_parity_test.rs` and surfaced by `keel skill-lint` or the `skill_list` MCP tool): `software-development-life-cycle`, `web-development-life-cycle`, `mobile-development-life-cycle`, `backend-and-data-architecture`, `domain-driven-design`, `cloud-and-devops-expert`, `qa-and-automation-engineer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `reviewer`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `memory-status-reporter`, `api-contract-design`, `react-performance-audit`, `postgres-migration-safety`, `stripe-integration`, `websocket-realtime-design`, `observability-and-incident-response`, `dependency-and-supply-chain`, `data-and-ml-engineering`, `authentication-and-identity`, `cloud-cost-and-finops`, `internationalization-and-localization`, `dart-and-flutter-expert`.

## Schema Compliance Notes

**SKILL.md frontmatter** follows the official harness skill spec. Per the docs, all SKILL.md frontmatter fields are technically optional, but `name` and `description` are **strongly recommended** because the skill matcher uses them to decide when to load the skill. The combined `description` + `when_to_use` text is capped at 1,536 characters. The fields currently used by keel's own skills are: `name`, `description`, `when_to_use`, `allowed-tools`, `effort`, and `paths`. All other official fields are supported and documented here for completeness:

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
| `description` | Yes | When the harness should delegate to this subagent |
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

**Hook events** (`.claude/hooks.json`) are wired through `keel hook <event>` for every harness lifecycle event the manager observes. The `HOOK_EVENTS` table in `rust/crates/keel/src/hooks/claude.rs` is the single source of truth — it defines every event, its slug/matcher, and whether it auto-installs into `settings.json`. Most rows install automatically. Twelve rows carry installs_in_settings: false (pinned by only_known_events_opt_out_of_install): reserved no-op events (TaskCreated, TaskCompleted, TeammateIdle, WorktreeCreate, WorktreeRemove, Setup, InstructionsLoaded, ConfigChange, Elicitation, ElicitationResult) plus structural opt-outs (FileChanged — matcher is the watch list so empty matcher ships dead config; MessageDisplay — would silently rewrite on-screen text). All 31 official events remain dispatchable via keel hook <slug>. Both still dispatch for ad-hoc invocations (`keel hook file-changed`, `keel hook message-display`). Actively implemented handlers: `SessionStart` (MCP self-heal + bootstrap context + watchPaths), `UserPromptSubmit` (skill routing + compression nudge), `PreToolUse` (command rewrite + auto-allow rules), `PostToolUse` (timings + observations + comment-style lint on Edit/Write + system map refresh, runs async; the comment lint is advisory, env-gated by `CLAUDE_SKILLS_COMMENT_LINT_GATE` — `off` disables — and catches over-length/chatty/first-person/em-dash comments at write time so they don't wait for review), `PostToolUseFailure` (timings + failure observation, runs async), `PostToolBatch` (review/brief/closeout gates), `Stop` (silenced — `additionalContext` on Stop means "keep going" → infinite loop, so the handler exits 0 with no context; the closeout reminder rides PostToolBatch instead), `SubagentStart` (iron law context injection), `SubagentStop` (lifecycle), `PermissionRequest` (auto-approve keel commands), `PermissionDenied` (retry signaling), `CwdChanged` (system map refresh), `SessionEnd` (auto-capture + learning + prune), `PreCompact`/`PostCompact` (learning + system map), `Notification` (terminal bell). Remaining events dispatch through the lifecycle wildcard for forward-compatibility. Hook types supported: `command` (shell scripts), `http` (POST event JSON to a URL), `mcp_tool` (call an MCP server tool), `prompt` (evaluate a prompt with an LLM using `$ARGUMENTS` for context), and `agent` (run an agentic verifier with tools for complex verification). Elicitation events (`Elicitation`, `ElicitationResult`) handle MCP server user-input requests — the hook can accept/decline/cancel via `hookSpecificOutput.action`. When Anthropic adds or renames events, update both `HOOK_EVENTS` and the generated `.claude/hooks.json`. Reference: https://code.claude.com/docs/en/hooks.

**Output styles**: the harness ships four built-in output styles — `Default`, `Proactive`, `Explanatory`, and `Learning`. The active style for this project is set in `.claude/settings.local.json`. Reference: https://code.claude.com/docs/en/output-styles.

**Plugin manifest** (`.claude-plugin/plugin.json`) follows the official plugin schema. Only `name` is required; `displayName`, `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`, `skills`, `agents`, `hooks`, `commands`, `mcpServers`, `outputStyles`, `lspServers`, `experimental.themes`, `experimental.monitors`, `userConfig`, `channels`, and `dependencies` are optional. Notable fields: `displayName` (human-readable name with spaces, shown in `/plugin` picker; `name` is used for namespacing only), `defaultEnabled` (ship plugin in disabled state; users opt in via `/plugin` or `claude plugin enable`), `shell` (for hook scripts; `powershell` on Windows requires `CLAUDE_CODE_USE_POWERSHELL_TOOL=1`). This project uses `skills`, `agents`, `commands` (set to `["./commands/"]`), `hooks`, `mcpServers`, `userConfig` (configurable review strictness, system map refresh interval, and memory retention days), and `experimental.monitors` (a build-watcher that reports Rust compilation errors on review invocation). Per the official reference, listing `commands` **replaces** the default `commands/` scan, so the explicit `["./commands/"]` keeps the default directory. Command `.md` files live at the plugin root `commands/` (not inside `.claude-plugin/`). For `hooks` and `mcpServers`, multiple source paths are merged rather than replaced. Reference: https://code.claude.com/docs/en/plugins-reference.

**Token-saving proxy**: command-output compaction lives in `rust/crates/keel/src/proxy/`. The native `keel run -- <command>`, `keel rewrite`, and `keel gain` surfaces own this work. When the harness introduces native compaction primitives, prefer them and keep this layer thin.

**Native Auto memory**: recent the harness ships *Auto memory* (notes the model writes itself to `~/.claude/projects/<project>/memory/MEMORY.md` from your corrections, loaded automatically each session; docs: https://code.claude.com/docs/en/memory). This is complementary to keel's memory surfaces, not redundant: native Auto memory is passive and machine-local for incidental learnings; keel's `SYSTEM_MAP`, working briefs, completion gate, FTS5 recall, and unified `memory` family records are explicit, structured, reconcilable artifacts. Prefer native Auto memory for incidental notes and the structured commands when an artifact must survive compaction or be reconciled against the request; do not duplicate the same fact into both.

**Memory ownership boundary (no double-write)**: keel and native Auto memory write to *disjoint* paths, so there is no collision to guard against. Native Auto memory owns `~/.claude/projects/<project>/memory/MEMORY.md`. Keel's autonomous learning and explicit memory surfaces own `~/.keel/memory/`, `~/.keel/memories/`, and `~/.keel/working-briefs/`; generated skills and agents remain under the host-read `~/.claude/skills/` and `~/.claude/agents/` directories. Keel never writes the native `MEMORY.md`. The two layers are read together at SessionStart (native loads its file, Keel injects its digest) and never overwrite each other.

**Recall vs code-search boundary**: `recall` (FTS5 over `memory`, `memories`, `working-briefs`) is the *memory* index for durable notes, briefs, and learnings. It deliberately does not index project source, so a code question is not answered with stale memory. `code-index` owns the persistent workspace index; `code-search search` queries ranked files, symbols, chunks, paths, and verified relationships from that index. `code-search siblings` uses the same index for completeness scans. Neither surface performs a hidden live-scan fallback.

**MCP server**: `keel mcp serve` runs a JSON-RPC 2.0 stdio server registered through `.claude-plugin/plugin.json` `mcpServers.keel`, pinned with `alwaysLoad: true` so the harness keeps the server's tools in context instead of deferring them (`alwaysLoad` is per-server, so all tools are pinned together). The harness auto-discovers it and gets the MCP tool set plus two resources (`keel://system-map`, `keel://recall/status`). The MCP tool count is derived from `mcp/tools.rs` and asserted by `tests/doc_parity_test.rs` (the test counts `\"inputSchema\":` definitions), so the docs do not hardcode a number — add or remove a tool and the test stays correct. The tools fall into these groups: **awareness** (`context_brief` — one call returning the iron law, the full skill catalog, memory health, and the newest working brief, so the agent knows what exists even when no skill auto-loaded), **search/compaction** (`recall`, `run_command`, `command_output`, `command_kill`, `recall_status`), **skills** (`skill_route`, `skill_get`, `skill_list`), **memory** (`memory`, `memory_status`, `brief_list`, `brief_get`, `brief_create`), **workspace map** (`system_map`, `system_map_refresh`), **delivery** (`anvil` — compile→cast→sieve→stamp→loop, the only delivery loop), **delivery/quality** (`review`, `git_workflow`, `flow`, `config_audit`, `skill_lint`, `skill_eval`), **analysis/ops** (`code_search`, `code_graph`, `gain`, `session`, `raw`, `rewrite`, `telemetry`, `observe`, `learn`, `doctor`, `design_intelligence`), and a **generic passthrough** (`cli`) that runs any remaining keel subcommand in-process. The full name list lives in README's MCP row and is test-enforced against `mcp/tools.rs` by `every_mcp_tool_is_listed_in_readme`. `cli` gates destructive/management subcommands (`install`, `update`, `repair`, `uninstall`, `remove`, `validate`, `all`, `__self-replace`, `hook install`/`hook uninstall`) behind an explicit `confirm: true`, and refuses `mcp` outright. The `run_command` and `cli` tools run through the same proxy capture+compaction pipeline as `keel run --`, so command-output compaction applies on the MCP surface too. The skill/memory/brief tools mirror capabilities the lifecycle hooks otherwise deliver (e.g. `skill_route` is the on-demand equivalent of the per-prompt skill-brief injection) so they stay reachable on platforms where hooks are unreliable; each is a thin wrapper over the same function that backs the corresponding CLI surface, so MCP and CLI never drift. The `initialize` handshake echoes the client's requested `protocolVersion` (falling back to the server default) so the server stays compatible as the MCP spec revises. If the server's tools ever go missing or stop loading, `keel doctor` reports the registration and its `alwaysLoad` state and `keel repair` re-pins the `~/.claude.json` entry. Tool dispatch lives in `mcp/tools.rs`; `mcp/mod.rs` keeps JSON-RPC framing, the serve loop, and the resource surface.

**OpenCode host (bridge)**: the harness's lifecycle hooks do not fire in OpenCode, so the automatic behaviors (skill routing, session-start context push, observation capture, session-end learning, post-compact re-push) are delivered through an OpenCode plugin instead. `keel install` copies `opencode/keel.ts` into `~/.config/opencode/plugins/` (the plural directory OpenCode loads) and merges a `keel` MCP entry into `~/.config/opencode/opencode.json` (merge, never clobber; BOM-tolerant). The plugin maps OpenCode events to a host-neutral CLI surface, `keel bridge <event>`: `session-start` and `post-compact` print the bootstrap/digest/post-compact context; `pre-compact` runs the learning checkpoint before the window is rewritten; `user-prompt` prints compact routing and enforcement pointers; `observe` records a tool observation from stdin JSON; `session-end` runs the learning cycle + session summary capture; `gate-status` prints fired/cleared gates; `pre-tool-use` emits the Iron Law edit-gate decision (`KEEL_GATE_ALLOW` / `KEEL_GATE_DENY <reason>`) as text; `rewrite` reroutes shell commands through compaction. OpenCode, Codex, Pi, and Cursor translate the bridge gate and rewrite responses into their host-native controls. Claude Desktop (cowork) is **MCP-only** because Desktop exposes no hook API. Pi and OMP use the same `pi/keel-pi.ts` extension: `session_start` caches the bootstrap, `before_agent_start` appends that one-time context plus the current prompt pointer to the turn's system prompt, `session_before_compact` checkpoints learning, and `session_compact` caches recovery context for the next agent start. The bridge reuses the same Rust handlers as the hook path, so lifecycle logic has one source of truth.


0. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed — the owning module, the framework, the real requirement. Do not guess, do not assume, do not build against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode here: it passes review and still gets thrown away. Researching first is what saves you from building the wrong thing. If the request is ambiguous in a way that changes what you build, ask before building, not after. This is rule zero because it gates every rule below — there is no point routing a skill or refreshing memory for the wrong task.

1. Routing is driven by the harness's native skill matcher against the installed `~/.claude/skills/<name>/SKILL.md` files — each skill's frontmatter (`description`, `when_to_use`) is what triggers selection. The bootstrap skill `using-keel/SKILL.md` is injected verbatim into `SessionStart` `hookSpecificOutput.additionalContext` per the official harness hooks schema, so the iron law (understand before building, research first, invoke skills before responding, find the root cause) and the full skill catalog land in model context once at session start. `UserPromptSubmit` then restates the iron law in compact form on every turn.
2. Run `preserve-existing-flow` before editing any existing source file.
3. Run `reviewer` before closing **non-trivial** work. Trivial exemptions: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work the user asked for. Everything else (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield rewrites) goes through `reviewer` before close.
4. Delegate to the matching `.claude/agents/<name>.md` subagent for heavy work that benefits from an isolated context window (saves main-thread tokens). Subagents cannot spawn other subagents; use `Skill` tool or chain from the main conversation instead.
5. Use `templates/` for commit bodies, PR bodies, final responses, and review summaries.
6. Read `WORKFLOW.md` for branch naming, commit format, and completion rules.
7. **Agent teams** (agent teams are different from subagents): experimental — enable with `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Teammates communicate via `SendMessage` tool with the agent's ID as the `to` field. Resumed subagents retain full conversation history and auto-resume in the background when they receive a `SendMessage`. The `SubagentStop` event fires when a subagent finishes; `TeammateIdle` fires when a teammate is about to go idle — both support matchers to target specific agent types. Background subagents run concurrently and surface their permission prompts in the main session; foreground subagents block until complete. Set `CLAUDE_CODE_FORK_SUBAGENT=1` to make every subagent spawn a fork that inherits the full conversation history (fork mode also forces background and nullifies the `background` frontmatter field). Reference: https://code.claude.com/docs/en/agent-teams.
8. **Writing Discipline** applies to all written output — docs, code comments, commit/PR text, review notes, chat replies: write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.

### Enforcement Gates (PostToolBatch)

The enforcement gates are the model-independent backstop for the Iron Law — hooks cannot force a tool/Skill call, but they can inject a reminder when a required artifact is missing. All gates fire on PostToolBatch (mid-turn checkpoint); each is controlled by its own `CLAUDE_SKILLS_*_GATE` env var. The `Stop`/`SubagentStop` handlers are **silenced** (exit 0, no context): per the harness schema `additionalContext` on Stop means "keep going", which would loop, and a non-zero exit re-runs the turn — so Stop cannot safely inject context. Closeout reminders ride PostToolBatch only. All gate implementations live in `rust/crates/keel/src/runner/hook_lifecycle/mod.rs` (`run_hook_post_tool_batch`, ~`:3227`):

- **Working-brief gate** (`CLAUDE_SKILLS_BRIEF_GATE`, front of the law) — fires once when a session edits code but no working brief was written this session. Clear it by writing one: `keel memory working-brief write --request "..." --acceptance-criteria "..."`, or the `brief_create` MCP tool.
- **Review gate** (`CLAUDE_SKILLS_REVIEW_GATE`, back of the law) — fires once when a session edits code but records no reviewer pass since the last edit. Clear it by running `keel review pre-pr` (or `pre-commit` / `gates check`) **and it must pass** — a failed review no longer writes the marker. Invoking the `reviewer` skill alone does not clear it (only the CLI writes the marker).
- **Memory gate** (`CLAUDE_SKILLS_MEMORY_GATE`) — fires when code is edited without a recent memory capture, nudging toward `recall`/`memory-consolidation`.
- **Learned-skill gate** (`CLAUDE_SKILLS_LEARNED_SKILL_GATE`): fires when the learning loop has promoted a skill that has not yet been loaded this session. The promotion writes a deterministic template stub (no LLM, at SessionEnd); the next SessionStart surfaces a synthesis nudge so the session agent refines the prose, protected by a content-hash no-clobber guard. Set `CLAUDE_SKILLS_LEARNED_SKILL_ENRICH=off` to disable the synthesis nudge without disabling learning.
- **Research gate** (`CLAUDE_SKILLS_RESEARCH_GATE`) — fires when a session changed code without web-search or `recall` evidence of fresh research (pairs with the `research-enforcement` skill). There is no dependency detection: any code edit without research evidence trips it. Cleared by a WebSearch/WebFetch, the context7 MCP, or the keel `recall` tool — not by a `keel run -- recall` Bash command (the gate matches tool names, not shell commands).
- **Completeness gate** (`CLAUDE_SKILLS_COMPLETENESS_GATE`) — fires when a session changed code without a sibling scan. Clear it with `keel code-search siblings` (or MCP `code_search` action=siblings). Defaults to **Block**. A one-site fix is unfinished.

Why the gates ride PostToolBatch (not Stop): PostToolBatch fires after every tool batch and can nudge mid-turn without forcing an extra turn. Stop cannot inject context safely *unconditionally* (see above; the schema does allow it there as non-error feedback, but an ungated per-turn injection loops). `SessionEnd` carries no `hookSpecificOutput` at all. PostToolBatch is the *primary* checkpoint because it fires most often; the honest-closeout gate specifically is the end-of-turn backstop.

Each gate env var maps to a `GateMode` with **four** values: **`nudge` → non-blocking advisory** (reminder injected via `hookSpecificOutput.additionalContext`); **`block` → escalated feed-forward** (an *imperative* reminder — "Do NOT present this work as done; state each open item as an honest gap and loop back" — still via `additionalContext`, never `decision: "block"`); **`off` (or `…_MAX_BLOCKS=0`) → disabled**; **unset / any unrecognized value → `Escalate`** (the default — nudge on the first fire, block thereafter; the warn-once-then-block behavior). Three gates default *stricter*: the **review**, **working-brief**, and **completeness** gates default to **`Block`** when unset, not `Escalate` (the review gate also honors the plugin `review_strictness` userConfig when its env var is unset). No gate ever halts or stops a turn on any host: a missed gate feeds the corrective instruction forward into the agent's context so it self-corrects, rather than blocking. Whichever mode is set, each gate fires at most `…_MAX_BLOCKS` time(s) per session (default 2 for an escalating gate, 3 for a block-mode gate) via a monotonic counter, then falls through to the generic advisory — so it can neither loop nor spam — and fails open to the advisory on any telemetry error. Pure-research and question turns (no code edits) never fire a gate (edit-count 0 short-circuits). In OpenCode the agent can self-audit fired gates with `keel bridge gate-status`.

## Commands

- `keel anvil compile|cast|sieve|stamp|loop|run` — Only delivery loop
- `keel review pre-commit` — Pre-commit review
- `keel review pre-pr` — Pre-PR review
- `keel run -- <command>` — Run with output compaction
- `keel memory scope resolve --create-missing --refresh-system-map` — Refresh memory
- `keel eval` — Run the real compaction eval: drives the genuine adapter pipeline over embedded fixtures and reports EXACT o200k_base token deltas, with measured floors asserted in CI. This is the real measurement; the legacy `bench` is only a runtime-provenance/feature-parity marker (see below).
- `keel observe` — Read-only aggregator over recall health + working-brief count; defers the token-savings axis to `gain`/`session` rather than recomputing it.
- `keel skill-eval` — Behavioral gate over the skill matcher: replays routing fixtures (prompt -> expected skill, including "must stay silent" cases) and reports pass/fail. `skill-lint` checks a skill's structure; this checks that it actually triggers.
- `keel design-intelligence recommend` — Produce a stack/component-library-aware UI design recommendation (density and variance knobs, optional `--persist` to a project/page record).
- `keel telemetry summary` — Read-only tool-timing report over `<claude_home>/state/tool-timings/<date>.jsonl` (`--days`, `--session`, `--top`, `--json`).
- `keel session` — Per-session compaction-savings table (commands, tokens before/after, savings percentage, compacted count).
- `keel learn status|dry-run|run|synthesize` — Drive the autonomous learning cycle: inspect observation signal and recorded instincts, preview what a cycle would generate, run it (writes instincts + generated skills), or emit refinement briefs for template-state generated skills. The binary never calls an LLM.
- `keel hook install` — Wire hooks into the harness's `settings.json`
- `keel doctor` — Report MCP registration health (including `alwaysLoad` state)
- `keel repair` — Re-pin the MCP server entry in `~/.claude.json`

**Additional CLI surfaces** not yet wired to `keel` subcommands:
- `claude --agents '<json>'` — Pass inline subagent definitions (JSON) for the current session only; supports the same frontmatter fields as file-based subagents including `prompt`, `tools`, `model`, `maxTurns`, `mcpServers`, `hooks`, `skills`, `memory`, `effort`, `background`, `isolation`, and `color`
- `claude --agent <name>` — Run the entire session as a named subagent; the subagent's system prompt replaces the default, `CLAUDE.md` still loads normally
- `claude --plugin-dir <path>` or `claude --plugin-url <url>` — Load a plugin for the current session without installing it
- `@skills-dir` plugins: any `~/.claude/skills/<name>/` directory containing `.claude-plugin/plugin.json` loads as `<name>@skills-dir` with no install step; also scaffoldable via `claude plugin init <name>`. Project skills load from `.claude/skills/` in the launch directory and every parent directory up to the repository root; launch from the repo root or run `/reload-plugins` after changing directories

### Managed Profile Schema

Managed profiles (`<name>/agents/claude.yaml`) wire the `keel` runtime to a specialist's reasoning effort, interface text, and invocation policy. The CLI parser (`manager/agent_config.rs::parse_agent_config`) reads these fields:

| Field | Purpose |
|---|---|
| `reasoning_effort` | Reasoning baseline rendered as `model_reasoning_effort`: `low`, `medium`, `high`, `xhigh`, `max`. Defaults to `high` when absent. |
| `interface.display_name` | Human-readable specialist name. |
| `interface.short_description` | One-line description rendered as the agent `description`. |
| `interface.default_prompt` | Required. The specialist operating prompt rendered as `developer_instructions`; missing -> parse error. |
| `policy.allow_implicit_invocation` | Whether the profile may be invoked implicitly (`false` = explicit only). |

Note: the subagent frontmatter table above (`model`, `permissionMode`, `maxTurns`, etc.) describes `.claude/agents/<name>.md`, a different artifact. Do not conflate the two schemas.

### Declarative Filter Registry

`keel run` supports project-specific TOML filter files that compact command output without writing Rust code.

Place a filter file at either:
- `.keel/filters.toml`
- `keel.filters.toml` (project root)

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
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Skill Lint

`keel skill-lint --repo-root .` validates that every `<name>/SKILL.md`
has the structural properties the harness matcher needs to *trigger* the
skill — non-empty `description`, `name` matching the directory, combined
`description` + `when_to_use` within the 1536-char matcher budget, scoped
`allowed-tools` (warn), trigger language in the description (warn — a passive
"X is a Y specialist" description activates less reliably than one that says when
to use it), and no dangling `references/*.md` links. It is the
superpowers-style "test that skills trigger" gate, expressed as deterministic
checks. Add `--json` for machine output; exits non-zero when any skill fails.

## Config Audit

`keel config-audit --repo-root .` is an AgentShield-style security audit
of keel's *own* config surface (not user code): `.claude/hooks.json`,
`.claude/settings.json` permissions, and `.claude-plugin/plugin.json`. It flags
shell-metacharacter injection and network fetches in hook commands,
`bypassPermissions` mode, unscoped `Bash` allow rules, and committed secret
literals in MCP env. Exits 2 on any high-severity finding. Add `--json` for
machine output. Distinct from the `security-and-compliance-auditor` skill, which
audits the user's application code.


## Code Graph

`keel code-index refresh|status|map` is the primary deterministic workspace
index. It persists files, symbols, chunks, exact ranges, paths, and verified
relationships in the global per-workspace lane. `keel code-search` queries that
index with exact-symbol, FTS5, path, and graph rank fusion; `SYSTEM_MAP.md` is
generated from the same generation and commit evidence.

`keel code-graph build|impact` remains the explicit review artifact for
transitive reverse-dependency closure. Its import edges are emitted only when
the target resolves to a real in-repo file, so unresolved package imports are
never invented. Supported languages remain Rust, JavaScript/TypeScript, Python,
and Go.

## Anvil

`keel anvil` is the only delivery loop. It replaces sprint, user-story, work,
dispatch, workflow, orchestration, and team command surfaces.

```text
compile  goal + named bar -> anvil.lock.json + prefix.md + gates
cast     N isolated workspaces from the frozen prefix
sieve    0-LLM deterministic gates
stamp    local PPT (Bradley-Terry ring on gate_ok + compactness)
loop     bounded refine only if gates still fail
run      compile -> cast -> sieve -> stamp -> loop
```

Live `cast`/`stamp`/`run` inherit the current host CLI for LLM work (no external
model client). The job bank is `<keel-home>/memories/workspaces/<slug>/anvil/`,
never `{cwd}/anvil/`. Isolated casts use temp dirs and are deleted after the
result is copied into the bank. Use `--dry-run` for the offline path. Skill:
`running-anvil`.
