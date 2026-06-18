---
name: running-a-sprint
description: Run confirmed user stories as a Scrum-style sprint loop — turn the agreed stories into a sprint backlog, drive each one implement → verify-against-its-Gherkin → review until it meets Definition of Done, and LOOP until every story is Done before presenting the increment. Use when a request has multiple user stories or any multi-step build that must finish completely, not partially. Backed by `keel sprint` (durable per-story state) so the loop survives compaction. The orchestration layer above writing-user-stories (which produces the backlog) and the implementation skills (which do each story).
when_to_use: A confirmed multi-story requirement, or any build where "loop until all of it is actually done" matters — features with several acceptance criteria, multi-part asks, work that must not be presented half-finished. Runs after writing-user-stories has produced and confirmed the backlog; hands each story to TDD/the lifecycle skills and gates the close on every story meeting Definition of Done.
allowed-tools: Read, Grep, Glob, Bash(keel sprint:*), Bash(keel memory:*), Bash(keel review:*)
effort: medium
---

# Running a Sprint

## Purpose

User stories say *what*. A sprint is the loop that gets *all of it actually done*.
This skill takes the confirmed story backlog and drives it the Scrum way: each
story is worked, verified against its own acceptance criteria, and reviewed; the
sprint does not close while any story is short of **Definition of Done**. The point
is the loop — "loop until all okay" — so nothing half-built is ever presented as
finished.

It orchestrates surfaces that already exist rather than reinventing them:
`writing-user-stories` produces and confirms the backlog; `test-driven-development`
and the lifecycle skills implement each story; `reviewer` is the per-story and
final quality gate; `keel sprint` is the durable ledger that makes the
loop resumable after compaction.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. This skill is
**Goal-Driven Execution** at the iteration scale: the sprint backlog is the set of
verifiable goals, and the review gate is the closure check. **Simplicity First**
applies to the sprint too — only the confirmed stories are in scope; do not invent
backlog items the stories did not ask for.

## Scrum, mapped to this harness

| Scrum concept | Here |
|---|---|
| Product/Sprint backlog | The confirmed user stories (`writing-user-stories`), loaded as sprint items via `keel sprint plan` |
| Sprint goal | The working brief's request + acceptance criteria |
| The increment | The working, reviewed code at sprint close |
| Definition of Done | Per story: its Gherkin Given/When/Then acceptance scenarios pass **and** the change passed `reviewer` (and the release-ladder rungs that apply) |
| Daily scrum | `keel sprint status` — what's done, what's open, what's blocked |
| Sprint review | `keel sprint review` — the fail-closed gate: complete only when every story is Done |
| Sprint retrospective | Lessons captured to memory at close |

This is a single active sprint per workspace — a focused loop to finish the agreed
scope, not a multi-sprint release planner.

## The Loop

### 1. Plan the sprint (build the backlog)

- Start from the **confirmed** stories (`writing-user-stories` already ran and the
  user ratified them). Do not plan a sprint from unconfirmed requirements.
- Load each story as a backlog item:
  `keel sprint plan --story "As a <role>, I want <goal>, so that <benefit>"`.
  Each item starts in `todo`.
- Confirm the backlog mirrors the stories 1:1 — one item per story, nothing extra.

### 2. Work each story to Definition of Done

For each story, in priority order:

- Mark it active: `keel sprint advance --id <id> --state in-progress`.
- `preserve-existing-flow` first if it touches existing behavior.
- Implement it — prefer `test-driven-development`, turning the story's
  Given/When/Then scenarios into the failing tests, then making them pass. Hand
  domain work to the relevant lifecycle skill.
- **Verify against the story's own acceptance criteria**, not a general "looks
  done". Every Gherkin scenario must actually pass.
- Run `reviewer` on the change (Stage 1 reconciles it against this story; Stage 2
  checks quality).
- Only when the acceptance scenarios pass **and** review is clean:
  `keel sprint advance --id <id> --state done`.
- If something blocks it, `--state blocked --note "<why>"` and move on — a blocked
  story keeps the sprint open (it is never silently counted as done).

### 3. Review the sprint — the loop gate

- Run `keel sprint review`. It is **fail-closed**: it reports COMPLETE
  (exit 0) only when every story is Done, and NOT COMPLETE (non-zero) while any
  story is `todo`, `in-progress`, or `blocked`, naming the open ones.
- **If not complete, loop back to step 2** for the open stories. Do not present the
  work as finished. This is the discipline the whole skill exists for: re-entering
  the loop is the default, closing is the exception that requires every story Done.
- Re-run review after each pass until it reports COMPLETE.

### 4. Close: increment + retrospective

- When review is COMPLETE, the increment is the reviewed, passing code. Summarize
  what shipped (story by story).
- Capture a short retrospective to memory — what worked, what slowed the loop, any
  reusable lesson (`keel memory ...`, e.g. a research-cache or instinct
  note). This is how the next sprint starts ahead.
- Reconcile against the working brief one last time before the final answer.

## Hand-Off / Relationships

- **Upstream:** `writing-user-stories` — produces and confirms the backlog this
  sprint runs. Never run a sprint on unconfirmed stories.
- **Per story:** `test-driven-development`, `preserve-existing-flow`, the lifecycle
  skills (implementation); `reviewer` (per-story and final gate).
- **Closeout:** `finishing-a-development-branch` when the sprint's increment is a
  branch to merge.

## Anti-Patterns

- Presenting the work as done while `sprint review` still reports open stories —
  the exact failure this skill prevents.
- Treating a `blocked` story as done to close the sprint. A blocker is surfaced and
  resolved or explicitly accepted by the user, never hidden.
- Marking a story `done` on "it compiles" rather than its acceptance scenarios
  passing and a clean review.
- Planning backlog items no confirmed story asked for (scope creep), or dropping a
  confirmed story (scope loss).
- Running the loop only in chat state — use `keel sprint` so the
  "what's still open" fact survives compaction.

## Validation

Drive the loop with `keel sprint status` (progress) and
`keel sprint review` (the gate). The sprint is done only when `review`
reports COMPLETE — every story at Definition of Done. Self-check before the final
answer: does `sprint review` exit COMPLETE, is every story's acceptance criteria
proven, did each non-trivial story pass `reviewer`, and is a retro captured? If any
answer is no, loop — do not close.
