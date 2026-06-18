# Demo: Greenfield Feature Flow

This demo captures a real greenfield feature delivery path from this repository.

> Historical transcript — the `feat/<topic>` branch names shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#41](https://github.com/UntaDotMy/keel/pull/41)
- Branch: `feat/workflow-route-first-run`
- Merge time: `2026-04-07T03:50:41Z`
- Problem shape: improve the first-run workflow route experience without weakening proof or widening the branch beyond one feature

## What actually happened

PR `#41` tightened the top-layer route surface so a new operator sees a shorter "Start Now" command first, gets a plain-language explanation of the recommended mode, and can still see the scoped variant when needed.

The work stayed narrow:

1. improve a user-visible workflow entrypoint
2. keep the route helpers maintainable by splitting them into a focused internal file
3. rerun targeted workflow tests and repo-wide proof
4. pass review and hosted checks before merge

## Commands used in the real delivery path

~~~bash
cargo test --workspace
cargo test --workspace
cargo test --workspace
cargo run --bin keel -- review pre-pr --base-ref origin/main --repo-root . --issue-id 40 --pr-title "feat: improve workflow route first-run UX"
cargo run --bin keel -- git-workflow preflight --repo-root . --base-ref origin/main
~~~

## Success metrics

- operator-visible improvement shipped: yes
- post-PR repair commits: `0`
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "greenfield feature delivery with user-facing UX improvement." It shows that the repo can make the happy path friendlier without dropping the proof posture.
