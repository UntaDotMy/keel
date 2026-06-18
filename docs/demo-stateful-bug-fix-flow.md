# Demo: Stateful Bug-Fix Flow

This demo captures a real stateful bug fix where the durable workflow source of truth had to be corrected instead of patched around.

> Historical transcript — the `feat/<topic>` branch names shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#27](https://github.com/UntaDotMy/keel/pull/27)
- Branch: `feat/memoriesv2-hardening`
- Merge time: `2026-04-06T03:48:40Z`
- Problem shape: canonical execution-trace ownership drifted between memoriesv2 and legacy compatibility files, so closure and resume surfaces could report the wrong state

## What actually happened

PR `#27` traced the bug beyond the first suspicious branch. The fix restored canonical memoriesv2 ownership, updated the callers that read those artifacts, and added compatibility drift warnings instead of silently trusting stale files.

The repair loop was source-of-truth focused:

1. trace where execution-trace ownership should live
2. update load and save paths to prefer the canonical memoriesv2 artifacts
3. teach resume-status and workflow/orchestration surfaces to report drift honestly
4. add regression coverage for fallback, sync, drift, and error branches
5. rerun repo-wide proof before merge

## Commands used in the real repair path

~~~bash
cargo test --workspace
cargo test --workspace
cargo test --workspace
cargo run --bin keel -- review pre-commit --repo-root . --format markdown
cargo run --bin keel -- review pre-pr --repo-root . --base-ref origin/main --format markdown
cargo run --bin keel -- git-workflow preflight --repo-root . --base-ref origin/main --format markdown
~~~

## Success metrics

- root-cause fix landed at the authoritative ownership path: yes
- compatibility drift is now surfaced explicitly: yes
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "stateful bug fix with root-cause tracing." It proves the repo can repair durable workflow ownership instead of hiding the symptom in one consumer branch.
