---
name: using-git-worktrees
description: Isolate feature or experimental work in its own checkout so parallel work and the main tree never collide. Use when starting work that needs isolation — a risky change, a parallel investigation, or work you may abandon — and the harness does not already provide an isolated environment. Prefer the host's native isolation when it exists; fall back to a git worktree (a second working directory on its own branch sharing one repo). Use when the user says "work in isolation", "spin up a worktree", "don't touch main", or when dispatching parallel agents that each need their own tree. Pairs with dispatching-parallel-agents and finishing-a-development-branch.
when_to_use: Starting isolated or parallel work that should not disturb the main working tree. Prefer native harness isolation; otherwise create a git worktree on a feature branch. Clean it up when the work merges or is abandoned. Pairs with dispatching-parallel-agents and finishing-a-development-branch.
allowed-tools: Read, Grep, Glob, Bash(git worktree:*), Bash(git branch:*), Bash(git status:*)
effort: medium
---

# Using Git Worktrees

## Purpose

Give a piece of work its own checkout so it cannot collide with the main tree or
with other concurrent work. A git worktree is a second working directory backed by
the same repository, on its own branch — you can build, test, and even abandon it
without disturbing anything else. The failure this prevents is doing risky or
parallel work in one shared tree, where a half-finished change blocks an urgent fix
or two efforts stomp on each other's files.

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline. Isolation does
not relax the rules: work in a worktree still goes through **Goal-Driven Execution**
and **Surgical Changes**, and a worktree you abandon must be cleaned up (the
"clean up only your own mess" rule applies to checkouts, not just code).

## Prefer Native Isolation First

If the harness or environment already provides an isolated workspace (a fresh
container, a sandbox, a per-task checkout), use it — do not layer a worktree on top
of isolation you already have. Reach for `git worktree` when you are in a single
shared checkout and need a second isolated tree the host does not give you.

## Creating And Using A Worktree

### 1. Create the worktree on a feature branch

```bash
git worktree add ../<repo>-<task> -b task/<task> feat
```

- This creates a new directory `../<repo>-<feature>` checked out on a new
  `task/<task>` work branch off `feat`, sharing the same `.git`. The main tree is untouched.
- Name the directory and branch for the work so parallel worktrees are
  distinguishable at a glance. All hands-on work uses `task/<task>`
  work branches off `feat` (e.g. `task/rgb-sync`, `task/sensor/timeout`); fixes for in-flight
  work stay on that feature's existing branch.

### 2. Work in it normally

- Build, test, and commit inside the worktree as if it were a normal checkout.
  Tooling that respects the working directory just works.
- Each worktree has its own branch checked out — you cannot check out the same
  branch in two worktrees, which is the guard that keeps them from colliding.

### 3. List and track active worktrees

```bash
git worktree list
```

- Use this to see every active tree and which branch each holds — especially when
  several parallel agents each own one.

## Cleaning Up

When the work merges or is abandoned, remove the worktree so stale checkouts do not
accumulate:

```bash
git worktree remove ../<repo>-<feature>   # refuses if there are uncommitted changes
git worktree prune                        # clears administrative records of deleted trees
```

- `git worktree remove` refuses by default if the tree has uncommitted changes —
  that guard is intentional; do not force past it without confirming the changes are
  truly disposable.
- Removing a worktree removes the extra **checkout**, not the branch. This repository
  never deletes branches — the branch stays after the worktree is gone.
- A worktree left behind after its branch merges is exactly the "clean up your own
  mess" violation — remove the worktree (not the branch) as part of
  `finishing-a-development-branch`.

## Parallelism

When `dispatching-parallel-agents` fans out work that each agent must build or test
in isolation, give each its own worktree so their checkouts cannot collide. The
independence test still applies — worktrees isolate the *checkout*, but two agents
targeting the same logical change still conflict at merge.

## Anti-Patterns

- Layering a worktree on top of a harness that already gives you an isolated
  checkout — redundant overhead.
- Doing risky or long-lived experimental work directly in the main tree, so an
  urgent fix is blocked by your half-finished state.
- Forcing `git worktree remove` past its uncommitted-changes guard without
  confirming the changes are disposable.
- Leaving merged-branch worktrees lying around until `git worktree list` is a
  graveyard.
- Trying to check out the same branch in two worktrees and fighting the error
  instead of branching.

## Validation

Methodology skill; uses `git worktree`. Self-check: is the isolated work on its own
branch in its own tree (or native sandbox), is the main tree undisturbed, and — when
the work is done or dropped — has the worktree been removed and pruned? A lingering
worktree after merge means the cleanup half was skipped.
