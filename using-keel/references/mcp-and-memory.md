# MCP tools and memory writes (on-demand)

Deep MCP and memory-writer reference. The compact bootstrap lists only the core tools.

## MCP server (`keel mcp serve`)

`.claude-plugin/plugin.json` registers `mcpServers.keel` at user scope, so the harness auto-discovers the server on **every** project — you do not need to start it. These tools are always available; **prefer them over guessing or ad-hoc file reading**:

- Tool `context_brief` (full name `mcp__keel__context_brief`) — **call this first when you start a session or task.** One call returns the iron law, the full installed skill catalog (name + when_to_use), durable-memory health, and the newest working brief. This is how you become aware of what the toolkit offers when no skill loaded automatically. After reading it, route with `skill_route`, load with `skill_get`, and reach anything else through `cli`.
- Tool `system_map` (full name `mcp__keel__system_map`) — **call this before any claim about the current repo's structure or layout** ("what is this project", "how is this organized", "where does X live") instead of guessing or spelunking files blind. Returns the workspace SYSTEM_MAP.md (auto-refreshed copy preferred, freshly rendered fallback).
- Tool `recall` (full name `mcp__keel__recall`): **call this before claiming what you remember or previously learned.** Full-text search over `~/.claude/memories`, `working-briefs`, and related lanes via the FTS5 index. Same code path as `keel memory recall`.
- Tool `run_command` (full name `mcp__keel__run_command`) — run a noisy shell command through the proxy capture+compaction pipeline so the compacted output lands in context instead of the raw stream. Prefer it for test/build/lint/log/search commands.
- Tool `recall_status` (full name `mcp__keel__recall_status`) — recall index health snapshot (document count, schema version, last-sync timestamp).
- Tool `skill_route` (full name `mcp__keel__skill_route`) — **the on-demand equivalent of the per-prompt skill router.** Pass a prompt; get the single distinctive skill match plus a bounded inline brief of its guidance. Use this when the lifecycle hook that normally injects the brief did not fire (it is unreliable on some platforms) or whenever you are unsure which skill applies.
- Tool `skill_get` (full name `mcp__keel__skill_get`) — load the full SKILL.md body for an installed skill by name. The full-body upgrade after `skill_route`.
- Tool `skill_list` (full name `mcp__keel__skill_list`) — list every installed skill with its name, description, and when_to_use. Discover what skills exist before routing.
- Tool `memory_status` (full name `mcp__keel__memory_status`) — durable-memory health: the recall index snapshot plus per-family record counts. Read-only.
- Tool `brief_list` / `brief_get` / `brief_create` (full names `mcp__keel__brief_*`) — read and write working briefs under `~/.claude/working-briefs`. `brief_create` records the restated request, constraints, acceptance criteria, and assumptions so they survive compaction; the read pair retrieves them.
- Tool `system_map_refresh` (full name `mcp__keel__system_map_refresh`) — regenerate the cached SYSTEM_MAP.md (`system_map` only reads it). Call after creating, deleting, moving, or renaming files.
- Tool `cli` (full name `mcp__keel__cli`) — run any other keel subcommand (`review`, `git-workflow`, `workflow`, `memory`, `orchestration`, `flow`, `code-search`, `config-audit`, `skill-lint`, `checkpoint`, `gain`, `telemetry`, `status`, `doctor`, ...) and get its compacted output. Pass `args` as a string array. Destructive/management subcommands (`install`, `update`, `repair`, `uninstall`, `validate`, `all`, `__self-replace`, `checkpoint restore`) require `confirm: true`; `mcp` is refused. Prefer a dedicated tool when one fits; use `cli` for the rest.
- Resource `keel://system-map` (`text/markdown`) and `keel://recall/status` (`application/json`).

The same 1% rule that governs skills applies here: if a tool could answer the question more authoritatively than your own recall, use it before responding.

**If these tools seem absent — they are almost never actually missing.** MCP tools are namespaced `mcp__keel__<tool>` and may be *deferred* behind `ToolSearch` (the harness forces deferral whenever tool search is on or `ANTHROPIC_BASE_URL` points at a non-first-party gateway). Two traps:

