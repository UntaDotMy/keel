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

See `../_shared/common-discipline.md` § Code Implementation Discipline. Delegation does
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

- Context inheritance varies by host and invocation. Give the subagent the task,
  relevant files, success criteria, and exact verification check explicitly even
  when the host can fork conversation context. A vague hand-off produces vague work.
- Specify the return shape: what the subagent should report back (the diff, the
  test output, the decision), not a human-facing essay.

### 2. Dispatch and let it work in isolation

- Use the Task tool with the right specialist subagent type when one fits.
- The subagent works in its own window; the controller does not micromanage it.

#### Provider Model Recommendations
- **Google (Antigravity `/boost`)**:
  - Light Tasks / Implementers / Explorers: `gemini-3.7-flash` (high reasoning).
  - Critics / Architecture / Planners: Gemini Pro / Thinking (AGI / deep reasoning mode) or `gemini-3.8-flash` (high reasoning).
- **OpenAI (Codex)**:
  - Light Tasks: `gpt-5.6-luna` (max reasoning).
  - Critics / Planners: `gpt-6-Astra` (low reasoning).
- **Anthropic (Claude Code)**:
  - Light Tasks: `claude-3-5-haiku`.
  - Critics / Planners: `claude-3-7-sonnet` (high effort) or `claude-3-opus`.

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

## Worktree Isolation — Mandatory For File-Editing Subagents

Every subagent that edits files MUST work in its own isolated git worktree. No exceptions. Two subagents sharing a working tree is a collision waiting to happen — the last write wins, the other's work vanishes, and the orchestrator cannot tell which diff belongs to whom.

### Before Dispatching

1. **Create a worktree per subagent.** `git worktree add ../wt-<task-name> -b subagent/<task-name>` creates an isolated checkout on its own branch. The subagent works there and only there.
2. **Pass the worktree path to the subagent.** The subagent's prompt must state the worktree path as its working directory. The subagent does NOT touch the main tree.
3. **One worktree per task.** If two tasks are independent enough to parallelize, they get two worktrees. If they are not independent enough for separate worktrees, they are not independent enough to parallelize — sequence them.

### After Subagent Returns

4. **Orchestrator reviews the diff.** `git -C ../wt-<task-name> diff main` shows the subagent's work. The orchestrator reads it, runs the verification check, and confirms it matches the task scope. The subagent's claim of "done" is a hypothesis until the orchestrator verifies the diff.
5. **Merge only after review passes.** If the diff is correct, merge the subagent's branch: `git merge subagent/<task-name>`. If it is wrong, re-dispatch with the correction or fix in-thread.
6. **Clean up the worktree.** `git worktree remove ../wt-<task-name>` after merge. Keep the branch — this repo never deletes branches.

### When Worktrees Are Not Needed

- Read-only subagents (explore, search, research) do not need worktrees — they don't write files.
- Subagents that only return text (analysis, plan, decision) do not need worktrees.
- The test: if the subagent calls Edit, Write, or Bash with a write command, it needs a worktree.

### Anti-Patterns

- Letting two subagents work in the same tree because "they touch different files" — the index is shared, and a `git add` from one clobbers the other's staging.
- Merging a subagent's branch without reading the diff — the subagent may have added scope creep or broken a pattern.
- Skipping worktree cleanup — stale worktrees accumulate and confuse future sessions.

## Relationship To Parallelism

Delegation is about *context isolation*; parallelism is about *concurrency*. A task
can be delegated and run alone, or delegated and run alongside others. When the
plan marks tasks **independent** (disjoint files, no shared output), hand them to
`dispatching-parallel-agents` to fan out. When tasks depend on each other, delegate
them sequentially even if each is isolated. Model choice per delegate (frontier vs cheap) is host configuration; keel does not route models: state it in the brief.

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
