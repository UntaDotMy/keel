# Issue, Branch, and PR Flow

## Objective

Support optional structured collaboration flow when the user explicitly requests it.

## Branch Model

Hierarchy, promoted one direction only. Integration tiers are permanent; task branches carry hands-on commits:
- **`main`** — final stable, verified. Only merges from `dev`.
- **`dev`** — staging.
- **`feat`** — integration. Receives merges from `task/<task>`. Bare name only (not `feat/<task>`).
- **`task/<task>`** — one task. Branch off `feat` (or a parent task when stacked).
- **`task/<task>-<subtask>`** — one flat subtask branch. Branch off `task/<task>`; Git cannot store a nested child ref below an existing parent ref.

Promotion flow: `task/<task>-<subtask>` → `task/<task>` → `feat` → `dev` → `main`.

## Flow (Optional, User-Requested)

1. Create or confirm issue context.
2. Create a `task/<task>` work branch from `feat` (or flat `task/<task>-<subtask>` from the parent task).
3. Implement in small commits using `Add : FEATURE : short information`. Fixes stay on the same branch. Commit locally first.
4. Open PR against the correct parent (`task/<task>` for a subtask, `feat` for a task) with clear rationale and validation evidence.
5. Address feedback and update PR.
6. Request human review.
7. After verification, promote upward (`task` → `feat` → `dev` → `main`). Never delete the work branch.

## Issue and Branch Guidance

- Keep issue scoped to a clear user problem and acceptance criteria.
- All hands-on work uses `task/<task>` (or flat `task/<task>-<subtask>`):
  - e.g. `task/rgb-sync`, `task/sensor/i2c-timeout`
- Fixes and subtasks for in-flight work stay on that task's branch (or a nested subtask branch), never a random new prefix.
- Legacy `add/` / `feature/` branches may finish in flight; new work uses `task/`.
- Never delete a branch after pushing or merging it. Branches are permanent in this model.
- Always commit locally before pushing to the server.

## PR Guidance

PR description should include:

- Problem statement
- Solution summary
- Risk and rollback notes
- Validation evidence (tests/lint/build/manual checks)
- Linked issue references (for example closing keywords when appropriate)

## Review Loop Guidance

- Address reviewer comments in focused follow-up commits.
- Re-run relevant checks after each fix cycle.
- Summarize what changed since last review round.
