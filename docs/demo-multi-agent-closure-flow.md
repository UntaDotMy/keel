# Demo: Multi-Agent Closure Flow

This demo captures a real closure fix where hosted reviewer proof had to count without forcing a fake reviewer-agent registry lane.

## Scenario

- PR: [#38](https://github.com/UntaDotMy/keel/pull/38)
- Branch: `fix/reviewer-closure-proof`
- Merge time: `2026-04-07T02:14:24Z`
- Problem shape: orchestration task completion refused to close even after hosted review was green because closure logic expected a reviewer lane registry entry instead of honoring the recorded proof source

## What actually happened

PR `#38` fixed the closure contract so hosted or native reviewer proof stored in the execution trace can satisfy completion. The intent was not to weaken closure, but to point closure at the right evidence source.

The repair loop did all of the following:

1. trace why closure was blocked after real hosted review passed
2. update validation to accept execution-trace reviewer proof
3. add regression coverage for validator, task-sync, and CLI completion flows
4. replay the previously blocked closure path
5. rerun proof and merge only after green validation

## Commands used in the real repair path

~~~bash
cargo test --workspace
cargo run --bin keel -- memory completion-gate check --workspace-root . --workstream-key feat-workflow-start-defaults --require-closure-ready
cargo run --bin keel -- review pre-pr --repo-root . --base-ref origin/main --format markdown
cargo run --bin keel -- git-workflow preflight --repo-root . --base-ref origin/main
~~~

## Success metrics

- reviewer proof source corrected: yes
- fake reviewer lane required: no
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "multi-agent coordination with real closure proof." It shows the repo can keep strict closure without inventing extra agent state just to satisfy bookkeeping.
