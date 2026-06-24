# keel Codex Plugin

Bridges Codex CLI lifecycle events to the `keel` Rust CLI for context injection, observation recording, learning checkpoints, and session management.

## Prerequisites

1. The `keel` binary must be installed at `~/.claude/keel` (unix) or `~/.claude/keel.exe` (win32), or on `PATH`. The plugin resolves the binary once at init, preferring the explicit `~/.claude/` path.
2. Codex CLI must be installed and functional.
3. `tsx` must be available (via `npx` or globally) for TypeScript execution. Alternatively, compile the adapter to JavaScript first.

## Install

### Option A: Local marketplace (recommended)

1. Copy the plugin folder into your personal plugins directory:

```bash
mkdir -p ~/.codex/plugins
cp -R codex ~/.codex/plugins/keel
```

On Windows:

```powershell
New-Item -ItemType Directory -Path "$env:USERPROFILE\.codex\plugins" -Force
Copy-Item -Recurse codex "$env:USERPROFILE\.codex\plugins\keel"
```

2. Create or update `~/.agents/plugins/marketplace.json`:

```json
{
  "name": "personal-keel",
  "interface": {
    "displayName": "keel"
  },
  "plugins": [
    {
      "name": "keel",
      "source": {
        "source": "local",
        "path": "~/.codex/plugins/keel"
      },
      "policy": {
        "installation": "AVAILABLE",
        "authentication": "ON_INSTALL"
      },
      "category": "Productivity"
    }
  ]
}
```

3. Restart Codex and enable the plugin from the plugin directory.

### Option B: Project-scoped install

1. Copy the plugin folder into your repo:

```bash
cp -R codex plugins/keel
```

2. Add to `$REPO_ROOT/.agents/plugins/marketplace.json`:

```json
{
  "name": "repo-keel",
  "plugins": [
    {
      "name": "keel",
      "source": {
        "source": "local",
        "path": "./plugins/keel"
      },
      "policy": {
        "installation": "AVAILABLE",
        "authentication": "ON_INSTALL"
      },
      "category": "Productivity"
    }
  ]
}
```

### Option C: Inline hooks (no plugin)

Add these blocks to `~/.codex/config.toml` directly:

```toml
[[hooks.SessionStart]]
matcher = ""

[[hooks.SessionStart.hooks]]
type = "command"
command = "npx tsx ~/.codex/plugins/keel/hooks/keel-codex.ts"
statusMessage = "Preparing keel session state"

[[hooks.UserPromptSubmit]]
matcher = ""

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "npx tsx ~/.codex/plugins/keel/hooks/keel-codex.ts"
statusMessage = "Injecting keel context"

[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "npx tsx ~/.codex/plugins/keel/hooks/keel-codex.ts"
statusMessage = "Recording tool observation"

[[hooks.Stop]]
matcher = ""

[[hooks.Stop.hooks]]
type = "command"
command = "npx tsx ~/.codex/plugins/keel/hooks/keel-codex.ts"
statusMessage = "Running keel turn-end checkpoint"
```

### Review and trust hooks

After installing, Codex will prompt you to review and trust the new hooks. Open `/hooks` in the Codex CLI to inspect and trust the keel hook definitions.

## Event to Bridge Call Mapping

| Codex Hook Event | Bridge Subcommand | Timing | Blocking? |
|---|---|---|---|
| `SessionStart` (1st per session) | `bridge session-start` — injects bootstrap + workspace digest | Before model sees message | Yes (500ms timeout) |
| `UserPromptSubmit` (every) | `bridge user-prompt` — injects iron law + skill brief | Before model sees message | Yes (500ms timeout) |
| `PreToolUse` (Bash) | `bridge observe` — records tool observation | After tool completes | Fire-and-forget (500ms timeout) |
| `PostToolUse` (Bash) | `bridge observe` — records tool observation | After tool completes | Fire-and-forget (500ms timeout) |
| `PreCompact` | `bridge post-compact` — learning checkpoint + context injection | During compaction | Yes (500ms timeout) |
| `PostCompact` | `bridge post-compact` — learning checkpoint | After compaction | Fire-and-forget (500ms timeout) |
| `Stop` | `bridge post-compact` — turn-end checkpoint | On turn end | Yes (500ms timeout) |

## Design

### Feed-forward, never block

Every hook body is wrapped in try/catch. A bridge timeout or error silently degrades to "no context injected" — the user's turn proceeds normally with no visible interruption. Errors are logged to stderr.

### Session-start deduplication

The first `SessionStart` per session calls `bridge session-start` and caches via an on-disk marker at `~/.claude/state/codex-session-started/<sessionID>`. Subsequent `SessionStart` calls for the same session skip the startup injection. Markers are cleaned on session end.

### 500ms hard timeout

Every bridge call uses `execFileSync` with `timeout: 500` — Node.js built-in timeout that kills the subprocess. This guarantees the plugin never blocks a turn for more than half a second.

### Binary resolution

Prefer the explicit `~/.claude/keel` path (with `.exe` suffix on win32). Fall back to bare command name for PATH lookup. Resolved once at script init.

### Plugin environment variables

Codex injects `PLUGIN_ROOT` and `PLUGIN_DATA` into hook command environments. The adapter script uses `$PLUGIN_ROOT` in hook commands to locate itself relative to the plugin root.

## File Structure

```
keel/
├── .codex-plugin/
│   └── plugin.json          # Plugin manifest
├── hooks/
│   ├── hooks.json           # Lifecycle hook registrations
│   └── keel-codex.ts        # Adapter script (this file)
├── skills/                  # Optional: keel skills for Codex
└── README.md                # This file
```

## Differences from the OpenCode Adapter

| Aspect | OpenCode Adapter | Codex Adapter |
|---|---|---|
| Runtime | Bun (TypeScript native) | Node.js via tsx or compiled JS |
| Plugin format | TypeScript module exports | hooks.json + script files |
| Event model | Named hooks with typed I/O | JSON stdin → stdout per invocation |
| Compaction hook | `experimental.session.compacting` (awaited) | `PreCompact` (synchronous) |
| Session-end trigger | `session.deleted` event | `SessionEnd` event |
| Marker directory | `opencode-session-started` | `codex-session-started` |
