---
name: systematic-debugging
description: Root-cause-first debugging for defects, regressions, and flaky behavior. Use when a test fails, a bug is reported, output is wrong, or behavior is intermittent — reproduce the symptom, trace it end-to-end to the true cause with file:line evidence, fix the source of truth, and prove it with a regression test. Use instead of patching the first suspicious-looking line. Pairs with test-driven-development (the regression test) and preserve-existing-flow (when the fix touches an existing owner).
when_to_use: Any defect, regression, incident, or intermittent failure where the cause is not yet proven. Stop guessing and trace. Pairs with TDD for the regression test and preserve-existing-flow when the fix lands in existing behavior.
allowed-tools: Read, Grep, Glob, Edit, Bash
effort: high
---

# Systematic Debugging

## Purpose

Find the true cause of a defect before changing anything, and prove the fix.
The failure mode this skill exists to prevent is patching the line that *looks*
guilty, shipping it, and leaving the real cause in place — which burns review
cycles and often adds a second bug. The discipline is: reproduce, trace to root,
fix the source of truth, prove with a test.

## Code Implementation Discipline

See `../_shared/common-discipline.md` § Code Implementation Discipline. This skill
operationalizes its **Think Before Coding** deep-dive ("a function that looks
like the cause is a hypothesis, not a finding") and **No Workarounds, No Silent
Fallbacks** ("fix the root cause, repair the source of truth instead of patching
downstream symptoms").

## The Four Phases

### 1. Reproduce

- Get the symptom to happen on demand. A bug you cannot reproduce, you cannot
  prove you fixed.
- Capture the exact trigger: input, state, environment, sequence. Write it down
  in the working brief so it survives compaction.
- For intermittent failures, find the condition that makes it deterministic
  (ordering, timing, shared state, a specific data shape). Replace any flaky
  `sleep`/timeout with event-based waiting so the repro is reliable.

### 2. Trace to root cause

- Follow the actual execution path from the trigger to the observable wrong
  effect: input → handler → branch → effect. Cite `file:line` at every hop.
- At each hop, confirm the data and control flow match your expectation. The
  first place reality diverges from expectation is the lead, not the first
  place that looks unfamiliar.
- "Oh, this might be it" is a stop signal, not a green light. Either gather the
  evidence that confirms it (a log line, a value, a trace) or keep reading.
- Distinguish the *symptom site* (where it blows up) from the *cause site* (where
  the wrong value or wrong decision originated). They are usually different files.

### 3. Fix the source of truth

- Change the cause, not the symptom. If a downstream consumer received a bad
  value, fix where the value was produced, not where it was consumed — unless the
  consumer is genuinely the owner of that decision.
- If the fix touches existing behavior with its own owner (a loop, a queue, a
  state machine, a transport path), invoke `preserve-existing-flow` first so the
  fix layers through the owner instead of bypassing it.
- Resist defense-in-depth band-aids: adding a null check three layers down hides
  the real bug. Add guards only where the contract actually says a guard belongs.

### 3b. Find every sibling instance of the same cause

- A root cause is a *pattern*, not a single line. The moment you name it, run
  `keel code-search siblings --query "<the bug shape>"` (MCP `code_search`
  action=siblings). That is the scan owner — not an optional extra grep. The
  instance you reproduced is rarely the only one.
- Concrete: a string-vs-number parse bug at one site usually exists at every site
  that parses that field; a renamed function breaks every caller; a wrong default
  recurs in every copy of the idiom. Fix all of them in this turn, not just the one
  the test happened to hit.
- Show the search. Paste the grep/code-search query and its hit list, then confirm
  each hit is fixed or explicitly out of scope. "I fixed the one I found" with no
  search of the rest is an unfinished fix, not a done one.

### 4. Prove the fix

- Write a regression test that fails against the old code and passes against the
  fix (this is the bug-fix branch of `test-driven-development`). The test is the
  proof and the guarantee it cannot silently return.
- Re-run the broader suite — a root-cause fix can shift behavior elsewhere.
- State what you reproduced, the cause you found with `file:line`, the fix, and
  the test that now guards it.

## Tactics

- **Bisect** when the cause is hidden: halve the input, the commit range, or the
  code path until the smallest failing case remains. `git bisect` for regressions
  introduced by a known-good → known-bad range.
- **Read the full function**, not the snippet around the error line. The cause is
  often in a branch that ran earlier or a caller that passed bad state.
- **Trust the trace over the hypothesis.** When the evidence contradicts what you
  "know" the code does, the evidence wins — re-read.
- **One change at a time** while diagnosing. Changing three things and seeing the
  symptom vanish tells you nothing about which one mattered.

## Anti-Patterns

- Patching the first line that throws without tracing where the bad value came
  from.
- Adding `try/catch` that swallows the error and returns a default — this hides
  the bug and creates a harder one later.
- "Fixed it" with no regression test — nothing stops the bug from returning.
- Changing code on a hunch, seeing the symptom move, and calling it solved
  without confirming the cause.
- Tweaking the same suspect line more than twice with no new hypothesis. After two
  failed attempts, stop and re-trace from the symptom — the target is likely wrong.
- Fixing the one instance the failing test exercised and stopping, without grepping
  for the other sites that share the same root cause. A class fixed at one of N
  sites ships N−1 live bugs.

## Validation

§3b is enforced by `keel code-search siblings` (MCP `code_search` action=siblings).
That command writes the completeness marker. PostToolBatch and `keel review`
fail closed until it ran after the latest edit. Self-check before claiming a
defect fixed: can you reproduce the original symptom, name the cause with
`file:line`, point at the source-of-truth change, show the sibling scan hit list,
and show a regression test that fails without the fix? If any of those is missing,
the bug is diagnosed at best, not fixed.
