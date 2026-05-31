---
name: writing-skills
description: Author and revise skills with evidence that the prose actually changes behavior, not just that it reads well. Use when creating a new skill, editing an existing SKILL.md, or verifying a skill works — apply TDD to the instructions themselves. Iron law — no skill claim without a failing test first — run the target behavior past a fresh subagent WITHOUT the skill under stacked pressure to capture how it fails and what it rationalizes, write the minimum skill prose that targets those exact failures, then re-test under pressure until the subagent makes the right call and cites the skill. Pairs with skill-lint (the structural gate) and reviewer (the fail-closed verdict).
when_to_use: Creating, editing, or validating any SKILL.md. Use the RED-GREEN-REFACTOR-on-prose loop and the subagent pressure-test before claiming a skill works. Run claude-skills skill-lint for the structural gate; this skill is the behavioral gate above it.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(claude-skills skill-lint:*), Task
effort: high
---

# Writing Skills

## Purpose

Apply test-driven development to skill *prose*. A skill is an instruction meant
to change how the model behaves under pressure. Reading well is not evidence it
works — the only evidence is that a model which would otherwise make the wrong
call makes the right one *because* of the skill. This skill produces that
evidence. `claude-skills skill-lint` is the structural gate (does the skill
trigger at all); this skill is the behavioral gate above it (does the skill
actually change the decision).

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. This skill is
**Goal-Driven Execution** applied to instructions: "write the failing test, then
make it pass" becomes "prove the model fails without the skill, then write the
minimum prose that makes it pass." **Simplicity First** governs the prose — the
shortest instruction that closes the observed failure wins; speculative rules
that no scenario exercised are cut.

## The Iron Law

**No skill claim without a failing test first.** Before you write or trust a
skill, you must have watched a fresh subagent make the wrong call on the exact
situation the skill targets — *without* the skill loaded. If you cannot make it
fail, you do not yet know what the skill is for, and any prose you write is a
guess. See `references/10-testing-skills-with-subagents.md` for the full method.

## The Loop (RED → GREEN → REFACTOR on prose)

### RED — prove the failure without the skill

- Write a pressure scenario that stacks 3+ real pressures (time, sunk cost,
  authority, money, a plausible-sounding shortcut). The scenario must have one
  correct action the skill would compel.
- Dispatch a fresh subagent (Task tool) the scenario *without* the skill loaded.
  Capture its decision and — critically — the **rationalizations** it invents to
  justify the wrong call ("the tests probably still pass", "this is just a small
  change", "the user is in a hurry so I'll skip the review").
- If the subagent already does the right thing without the skill, the skill is
  unnecessary or the scenario is too weak. Strengthen the pressure or stop.

### GREEN — minimum prose that targets the failure

- Write the least skill text that would have flipped that decision. Target the
  specific rationalizations you captured — name them and answer them, because the
  model will reach for those exact excuses again.
- Do not write the general essay on the topic. Write only what closes the
  observed failure. Unexercised prose is the YAGNI violation of skill authoring.

### REFACTOR — close loopholes, re-test under pressure

- Re-run the scenario *with* the new skill loaded. A pass means the subagent
  makes the right call AND cites the skill's reasoning, under maximum pressure.
- When it passes, look for the next loophole: a sibling scenario with different
  pressures that the prose does not yet cover. Each new failure mode is a new RED.
- Stop when the skill holds across the pressure variants and reads clean. Then run
  `claude-skills skill-lint` for the structural gate (name match, description +
  when_to_use ≤ 1536 chars, no dangling references).

## Writing Prose That Survives Rationalization

The model talks itself out of skills. Write to defeat that:

- **Name the rationalization, then answer it.** A "Red Flags" table that lists the
  exact excuse ("I'll just code this quickly") next to the reality is more durable
  than an abstract rule. See `using-claude-core/SKILL.md` for the pattern.
- **State the iron law inline.** "This is not negotiable. You cannot rationalize
  your way out of it" closes the wiggle room that a softer phrasing leaves open.
- **Make the trigger concrete in `description`.** The matcher fires on the
  description — phrase it as the situation ("Use when…"), not the topic.
- **Prefer one non-negotiable rule over five soft suggestions.** A skill that says
  five things weakly changes nothing; one thing forcefully changes the decision.

## Anti-Patterns

- Writing the skill first and never testing whether a model fails without it — you
  have prose, not evidence. This is the exact failure the iron law names.
- A scenario with no real pressure — the subagent does the right thing anyway and
  you "prove" a skill that does nothing.
- Testing with the skill already in context, so you never saw RED.
- Writing the encyclopedic version of the topic instead of the minimum that closes
  the observed failure.
- Shipping on a clean `skill-lint` alone — that proves the skill *triggers*, not
  that it *works*. Structural pass is necessary, not sufficient.

## Validation

Two gates, in order. Structural: `claude-skills skill-lint` must pass (the skill
triggers and has no dangling references). Behavioral: you must be able to show one
scenario where a subagent makes the wrong call without the skill and the right
call — citing the skill — with it. Self-check before claiming a skill done: do you
have a captured RED transcript and a passing re-test under the same pressure? If
the skill was never seen to fail without it, you have not tested it.
