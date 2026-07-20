---
name: finishing-a-development-branch
description: Close out a completed branch the right way — verify, review, then present merge/PR options rather than acting unilaterally. Use when implementation is done and tests pass and you are ready to integrate — run the full suite, confirm the completion gate, route non-trivial work through reviewer, then offer the merge/PR path into dev. Use when the user says "finish this", "wrap up the branch", "open the PR", or "merge it". Never force-push, never merge to main unilaterally, never delete a branch — this repo keeps every branch permanently after merge. Pairs with reviewer, git-expert, and using-git-worktrees.
when_to_use: Implementation complete and tests green, ready to integrate a branch. Verify the full suite, confirm the completion gate, review non-trivial work, then present merge/PR options into dev. Branches are never deleted. Pairs with reviewer, git-expert, and using-git-worktrees.
allowed-tools: Read, Grep, Glob, Bash(git status:*), Bash(git log:*), Bash(git diff:*), Bash(keel memory:*)
disallowed-tools: Edit, Write, Bash(git push:*)
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
completion gate reconciled. Destructive git verbs (force-push, hard reset, merge to
main) require explicit user confirmation — that is not optional politeness, it is the
safety contract. This repository **never deletes branches** after merge: a finished
branch stays in place permanently, so closeout means integrate-and-keep, not
integrate-and-delete.

## The Closeout Sequence

### 1. Verify everything, with fresh evidence

- Run the **full** relevant suite now — not "tests passed earlier." No completion
  claim without fresh output in this turn.
- Run `keel memory completion-gate check` to reconcile the result against
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

- **Open a PR/MR** against the correct parent (`feat` for `task/<task>`; the parent task for a subtask) — concise
  title under ~70 chars, description covering what changed, what was tested, and
  anything deferred. Promotion `feat` → `dev` → `main` happens separately, after the
  feature is verified on staging.
- **Merge** — only when the user asks, and never directly to `main` without
  explicit confirmation. Work branches merge into `feat`; `feat` promotes to `dev`; `dev` promotes to `main`.
- **Keep the branch** for more work.

Push to the `task/<task>` (or `task/<task>/<subtask>`) work branch with upstream tracking (`git push -u`), never directly to
`main`. Fixes for in-flight work stay on the same work branch. After merge, the branch stays — this repo never deletes branches.

### 5. Confirm worktree state — do not delete the branch

- This repository **never deletes branches** after merge, so closeout does not include
  `git branch -d/-D` or `git push origin --delete`. The branch is permanent history.
- If the work used a git worktree, you may remove the worktree directory
  (`using-git-worktrees` cleanup) and prune — that removes the extra checkout, not the
  branch. The branch itself remains.

## Anti-Patterns

- "Tests passed earlier, we're good" — re-run with fresh evidence or it is not
  verified.
- Force-pushing, hard-resetting, or merging to main without explicit confirmation.
- Finishing over an open `reviewer` finding.
- Deleting a branch after merge — this repo keeps every branch permanently.
- A PR description that says "various fixes" instead of what changed, what was
  tested, and what was deferred.
- Leaving the feature's worktree behind after merge (remove the worktree, but keep
  the branch).

## Validation

Methodology skill; uses read-only git inspection and the completion gate. Self-check
before calling a branch finished: did the full suite pass with fresh output, did the
completion gate reconcile, did non-trivial work pass reviewer, is the branch clean,
and did you present integration options rather than acting unilaterally on a
destructive verb? If you merged or force-pushed without asking, the safety contract
was broken, not honored.
