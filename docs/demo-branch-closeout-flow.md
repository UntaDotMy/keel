# Demo: Branch-Closeout Flow

This demo captures a real scoped feature branch carried from synced `main` to hosted-green merge closeout.

> Historical transcript — branch names and the `feat/<topic>` → PR-into-`main` flow shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#33](https://github.com/UntaDotMy/keel/pull/33)
- Branch: `feat/comparison-and-release-docs`
- Merge time: `2026-04-06T13:02:40Z`
- Problem shape: sync `main`, ship one scoped feature, open PR, watch hosted checks to terminal state, merge only after green proof

## What actually happened

After PR `#32` merged, local `main` was synced, a fresh feature branch was created, the comparison and release-doc feature was implemented, contract proof and repo-wide proof were rerun, the PR was opened, and hosted checks were watched all the way to terminal state.

The branch did not need a post-PR repair commit. It still went through the full closeout path:

1. sync local `main`
2. create one fresh feature branch
3. run local proof and native review
4. push and open the PR
5. watch hosted lanes until the full matrix and summary are green
6. merge only after hosted proof is complete

## Commands used in the real closeout path

~~~bash
git checkout main
git pull --ff-only origin main
git checkout -b feat/comparison-and-release-docs
cargo test --workspace
cargo run --bin keel -- review pre-pr --repo-root . --base-ref origin/main --format markdown
git push -u origin feat/comparison-and-release-docs
gh pr create --base main --head feat/comparison-and-release-docs --title "docs(release): add comparison and release note surfaces"
gh pr checks 33 --watch
~~~

Current Anvil-first equivalent:

~~~bash
keel memory working-brief write --request "comparison and release docs"
keel anvil compile --goal "comparison and release docs" --workspace-root .
keel anvil run --dry-run --workspace-root .
keel review pre-pr --repo-root . --base-ref origin/main --format markdown
keel memory completion-gate check --brief-id <id> --proof "review and hosted checks passed"
~~~

## Success metrics

- local proof passed before PR open: yes
- repair commits after PR open: `0`
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "single-feature branch closeout with real proof." It shows that the stricter workflow can still move cleanly from branch creation to merge without skipping the hosted-check gate.
