---
name: writing-user-stories
description: Turn a user's prompt into strict Agile/Jira user stories before building, then confirm them with the user. Use on EVERY requirement-bearing prompt — not only "build X", but any "add/change/fix/improve/make it do Y" ask — to convert the raw request COMPLETELY into Connextra-format stories ("As a <role>, I want <goal>, so that <benefit>") with Gherkin acceptance criteria (Given/When/Then), validated against INVEST. The stories are the anti-drift spec: every requirement maps to a story, and nothing gets built that no story asked for. Captured in the working brief so reviewer Stage 1 reconciles the diff against them.
when_to_use: Any prompt that carries a requirement — a feature, change, fix, or behavior request ("build", "add", "make it", "it should", "fix", "change", "I want"). Decompose the prompt into complete strict user stories, confirm correctness and completeness with the user via AskUserQuestion (do not end the turn), then hand the confirmed stories to brainstorming/TDD/the lifecycle skills. Skip only for pure questions, lookups, or already-confirmed trivial mechanical edits.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*), Bash(keel user-story:*)
effort: medium
---

# Writing User Stories

## Purpose

Every prompt that asks for something is a requirement. Before building, convert
that requirement **completely** into strict, standard-format user stories, then
**confirm them with the user**. The stories — not your interpretation of the chat
— are the spec. They are how the work stays anchored: every requirement maps to a
story, and nothing is built that no story asked for. This is the structural cure
for the two most expensive failure modes: drifting off the requirement, and
building something that was never requested.

This is the requirements-capture front of the workflow. `brainstorming` explores
*how* to build once the *what* is agreed; this skill pins the *what* down first,
in a format that is unambiguous and reconcilable.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. This skill is
the requirements half of **Goal-Driven Execution** (turn a vague task into a
verifiable goal) and the front-line defense for **Simplicity First** (a story that
no acceptance criterion needs is scope creep — cut it). Build only what a confirmed
story requires.

## When To Use It

Use it for **any requirement-bearing prompt**, not only greenfield "build X":

- New features and capabilities ("build", "add", "create", "implement").
- Changes and improvements ("change", "make it", "it should", "improve", "refactor so that").
- Bug fixes (the requirement is the corrected behavior — write the story for what
  *should* happen, with Given/When/Then capturing the repro and the fix).
- Any prompt where you would otherwise start coding against your own reading of
  what the user "probably meant".

Skip it only for genuinely non-requirement turns: a pure question, a lookup, a
status/progress request, or an already-confirmed trivial mechanical edit (a typo,
a rename the user spelled out exactly). "It feels small" is not the test — the
test is whether there is a requirement that could be misread. If there is, write
the stories.

Match ceremony to stakes: a one-line change gets one short story; a multi-part
feature gets one story per distinct requirement. Never collapse several distinct
asks into one vague story — that is exactly how requirements get dropped.

## The Practice

### 1. Analyze the prompt into discrete requirements

- Read the user's prompt literally. List every distinct thing it asks for —
  including implicit acceptance conditions ("strict", "completely", "without
  breaking X") and explicit non-goals.
- A multi-part prompt becomes multiple stories. One requirement per story. If you
  cannot tell whether two asks are one requirement or two, split them — merging is
  the lossy direction.

### 2. Write each story in the strict format

**Connextra template** (the title/narrative of every story):

```
As a <role>, I want <goal/capability>, so that <benefit/business value>.
```

- **Role** — who benefits. A real actor (end user, admin, API consumer, the agent
  itself, a maintainer). Not "the system".
- **Goal** — the capability, stated as outcome, not implementation. ("I want my
  session to resume after a disconnect", not "I want a WebSocket reconnect timer".)
- **Benefit** — why it matters. If you cannot state the benefit, question whether
  the story is real.

**Gherkin acceptance criteria** — at least one scenario per story, each:

```
Given <initial context / precondition>
When <action or event>
Then <observable, testable outcome>
```

- Acceptance criteria are the *testable* definition of done. Vague criteria
  ("works well", "is fast") are rejected — state the observable outcome.
- Cover the happy path plus the named edge cases and error behavior.

### 3. Check every story against INVEST

A story is ready only if it is:

- **I**ndependent — minimal overlap with other stories; deliverable on its own.
- **N**egotiable — captures intent, not a frozen implementation contract.
- **V**aluable — the benefit clause is real and user-visible.
- **E**stimable — concrete enough to size; if not, it needs more detail or a split.
- **S**mall — fits a single focused change; split an epic into stories.
- **T**estable — the acceptance criteria can be mechanically verified.

If a story fails INVEST, fix or split it before showing the user.

### 4. Validate the format deterministically

Run the validator so a malformed story is caught before it reaches the user:

```
keel user-story lint --file <stories.md>
```

It fails when a story is missing any Connextra clause (role/goal/benefit) or has
no `Given/When/Then` acceptance scenario, and it flags INVEST risks (e.g. a story
with no testable criteria). Fix what it reports. (For a quick inline check you can
pipe text with `--stdin` instead of a file.)

### 5. Confirm with the user — do not end the turn

Present the complete story set and **ask the user to confirm correctness and
completeness using the `AskUserQuestion` tool** — not by ending your turn and
waiting. The question should let the user accept, correct, or add missing
requirements. Frame it concretely, for example:

- "Here are the N user stories I derived from your request. Do these capture
  everything correctly, or is something missing/wrong?" with options like
  *Accept all*, *Edit a story*, *Add a missing requirement*.

This is mandatory: building before the user confirms the stories is the drift this
skill exists to prevent. The stories are a hypothesis about the requirement until
the user ratifies them.

### 6. Capture the confirmed stories

- Write the confirmed stories into the working brief
  (`keel memory working-brief write`) as the acceptance criteria, so they
  survive compaction and a fresh session.
- The captured stories are exactly what `reviewer` Stage 1 reconciles the
  implementation against. A story that lives only in chat cannot gate the diff.

## Hand-Off

Once stories are confirmed and captured, hand to:

- `brainstorming` if *how* to build is still open (the stories are now its input).
- `test-driven-development` to turn each Gherkin scenario into a failing test.
- the relevant lifecycle skill for domain implementation.
- `preserve-existing-flow` first if the change touches existing behavior.

`reviewer` Stage 1 later checks the diff against these stories: every story
delivered, no code that no story asked for.

## Anti-Patterns

- Starting to build before the stories are confirmed by the user.
- Collapsing several distinct requirements into one vague story (drops requirements).
- A story with no benefit clause, or acceptance criteria that are not testable.
- Writing implementation as the "goal" ("I want a Redis cache") instead of the
  outcome ("I want repeat lookups to return without re-querying the database").
- Inventing stories the user never asked for (scope creep) — the inverse drift.
- Treating the stories as final without confirmation, or confirming by ending the
  turn instead of using AskUserQuestion.

## Validation

Run `keel user-story lint --file <stories.md>` to deterministically check
the strict format. Self-check before hand-off: is there one story per distinct
requirement, each in Connextra form with Gherkin acceptance criteria, each passing
INVEST, validated, **confirmed by the user**, and captured in the working brief? If
any answer is no, the requirement is not yet pinned down — do not start building.
