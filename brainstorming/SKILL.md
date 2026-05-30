---
name: brainstorming
description: Socratic design exploration before implementation. Use when a request is open-ended, the approach is not obvious, multiple designs are plausible, or the user says "how should we", "what are the options", "help me think through", or "design". Refines a vague idea into a concrete, agreed design through questions and trade-off comparison before any code is written — and captures the decision so it survives compaction. The generative counterpart to Think-Before-Coding (which guards against guessing); this skill produces the design to commit to.
when_to_use: Open-ended or ambiguous work where the design is not yet decided — new features, architecture choices, "how should we approach X". Stop before coding and explore. Hand the agreed design to the implementation skills (TDD, the lifecycle skills).
allowed-tools: Read, Grep, Glob, Bash(claude-skills memory:*)
effort: medium
---

# Brainstorming

## Purpose

Turn a vague or open-ended request into a concrete design the user has agreed to,
*before* writing code. `Think-Before-Coding` (in `_shared/common-discipline.md`)
is the defensive rule — do not guess, surface tradeoffs, ask when unclear. This
skill is the generative practice that satisfies it: a short structured
exploration that ends in a decision and a written design, so implementation
starts from an agreed target instead of an assumption.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. This skill is
the front half of **Goal-Driven Execution**: it produces the verifiable goal and
success criteria that the implementation loop then drives toward. **Simplicity
First** governs the options you propose — bias toward the least complex design
that meets the need, and say so.

## When To Use It

Invoke brainstorming when any of these hold:

- The request is open-ended ("how should we…", "what's the best way to…").
- More than one reasonable design exists and the choice has consequences.
- The requirements are not yet sharp enough to write a failing test against.
- The change is large enough that committing to the wrong shape is expensive.

Skip it for mechanical or unambiguous work (a rename, a known one-line fix, a
clearly specified change). Brainstorming a trivial task wastes the user's time —
match the ceremony to the stakes.

## The Practice

### 1. Understand before proposing

- Restate the problem in your own words and confirm it. A surprising amount of
  wasted work comes from solving a subtly different problem than the one asked.
- Ask the questions that actually change the design: constraints, scale, existing
  systems it must fit, what "good" looks like, what is explicitly out of scope.
- Ask one focused round at a time, not a 20-question interrogation. Each question
  should be one whose answer changes what you would build.

### 2. Explore options, with trade-offs

- Put forward 2-3 genuinely different approaches, not one dressed three ways.
- For each: how it works, what it costs, what it buys, where it breaks. Name the
  failure modes honestly — the cheapest option usually has the sharpest edge.
- Recommend one and say why. Brainstorming is not neutral menu-presentation; you
  are the expert in the room. But hold the recommendation loosely if the user
  pushes back with context you did not have.

### 3. Converge on a decision

- Drive toward a single agreed design. An exploration that ends in "so, lots of
  options!" failed — the point is to *decide*.
- Make the chosen design concrete: the shape of the change, the pieces involved,
  the success criteria, what is deferred.

### 4. Capture the design

- Write the decision down so it survives compaction and a fresh session. Use the
  working brief (`claude-skills memory working-brief write`) to record the agreed
  approach, the success criteria, and the files expected to change — *before*
  implementation starts.
- The captured design is what `reviewer` later checks the implementation against
  (Stage 1, spec compliance). A decision that lives only in the chat cannot be
  reviewed against.

## Hand-Off

When the design is agreed and captured, brainstorming is done. Hand to:

- `test-driven-development` to drive the build test-first against the success
  criteria.
- the relevant lifecycle skill (`software-development-life-cycle`,
  `backend-and-data-architecture`, etc.) for domain implementation.
- `preserve-existing-flow` first if the design touches existing behavior.

## Anti-Patterns

- Jumping to code on an open-ended request without exploring the design ("I'll
  just build something and we'll see"). This is the exact failure Think-Before-
  Coding names.
- Presenting one option as if it were the only one.
- Endless option-generation with no convergence — exploration that never decides.
- Agreeing on a design verbally and not writing it down, so it is lost after
  compaction and cannot be reviewed against.
- Over-brainstorming a trivial, unambiguous task.

## Validation

Methodology skill; no `claude-skills` subcommand beyond the working-brief write.
Self-check before moving to implementation: is there a single agreed design, with
explicit success criteria, captured in the working brief? If the design lives only
in the conversation, capture it first — otherwise the implementation has nothing
durable to be verified against.
