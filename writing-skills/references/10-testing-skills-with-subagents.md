# Testing Skills With Subagents

The method behind `writing-skills`' iron law: prove a skill changes behavior by
watching a fresh subagent fail without it, then pass with it. This file is the
detailed procedure the skill summarizes.

## Why subagents

A subagent (dispatched via the Task tool) starts with a fresh context window. It
does not carry your intent, your knowledge of the skill, or the conversation that
led here. That makes it a clean test subject: if it makes the right call, it did
so on the strength of what you gave it, not on shared context you forgot you had.
Testing a skill in your own context is the equivalent of writing a test after the
code and never watching it fail — you cannot trust the result.

## Building a pressure scenario

A scenario is a realistic task with exactly one correct action and several forces
pulling toward the wrong one. Weak scenarios "pass" trivially and prove nothing.
Stack at least three real pressures:

- **Time** — "the demo is in ten minutes."
- **Sunk cost** — "you already wrote 300 lines; starting over feels wasteful."
- **Authority** — "the senior engineer said to just ship it."
- **Money** — "this blocks a paying customer."
- **Plausible shortcut** — a wrong path that looks reasonable and faster.

The scenario should make the wrong choice *tempting*, not obviously bad. If the
correct action is the comfortable one, the scenario tests nothing.

## RED — capture the failure and its rationalizations

1. Dispatch a subagent the scenario with **no skill loaded**. Phrase it as a real
   request, not a quiz.
2. Record the decision it makes.
3. Record the **exact rationalizations** it uses to justify a wrong call. These
   are gold: the model will reach for the same excuses in production, so the skill
   must name and answer them by name. Typical ones:
   - "The tests probably still pass."
   - "This is a small change, review is overkill."
   - "The user is in a hurry, I'll skip the failing-test step."
   - "I'm fairly sure this is the cause" (without tracing).
4. If the subagent makes the right call unprompted, the skill is not needed for
   this situation, or the pressure is too weak. Strengthen it or drop the claim.

## GREEN — write the minimum prose that flips the decision

- Target the captured rationalizations directly. If the subagent said "the tests
  probably still pass," the skill must say "re-run the tests; 'probably' is not
  evidence" — in those terms.
- Write the least text that would have changed that one decision. Resist writing
  the full topic overview; unexercised prose is dead weight that dilutes the
  rules that matter.
- Use the durable phrasings: a Red Flags table (rationalization → reality), an
  inline iron law, a concrete trigger in the `description`.

## REFACTOR — re-test, then hunt the next loophole

1. Re-run the *same* scenario with the new skill loaded. Pass = the subagent makes
   the right call AND cites the skill's reasoning, under the same pressure.
2. Vary the pressures (swap authority for money, add a second shortcut) and re-run.
   A skill that only holds for one exact wording has a loophole. Each new failure
   is a fresh RED.
3. Stop when the skill holds across variants and reads clean. Then run the
   structural gate: `keel skill-lint`.

## What a passing skill looks like

- A captured RED transcript: a subagent making the wrong call without the skill.
- A captured GREEN transcript: the same scenario, skill loaded, right call + cited
  reasoning, under maximum pressure.
- A clean `keel skill-lint` (triggers, within budget, no dangling links).

If you have only the third, you have proven the skill *loads*, not that it
*works*. Both gates, in order, before you claim a skill done.

## Relationship to the autonomous learning loop

keel's `learn` loop (`runner/learning.rs`) authors skills automatically
from observed behavior, gated by statistical thresholds (recurrence, confidence)
and a content-hash no-clobber guard. That is a different mechanism — it answers
"did this pattern recur enough to be worth a skill," not "does this prose change
behavior under pressure." When you hand-author or refine a skill, this
subagent-pressure method is the behavioral evidence the statistical loop does not
provide. Use it on any skill whose prose you wrote or edited by hand.
