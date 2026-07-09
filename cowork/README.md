# keel Cowork Plugin for Claude Desktop

Native delivery rails for Claude Desktop (Cowork) — managed specialist skills, subagents, hooks, review gates, and the keel CLI for workflow, memory, and command compaction.

## Overview

This plugin bridges Claude Desktop's lifecycle events to the `keel` Rust CLI, enabling:

- **Iron Law Enforcement** — Research-first operating contract on every prompt
- **Command Compaction** — Token-saving output compression for noisy commands
- **Review Gates** — Pre-commit and pre-PR review enforcement
- **Working Brief Reminders** — Structured spec capture before non-trivial work
- **Memory Management** — FTS5-backed recall and structured memory families
- **Sprint Management** — Scrum-style sprint loop with fail-closed completion

## Prerequisites

1. **keel CLI** must be installed at `~/.claude/keel` (macOS/Linux) or `~/.claude/keel.exe` (Windows)
2. **Claude Desktop** (Cowork) with plugin support enabled

### Install keel CLI

```bash
# macOS / Linux / WSL
curl -fsSL https://raw.githubusercontent.com/UntaDotMy/keel/main/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/UntaDotMy/keel/main/install.ps1 | iex
```

## Installation

### Option 1: Via Plugin Command (Recommended)

```bash
# In Claude Desktop, run:
/plugin install keel-cowork
```

### Option 2: Manual Installation

1. Clone or copy the `cowork/` directory to your Claude Desktop plugins folder:

   **macOS/Linux:**
   ```bash
   cp -r cowork ~/.claude/plugins/keel-cowork
   ```

   **Windows:**
   ```powershell
   Copy-Item -Recurse cowork $env:USERPROFILE\.claude\plugins\keel-cowork
   ```

2. Install npm dependencies:
   ```bash
   cd ~/.claude/plugins/keel-cowork
   npm install
   ```

3. Restart Claude Desktop

## Features

### Lifecycle Hooks

The plugin wires the following lifecycle events:

| Event | Action |
|---|---|
| `onInit` | Verify keel binary and log connection status |
| `onSessionStart` | Inject bootstrap context + workspace digest |
| `onUserPromptSubmit` | Inject iron law + skill brief before model response |
| `onPreToolUse` | Command rewrite for compaction |
| `onPostToolUse` | Record observation for learning loop |
| `onPostToolBatch` | Fire review/working-brief gates |
| `onSessionEnd` | Trigger learning checkpoint |
| `onSessionCompacting` | Inject post-compact context |

### Slash Commands

The plugin provides these slash commands:

| Command | Description |
|---|---|
| `/keel` | Run any keel CLI command |
| `/keel:recall` | Full-text search over memories and working-briefs |
| `/keel:sprint` | Sprint backlog management |
| `/keel:review` | Pre-commit or pre-PR review |
| `/keel:memory` | Memory scope and system-map management |
| `/keel:workflow` | Workflow routing and management |
| `/keel:work` | Work item tracking |

### MCP Server

The plugin configures the keel MCP server, providing these tools:

- `context_brief` — Bootstrap context with iron law and skill catalog
- `recall` — FTS5 full-text search
- `run_command` — Compacted command execution
- `skill_route` — Skill routing and selection
- `skill_get` — Get skill content
- `skill_list` — List available skills
- `memory_status` — Memory health overview
- `brief_list` — List working briefs
- `brief_get` — Get working brief content
- `brief_create` — Create working brief
- `system_map` — Workspace structure map
- `system_map_refresh` — Force refresh system map
- `sprint` — Sprint management
- `user_story_lint` — Validate user story format
- `cli` — Generic CLI passthrough

### Configuration

The plugin respects these userConfig settings:

| Setting | Default | Description |
|---|---|---|
| `review_strictness` | `advisory` | Review gate strictness: `advisory`, `strict`, or `off` |
| `system_map_refresh_interval` | `10` | Edit-class calls between auto-refresh |
| `memory_retention_days` | `14` | Days to retain raw output data |

## Architecture

```
Claude Desktop (Cowork)
    │
    ├── Plugin System
    │   └── keel-cowork plugin (TypeScript)
    │       │
    │       ├── Lifecycle Hooks → keel bridge
    │       ├── Slash Commands → keel CLI
    │       └── Skills → SKILL.md files
    │
    └── MCP Server
        └── keel mcp serve (Rust binary)
            │
            ├── Command Compaction (proxy layer)
            ├── FTS5 Recall Index
            ├── Sprint Ledger
            └── Memory Families
```

## Comparison with CLI Version

| Feature | CLI | Desktop (Cowork) |
|---|---|---|
| Plugin manifest (skills, agents, commands) | ✅ | ✅ |
| Lifecycle hooks | ✅ | ✅ |
| MCP server (31 tools) | ✅ | ✅ |
| Command compaction | ✅ | ✅ |
| Review/sprint/working-brief gates | ✅ | ✅ |
| OpenCode bridge | N/A | ✅ |
| Desktop notifications | ✅ | ✅ |

## Troubleshooting

### Plugin not loading

1. Verify plugin is in correct location: `~/.claude/plugins/keel-cowork/`
2. Check npm dependencies installed: `cd ~/.claude/plugins/keel-cowork && npm install`
3. Check Claude Desktop logs for plugin loading errors

### Commands not working

1. Verify keel binary is installed: `~/.claude/keel --version`
2. Check binary is executable: `chmod +x ~/.claude/keel` (macOS/Linux)
3. Test bridge manually: `~/.claude/keel bridge gate-status`

### Hooks not firing

1. Check plugin is enabled in Claude Desktop settings
2. Verify plugin.json has correct hooks configuration
3. Check Claude Desktop supports the hook events used

## Development

### Building the Plugin

```bash
cd cowork
npm install
npm run build
```

### Testing

```bash
# Test bridge locally
./keel bridge session-start --session test --cwd .

# Test recall
./keel recall "test query"
```

## License

MIT — same as main keel project.
