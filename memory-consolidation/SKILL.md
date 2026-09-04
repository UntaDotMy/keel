---
name: memory-consolidation
description: Consolidates recent tool-call observations into durable memory notes by extracting patterns, decisions, and solutions. Use when the agent should review recent work, distill what was learned, and write structured notes to keel's memory store so future sessions start ahead. Triggered by the agent at session-end, compaction, or when the user says "consolidate", "what did we learn", or "save what we did".
when_to_use: |
  Use when consolidating recent work into durable memory:
  - At session end or compaction, before context is lost
  - When the user says "consolidate", "save what we learned", "remember this"
  - After solving a non-trivial problem worth keeping
  - Before starting a new task that builds on recent work

  Do NOT use for:
  - Capturing a single solved problem (use compounding-knowledge instead)
  - Writing working briefs (use the brief tools instead)
  - System map refresh (use keel memory system-map refresh)
allowed-tools: Read, Grep, Glob, Bash(keel learn:*), Bash(keel memory:*), Bash(keel run:*)
effort: medium
---

# Memory Consolidation

Consolidate recent tool-call observations into durable, searchable memory notes. This is the "neocortex" layer — where episodic observations (what tools were used, what commands ran, what files were edited) get distilled into consolidated knowledge (what patterns emerged, what decisions were made, what solutions worked).

## When to Consolidate

Consolidate at these moments:
1. **Session end** — before context is lost, distill what happened
2. **Compaction** — the harness is about to compact; save what matters first
3. **User request** — "consolidate", "save what we learned", "remember this"
4. **After a hard-won fix** — a solution worth keeping for future sessions

## How to Consolidate

### Step 1: Review Recent Observations

Run this to see what was observed:

```bash
keel learn status --json
```

This shows recent observation counts, instinct signals, and qualifying patterns. If observations are empty, there is nothing to consolidate — stop.

### Step 2: Extract Patterns, Decisions, and Solutions

For each significant cluster of activity, extract:

- **Pattern**: What recurred? (e.g., "ran `cargo test --workspace` 12 times, 10 passed, 2 failed")
- **Decision**: What was decided? (e.g., "chose to add `glab` to all 3 compaction surfaces for consistency with `gh`")
- **Solution**: What worked? (e.g., "fixed observation capture by wrapping tool_input in an envelope before passing to derive_signature")
- **Failure**: What went wrong? (e.g., "CI failed because doc_parity test expected 45 skills but manifest had 46")
- **Context**: Why does this matter? (e.g., "without this fix, no observations were written under OpenCode, breaking the entire memory chain")

### Step 3: Consolidate Through the Native Store

Use Keel's implemented consolidation command so records remain in the unified,
indexed memory layout instead of creating a parallel directory by hand:

```bash
keel memory consolidate --window 7
```

When the review identifies a reusable semantic finding that is not already in a
working brief or family record, write it through the supported research-cache
surface with its evidence and freshness guidance:

```bash
keel memory research-cache record --question "<topic>" --answer "<finding and evidence>" --source "<file, PR, or URL>" --freshness "<guidance>"
```

### Step 4: Deduplicate

Before writing a new note, check if a similar note already exists:

```bash
keel memory recall "[topic keywords]"
```

If a similar record exists:
- **Same topic, new info**: record only the changed finding and identify what it supersedes
- **Same topic, same info**: skip — don't duplicate
- **Different topic**: create a new scoped record

### Step 5: Verify Recall

After writing, verify the note is searchable:

```bash
keel memory recall "[keywords from the note]"
```

The record should appear in the results. If it doesn't, rebuild the recall index and retry:

```bash
keel memory index
keel memory recall "[keywords from the note]"
```

## What NOT to Consolidate

- Trivial actions (read a file, ran ls, opened a tab)
- Failed approaches that were abandoned (unless the failure itself is instructive)
- Information already captured by a working brief or system map
- Raw command output (that's what `keel run --` raw recovery is for)

## Relationship to Other Memory Surfaces

| Surface | What it stores | Who writes it | When |
|---|---|---|---|
| Observations (episodic) | Raw tool-call signatures | keel automatically | Every tool call |
| Instincts | Distilled behavioral patterns | keel learning loop | Session end / compaction |
| Consolidated family records | Structured knowledge | Keel + agent review | Session end / user request |
| Working briefs | Task scope + constraints | Agent (brief tools) | Before non-trivial work |
| System map | Project structure | keel automatically | Session start / refresh |

Consolidated family records bridge raw observations (episodic, high-volume, low-signal) and instincts (distilled, low-volume, high-signal). They capture reusable findings without adding a second memory tree.

## Brain Analogy

This skill replicates the hippocampus→neocortex consolidation that happens during sleep:

1. **Hippocampus** (observations): Records episodic memories — what happened, when, in what order. High-fidelity but high-volume.
2. **Neocortex** (consolidated notes): Extracts semantic knowledge — what patterns emerged, what decisions matter, what solutions generalize. Low-volume, high-signal.
3. **Cue-recall** (keel memory recall): Retrieves relevant knowledge when cued by a query. Hybrid lexical (FTS5) + semantic (vector, if available).

The agent (inherited LLM) IS the consolidation mechanism — keel provides the storage and recall, the agent provides the understanding. This is why the skill is agent-driven: keel cannot call the LLM, but the agent IS the LLM.
