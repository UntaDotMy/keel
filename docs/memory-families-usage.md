<!--
Purpose: Honest inventory of unified memory family surfaces and when they are empty vs used.
Caller: Agents deciding whether a zero-record family is dead code or intentional scaffold.
-->
# Memory family usage

These families live under `~/.claude/memory/<family>/` (CLI: `keel memory <family> …`,
MCP: `memory` tool / `memory_status`). Implementation: `utility/memory_families.rs`.

| Family | Status | Notes |
|---|---|---|
| `research-cache` | **Active** | Written by research-enforcement / explicit cache saves |
| `instincts` | **Active** | Learning loop at SessionEnd |
| `agent-registry` | Scaffold (often 0) | Multi-agent registry; CLI ready; no auto-writer yet |
| `agent-packets` | Scaffold (often 0) | Packet bus for agent teams; CLI ready |
| `loop-guard` | Scaffold (often 0) | Signature anti-loop records; CLI ready |
| `entities` | Scaffold (often 0) | Typed entity upsert; CLI ready |
| `graph` | Scaffold (often 0) | Relation edges; CLI ready — distinct from `code-graph` |

**Policy:** Do not delete scaffold families as "dead code". They are intentional
CLI/MCP surfaces. Prefer wiring a real writer when a feature needs them.
Zero records in `memory_status` is healthy for a fresh or single-agent workspace.
