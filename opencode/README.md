# keel OpenCode Plugin

Bridges OpenCode lifecycle events to the `keel` Rust CLI for context injection, observation recording, learning checkpoints, and session management.

## Install

`keel install` auto-detects OpenCode (via `~/.config/opencode/` dir, `OPENCODE` env var, or `opencode` binary on PATH) and wires this adapter automatically. Use `--without opencode` to skip, `--with opencode` to force.

Manual install (if needed):

Copy the plugin file into the OpenCode global plugin directory:

```bash
cp opencode/keel.ts ~/.config/opencode/plugins/
```

On Windows:

```powershell
Copy-Item opencode\keel.ts $env:USERPROFILE\.config\opencode\plugins\
```

OpenCode auto-loads `.ts` files from `~/.config/opencode/plugins/` at startup. No build step required — OpenCode runs TypeScript directly via Bun.

Prerequisite: the `keel` binary must be installed at `~/.keel/keel` (unix) or `~/.keel/keel.exe` (win32), or on `PATH`. The plugin resolves the binary once at init, preferring the explicit `~/.keel/` path.

## Event → Bridge Call Mapping

| OpenCode Event / Hook | Bridge Subcommand | Timing | Blocking? |
|---|---|---|---|
| `chat.message` (1st per session) | `bridge session-start` — injects bootstrap + workspace digest | Before model sees message | Yes (awaited, 500ms timeout) |
| `chat.message` (every) | `bridge user-prompt` — injects iron law + skill brief | Before model sees message | Yes (awaited, 500ms timeout) |
| `event` type=`tool.execute.after` | `bridge observe` — records tool observation | After tool completes | No (fire-and-forget) |
| `event` type=`session.compacted` | `bridge pre-compact` (learning checkpoint before window rewrite) + `bridge post-compact` (post-compaction context) | On compaction event | No (fire-and-forget) |
| `experimental.session.compacting` | `bridge pre-compact` (learning) + `bridge post-compact` (injects context into compaction summary) | During compaction prompt generation | Yes (awaited, 500ms timeout) |
| `event` type=`session.deleted` | `bridge session-end` — learning + save session summary | On session deletion | No (fire-and-forget) |

## Design

### Feed-forward, never block

Every hook body is wrapped in try/catch. A bridge timeout or error silently degrades to "no context injected" — the user's turn proceeds normally with no visible interruption. Errors are logged to stderr (`console.error`).

### Session-start deduplication

The first `chat.message` per session calls `bridge session-start` and caches via an on-disk marker at `~/.claude/state/opencode-session-started/<sessionID>`. Subsequent `chat.message` calls for the same session skip the startup injection. Markers are cleaned on `session.deleted`.

### Session-end on deletion, not idle

`bridge session-end` fires on `session.deleted`, not `session.idle`. The `session.idle` event fires after every turn and would cause excessive bridge calls. `session.deleted` fires once when the user explicitly ends or deletes a session, making it the correct trigger for learning + session summary save.

### Compaction: dual hooks

The `session.compacted` event (fire-and-forget) runs `bridge pre-compact` (learning checkpoint before the window is rewritten) then `bridge post-compact` (post-compaction context + idempotent learning upsert). The `experimental.session.compacting` named hook (awaited) calls `bridge pre-compact` then `bridge post-compact` for the returned text and pushes it into `output.context`, injecting bridge state into the compaction summary so it survives across context windows.

### Observations

`tool.execute.after` events are shipped to `bridge observe` with the tool input serialized as JSON on stdin and an optional `--failed` flag. Observation recording is fire-and-forget; bridge failures are silently swallowed.

### Binary resolution

Prefer the explicit `~/.keel/keel` path (with `.exe` suffix on win32). Fall back to bare command name for PATH lookup by Bun shell. Resolved once at plugin init.

### 500ms hard timeout

Every `$` shell call uses `.timeout(500)` — Bun's built-in timeout that kills the subprocess. This guarantees the plugin never blocks a turn for more than half a second.

## API Notes

1. **`chat.message` hook**: This hook is the primary context-injection seam. It is not listed on the public OpenCode plugin docs page (https://opencode.ai/docs/plugins), but it is a real OpenCode plugin hook used by community plugins (e.g. `oh-my-openagent`, whose GitHub issue code-yeongyu/oh-my-openagent#885 "chat.message hook output not visible when plugin injects into output.parts" confirms plugins inject into `output.parts` via this hook). The adapter prepends injected context to `output.parts` before the model sees the message. If a future OpenCode version removes it, context injection degrades silently — `event`/`experimental.session.compacting` would still operate normally.

2. **`tool.execute.after` event shape**: The event carries `{ type, tool, input, failed? }` (per the sst/opencode source). The plugin accesses `event.tool`, `event.input`, and `event.failed` via `as unknown as {...}` cast. If the actual shape differs (e.g. nested under `event.properties.tool`), observations will be recorded with an empty tool name and `{}` input — non-breaking but incomplete.

3. **`Bun.Shell.quiet()` availability**: The `.quiet()` method is documented in Bun's shell API. If unavailable in the OpenCode Bun runtime, the subprocess stderr will leak into stdout, potentially polluting the returned text — non-breaking but may produce unexpected injected context.