---
name: finishing-a-development-branch
description: Close out a completed branch the right way — verify, review, then present merge/PR/cleanup options rather than acting unilaterally. Use when implementation is done and tests pass and you are ready to integrate — run the full suite, confirm the completion gate, route non-trivial work through reviewer, then offer the merge/PR path and clean up the branch and any worktree. Use when the user says "finish this", "wrap up the branch", "open the PR", or "merge it". Never force-push, never merge to main unilaterally, never delete a branch without confirmation. Pairs with reviewer, git-expert, and using-git-worktrees.
when_to_use: Implementation complete and tests green, ready to integrate a branch. Verify the full suite, confirm the completion gate, review non-trivial work, then present merge/PR/cleanup options. Pairs with reviewer, git-expert, and using-git-worktrees.
allowed-tools: Read, Grep, Glob, Bash(git status:*), Bash(git log:*), Bash(git diff:*), Bash(claude-skills memory:*)
effort: medium
---

# Finishing a Development Branch

## Purpose

Bring a finished branch to a clean, integrated, reviewed close — with the
verification done and the integration choice left to the user, not taken
unilaterally. The failures this prevents are two-sided: declaring done without
running the full suite, and silently force-pushing or merging to a shared branch
without asking. Finishing is a gate plus a hand-off, not an automatic merge.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline and the repo's
`<git_safety>` rules. This skill is the closeout expression of **Goal-Driven
Execution**: "loop until verified" ends here, with the full suite green and the
completion gate reconciled. Destructive git verbs (force-push, hard reset, branch
delete, merge to main) require explicit user confirmation — that is not optional
politeness, it is the safety contract.

## The Closeout Sequence

### 1. Verify everything, with fresh evidence

- Run the **full** relevant suite now — not "tests passed earlier." No completion
  claim without fresh output in this turn.
- Run `claude-skills memory completion-gate check` to reconcile the result against
  the working brief's success criteria. A failure points at a requirement with no
  evidence yet — close it before finishing.

### 2. Review non-trivial work

- Route a non-trivial diff (logic, multi-file, public API, security-sensitive,
  brownfield) through `reviewer` for the fail-closed verdict. Trivial work
  (docs-only, formatting, single-line typo) is exempt.
- If review returns Conditional Pass or Fail, fix and re-review before finishing —
  do not "finish" over an open finding.

### 3. Confirm the branch state is clean

```bash
git status      # working tree clean, everything committed
git log --oneline <base>..HEAD   # the commits that will integrate
```

- Confirm the working tree is clean and the commit history is what should land.
  Flag any file that looks like it holds secrets (`.env`, credentials) before it
  integrates.

### 4. Present the integration options — do not act unilaterally

Offer the paths and let the user choose; hand the mechanics to `git-expert`:

- **Open a PR/MR** against the base branch (the default for shared repos) — concise
  title under ~70 chars, description covering what changed, what was tested, and
  anything deferred.
- **Merge** — only when the user asks, and never directly to main/master without
  explicit confirmation.
- **Keep the branch** for more work.

Push to a feature branch with upstream tracking (`git push -u`), never directly to
main unless explicitly told.

### 5. Clean up

- After the branch integrates, remove any git worktree it used
  (`using-git-worktrees` cleanup) and prune. Delete the local branch only with
  confirmation.

## Anti-Patterns

- "Tests passed earlier, we're good" — re-run with fresh evidence or it is not
  verified.
- Force-pushing, hard-resetting, or merging to main without explicit confirmation.
- Finishing over an open `reviewer` finding.
- Deleting branches or worktrees without asking.
- A PR description that says "various fixes" instead of what changed, what was
  tested, and what was deferred.
- Leaving the feature's worktree behind after merge.

## Validation

Methodology skill; uses read-only git inspection and the completion gate. Self-check
before calling a branch finished: did the full suite pass with fresh output, did the
completion gate reconcile, did non-trivial work pass reviewer, is the branch clean,
and did you present integration options rather than acting unilaterally on a
destructive verb? If you merged or force-pushed without asking, the safety contract
was broken, not honored.
