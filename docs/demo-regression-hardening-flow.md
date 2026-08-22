# Demo: Regression-Hardening Flow

This demo captures a real regression-hardening pass for an operator-visible default.

> Historical transcript — the `feat/<topic>` branch names shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#45](https://github.com/UntaDotMy/keel/pull/45)
- Branch: `feat/anvil-compile-defaults`
- Merge time: `2026-04-07T04:53:36Z`
- Problem shape: make the first-run Anvil compile path lower-friction while proving the new default with explicit regression coverage

## What actually happened

PR `#45` changed the Anvil compile defaults so users get a predictable
first-run goal and standard validation tier when they do not specify optional
values explicitly.

The branch mattered as a regression-hardening shape because the user-visible default needed deterministic proof:

1. change the runtime default in the app layer
2. add or refresh the direct tests that pin the expected default request
3. rerun the managed prompt surface contract coverage
4. hold the branch to proof before merge

## Commands used in the real hardening path

~~~bash
cargo test --workspace
cargo test --workspace
cargo run --bin keel -- anvil compile --goal "first-run default" --workspace-root .
cargo run --bin keel -- git-workflow preflight --repo-root . --base-ref origin/main
~~~

## Success metrics

- user-visible default changed safely: yes
- explicit regression guard present: yes
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "regression fix with explicit test hardening." It shows the repo can make the default path easier while still pinning the behavior with deterministic proof.
