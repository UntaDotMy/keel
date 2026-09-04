---
name: executing-plans
description: Execute a written implementation plan step by step, verifying each step before moving to the next. Use when you have a captured plan (from writing-plans or a spec) and are ready to implement — work one step at a time, run that step's verification check, and stop on a failed check instead of pushing ahead. Use when the user says "execute the plan", "start implementing", "work through the steps", or hands you a plan. Keeps the working brief and ledger current so progress survives compaction. The back half of planning; writing-plans decides the steps, this drives them to done.
when_to_use: Implementing against a captured plan, one verifiable step at a time. Run each step's check before advancing; record progress so it survives compaction. Pairs with writing-plans (the input), subagent-driven-development (delegating independent steps), and reviewer (the final gate).
allowed-tools: Read, Grep, Glob, Edit, Write, Bash
effort: high
---

# Executing Plans

## Purpose

Drive a written plan to completion one verifiable step at a time, proving each
step before starting the next. The failure this prevents is racing through all
the steps, then discovering at the end that step 2 was wrong and steps 3-6 built
on it. `writing-plans` produced the ordered, per-step-verifiable plan; this skill
executes it with a check after every step and durable progress that survives
compaction.

## Code Implementation Discipline

See `../_shared/common-discipline.md` § Code Implementation Discipline. This skill
runs the **Goal-Driven Execution** loop the plan set up — each step's verify check
is the goal, and "loop until verified" is the per-step rule. **Surgical Changes**
governs every edit: implement the step as written, do not expand scope mid-plan;
a discovered new requirement is a plan change, recorded, not a silent detour.

## The Execution Loop

For each step, in plan order:

### 1. Load the step and its check

- Read the step, the files it names, and its verification check. If the step
  touches existing behavior, invoke `preserve-existing-flow` before editing.

### 2. Implement the minimum for that step

- Make the change the step describes — and only that change. Resist pulling the
  next step's work forward "while you're in the file." One step at a time is what
  makes a failed check diagnosable.
- If the step has a testable behavior, drive it with `test-driven-development`
  (failing test → minimum code → refactor) rather than ad-hoc edits.

### 3. Run the step's verification check

- Run the exact check the plan named. Confirm it passes and that previously-passing
  checks still pass. A green step check with a broken earlier check is not progress.

### 4. Record progress, then advance

- Update the working brief / ledger so the completed step survives compaction
  (`keel memory working-brief write`, or the workflow ledger if one is
  open). Then move to the next step.

## When A Step Fails Its Check

- **Stop. Do not start the next step.** Pushing ahead builds on an unproven base.
- Diagnose with `systematic-debugging`: trace to the real cause, do not patch the
  symptom to force the check green.
- If the failure reveals the *plan* was wrong (a missing step, wrong order, a bad
  assumption), that is a plan change — revise the plan in the working brief and say
  so, rather than silently improvising around it.
- After two failed attempts on the same step with no new hypothesis, stop and
  re-trace from the symptom (the common-discipline two-try rule).

## Delegation and Parallelism

- Steps the plan marked **independent** (disjoint files, no shared output) are
  candidates for `dispatching-parallel-agents` — fan them out only if they truly
  cannot collide.
- Steps that benefit from an isolated context window hand to
  `subagent-driven-development` — delegate the step with its check, integrate the
  result, verify again in the main thread.

## Closing Out

When the last step's check passes:

- Run the full relevant suite, not just the last step's check.
- Run `keel memory completion-gate check` to reconcile the result against
  the plan's success criteria — it points at any requirement with no evidence yet.
- Route a non-trivial result through `reviewer`, then `finishing-a-development-branch`
  for the merge/PR closeout.

## Anti-Patterns

- Implementing all steps, then verifying once at the end — a mid-plan error
  contaminates everything after it and is far harder to localize.
- Skipping a step's check because "it obviously works."
- Forcing a check green by weakening it instead of fixing the code (a `reviewer`
  fail condition).
- Silently changing the plan when a step exposes a flaw, instead of recording the
  plan change.
- Letting progress live only in context, so a compaction loses which steps are done.

## Validation

Methodology skill; calls into `keel memory` for progress and the
completion gate. Self-check before claiming the plan complete: did every step pass
its own verification check in order, does the full suite pass, and does the
completion gate reconcile against the plan's success criteria? If any step was
skipped or its check never run, the plan is not executed — it is partially
attempted.
