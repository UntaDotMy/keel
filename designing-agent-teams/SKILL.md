---
name: designing-agent-teams
description: Design a coordinated multi-agent team and the skills its agents need, from a one-line domain description. Use when a task is too big or too multi-disciplinary for a single agent and you need to decide how to split it into specialist agents that hand off cleanly — choose a team-architecture pattern (pipeline, fan-out/fan-in, expert pool, producer-reviewer, supervisor, hierarchical), define each agent's role, inputs, outputs, and verification, and wire the orchestration. Use when the user says "design an agent team", "how should I split this across agents", "set up a multi-agent workflow", or "build a harness for X". Pairs with dispatching-parallel-agents (the execution gate) and subagent-driven-development (the per-agent loop).
when_to_use: Designing how to decompose a domain or large task into a coordinated team of specialist agents with clean hand-offs. Pick an architecture pattern, define roles and contracts, wire orchestration. Hands execution to dispatching-parallel-agents and subagent-driven-development.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*)
effort: high
---

# Designing Agent Teams

## Purpose

Turn a domain or oversized task into a coordinated team of specialist agents with
defined roles, clean hand-off contracts, and an orchestration pattern that fits
the work. The failure this prevents is either cramming a multi-disciplinary job
into one overloaded agent (which loses focus and context) or fanning out agents
with no contract between them (which produces conflicting, unmergeable work). This
skill decides the *shape* of the team; `dispatching-parallel-agents` gates which
parts may run concurrently, and `subagent-driven-development` runs each agent's
loop.

## Code Implementation Discipline

See `../_shared/common-discipline.md` § Code Implementation Discipline. Team design is
**Goal-Driven Execution** at the orchestration layer — each agent gets a verifiable
goal and an explicit output contract before any work starts. **Simplicity First**
governs the team size: the fewest agents that cover the work with clean seams, not
a sprawling org chart. A single capable agent is the right "team" more often than
not — only split when the work genuinely has separable concerns.

## Step 1 — Decide whether a team is even warranted

A team adds coordination cost. Justify it before designing one. Split into multiple
agents only when at least one holds:

- The work spans **distinct disciplines** (e.g. backend + design + security review)
  that benefit from focused, separate framing.
- The work has **independent parallelizable branches** that would otherwise run
  serially and slowly.
- The controller's **context would overflow** if it held every task's detail.
- A task needs an **adversarial second perspective** (a producer and a separate
  reviewer) to be trustworthy.

If none hold, use one agent (or stay in the main thread) and stop here. Over-teaming
a simple task is the YAGNI violation of orchestration.

## Step 2 — Choose the architecture pattern

Match the pattern to the work's dependency structure:

| Pattern | Use when | How it works |
|---|---|---|
| **Pipeline** | Stages strictly depend on the prior stage's output | Sequential agents, each consuming the previous output; one ordering |
| **Fan-out / Fan-in** | Several independent sub-tasks, then one synthesis | Parallel workers, then a synthesizer merges results (the most common useful shape) |
| **Expert Pool** | Input type determines who should handle it | A router dispatches each item to the right specialist by type |
| **Producer–Reviewer** | Output must be validated by an independent perspective | A generator produces, a separate reviewer validates; bounded retry loop (cap 2–3) |
| **Supervisor** | Task allocation must adapt at runtime | A central agent reallocates work dynamically as results arrive |
| **Hierarchical** | A large task decomposes recursively | A lead decomposes into sub-leads; cap recursion at ~2 levels to stay legible |

Pick one as the spine. Complex jobs nest patterns (e.g. a pipeline whose middle
stage is a fan-out), but name the top-level spine first.

## Step 3 — Define each agent's contract

For every agent in the team, specify all four — an agent without a contract is a
coordination bug waiting to happen:

- **Role**: the one responsibility it owns. If you cannot state it in a sentence,
  the split is wrong.
- **Inputs**: exactly what it receives (and from which upstream agent).
- **Output**: the concrete artifact it returns and its shape — a file path, a
  structured result, a verdict. Subagents return data, not human prose.
- **Verification**: the check that proves its output is good before a downstream
  agent consumes it.

Pass data between agents through durable artifacts (files / the working brief), not
just chat — hand-offs must survive a context window ending.

## Step 4 — Wire orchestration and failure handling

- Decide the execution mode: parallel fan-out (apply the `dispatching-parallel-agents`
  four-condition independence test before parallelizing anything), sequential
  pipeline, or hybrid.
- Define what happens on failure: a single agent failing, a reviewer rejecting
  output past the retry cap, a timeout, or two agents producing conflicting results.
  Name the fallback for each rather than discovering it live.
- Capture the team design in the working brief so the orchestration survives
  compaction and a fresh session can resume it.

## Anti-Patterns

- Designing a team for a task one focused agent could do — coordination overhead
  with no benefit.
- Agents with overlapping responsibilities, so two of them edit the same artifact
  and collide (the parallel-dispatch independence test exists for this).
- Hand-offs over chat only, with no durable artifact — the next stage starts blind
  after a compaction.
- A producer–reviewer loop with no retry cap, which can spin indefinitely.
- Unbounded hierarchical decomposition that no one can hold in mind.
- No defined failure path, so one agent's error silently corrupts the whole run.

## Validation

Methodology skill; captures the design via `keel memory working-brief`.
Self-check before executing the team: is a team actually warranted, is there a named
top-level pattern, does every agent have role + inputs + output + verification, are
parallel branches genuinely independent, and is there a defined failure path for
each coordination point? If any agent lacks an output contract, the team will drift
— define it before dispatching.
