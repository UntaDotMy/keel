---
name: git-expert
description: Guides safe Git workflows: branching, commits, pull requests, merges, conflict resolution, and history repair, with explicit risk explanations and reversible defaults. Use when planning non-trivial Git operations, recovering shared history, cleaning up secrets, or coordinating issue-driven worktree, branch, and PR flow.
when_to_use: Safe Git workflow and version control.
allowed-tools: Read, Grep, Glob, Bash(git:*), Bash(gh:*), Bash(keel git-workflow:*)
argument-hint: "[branch-name] [base-ref] [commit-message]"
shell: bash
effort: medium
---

# Git Expert

## Purpose
Safe Git: inspect first, explain risk, prefer reversible ops, never auto-commit/push/merge.

## Arguments
`$0` branch · `$1` base ref (default `origin/feat`) · `$2` commit subject. Empty → `git status` / current branch. Never destructive from args alone.

## Shared discipline
`_shared/common-discipline.md`. No destructive silent fallbacks ("if reset fails, force-push").

## When
Non-trivial Git, history recovery, secret cleanup, worktree/branch/PR coordination, hosted check triage.

## Live state (injected)
- Status: !`git status --short --branch 2>/dev/null`
- Recent: !`git log --oneline -5 2>/dev/null`

Empty → confirm repo root before mutating.

## Non-negotiables
1. **Safety first** — inspect, explain, reversible default.
2. **User control** — no auto-commit/push/merge without explicit request.
3. **State-aware** — recommendations from real branch/remote topology.
4. **Fixes stay on the same work branch** — never open a new branch for in-flight fixes.
5. **Never delete branches** after push/merge (`-d/-D`, `push --delete`) without genuine explicit exception + confirm.
6. **Local commit before server** — no direct server commits.

## Branch model (this toolkit)
`main` ← `dev` ← `feat` ← `task/<task>` [← `task/<task>/<subtask>`]. Never commit directly to main/dev/feat. Do not use `feat/<task>` (collides with bare `feat`). Legacy `add/`/`feature/` may finish in flight.

Commit subject: `Add : FEATURE : short info` (Category capitalized; FEATURE uppercase; spaces around colons). Branch uses slash (`task/sensor`); commit uses colons.

Preflight: `keel git-workflow preflight --repo-root . --base-ref origin/feat`

## High-risk (explicit approval only)
`commit --amend`, interactive rebase, `reset --hard`, force-push, `filter-repo`. Backup ref + blast radius + rollback before any rewrite. Prefer `revert` on shared history.

## Conflict / PR / clean push
Resolve markers in-place then continue/abort cleanly. PR via `gh`/`glab`; merge only when required checks green (or documented exception). Diff matches task; no secrets; no accidental lockfile/generated churn.

## Target-repo conventions (generic)
Scope_Topic naming; separate requirements/docs/source at top level; always ship `.gitignore` from commit one. Full playbooks: `references/` (00 map, 10 safe ops, 20 issue-branch-PR, 30 review handoff, 40 recovery, 50 Windows, 60 conflicts, 99 anchors).

## Windows
`_shared/common-discipline.md` § Windows + `references/50-windows-git-workflows.md`.
