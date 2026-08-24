---
name: compounding-knowledge
description: Capture each solved problem as a durable, discoverable knowledge artifact so future work starts ahead instead of re-deriving it. Use after solving a non-trivial problem, debugging a tricky failure, or making a reusable decision — write a categorized solution note (problem, root cause, solution, evidence) to a durable location, dedupe against existing notes, and wire it into the project's discoverability pointers (CLAUDE.md / AGENTS.md / SYSTEM_MAP) so the next agent actually finds it. Use when the user says "capture this", "remember how we solved this", "write this up", or after a hard-won fix worth keeping. The deliberate counterpart to the automatic learn loop; this is the human-readable, project-local knowledge base.
when_to_use: After solving a non-trivial problem or making a reusable decision worth keeping. Write a categorized, deduped solution note and make it discoverable from the project's pointer files. Complements the automatic learn loop (which generates skills from behavior) with durable human-readable artifacts.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(keel memory:*), Bash(keel learn:*)
user-invocable: false
effort: medium
---

# Compounding Knowledge

## Purpose

Make each solved problem reduce the cost of the next one. The failure this prevents
is solving something hard, moving on, and having the next agent (or the same one
after compaction) re-derive the identical solution from scratch — paying the full
cost again. Compounding means the knowledge lands in a durable, *discoverable* place
so future work starts from the answer. This skill is the deliberate, human-readable
counterpart to keel's automatic `learn` loop: `learn` promotes recurring
*behavior* into skills statistically; this captures a *specific solution* on demand
as a readable artifact and wires it into discovery.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. Capturing
knowledge is **Goal-Driven Execution** closing the loop — the artifact records the
verified outcome and the evidence that proved it. **No Duplication** is the core
discipline here: dedupe against existing notes before writing, and consolidate
rather than spawn a near-identical fourth note on the same topic.

## What Is Worth Capturing

Capture when the knowledge is reusable and was expensive to obtain. Good candidates:

- A non-obvious root cause and its fix (especially one that took real tracing).
- A reusable decision with a rationale ("we use X over Y here because Z").
- A workaround for an external constraint (a platform quirk, an API limitation).
- A repeatable procedure that is easy to get wrong.

Skip the trivial and the obvious. A note for "renamed a variable" is noise that
dilutes the store and makes the real knowledge harder to find. Match the artifact
to the cost of re-deriving it.

## The Practice

### 1. Check for an existing note first

- Before writing, search the knowledge store and memory (`keel memory
  recall <terms>`) for an existing note on this topic. If one exists, **extend or
  correct it** rather than adding a parallel note — duplication is the failure mode
  that makes a knowledge base untrustworthy.

### 2. Write a structured, categorized note

- Record, concisely: the **problem/symptom**, the **root cause** (with file:line if
  it is a code issue), the **solution** that worked, and the **evidence** that
  proved it (the test, the command output, the trace). A note without evidence is a
  guess preserved for posterity.
- Categorize it (by module, problem type, or domain) so it is findable by topic.
- Tag it with enough keywords that a future search on the *symptom* surfaces it —
  the next person hits the symptom, not your title.

### 3. Make it discoverable — the step that makes it compound

A note no one finds compounds nothing. Wire it into the project's discovery layer:

- Add a pointer in the project's `CLAUDE.md` / `AGENTS.md` (or the relevant
  `references/` index) so an agent reading the project guide is routed to the note.
- Refresh `SYSTEM_MAP` when the note records a structural fact, and persist the note
  reference in the working brief for the current workstream.
- The test: would a fresh agent, starting cold on a related task, be led to this
  note by the files it reads first? If not, the discoverability step is incomplete.

### 4. Keep the store healthy over time

- When a note goes stale (the code changed, the decision was reversed), update or
  retire it. A knowledge base full of wrong-but-confident notes is worse than none.
- Consolidate notes that have drifted into near-duplicates.

## Relationship To The Automatic Learn Loop

`keel learn` observes behavior across sessions and promotes recurring
patterns into generated skills automatically (statistical thresholds, no manual
step). This skill is the manual, surgical complement: it captures *one specific
hard-won solution* as a readable artifact the moment it is worth keeping, and wires
it into project-local discovery. Use the learn loop for emergent conventions; use
this when you just solved something you (or a teammate) will clearly need again.

## Anti-Patterns

- Solving something hard and capturing nothing — the next agent pays full price.
- Writing a note but not wiring it into any discovery pointer, so it is never found.
- A note with the fix but no evidence or root cause — it cannot be trusted or
  reapplied safely.
- Spawning a fourth near-duplicate note instead of consolidating the existing three.
- Capturing trivia, drowning the genuinely valuable notes in noise.
- Letting stale notes accumulate until the store misleads more than it helps.

## Validation

Methodology skill; uses `keel memory` for recall and the working brief.
Self-check before claiming knowledge captured: did you dedupe against existing
notes, write problem + root cause + solution + evidence, categorize and tag it for
symptom-based search, and wire a discovery pointer so a cold-start agent would find
it? If the note exists but nothing routes a future reader to it, it will not
compound — finish the discoverability step.
