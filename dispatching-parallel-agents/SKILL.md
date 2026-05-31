---
name: dispatching-parallel-agents
description: Fan out genuinely independent work to concurrent subagents, and refuse to when the work is not independent. Use when facing 2+ tasks that touch disjoint files, share no output, and cannot cancel each other's work — dispatch them in one batch for concurrency. Use when the user says "do these in parallel", "fan out", "run these at once", or when a plan marks steps independent. The discipline is the four-condition independence test: same-file collisions, output dependencies, or order-sensitive effects mean dispatch sequentially instead. Pairs with subagent-driven-development (context isolation) and executing-plans (which marks the independent steps).
when_to_use: 2+ candidate tasks that might run concurrently. Apply the independence test first — only fan out work that touches disjoint files, shares no output, and is order-independent; otherwise sequence it. Pairs with executing-plans and subagent-driven-development.
allowed-tools: Read, Grep, Glob, Task
effort: medium
---

# Dispatching Parallel Agents

## Purpose

Run independent work concurrently to save wall-clock time — and just as
importantly, *refuse* to parallelize work that is not independent, because
concurrent agents touching the same file or depending on each other's output
produce conflicting, unmergeable, or silently-wrong results. The value of this
skill is as much the gate as the fan-out. Speed from parallelism is real only when
the tasks genuinely cannot interfere.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. Parallel
dispatch is **Goal-Driven Execution** at fan-out scale: each agent gets its own
verifiable goal and check, and results are reconciled in the controller.
**Surgical Changes** is why the independence test matters — two agents editing the
same file both believe they own it, and the merge is neither one's intent.

## The Independence Test (all four must hold)

Fan out only when **every** condition is true. If any fails, dispatch sequentially.

1. **Disjoint files.** No two parallel agents write the same file. Two agents that
   could edit the same file will clobber each other; the last write wins and the
   other's work vanishes.
2. **No output dependency.** No agent needs another agent's result as input. If
   B needs A's output, B must run after A — that is a pipeline, not a fan-out.
3. **Order-independent effects.** The combined result is the same regardless of
   which agent finishes first. Order-sensitive side effects (a shared counter, a
   sequential migration, an append to one log) break under concurrency.
4. **No mutual cancellation.** One agent's finding cannot invalidate another's
   work. If discovering X means Y should not have been done, run X first and decide.

Read-only sweeps across different areas (independent investigations, disjoint
searches) almost always pass. Edits across different modules often pass. Edits to
a shared file, sequential migrations, and "investigate then act on the finding"
almost always fail.

## How To Dispatch

### When the test passes — fan out

- Dispatch all independent tasks in a **single batch** (multiple Task calls in one
  turn) so they run concurrently rather than one-then-the-next.
- Give each agent a self-contained brief: its task, its files, its success
  criteria, its verification check, and the return shape. Subagents do not share
  context — frame each from zero (see `subagent-driven-development`).
- Keep the fan-out to genuinely independent slices. Splitting one coupled task into
  "parallel" pieces that actually share state is the failure this skill prevents.

### Reconcile in the controller

- Collect every result. Re-verify each against its check in the main thread —
  parallel does not mean unverified.
- Integrate in a deterministic order and run the combined suite. Independent edits
  can still surface an integration issue when assembled.

### When the test fails — sequence instead

- Run the tasks one at a time, in dependency order. Use `executing-plans` for the
  step-by-step loop. Say *why* they are sequential (which condition failed) so the
  choice is deliberate, not an oversight.

## Anti-Patterns

- Fanning out edits to the same file because it "feels faster" — the writes
  collide and one agent's work is lost.
- Parallelizing an investigate-then-fix flow where the fix depends on the finding.
- Splitting one tightly-coupled task into pseudo-parallel pieces that share state.
- Dispatching agents one per turn and waiting between them, when they are
  independent and could have gone in one batch.
- Accepting all parallel results without re-verifying each and running the combined
  suite.

## Validation

Methodology skill; uses the Task tool. Self-check before fanning out: can you state,
for each pair of tasks, that they touch disjoint files, share no output, are
order-independent, and cannot cancel each other? If you cannot assert all four for
every pair, sequence the dependent ones instead. After a fan-out, did you re-verify
each result and run the combined suite? Unreconciled parallel results are not done.
