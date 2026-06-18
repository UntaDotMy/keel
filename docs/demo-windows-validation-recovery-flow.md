# Demo: Windows Validation Recovery Flow

This demo captures a real hosted validation hardening pass for cross-platform workflow reliability.

## Scenario

- PR: [#36](https://github.com/UntaDotMy/keel/pull/36)
- Branch: `fix/validate-stable-checkout`
- Merge time: `2026-04-07T00:57:25Z`
- Problem shape: late-starting hosted matrix lanes could resolve the wrong branch checkout ref, especially in Windows-heavy validation flows where timing differences matter

## What actually happened

PR `#36` switched the validate workflow from branch-name refs to stable commit SHA refs and kept `fetch-depth` at `0` so delayed matrix lanes could still resolve the intended checkout reliably.

The hardening loop stayed tight:

1. trace the hosted checkout failure mode
2. patch the workflow to use stable commit refs
3. keep the matrix lanes capable of resolving that commit
4. add direct workflow contract coverage
5. rerun repo-wide proof before merge

## Commands used in the real hardening path

~~~bash
cargo test --workspace
cargo test --workspace
cargo run --bin keel -- review pre-pr --repo-root . --base-ref origin/main --format compact
cargo run --bin keel -- git-workflow preflight --repo-root . --base-ref origin/main --format compact
~~~

## Success metrics

- hosted checkout hardening shipped: yes
- stable commit ref used: yes
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "Windows-heavy validation or recovery flow." It proves the repo can turn hosted timing failures into a deterministic workflow contract fix.
