---
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

### Step 3: Write Structured Notes

Write each consolidated note as a Markdown file to `~/.claude/memory/consolidated/`:

```bash
keel run -- mkdir -p ~/.claude/memory/consolidated
```

File naming: `YYYY-MM-DD-HHmm-short-slug.md` (e.g., `2026-06-26-1340-compaction-reroute-all-agents.md`)

Note structure:

```markdown
# [Short title]

## Context
[1-2 sentences: what was the situation, what was the goal]

## What Happened
[3-5 sentences: what was done, what tools were used, what decisions were made]

## Key Decisions
- [Decision 1: why]
- [Decision 2: why]

## Solutions
- [Solution 1: file:line evidence]
- [Solution 2: file:line evidence]

## Failures (if any)
- [Failure 1: root cause + fix]

## Evidence
- PR #[N]: [title]
- Files: [list]
- Tests: [pass count]
```

### Step 4: Deduplicate

Before writing a new note, check if a similar note already exists:

```bash
keel memory recall "[topic keywords]"
```

If a similar note exists:
- **Same topic, new info**: Append a "## Update YYYY-MM-DD" section to the existing note
- **Same topic, same info**: Skip — don't duplicate
- **Different topic**: Write a new note

### Step 5: Verify Recall

After writing, verify the note is searchable:

```bash
keel memory recall "[keywords from the note]"
```

The note should appear in the results. If it doesn't, the recall index may need a refresh:

```bash
keel memory recall status
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
| Consolidated notes | Structured knowledge | Agent (this skill) | Session end / user request |
| Working briefs | Task scope + constraints | Agent (brief tools) | Before non-trivial work |
| System map | Project structure | keel automatically | Session start / refresh |

Consolidated notes are the bridge between raw observations (episodic, high-volume, low-signal) and instincts (distilled, low-volume, high-signal). They capture the "what did we learn" that neither observations nor instincts express well.

## Brain Analogy

This skill replicates the hippocampus→neocortex consolidation that happens during sleep:

1. **Hippocampus** (observations): Records episodic memories — what happened, when, in what order. High-fidelity but high-volume.
2. **Neocortex** (consolidated notes): Extracts semantic knowledge — what patterns emerged, what decisions matter, what solutions generalize. Low-volume, high-signal.
3. **Cue-recall** (keel memory recall): Retrieves relevant knowledge when cued by a query. Hybrid lexical (FTS5) + semantic (vector, if available).

The agent (inherited LLM) IS the consolidation mechanism — keel provides the storage and recall, the agent provides the understanding. This is why the skill is agent-driven: keel cannot call the LLM, but the agent IS the LLM.
