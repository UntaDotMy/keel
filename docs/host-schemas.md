<!--
Purpose: Canonical technical schemas for skills, subagents, hooks, manifests, profiles, and declarative filters.
Caller: Developers, agents, and tooling authoring or validating host artifacts.
Dependencies: .claude-plugin/plugin.json, rust/crates/keel/src/hooks/claude.rs, rust/crates/keel/src/manager/agent_config.rs.
-->
# Host Schemas and Authoring Reference

This document provides canonical technical schema specifications for artifacts used across AI coding agent hosts.

## SKILL.md Frontmatter Schema

File location: `<skill-name>/SKILL.md` (source) or `~/.claude/skills/<skill-name>/SKILL.md` (installed).
Frontmatter follows YAML format between triple-dash delimiters (`---`).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Recommended | Display name. Defaults to directory name. |
| `description` | string | Recommended | Primary trigger text for skill matcher. Truncated at 1,536 characters combined with `when_to_use`. |
| `when_to_use` | string | Optional | Supplemental activation conditions appended to description. Counts toward 1,536 character cap. |
| `disable-model-invocation` | boolean | Optional | When true, prevents automatic matcher loading. Only explicit `/name` loads it. Default: `false`. |
| `user-invocable` | boolean | Optional | When false, hides from `/` menu. Default: `true`. |
| `allowed-tools` | list/string | Optional | Allowlist of tool names or scoped patterns (for example `Bash(git diff:*)`). |
| `disallowed-tools` | list/string | Optional | Denylist of tools disabled while skill is active. |
| `model` | string | Optional | Model override for remaining turn: `sonnet`, `opus`, `haiku`, full ID, or `inherit`. |
| `effort` | string | Optional | Effort level: `low`, `medium`, `high`, `xhigh`, `max`. |
| `context` | string | Optional | Set to `fork` to execute skill in a forked subagent context. |
| `agent` | string | Optional | Subagent type used when `context: fork` is configured. |
| `hooks` | object | Optional | Lifecycle hooks scoped to this skill. |
| `paths` | list/string | Optional | Glob patterns limiting activation paths (avoid in first-party skills for cross-host resolution). |
| `shell` | string | Optional | Shell used for inline commands: `bash` (default) or `powershell`. |
| `argument-hint` | string | Optional | Autocomplete placeholder shown in UI for expected arguments. |
| `arguments` | list | Optional | Positional parameter mappings for `$name` substitution in body. |

String substitutions available in skill content: `$ARGUMENTS`, `$ARGUMENTS[N]`, `$name`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}`.

## Subagent Frontmatter Schema

File location: `.claude/agents/<agent-name>.md` (project) or `~/.claude/agents/<agent-name>.md` (user).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Lowercase identifier with hyphens. Passed to hooks as `agent_type`. |
| `description` | string | Yes | Trigger description explaining when the controller should delegate. |
| `tools` | list | Optional | Bare tool name allowlist (scoped patterns not supported). |
| `disallowedTools` | list | Optional | Tool denylist applied before allowlist resolution. |
| `model` | string | Optional | Model identifier or `inherit` (default). |
| `permissionMode` | string | Optional | Mode: `default`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`, `plan`. |
| `maxTurns` | integer | Optional | Turn ceiling before subagent halts. |
| `skills` | list | Optional | Preloaded skill names injected at agent startup. |
| `mcpServers` | object/list | Optional | MCP server definitions scoped to subagent. |
| `hooks` | object | Optional | Scoped lifecycle hooks. |
| `memory` | string | Optional | Memory scope: `user`, `project`, or `local`. |
| `background` | boolean | Optional | When true, executes as background task. |
| `effort` | string | Optional | Reasoning effort: `low`, `medium`, `high`, `xhigh`, `max`. |
| `isolation` | string | Optional | Set to `worktree` for temporary git worktree checkout. |
| `color` | string | Optional | UI badge color: `red`, `blue`, `green`, `yellow`, `purple`, `orange`, `pink`, `cyan`. |
| `initialPrompt` | string | Optional | Turn prompt submitted on agent start. |

## Managed Profile Schema

File location: `<specialist-name>/agents/claude.yaml`.
Consumed by Keel CLI runtime (`manager/agent_config.rs`).

| Field | Type | Description |
|---|---|---|
| `reasoning_effort` | string | Baseline reasoning level: `low`, `medium`, `high`, `xhigh`, `max`. Default: `high`. |
| `interface.display_name` | string | Specialist display label. |
| `interface.short_description` | string | Brief description used for summary listings. |
| `interface.default_prompt` | string | Required developer instructions for profile operation. |
| `policy.allow_implicit_invocation` | boolean | Indicates whether profile permits implicit activation. |

## Declarative Filter Registry Schema

File location: `.keel/filters.toml` or `keel.filters.toml` in project root.
Consumed by `keel run -- <command>` compaction engine.

```toml
[[filter]]
name = "cargo-test"
command = "cargo test"
match_mode = "starts_with"
keep = ["FAILED", "error", "test result"]
remove = ["running", "Doc-tests"]
max_lines = 50
```

| Field | Required | Default | Description |
|---|---|---|---|
| `name` | Yes | - | Unique filter identifier. |
| `command` | Yes | - | Command substring or regex pattern to match. |
| `match_mode` | No | `starts_with` | Match strategy: `starts_with`, `exact`, `contains`, `regex`. |
| `exit_code` | No | Any | Apply only when process exits with this code. |
| `keep` | No | `[]` | Line substrings to preserve (empty keeps non-removed lines). |
| `remove` | No | `[]` | Line substrings to discard before retention pass. |
| `max_lines` | No | `40` | Maximum compacted lines retained. |
| `enabled` | No | `true` | Enables or disables filter rule. |

## Plugin Manifest Schema

File location: `.claude-plugin/plugin.json`.

| Field | Type | Description |
|---|---|---|
| `name` | string | Unique plugin namespace identifier (required). |
| `displayName` | string | Human-readable title shown in picker. |
| `version` | string | Semantic version string. |
| `description` | string | Plugin description. |
| `skills` | list | Relative paths to skill directories. |
| `agents` | list | Relative paths to agent markdown files. |
| `commands` | list | Relative paths to custom command files (replaces default scan). |
| `hooks` | string/list | Path to hook configuration file (`./.claude/hooks.json`). |
| `mcpServers` | object | MCP server configurations. |
| `outputStyles` | string/list | Paths to output style configurations. |
| `lspServers` | string | Path to language server configuration. |
| `userConfig` | object | Configurable settings: review strictness, refresh intervals. |
| `experimental.monitors` | list | Build and file system monitoring definitions. |
