# CLAUDE.md — claude-core Project Guide

## Project Overview

This is the claude-core project — native delivery rails for Claude Code. It provides:
- 1 bootstrap **skill** (`using-claude-core/SKILL.md`) injected verbatim at every `SessionStart` to establish the research-first iron law and list every other skill
- 18 specialist Claude Code **skills** for software delivery (`<name>/SKILL.md`)
- 18 matching Claude Code **subagents** for token-efficient delegation (`.claude/agents/<name>.md`)
- 18 internal **managed profiles** consumed by the CLI (`<name>/agents/claude.yaml`)
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
- `commands/` — Custom slash commands (`/claude-core:<name>`) wrapping the implemented CLI surfaces: `workflow`, `review`, `recall`, `gain`. Registered via the `commands` key in the plugin manifest. Note: these install through the **plugin path**; the native `claude-skills install` does not yet sync them to `~/.claude/commands/` (planned follow-up — the installer has `sync_skills`/`sync_agents` but no `sync_commands` yet).

## Specialist Layout

Each specialist contains three artifacts:
- `<name>/SKILL.md` — Skill definition (loaded by Claude Code when relevant)
- `.claude/agents/<name>.md` — Subagent definition (delegation target with isolated context)
- `<name>/agents/claude.yaml` — Managed profile (CLI runtime configuration)
- `<name>/references/` — Deep knowledge files referenced by SKILL.md

18 specialists: `software-development-life-cycle`, `web-development-life-cycle`, `mobile-development-life-cycle`, `backend-and-data-architecture`, `cloud-and-devops-expert`, `qa-and-automation-engineer`, `security-and-compliance-auditor`, `git-expert`, `preserve-existing-flow`, `reviewer`, `ui-design-systems-and-responsive-interfaces`, `ux-research-and-experience-strategy`, `memory-status-reporter`, `api-contract-design`, `react-performance-audit`, `postgres-migration-safety`, `stripe-integration`, `websocket-realtime-design`.

## Schema Compliance Notes

**SKILL.md frontmatter** follows the official Claude Code skill spec. Per the docs, all SKILL.md frontmatter fields are technically optional, but `name` and `description` are **strongly recommended** because the skill matcher uses them to decide when to load the skill. The combined `description` + `when_to_use` text is capped at 1,536 characters. The fields used in this project are all documented Claude Code fields: `name`, `description`, `when_to_use`, `allowed-tools`, `effort`, and `paths`. Reference: https://code.claude.com/docs/en/skills.

Other official optional fields not currently used here include `disable-model-invocation`, `user-invocable`, `argument-hint`, `arguments`, `model`, `context`, `agent`, `hooks`, and `shell`. Add them deliberately when a skill needs that capability.

**Subagent frontmatter** (`.claude/agents/<name>.md`) follows the official spec: `name` and `description` are required; `tools` (comma-separated bare tool names), `model` (`opus`, `sonnet`, `haiku`, or `inherit`), `color`, and `skills` (a YAML list of bare skill names to preload at startup) are optional. Note: scoped tool patterns like `Bash(git diff:*)` work in SKILL.md `allowed-tools` but not in subagent `tools` — subagents use bare tool names. Each managed subagent preloads its same-named skill via `skills:` so the full skill content is in context from startup rather than loaded on demand; `skills` is supported for plugin subagents and a missing/disabled skill is skipped with a debug-log warning. Reference: https://code.claude.com/docs/en/sub-agents.

**Hook events** (`.claude/hooks.json`) are wired through `claude-skills hook <event>` for every Claude Code lifecycle event the manager observes. The `HOOK_EVENTS` table in `rust/crates/claude-skills/src/hooks/claude.rs` defines **30 events**, of which 28 install into `settings.json`. Two rows carry `installs_in_settings: false`: `FileChanged` (its matcher doubles as a per-repo watch list, so an empty matcher would ship dead config) and `MessageDisplay` (no matcher, fires on every assistant message, emits `hookSpecificOutput.displayContent` — auto-installing would either be a no-op or silently rewrite on-screen text, so it stays opt-in). Both still dispatch for ad-hoc invocations (`claude-skills hook file-changed`, `claude-skills hook message-display`). Events the runtime does not actively emit are stubbed for forward-compatibility — the dispatcher no-ops until behavior is needed. When Anthropic adds or renames events, update both `HOOK_EVENTS` and the generated `.claude/hooks.json`. Reference: https://code.claude.com/docs/en/hooks.

**Output styles**: Claude Code ships four built-in output styles — `Default`, `Proactive`, `Explanatory`, and `Learning`. The active style for this project is set in `.claude/settings.local.json`. Reference: https://code.claude.com/docs/en/output-styles.

**Plugin manifest** (`.claude-plugin/plugin.json`) follows the official plugin schema. Only `name` is required; `displayName`, `version`, `description`, `skills`, `agents`, `hooks`, `commands`, `mcpServers`, `outputStyles`, `lspServers`, `experimental.themes`, `experimental.monitors`, `userConfig`, `channels`, and `dependencies` are optional. This project uses `skills`, `agents`, `commands` (set to `["./commands/"]`), `hooks`, and `mcpServers`. Per the official reference, listing `commands` **replaces** the default `commands/` scan, so the explicit `["./commands/"]` keeps the default directory. Command `.md` files live at the plugin root `commands/` (not inside `.claude-plugin/`). Reference: https://code.claude.com/docs/en/plugins-reference.

**Token-saving proxy**: command-output compaction lives in `rust/crates/claude-skills/src/proxy/`. The native `claude-skills run -- <command>`, `claude-skills rewrite`, and `claude-skills gain` surfaces own this work. When Claude Code introduces native compaction primitives, prefer them and keep this layer thin.

**MCP server**: `claude-skills mcp serve` runs a JSON-RPC 2.0 stdio server registered through `.claude-plugin/plugin.json` `mcpServers.claude_core`. Claude Code auto-discovers it and gets four tools (`recall`, `system_map`, `run_command`, `recall_status`) plus two resources (`claude_core://system-map`, `claude_core://recall/status`). The `run_command` tool runs through the same proxy capture+compaction pipeline as `claude-skills run --`, so command-output compaction now also applies when Claude Code reaches for the MCP tool surface instead of the bash tool.

## Routing Rules

1. Routing is driven by Claude Code's native skill matcher against the installed `~/.claude/skills/<name>/SKILL.md` files — each skill's frontmatter (`description`, `when_to_use`) is what triggers selection. The bootstrap skill `using-claude-core/SKILL.md` is injected verbatim into `SessionStart` `hookSpecificOutput.additionalContext` per the official Claude Code hooks schema, so the iron law (research first, invoke skills before responding, find the root cause) and the full skill catalog land in model context once at session start. `UserPromptSubmit` then restates the iron law in compact form on every turn.
2. Run `preserve-existing-flow` before editing any existing source file.
3. Run `reviewer` before closing **non-trivial** work. Trivial exemptions: docs-only, formatting-only, generated-only, single-line typo or comment fixes, and explicitly throw-away work the user asked for. Everything else (logic changes, multi-file edits, public-API touches, security-sensitive surfaces, brownfield rewrites) goes through `reviewer` before close.
4. Delegate to the matching `.claude/agents/<name>.md` subagent for heavy work that benefits from an isolated context window (saves main-thread tokens).
5. Use `templates/` for commit bodies, PR bodies, final responses, and review summaries.
6. Read `WORKFLOW.md` for branch naming, commit format, and completion rules.

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

