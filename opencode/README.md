# claude-core OpenCode Plugin

Bridges OpenCode lifecycle events to the `claude-skills` Rust CLI for context injection, observation recording, learning checkpoints, and session management.

## Install

Copy the plugin file into the OpenCode global plugin directory:

```bash
cp opencode/claude-core.ts ~/.config/opencode/plugins/
```

On Windows:

```powershell
Copy-Item opencode\claude-core.ts $env:USERPROFILE\.config\opencode\plugins\
```

OpenCode auto-loads `.ts` files from `~/.config/opencode/plugins/` at startup. No build step required — OpenCode runs TypeScript directly via Bun.

Prerequisite: the `claude-skills` binary must be installed at `~/.claude/claude-skills` (unix) or `~/.claude/claude-skills.exe` (win32), or on `PATH`. The plugin resolves the binary once at init, preferring the explicit `~/.claude/` path.

## Event → Bridge Call Mapping

| OpenCode Event / Hook | Bridge Subcommand | Timing | Blocking? |
|---|---|---|---|
| `chat.message` (1st per session) | `bridge session-start` — injects bootstrap + workspace digest | Before model sees message | Yes (awaited, 500ms timeout) |
| `chat.message` (every) | `bridge user-prompt` — injects iron law + skill brief | Before model sees message | Yes (awaited, 500ms timeout) |
| `event` type=`tool.execute.after` | `bridge observe` — records tool observation | After tool completes | No (fire-and-forget) |
| `event` type=`session.compacted` | `bridge post-compact` — learning checkpoint | On compaction event | No (fire-and-forget) |
| `experimental.session.compacting` | `bridge post-compact` — injects context into compaction summary | During compaction prompt generation | Yes (awaited, 500ms timeout) |
| `event` type=`session.deleted` | `bridge session-end` — learning + save session summary | On session deletion | No (fire-and-forget) |

## Design

### Feed-forward, never block

Every hook body is wrapped in try/catch. A bridge timeout or error silently degrades to "no context injected" — the user's turn proceeds normally with no visible interruption. Errors are logged to stderr (`console.error`).

### Session-start deduplication

The first `chat.message` per session calls `bridge session-start` and caches via an on-disk marker at `~/.claude/state/opencode-session-started/<sessionID>`. Subsequent `chat.message` calls for the same session skip the startup injection. Markers are cleaned on `session.deleted`.

### Session-end on deletion, not idle

`bridge session-end` fires on `session.deleted`, not `session.idle`. The `session.idle` event fires after every turn and would cause excessive bridge calls. `session.deleted` fires once when the user explicitly ends or deletes a session, making it the correct trigger for learning + session summary save.

### Compaction: dual hooks

The `session.compacted` event (fire-and-forget) triggers a learning checkpoint via `bridge post-compact`. The `experimental.session.compacting` named hook (awaited) calls `bridge post-compact` again for the returned text and pushes it into `output.context`, injecting bridge state into the compaction summary so it survives across context windows.

### Observations

`tool.execute.after` events are shipped to `bridge observe` with the tool input serialized as JSON on stdin and an optional `--failed` flag. Observation recording is fire-and-forget; bridge failures are silently swallowed.

### Binary resolution

Prefer the explicit `~/.claude/claude-skills` path (with `.exe` suffix on win32). Fall back to bare command name for PATH lookup by Bun shell. Resolved once at plugin init.

### 500ms hard timeout

Every `$` shell call uses `.timeout(500)` — Bun's built-in timeout that kills the subprocess. This guarantees the plugin never blocks a turn for more than half a second.

## API Uncertainties

1. **`chat.message` hook availability**: This hook is **not documented** in the public OpenCode plugin docs (https://opencode.ai/docs/plugins). It was provided in the task specification as verified against the sst/opencode source. If this hook does not exist in the installed OpenCode version, all context injection degrades silently — `chat.message` would never fire and `event`/`experimental.session.compacting` would still operate normally.

2. **`tool.execute.after` event shape**: The task spec says the event carries `{ type, tool, input, failed? }`. The plugin accesses `event.tool`, `event.input`, and `event.failed` via `as unknown as {...}` cast. If the actual shape differs (e.g. nested under `event.properties.tool`), observations will be recorded with an empty tool name and `{}` input — non-breaking but incomplete.

3. **`Bun.Shell.quiet()` availability**: The `.quiet()` method is documented in Bun's shell API. If unavailable in the OpenCode Bun runtime, the subprocess stderr will leak into stdout, potentially polluting the returned text — non-breaking but may produce unexpected injected context.