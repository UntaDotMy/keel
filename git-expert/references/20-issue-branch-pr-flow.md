# Issue, Branch, and PR Flow

## Objective

Support optional structured collaboration flow when the user explicitly requests it.

## Branch Model

Three permanent branch tiers, promoted one direction only:
- **`main`** — final stable, verified. Only merges from `dev`.
- **`dev`** — active development integration for daily commits; features verified here (staging) before promotion to `main`.
- **`feat/<topic>`** — all new work: features, fixes, subtasks. Branch off `dev`, merge back into `dev`.

## Flow (Optional, User-Requested)

1. Create or confirm issue context.
2. Create a `feat/<topic>` branch from `dev`.
3. Implement change in small, reviewable commits using the `<category>: <FEATURE>: <short information>` commit format.
4. Open PR against `dev` with clear rationale and validation evidence.
5. Address feedback and update PR.
6. Request human review.
7. After `dev` verifies the feature on staging, promote `dev` into `main`. Never delete the feature branch.

## Issue and Branch Guidance

- Keep issue scoped to a clear user problem and acceptance criteria.
- All new work — features, fixes, and subtasks — uses a `feat/<topic>` branch off `dev`:
  - `feat/<issue-id>-<short-topic>`
- Never delete a branch after pushing or merging it. Branches are permanent in this model.

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
