# Demo: Docs Governance Flow

This demo captures a real docs-only workflow-governance change that still had to clear executable proof.

> Historical transcript — the `feat/<topic>` branch names shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#43](https://github.com/UntaDotMy/claude_skills/pull/43)
- Branch: `feat/workflow-first-success-path`
- Merge time: `2026-04-07T04:13:09Z`
- Problem shape: publish a first-success guide that helps a new operator complete an end-to-end flow, then wire that guide into the main docs surfaces without treating docs as untestable

## What actually happened

PR `#43` added a dedicated first-success guide, linked it from README and WORKFLOW, and backed the new onboarding surface with contract coverage so the governance change could not quietly drift out of the product surface later.

The docs lane still followed proof discipline:

1. write the guide as a practical operator path
2. expose it from the main docs surfaces
3. add contract coverage for the links and current command flow
4. rerun repo-wide proof
5. merge only after hosted checks were green

## Commands used in the real docs lane

~~~bash
cargo test --workspace
cargo test --workspace
cargo run --bin claude-skills -- review pre-pr --base-ref origin/main --repo-root . --issue-id 42 --pr-title "docs: add workflow first-success path"
cargo run --bin claude-skills -- git-workflow preflight --repo-root . --base-ref origin/main
~~~

## Success metrics

- docs-only workflow surface shipped with tests: yes
- post-PR repair commits: `0`
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "docs-only or workflow-governance change." It shows that a docs branch can still be treated like product work with explicit proof.
