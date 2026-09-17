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
| `lessons` | **Active** | Evidence-backed lesson lifecycle (record/reinforce/promote/evaluate/demote) |
| `agent-registry` | Scaffold (often 0) | Multi-agent registry; CLI ready; no auto-writer yet |
| `agent-packets` | Scaffold (often 0) | Packet bus for agent teams; CLI ready |
| `loop-guard` | Scaffold (often 0) | Signature anti-loop records; CLI ready |
| `entities` | Scaffold (often 0) | Typed entity upsert; CLI ready |
| `graph` | Scaffold (often 0) | Relation edges; CLI ready — distinct from `code-graph` |

**Policy:** Do not delete scaffold families as "dead code". They are intentional
CLI/MCP surfaces. Prefer wiring a real writer when a feature needs them.
Zero records in `memory_status` is healthy for a fresh or single-agent workspace.

## Recall scope and lifecycle

`keel memory recall` and `keel memory retrieve` share the same scope and lifecycle
filters. Use `--workspace <scope>` to select a workspace explicitly; use
`--local-only` to require that scope and the current branch (with `unknown` retained
for legacy records). Eligibility filtering happens before the result limit, so a
foreign hit cannot consume a local result slot.

Recall excludes stale or expired research-cache records, quarantined or superseded
lessons, and superseded entities. The source files remain on disk for audit and
explicit family commands; only retrieval eligibility changes.

Every hit's `retrievalRef` replays the same scoped query, including `--workspace`
and `--local-only` when they were supplied. A recovery reference therefore cannot
silently widen a scoped search into a cross-workspace one.

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

## Entity provenance and supersede

Entity upserts carry `source`, `supersedes`, and `supersededBy` provenance
fields. Use `entity supersede --id <old> --by <new> --reason <why>` to link a
tombstone without deleting either record, so the decision stays auditable:

```bash
keel memory entity upsert --name OldAuth --type decision --summary v1 --source review-1
keel memory entity upsert --name NewAuth --type decision --summary v2 --supersedes decision-oldauth
keel memory entity supersede --id decision-oldauth --by decision-newauth --reason "v2 verified"
```

## Lesson lifecycle

Lessons are scoped, evidence-backed statements about a problem pattern and the
correct response. `record` refuses a lesson without evidence; `promote` requires
the confidence threshold; `evaluate` compares measured before/after behaviour
and demotes a lesson that regresses instead of leaving it active.

```bash
keel memory lessons record \
  --pattern "repeated failing strategy" \
  --evidence "two runs failed with the same signature" \
  --response "use the machine-readable output" --scope workspace
keel memory lessons reinforce --id repeated-failing-strategy
keel memory lessons promote   --id repeated-failing-strategy
keel memory lessons evaluate  --id repeated-failing-strategy \
  --before-tokens 4000 --after-tokens 780 --before-turns 4 --after-turns 2
keel memory lessons demote    --id repeated-failing-strategy --state quarantined --reason "regressed twice"
keel memory lessons list --status active
```

States: `candidate`, `active`, `questioned`, `quarantined`, `superseded`.
Promotion only changes lesson state; it never rewrites policy.

## Research cache expiry

`research-cache expire` is a dry run by default and only marks records `expired`
with `--apply`, so a bulk sweep never silently discards evidence:

```bash
keel memory research-cache expire --days 90
keel memory research-cache expire --days 90 --apply
```

## SessionEnd retention and storage bounds

Each JSON record is capped at 1 MiB. Each family directory is capped at 10,000
records and 64 MiB; writes and retention scans use one bounded sidecar lock per
family. SessionEnd prunes timestamped records in `events`, `agent-packets`,
`loop-guard`, and `graph` after 90 days by default. Records without a recognized
`recordedAt`, `createdAt`, or `updatedAt` timestamp are retained as legacy evidence.

The shared `memory_retention_days` user setting overrides the default. The
`CLAUDE_SKILLS_MEMORY_RECORD_RETENTION_DAYS` environment variable is the dedicated
operator override; set it to `0` to disable this SessionEnd prune.
