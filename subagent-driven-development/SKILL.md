---
name: subagent-driven-development
description: Execute independent plan tasks through fresh-context subagents to preserve the controller's context window. Use when a plan has tasks that can be handed off whole — delegate each task with its verification check to a subagent, integrate the returned result, and re-verify in the main thread. Use when the controller context is filling up, when a task is self-contained, or when the user says "delegate this", "use a subagent", or "farm this out". Keeps the orchestrating thread lean while specialist work happens in isolation. Pairs with executing-plans (the loop), dispatching-parallel-agents (when tasks are also independent), and reviewer (the gate on integrated results).
when_to_use: Executing plan tasks that are self-contained enough to hand to a fresh-context subagent, especially when preserving the controller's context matters. Delegate the task with its check, integrate, re-verify. Pairs with executing-plans and reviewer.
allowed-tools: Read, Grep, Glob, Edit, Bash, Task
effort: high
---

# Subagent-Driven Development

## Purpose

Run self-contained tasks in fresh-context subagents so the orchestrating thread
stays lean and each task starts from a clean window. The failure this prevents is
the controller burning its whole context window on the details of every task, then
losing the thread of the overall plan. The controller keeps the plan and the
integration; subagents do the deep work and return a result.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. Delegation does
not delegate responsibility: a returned result still goes through **Goal-Driven
Execution** (re-verify against the task's check in the main thread) and **Surgical
Changes** (the integrated diff traces to the request). A subagent's claim of "done"
is a hypothesis until the controller verifies it.

## When To Delegate vs Stay In-Thread

Delegate to a subagent when:

- The task is self-contained — it has clear inputs, a clear deliverable, and a
  verification check it can run itself.
- The controller's context is filling and the task's details do not need to stay
  resident afterward.
- The task benefits from a specialist subagent (the `.claude/agents/` roster) with
  its own framing.

Stay in the main thread when:

- The task is small enough that delegation overhead exceeds the work.
- The task is tightly coupled to decisions still being made in the controller.
- You need the task's details resident for the next decision.

## The Delegation Loop

### 1. Frame the task for a fresh context

- A subagent does not inherit the conversation. Give it the task, the relevant
  files, the success criteria, and the exact verification check — everything it
  needs to start from zero. A vague hand-off produces vague work.
- Specify the return shape: what the subagent should report back (the diff, the
  test output, the decision), not a human-facing essay.

### 2. Dispatch and let it work in isolation

- Use the Task tool with the right specialist subagent type when one fits.
- The subagent works in its own window; the controller does not micromanage it.

### 3. Integrate and re-verify in the main thread

- Treat the returned result as a proposal. Read the diff, run the task's
  verification check yourself, and confirm it does not break previously-passing
  checks. "The subagent said it passed" is not evidence — re-run it.
- If the result is wrong or incomplete, re-dispatch with the specific correction,
  or pull the task back in-thread. Do not paper over a bad result.

### Subagent Verification Checklist

Before marking ANY subagent task as complete, the main thread MUST verify ALL of
the following. A subagent's claim of "done" is a HYPOTHESIS until this checklist
passes.

1. **Compiles/Passes** — The subagent's output compiles (or equivalent type-check
   passes) and its own verification check passes when re-run in the main thread.
   Do not trust the subagent's report — re-run the check yourself.
2. **Scope Match** — The output matches the requested scope. Nothing extra was
   added (scope creep), nothing requested was missed (incomplete delivery). Compare
   the diff against the task description line by line.
3. **Pattern Match** — The code follows existing codebase patterns. It uses the
   same naming conventions, error handling style, module structure, and
   idiomatic patterns as the surrounding code. A subagent that writes correct but
   stylistically alien code creates a maintenance burden.
4. **No Regression** — The change does not break existing functionality. Run the
   project's existing test suite (or the narrowest relevant subset) and confirm
   all previously-passing tests still pass.
5. **One-to-One Communication** — If ANY ambiguity exists about scope, correctness,
   or intent, re-invoke the subagent with a follow-up question using the task
   continuation `session_id`. Do not guess what the subagent meant. Do not assume
   and fill in gaps yourself. The subagent has the context; use it.

Never trust a subagent's claim of "done" without verification. The subagent's
output is a HYPOTHESIS until the main thread verifies it. A returned result that
passes this checklist is verified done; one that fails any item goes back to the
subagent with a specific correction.

### 4. Record progress

- Update the working brief / ledger with the integrated step so it survives
  compaction, the same as any executing-plans step.

## Relationship To Parallelism

Delegation is about *context isolation*; parallelism is about *concurrency*. A task
can be delegated and run alone, or delegated and run alongside others. When the
plan marks tasks **independent** (disjoint files, no shared output), hand them to
`dispatching-parallel-agents` to fan out. When tasks depend on each other, delegate
them sequentially even if each is isolated.

## Anti-Patterns

- Trusting a subagent's "done" without re-verifying in the main thread.
- Delegating a task without its verification check, so neither the subagent nor the
  controller can prove it worked.
- Farming out tightly-coupled tasks that need shared, evolving context — they
  produce conflicting work.
- Delegating trivial tasks where the framing cost exceeds the work.
- Letting the integrated result drift from the plan because no one re-checked it
  against the success criteria.

## Validation

Methodology skill; uses the Task tool and `keel memory` for progress.
Self-check before accepting a delegated result: did you read the returned diff, run
the task's verification check yourself in the main thread, and confirm earlier
checks still pass? If you accepted the subagent's claim without re-running the
check, the task is reported done, not verified done.
