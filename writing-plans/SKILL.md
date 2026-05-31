---
name: writing-plans
description: Turn an agreed design into a granular, verifiable implementation plan before touching code. Use when you have a spec or agreed design and the work is more than a trivial single edit — break it into ordered steps, each with its own verification check and the files it touches, so execution can proceed without re-deciding the approach mid-stream. Use when the user says "write a plan", "break this down", "what are the steps", or before starting multi-file or multi-session work. Hands the plan to executing-plans and captures it in the working brief. The front half of execution; brainstorming decides WHAT, this decides the ORDER.
when_to_use: After a design is agreed (often via brainstorming) and before implementation, for any work spanning multiple files, steps, or sessions. Produces an ordered, per-step-verifiable plan captured in the working brief. Hand the plan to executing-plans.
allowed-tools: Read, Grep, Glob, Bash(claude-skills memory:*)
effort: medium
---

# Writing Plans

## Purpose

Convert an agreed design into an ordered plan where every step has a verification
check and names the files it touches — so execution is mechanical, not a series
of fresh decisions. A plan that cannot be verified step by step is a wish list.
`brainstorming` decides *what* to build; this skill decides the *order* and the
*proof* for each step. The plan becomes the working-brief artifact that
`executing-plans` drives against and `reviewer` Stage 1 checks the result against.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. This skill is
the planning expression of **Goal-Driven Execution** — its per-step verify checks
are the "transform vague tasks into verifiable goals" rule applied before any code
exists. **Simplicity First** governs the plan: the fewest steps that deliver the
agreed design, no speculative scaffolding steps for features nobody asked for.

## What a Good Plan Looks Like

Each step is independently verifiable and names its blast radius:

```
1. [Step] → files: [paths] → verify: [the check that proves this step done]
2. [Step] → files: [paths] → verify: [check]
3. [Step] → files: [paths] → verify: [check]
```

- **Ordered by dependency.** A step that needs another step's output comes after
  it. If two steps are independent, say so — that is the signal for
  `dispatching-parallel-agents`.
- **Verifiable per step.** "Add the endpoint" is not a step; "add POST /users
  returning 201 with the created id → verify: integration test asserts 201 + body"
  is. If a step has no check, you cannot know it is done.
- **Files named.** Listing the files each step touches surfaces collisions early
  (two steps editing the same file cannot run in parallel) and feeds the
  preserve-existing-flow trace when a step lands in existing behavior.
- **Smallest shippable slices.** Prefer steps that each leave the system working
  (the "no half-landed states" rule) over one giant step.

## The Practice

### 1. Start from the agreed design, not the raw request

- The input is a decided design (from `brainstorming` or an explicit spec), not an
  open question. If the design is not yet agreed, stop and brainstorm first — you
  cannot plan an undecided target.
- Restate the success criteria the plan must satisfy. These come from the design;
  every step traces back to one of them.

### 2. Decompose into ordered, verifiable steps

- Break the design into the smallest steps that each have a clear check.
- Sequence by real dependency, not by what feels natural to type first.
- Mark independent steps explicitly — they are parallelization candidates.
- For each step touching existing code, note that `preserve-existing-flow` runs
  first during execution.

### 3. Attach verification to each step

- Every step gets the concrete check that proves it: a test, a command, an
  observable output. This is what `executing-plans` runs after each step and what
  the final `completion-gate` reconciles against.

### 4. Capture the plan in the working brief

- Write the plan to the working brief (`claude-skills memory working-brief write`)
  so it survives compaction and a fresh session. A plan that lives only in chat is
  lost at the next compaction and cannot be executed across sessions.

## Hand-Off

A captured plan hands to `executing-plans` for step-by-step execution. Independent
steps hand to `dispatching-parallel-agents`. Steps touching existing behavior hand
to `preserve-existing-flow` first. The completed work is checked against this plan
by `reviewer` Stage 1.

## Anti-Patterns

- A plan with steps that have no verification check — you cannot tell when a step
  is done, so execution drifts.
- One enormous step ("build the feature") that hides all the real decisions.
- Planning an undecided design — if the approach is still open, brainstorm first.
- Leaving the plan in the conversation instead of the working brief, so it dies at
  compaction.
- Ordering steps by typing convenience instead of dependency, producing rework when
  a later step invalidates an earlier one.

## Validation

Methodology skill; the only `claude-skills` call is the working-brief write.
Self-check before handing off to execution: does every step have a verification
check and named files, are the steps ordered by real dependency, and is the plan
captured in the working brief? If any step cannot be verified, it is not yet a
plan step — refine it first.
