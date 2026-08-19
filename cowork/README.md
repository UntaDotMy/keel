# keel for Claude Desktop (Cowork)

Claude Desktop integration for keel. **Scope note up front:** Claude Desktop does
**not** support the Claude Code lifecycle-hook system or a JavaScript plugin API,
so the hook-driven parts of keel — the Iron Law edit gate, command compaction,
review/working-brief gates, and the learning loop — **cannot run in Claude
Desktop**. What Claude Desktop *does* support is MCP servers, and that is what this
integration wires.

## What you get in Claude Desktop

- **The keel MCP tools.** `keel install` registers the keel MCP server in Claude
  Desktop's `claude_desktop_config.json`, so the keel tools (recall, system-map,
  context-brief, skill routing/get/list, memory status, working-brief create/list,
  anvil, and the generic CLI passthrough) are available inside
  Desktop conversations.
- **The keel CLI.** Everything the `keel` binary does (anvil, memory, recall,
  review) is available from a terminal regardless of host.

## What is NOT available in Claude Desktop

Because Desktop fires no lifecycle hooks, the following — all of which depend on
hooks in Claude Code — do **not** happen automatically in Desktop:

- Iron Law PreToolUse edit gate
- Per-prompt / session-start context injection
- Command-compaction rewriting
- PostToolBatch review / working-brief gates
- Session-end learning checkpoints

Use Claude Code (CLI or IDE) if you need those. Track upstream parity in the
Claude Code issue tracker.

## Install

Install the keel binary, then run `keel install` (it auto-detects Desktop, or pass
`--with cowork`). Installation registers the MCP server in
`claude_desktop_config.json`; restart Claude Desktop to pick it up.

```bash
# macOS / Linux / WSL
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex
```

Config file locations Desktop reads:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`
- Linux: `~/.config/Claude/claude_desktop_config.json`

## Skills

Claude Desktop syncs skills through your claude.ai account (the **Customize**
surface), not from a local plugin manifest. Add the keel skills there if you want
them in Desktop; keel does not push them onto the Desktop filesystem because
Desktop would not read them.

## Uninstall

`keel uninstall` removes the keel MCP entry from `claude_desktop_config.json` and
cleans up the legacy `~/.claude/plugins/keel-cowork/` directory that older builds
created.

## License

MIT — same as the main keel project.
