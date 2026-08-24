# keel Cursor Adapter

Bridges Cursor IDE lifecycle events to the `keel` Rust CLI for Iron Law enforcement, compaction rerouting, observation recording, learning checkpoints, and session management — plus a static `.cursorrules` rules file and a native MCP server for tool access.

## What This Does

Cursor (1.7+) exposes a hook system (see https://cursor.com/docs/hooks) with events like `preToolUse`, `postToolUse`, `preCompact`, `stop`, and `sessionEnd`, and supports MCP servers (see https://cursor.com/docs/mcp). This adapter has **three layers**:

1. **`.cursorrules`** — the persistent keel iron law, skill catalog, workflow commands, and branch/commit rules, injected by Cursor as system instructions so the model has keel discipline from the first prompt. Cursor reads it from the project root (or `~/.cursorrules` globally).
2. **`hooks/`** — a `hooks.json` + `keel-cursor.sh` script that subscribes to Cursor's hook events, wiring each to a host-neutral `keel bridge` subcommand. This delivers the same automatic behavior the hook system provides in Claude Code, Codex, and Pi: the Iron Law edit gate (blocks edits until the model has read first), compaction rerouting for noisy shell commands, observation capture, and the compaction/session-end learning cycle.
3. **`mcp.json`** — registers the keel MCP server (`keel mcp serve`) so all keel tools (`recall`, `skill_route`, `skill_get`, `anvil`, `brief_create`, `system_map`, etc.) are available as native MCP tool calls in Cursor, no CLI shell-out required for tool access. The installer merges the `keel` entry into `~/.cursor/mcp.json` (preserving your other MCP servers); there is no `alwaysLoad` equivalent — Cursor loads MCP servers on demand, toggleable via the sidebar Customize menu.

## Prerequisites

1. Cursor IDE installed (1.7+ for hooks; `.cursorrules` works on any version).
2. The `keel` binary installed at `~/.keel/keel` (unix) or `~/.keel/keel.exe` (win32), or on `PATH`.
3. `jq` on `PATH` (required by `keel-cursor.sh` for safe JSON parsing and output escaping). On Windows, install via `winget install jqlang.jq` or `scoop install jq`. If `jq` is absent the hook script fails open (emits `{}`) and only the static `.cursorrules` layer is active.

## Install

`keel install` does NOT auto-detect Cursor (it's a desktop IDE with no reliable cross-platform detection). Use `--with cursor` to force wiring of **all three layers**: `.cursorrules` (to `~/.cursorrules`), the hooks (to `~/.cursor/hooks/`), and the MCP server (merged into `~/.cursor/mcp.json`). Use `--without cursor` to skip.

Manual install:

### Option A: Global (recommended — applies keel to every project)

Copy all three layers into your home directory:

```bash
# Rules layer
cp cursor/.cursorrules ~/.cursorrules

# Hooks config lives at ~/.cursor/hooks.json; the script stays in ~/.cursor/hooks.
mkdir -p ~/.cursor/hooks
cp cursor/hooks/hooks.json    ~/.cursor/hooks.json
cp cursor/hooks/keel-cursor.sh ~/.cursor/hooks/
chmod +x ~/.cursor/hooks/keel-cursor.sh

# MCP layer (merge the keel entry into ~/.cursor/mcp.json — preserves existing servers)
mkdir -p ~/.cursor
# If ~/.cursor/mcp.json already exists, edit it to add the "keel" entry under "mcpServers"
# instead of overwriting. The keel installer handles this merge automatically.
cp -n cursor/mcp.json ~/.cursor/mcp.json 2>/dev/null || true
```

On Windows PowerShell:

```powershell
# Rules layer
Copy-Item cursor\.cursorrules "$env:USERPROFILE\.cursorrules"

# Hooks config lives at $env:USERPROFILE\.cursor\hooks.json.
New-Item -ItemType Directory -Path "$env:USERPROFILE\.cursor\hooks" -Force
Copy-Item cursor\hooks\hooks.json    "$env:USERPROFILE\.cursor\hooks.json"
Copy-Item cursor\hooks\keel-cursor.sh "$env:USERPROFILE\.cursor\hooks\"

# MCP layer (merge manually, or let `keel install --with cursor` do it)
New-Item -ItemType Directory -Path "$env:USERPROFILE\.cursor" -Force
# If mcp.json exists, merge the "keel" entry into its "mcpServers" object instead.
if (-not (Test-Path "$env:USERPROFILE\.cursor\mcp.json")) {
    Copy-Item cursor\mcp.json "$env:USERPROFILE\.cursor\mcp.json"
}
```

Cursor watches `~/.cursor/hooks.json` and reloads it automatically. The hook commands reference `~/.cursor/hooks/keel-cursor.sh` via `bash`, so Cursor must be able to spawn `bash` (available via Git Bash on Windows, native on macOS/Linux).

### Option B: Project-scoped

Copy `.cursorrules` into your project root (Cursor discovers it natively), put the hook config at `<project>/.cursor/hooks.json`, and place the script under `<project>/.cursor/hooks/`:

```bash
cp cursor/.cursorrules /path/to/your/project/
mkdir -p /path/to/your/project/.cursor/hooks
cp cursor/hooks/hooks.json    /path/to/your/project/.cursor/hooks.json
cp cursor/hooks/keel-cursor.sh /path/to/your/project/.cursor/hooks/
chmod +x /path/to/your/project/.cursor/hooks/keel-cursor.sh
```

Project hooks run from the project root and take precedence over user hooks. Cloud agents load project hooks (`.cursor/hooks.json` in the repo) but not user-level hooks.

### Verify the install

After copying, on the first edit-class tool call in a fresh Cursor conversation, the Iron Law gate will block (`permission: "deny"`) until you have used a reading tool (Read/Grep) or a keel reading command — this is intended behavior. Check the hook is loaded via Cursor's hook inspection (the deny message surfaces in the client).

## What the Rules Include

### Iron Law (4 rules)

1. **Understand before building.** Restate the request, confirm the user story, research the owning module and framework. No guessing.
2. **Skills first.** Invoke the matching skill before writing code. The cost of skipping is shipping a regression.
3. **Native commands before raw shell.** Use `keel run -- <command>` for noisy commands. Never run raw and compact after.
4. **Find the root cause.** Trace symptoms end-to-end with file:line evidence before changing anything.

### Key Commands

| Command | Use |
|---|---|
| `keel review pre-pr --base-ref origin/feat` | Review before PR |
| `keel memory scope resolve --create-missing --refresh-system-map` | Refresh memory |
| `keel code-search search --workspace-root "$PWD" --query "..."` | Search code |
| `keel code-search siblings` | Completeness scan after a fix or implement |

### Branch and Commit Rules

- Branch model: `main` ← `dev` ← `feat` ← `task/<task>` [← `task/<task>/<subtask>`]
- Commit format: `[category]: [feature_category]: short info` (categories: Add, Config, Refactor, Wip, Fix, Docs; feature_category uppercase)
- Never delete a branch after push or merge

### Skill Catalog (44 skills)

Full catalog organized by domain: Security & Review, API & Backend, Infrastructure & DevOps, Data & ML, Frontend & Mobile, Quality & Testing, Architecture & Planning, Delivery & Git, Code Quality & Dependencies. Each skill includes its `whenToUse` guidance.

## Event → Bridge Call Mapping (hooks/keel-cursor.sh)

| Cursor Hook Event | Bridge Subcommand | Behavior |
|---|---|---|
| `preToolUse` (Read/Grep) | — | Marks Iron Law satisfied, allows |
| `preToolUse` (Write/Edit/Delete/...) | `pre-tool-use` (after gate satisfied) | **Blocks** with `permission: "deny"` until the model has read first; then records gate state |
| `preToolUse` (Shell) | `rewrite` | Reroutes noisy commands via `updated_input.command` |
| `preToolUse` (1st per session) | `session-start` | Bootstrap + workspace digest + MCP self-heal (marker-guarded) |
| `postToolUse` (Shell) | `observe` | Observation capture (fire-and-forget) |
| `preCompact` | `pre-compact` | Pre-compaction learning checkpoint |
| `stop` | `post-compact` | Turn-end checkpoint |
| `sessionEnd` | `session-end` | Learning cycle + session capture + marker cleanup |

The `preToolUse` matcher filters by tool **name** (regex): `Write|Edit|Delete|MultiEdit|ApplyPatch|Patch|Shell|Read|Grep`. Cursor tool names are capitalized (`Shell`, `Read`, `Write`, `Edit`, `Grep`, `Delete`, `Task`, `MCP:<name>`). Output contract: `{permission:"deny",user_message,agent_message}` to block, `{permission:"allow",updated_input:{command}}` to rewrite, `{}` to pass through.

### Iron Law enforcement

The Cursor adapter enforces keel's Iron Law — **keel research before editing** (STRICT default) — using Cursor's native `preToolUse` `permission: "deny"` mechanism and `keel bridge pre-tool-use` (same Rust core as OpenCode/Codex/Pi/Claude). Edit-class tools are denied until the session has evidence of a keel research tool (`system_map` / `recall` / `context_brief` / `skill_*` / `code_search`, or matching `keel …` CLI). Plain Read/Grep does **not** clear the gate. Satisfaction is tracked at the shared path `~/.claude/state/iron-law-satisfied/<sanitized-session>`, cleaned on `sessionEnd`.

## Differences from Other Adapters

| Aspect | OpenCode Adapter | Codex Adapter | Pi Adapter | Cursor Adapter |
|---|---|---|---|---|
| Mechanism | TypeScript plugin (Bun) | Codex plugin (hooks.json + tsx) | TypeScript extension + AGENTS.md | .cursorrules + hooks.json + bash script |
| Runtime bridge | Yes | Yes | Yes | Yes |
| Iron Law enforcement | throws from `tool.execute.before` | `PreToolUse` `permissionDecision:"deny"` | `tool_call` `{block:true}` | `preToolUse` `permission:"deny"` |
| Context injection | `chat.message` → output.parts | `UserPromptSubmit`/`SessionStart` stdout | `input`/`message_start` + AGENTS.md | `.cursorrules` static + `session-start` |
| Observation recording | `tool.execute.after` | `PostToolUse` | `tool_execution_end` | `postToolUse` |
| Learning checkpoints | `experimental.session.compacting` | `PreCompact`/`PostCompact` | `session_compact` | `preCompact`/`stop` |
| Session-end | `session.deleted` | `SessionEnd` | `session_shutdown` | `sessionEnd` |
| Marker dir | `opencode-*` | `codex-*` | `pi-*` | `cursor-*` |
| Runtime dep | Bun | Node/tsx + keel binary | Node + keel binary | bash + jq + keel binary |

The Cursor adapter now matches the other adapters in runtime coverage. The `.cursorrules` file carries the persistent iron law in the system prompt (the part Cursor injects into every turn), and the `hooks/` layer delivers the automatic, per-event behavior (Iron Law block, compaction reroute, observation, learning) that static rules alone cannot. The model can also call keel CLI commands directly for workflow, review, and memory operations.
