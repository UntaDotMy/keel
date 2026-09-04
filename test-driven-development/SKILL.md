---
name: test-driven-development
description: Disciplined RED-GREEN-REFACTOR loop for behavior changes. Use when adding a feature, fixing a bug, or changing behavior where a test can express the requirement — write the failing test first, watch it fail for the right reason, make it pass with the minimum change, then refactor under green. Use when the user asks for TDD, a failing test, or test-first work, or when a change has a verifiable observable outcome. Complements qa-and-automation-engineer (coverage strategy and the release ladder) and reviewer (the fail-closed verdict).
when_to_use: Any behavior change with a verifiable outcome — features, bug fixes, regressions. Pairs with qa-and-automation-engineer for coverage strategy; this skill governs the moment-to-moment test-first loop.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash
effort: medium
---

# Test-Driven Development

## Purpose

Drive each behavior change through a failing test before the implementation
exists. The test is the executable form of the requirement: it states what
"done" means, proves the code does not do it yet, and then proves the code does
it after. This skill governs the tight per-change loop. `qa-and-automation-engineer`
governs the broader coverage strategy and the release ladder; `reviewer` governs
the final fail-closed verdict. Use this one while you are actually writing code.
For **shared acceptance behavior** in product language (Gherkin, outside-in), use
`behavior-driven-development` at the outer seam and keep this skill for the
inner unit/module loop.

## Code Implementation Discipline

See `../_shared/common-discipline.md` § Code Implementation Discipline for the
canonical rules. This skill is the operational expression of its **Goal-Driven
Execution** pillar — "write the failing test, then make it pass" — and of
**Simplicity First** in the GREEN step: write the minimum code that turns the
test green, nothing speculative.

## The Loop

### RED — write a failing test first

- Express the requirement as one test before touching the implementation.
- Run it. Confirm it fails, and that it fails **for the reason you expect** (the
  assertion you wrote, not an import error or a typo). A test that fails for the
  wrong reason proves nothing.
- If you cannot write a failing test, you do not yet understand the requirement.
  That is a Think-Before-Coding signal — go clarify, do not start coding.

### GREEN — minimum code to pass

- Write the least code that makes the failing test pass. Resist solving the
  general case; solve the case the test states.
- Run the test. Confirm it now passes, and that previously-passing tests still
  pass. "Green" means the whole relevant suite, not just the new test.
- Do not add the next feature's code here. One red test at a time.

### REFACTOR — clean up under green

- With the test green, improve names, remove duplication, and simplify structure.
- Re-run the suite after each refactor. The tests are the safety net that lets
  refactoring be fearless; if they go red, the refactor broke behavior — revert
  or fix before continuing.
- Stop when the code is clean and the suite is green. Then start the next RED.

## Bug Fixes Are TDD Too

A bug means a missing test. The loop for a defect:

1. Write a test that reproduces the bug from the user's symptom (input → observed
   wrong output). It must fail against the current code.
2. Confirm it fails for the real reason — this is also how you prove you found the
   actual defect, not a look-alike. (See `systematic-debugging` for the root-cause
   trace that tells you *where* to write the fix.)
3. Fix the root cause. The test goes green. The regression can never silently
   return, because the test now lives in the suite.

## When A Unit Test Is The Wrong Tool

TDD is the discipline, not dogma about test granularity. Match the test to the
behavior:

- Pure logic, parsers, calculations → fast unit test.
- A request path or query → integration test at the seam that actually exercises
  it; do not over-mock the thing under test into meaninglessness.
- A UI interaction or end-to-end flow → the narrowest E2E that still observes the
  real behavior.
- Genuinely untestable-in-isolation work (a one-line config value, a generated
  file) → state that plainly and prove it by the most direct check available,
  rather than faking a unit test.

The rule is "prove the change with a check that would fail without it," not "a
unit test for every line."

## Anti-Patterns

- Writing the implementation first, then a test that just confirms what you wrote
  (the test can no longer surprise you — it encodes the bug too).
- Asserting on incidental output (log strings, formatting) instead of the
  behavior the requirement names.
- A test that passes the first time you run it — you never saw RED, so you never
  proved the test can fail.
- Skipping the re-run after REFACTOR.
- Deleting or weakening a failing test to "make the suite green" instead of fixing
  the code. This is a `reviewer` fail condition.

## Validation

This skill is methodology; there is no `keel` subcommand. Self-check
before claiming a TDD change complete: did you watch the test fail first, watch
it pass after the minimum change, and re-run the full relevant suite after
refactoring? If you cannot say yes to all three, the loop was not followed.