1. **Searching by bare name fails.** `ToolSearch("select:recall")` does an *exact* match on the full name and returns nothing, because the real name is `mcp__keel__recall`. Do **not** conclude "MCP isn't registered" from an empty `select:` result. Search by keyword (`ToolSearch("recall system map run command")`) or select the full namespaced name (`select:mcp__keel__recall`).
2. **Deferral is the bug, not absence.** The fix is `alwaysLoad: true` on the `~/.claude.json` entry, which pins the server's tools into context so they are never deferred. `alwaysLoad` is per-server, so every tool the server publishes is pinned together. Verify with `keel doctor` (it now reports the entry and `alwaysLoad`); repair with `keel repair`, then restart the harness.

## Memory writes (when you learn something durable)

Your working memory only lives in the current context window. Anything you want to survive compaction or the next session has to land on disk. Four memory subcommands actually write — call them when the trigger fires, do not wait for "later":

| Subcommand | Writes | Trigger — call it when |
|---|---|---|
| `keel memory scope resolve --workspace-root <abs> --create-missing --refresh-system-map` | `~/.claude/memories/workspaces/<slug>/reference/SYSTEM_MAP.md` | files moved, packages added, or you noticed the map is stale mid-session. Hooks already fire it at session start, pre-compact, and session end — only call by hand on top of that. |
| `keel memory system-map refresh` | same SYSTEM_MAP.md path | shorthand for the scope-resolve refresh when the workspace is already resolved. |
| `keel memory working-brief write` | `~/.claude/working-briefs/<id>.json` | starting non-trivial work. Capture the user's request, acceptance criteria, and the files you expect to touch *before* coding so completion can be reconciled against it. Update with `working-brief write` again as scope shifts. |
| `keel memory completion-gate check` | nothing (probe-only) | before claiming a task complete. Returns the gate's verdict; failures point at the requirement that has no evidence yet. |

Beyond the four writers above, these `keel memory <verb>` arms are implemented on the **single unified memory surface**: `research-cache`, `maintenance`, `agent-registry`, `agent-packets`, `loop-guard`, `entity`, `graph`, `retrieve`, `instincts`, `status`, and `consolidate` (family-directory status scan: counts/previews, not a merge). `report` is an alias for `status`, and `index` rebuilds the FTS5 recall index; both work. `working-brief record-summary` and `completion-gate record-requirement` are implemented. The `orchestration` group adds `task begin|progress|complete|list` and `checkpoint`. The only `memory` verb that does not run is `hook`: it exits with a pointer to `keel hook install|list|instructions|diagnose`, which owns the harness lifecycle hooks. There is no second memory CLI group; do not invent dual command groups. Check `keel help advanced` or `rust/crates/keel/src/utility/memory/` if unsure.

### research-cache flag shapes (agents get this wrong often)

```bash
# RECORD (save findings) — --question and --answer are required
keel memory research-cache record --question "stripe webhook verify" --answer "use constructEvent + raw body" --source "https://..."
# aliases accepted: --query ≈ --question, --result ≈ --answer (record only)

# LOOKUP (search cache) — uses --query
keel memory research-cache lookup --query "stripe webhook"

# MCP memory tool equivalent:
# action=research-cache, args=["record","--question","...","--answer","..."]
# action=research-cache, args=["lookup","--query","..."]
```

Prefer small payloads. Prefer MCP `memory_status` / dedicated `recall` over giant `memory` args.

**Relationship to the harness's native Auto memory.** Recent the harness ships its own *Auto memory* (notes the model writes to `~/.claude/projects/<project>/memory/MEMORY.md`). The two are complementary: native Auto memory is passive and machine-local; keel's unified `memory` surface is explicit and structured (SYSTEM_MAP, working briefs, completion gate, FTS5 recall, family records under `~/.claude/memories/` and related lanes). Use native Auto memory for incidental notes; use `keel memory ...` when an artifact must survive compaction and be reconcilable. Do not duplicate the same fact into both.

| Thought | Reality |
|---|---|
| "I'll remember this for the next turn" | Memory drifts mid-session. Hook auto-refresh covers SYSTEM_MAP only — working briefs are on you. |
| "The session will end soon, the hook will save it" | SessionEnd refreshes the map, not the brief. If you have a brief worth saving, write it now. |
| "Completion-gate is optional ceremony" | It is the only check that catches "I forgot a requirement" before the user does. Run it before claiming done. |
| "The map looks stale but I'll just guess the layout" | Refresh first: one command, bounded cost. Guessing is what landed us in this PR series in the first place. |
