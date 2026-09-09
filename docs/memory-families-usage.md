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

## Research cache contract

The research cache keeps two separate freshness concepts:

- `freshness` is cache lifetime guidance such as `90d`; recognized TTL values
  produce `expiresAt` and control whether lookup may reuse the record.
- `freshnessClass` is citation meaning: `fresh`, `historical`, or `local-only`.

Planner-reusable records also preserve `source`, `sourceType`, optional
`publicationDate`, `retrievedAt`, and comma-separated `usedBy` REQ/AC IDs:

```bash
keel memory research-cache record \
  --question "Exact request or research query" \
  --answer "Brief original supporting paraphrase" \
  --source "https://vendor.example/current-doc" \
  --source-type official-doc \
  --retrieved-at "2026-09-09T00:00:00Z" \
  --freshness-class fresh \
  --used-by REQ-001,AC-001 \
  --freshness 90d
```

Legacy records are preserved and remain visible through cache commands, but the
planner reuses only complete citation records. A matching complete stale record
blocks local fallback and asks for a new search. `plan research` then validates
the selected record against the project's retrieval-age policy as a separate
fail-closed check.
