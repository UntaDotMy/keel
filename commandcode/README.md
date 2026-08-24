# keel Command Code (cmdc) Mod

Bridges Command Code lifecycle events to the `keel` Rust CLI for compaction continuity, memory injection, the Iron Law gate, and the `run_command` compaction wrapper. Uses the same `keel bridge` surface the Codex/OpenCode/Pi/Cursor adapters use.

## Prerequisites

1. The `keel` binary must be installed at `~/.keel/keel` (unix) or `~/.keel/keel.exe` (win32), or on `PATH`. The mod resolves the binary once at init in this order: `$KEEL_HOME`, `~/.keel/`, the legacy `~/.claude/` path, then PATH.
2. Command Code must be installed and functional.

## Install

`keel install` auto-detects Command Code (via `~/.commandcode/` or the `cmdc` binary on PATH) and wires this adapter automatically. Use `--without commandcode` to skip, `--with commandcode` to force.

Manual install options:

### Option A: Personal (recommended)

Copy the mod into your personal mods directory:

```bash
mkdir -p ~/.commandcode/mods
cp commandcode/keel-cmdc.ts ~/.commandcode/mods/keel-cmdc.ts
```

On Windows:

```powershell
New-Item -ItemType Directory -Path "$env:USERPROFILE\.commandcode\mods" -Force
Copy-Item commandcode\keel-cmdc.ts "$env:USERPROFILE\.commandcode\mods\keel-cmdc.ts"
```

It loads on the next Command Code session.

### Option B: Project-scoped

Copy the mod into the repo's project mods directory (trust-gated):

```bash
mkdir -p .commandcode/mods
cp commandcode/keel-cmdc.ts .commandcode/mods/keel-cmdc.ts
```

### Option C: Test now

```bash
cmdc --mod ./commandcode/keel-cmdc.ts
```

## Event → bridge mapping

| keel capability | Command Code seam | `keel bridge` call |
|---|---|---|
| SessionStart context | `cmd.hooks({onSessionStart})` | `bridge session-start --session <id> --cwd <cwd>` (once per session) |
| Per-prompt memory push | `cmd.hooks({transformContext})` (after compaction) | `bridge user-prompt --session --cwd --prompt` |
| **Post-compact continuity** | `cmd.on('compaction_start')` + `cmd.on('compaction_done')` | `compaction_start` → `bridge pre-compact`; `compaction_done` → `bridge post-compact` (memory digest re-push) |
| Iron Law gate | `cmd.hooks({beforeToolCall})` | `bridge pre-tool-use --session --cwd --tool <name>` (`KEEL_GATE_DENY` → block) |
| `run_command` compaction wrapper | `cmd.hooks({beforeToolCall})` (shell tools) | `bridge rewrite --tool <name>` (stdin) → `KEEL_REWRITE <cmd>` |
| Observation | `cmd.hooks({afterToolCall})` | `bridge observe --session --cwd --tool <name> [--failed]` |
| Session end | `cmd.hooks({onSessionEnd})` | `bridge session-end --session --cwd` (learning, marker cleanup) |

## Compaction continuity (the point)

Command Code compacts history when the context window fills. The on-disk transcript is **never rewritten**; compaction appends a summary entry. But the model's *working memory* of the keel context (system map, working brief, memory digest) drops out of the window.

The mod closes that gap the same way the Claude Code PreCompact/PostCompact hooks do:

- `compaction_start` fires `bridge pre-compact` (persists what was learned before the window is rewritten).
- `compaction_done` calls `bridge post-compact` and keeps the returned memory digest (workspace scope summary + map/brief digest).
- The next run's `transformContext` re-injects that digest as the first user block, so the agent resumes knowing the job. The durable transcript is never touched.

## Iron Law gate

`beforeToolCall` blocks edit-class tools until the session has used a keel research tool (MCP `system_map`/`recall`/`context_brief`/`skill_*`/`code_search`, or matching `keel …` CLI). The gate is **fail-closed**: a timeout or bridge error blocks the edit rather than silently allowing it. The shared satisfaction marker lives at `~/.keel/state/iron-law-satisfied/<sanitized-session>` (legacy `~/.claude` fallback), matching every other adapter. Set `KEEL_IRON_LAW_GATE=balanced|off` to relax (see `docs/hook-usage.md`).

## Differences from other adapters

- **No stdin JSON envelope.** Command Code's ModApi exposes typed hooks (`onSessionStart`, `beforeToolCall`, `afterToolCall`, `transformContext`, `onSessionEnd`) plus `cmd.on('compaction_start'|'compaction_done')`; the adapter is a TS mod, not a shell/JSON hook script.
- **Compaction events are observed, not awaited.** `compaction_start`/`compaction_done` fire on the event bus; the mod stores the post-compact digest in its closure and re-injects it on the next `transformContext`. This mirrors OpenCode's `experimental.session.compacting` (awaited) and Pi's `session_compact`; the host provides the seam, keel fills it.
- **Session identity.** Command Code does not expose a session id on hook params, so the mod derives one from the workspace cwd (`cmdc-<sanitized-cwd>`). The session-start marker is stored via `cmd.session.appendCustomEntry` so it survives compaction and resume.
- **No `run_command` MCP requirement.** The compaction wrapper works through `beforeToolCall` shell rewrites; the `run_command` MCP tool (registered via the keel MCP server) remains the direct-call compaction proxy.

## Uninstall

```bash
rm ~/.commandcode/mods/keel-cmdc.ts
```

On Windows:

```powershell
Remove-Item "$env:USERPROFILE\.commandcode\mods\keel-cmdc.ts"
```

## Verification

1. Start Command Code with the mod loaded; confirm no `mod_error` appears in the feed.
2. Send a message that references the repo; the keel operating contract should appear in context.
3. Attempt an edit before running any keel research tool; it should be blocked with the Iron Law reason.
4. Run a noisy shell command; verify it is rewritten to `keel run -- ...`.
5. In a long session, trigger `/compact`; after compaction, ask "what is the job". The post-compact memory digest should let the agent answer from keel memory.
