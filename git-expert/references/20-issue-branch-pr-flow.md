# Issue, Branch, and PR Flow

## Objective

Support optional structured collaboration flow when the user explicitly requests it.

## Branch Model

Four tiers, promoted one direction only. The three upper tiers are permanent; work branches carry hands-on commits:
- **`main`** — final stable, verified. Only merges from `dev`.
- **`dev`** — active development, staging for daily commits.
- **`feat`** — new features, fixes, subtasks. Receives merges from work branches.
- **work branch** `<category>/<FEATURE>` — all hands-on commits. Branch off `feat`.

Promotion flow: `work branch` → `feat` → `dev` → `main`.

## Flow (Optional, User-Requested)

1. Create or confirm issue context.
2. Create a `<category>/<FEATURE>` work branch from `feat`.
3. Implement change in small, reviewable commits using the `[category]: [feature_category]: short information` commit format. Fixes for this in-flight work stay on the same branch — never a new branch. Always commit locally first, avoid direct commits to the server.
4. Open PR against `feat` with clear rationale and validation evidence.
5. Address feedback and update PR.
6. Request human review.
7. After the feature is verified, promote `feat` → `dev` → `main` (staging verify at `dev`). Never delete the work branch.

## Issue and Branch Guidance

- Keep issue scoped to a clear user problem and acceptance criteria.
- All hands-on work uses a `<category>/<FEATURE>` work branch off `feat`:
  - e.g. `add/RGB`, `fix/SENSOR`, or `<category>/<issue-id>-<TOPIC>`
- Fixes and subtasks for in-flight work stay on that feature's existing work branch, regardless of commit category.
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
