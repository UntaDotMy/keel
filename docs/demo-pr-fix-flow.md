# Demo: PR-Fix Flow

This demo captures a real hosted-check recovery loop from this repository.

> Historical transcript — the `feat/<topic>` branch names shown here predate the current four-tier model (work branch → `feat` → `dev` → `main`). See `WORKFLOW.md` for the current branch model; the workflow mechanics demonstrated here still apply.

## Scenario

- PR: [#32](https://github.com/UntaDotMy/claude_skills/pull/32)
- Branch: `feat/packaged-release-install-channel`
- Merge time: `2026-04-06T11:49:46Z`
- Problem shape: open PR, hosted Windows lane fails, branch must be repaired and returned to green without abandoning the PR

## What actually happened

On 2026-04-06, PR `#32` initially passed local proof and most hosted lanes, but the Windows manager loop failed. The failure was not in the feature logic itself. It was a Windows path-length problem inside the test fixture repo setup for the packaged-release update path.

The repair loop did all of the following on the same branch:

1. inspected hosted status instead of assuming local green was enough
2. opened the failing Windows job log
3. traced the root cause to fixture repo setup during `git add .`
4. patched the test fixture surface only
5. reran targeted manager tests, then uncached repo-wide proof
6. pushed a repair commit and re-watched the PR to terminal hosted state

## Commands used in the real recovery path

~~~bash
gh pr checks 32 --watch
gh run view 24029846531 --job 70076163527 --log
cargo test --workspace
cargo test --workspace
cargo run --bin claude-skills -- review pre-pr --repo-root . --base-ref origin/main --format markdown
git push origin feat/packaged-release-install-channel
gh pr checks 32 --watch
~~~

## Success metrics

- hosted failure repaired: yes
- failing lane: `windows-latest`
- repair commits after first hosted failure: `1`
- final hosted result: `6/6` required checks green
- final outcome: merged

## Why this matters

This is the benchmark shape for "real PR-fix under hosted pressure." It proves the repo can hold a strict proof posture without giving up when the first hosted lane fails.
