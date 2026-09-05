---
name: receiving-code-review
description: Evaluate code-review feedback with rigor instead of reflexive agreement, as the author acting on a review. Use when you receive review comments, a reviewer verdict, or change requests — judge each point on its merits, fix what is right at the root cause, push back with evidence on what is wrong, and re-verify before claiming the feedback addressed. Use when the user relays review comments, when reviewer returns Conditional Pass or Fail, or says "address the review", "the reviewer said", or "respond to these comments". The author-side counterpart to reviewer; reviewer renders the verdict, this acts on it honestly.
when_to_use: Acting on code-review feedback as the author. Evaluate each point on merit, fix root causes, push back with evidence where the feedback is wrong, and re-verify. Pairs with reviewer (the verdict), critic (in-flight findings as review input), and systematic-debugging (root-cause fixes).
allowed-tools: Read, Grep, Glob, Edit, Write, Bash
effort: medium
---

# Receiving Code Review

## Purpose

Turn review feedback into the right changes — honestly. Two opposite failure modes
exist, and this skill guards both: reflexively agreeing with every comment and
making shallow changes that satisfy the letter without fixing the cause, and
reflexively defending the code and dismissing valid findings. The discipline is to
judge each point on its merits, fix what is right at the root, and push back with
evidence (not ego) on what is wrong.

## Code Implementation Discipline

See `../_shared/common-discipline.md` § Code Implementation Discipline. Acting on review
is **No Workarounds, No Silent Fallbacks** under social pressure: fix the cause the
reviewer found, not a downstream patch that makes the comment go away. **Think
Before Coding** governs the response — a review comment is a hypothesis about a
problem; confirm the problem before "fixing" it, and confirm the comment is right
before disagreeing.

## Performative Agreement Is A Failure

Saying "good catch, fixed!" and making a change that does not actually address the
underlying concern is worse than disagreeing — it hides the unfixed problem behind
an apparent resolution. The reviewer believes it is handled; it is not. Treat every
"fixed" as a claim you must back with evidence (a test, a trace, a re-run), exactly
like any other completion claim.

## The Practice

### 1. Understand each point before reacting

- Read the comment for the *concern behind it*, not just the literal text. A
  reviewer pointing at one line often means the pattern, not only that line.
- Separate the comment into: a correctness issue, a design concern, a style
  preference, or a misunderstanding. They get different responses.

### 2. For valid points — fix the root cause

- Fix where the problem originates, not where the comment landed. If the reviewer
  flagged a symptom, trace to the cause (`systematic-debugging`) and fix there.
- If the fix touches existing behavior, run `preserve-existing-flow` first.
- Prove the fix the same way you prove any change — a test that fails without it,
  passes with it. "Fixed" without evidence is performative.

### 3. For points you disagree with — push back with evidence

- Disagreement is legitimate when you have evidence: a test that shows the current
  behavior is correct, a constraint the reviewer did not have, a trade-off the
  comment did not weigh. State it plainly and specifically.
- "I disagree" is not enough — show the file:line, the test, or the constraint that
  makes the comment wrong. The goal is the right code, not winning.
- If it is a style preference against the project's established convention, follow
  the convention (Surgical Changes: match existing style) and say so.

### 4. Re-verify and report what changed

- After addressing the feedback, re-run the relevant suite with fresh output.
- Report point by point: what you changed and the evidence, what you pushed back on
  and why, what was a preference you followed. A reviewer can re-check a specific
  claim; they cannot re-check "addressed all comments."

## Anti-Patterns

- "Good catch, fixed!" with a shallow change that does not address the real concern.
- Agreeing with everything to avoid friction, including points that are wrong.
- Dismissing valid findings to defend the code — ego over correctness.
- Patching the exact line the comment pointed at when the cause is elsewhere.
- Claiming the review is addressed without re-running the verification.
- Silently ignoring a comment you disagree with instead of pushing back with
  evidence — the reviewer cannot tell "handled" from "overlooked."

## Validation

Methodology skill; re-runs the project's checks. Self-check before claiming a review
addressed: for each comment, can you point to the root-cause fix with evidence, or
the specific evidence for why you pushed back, and did you re-run the suite with
fresh output? If any "fixed" has no backing evidence, it is performative agreement,
not a resolution.
