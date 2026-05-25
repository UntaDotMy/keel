# First Success Path

This guide is the fastest honest path through the current native workflow surface.

It is for a new operator who already has `claude_skills` installed and wants one satisfying end-to-end run without memorizing every workflow command first.

## Goal

Start from one broad request, open a tracked workflow ledger entry, keep the live operator view visible, prove the branch, and only then close the entry with the proof recorded.

## What the runtime actually exposes

The Rust runtime currently routes these workflow subcommands to real handlers:

- `claude-skills workflow route --request "..."` — recommends a preset for a broad request
- `claude-skills workflow start --request "..." [--preset autopilot|debug|tdd|review|eco|parallel]` — opens a ledger entry
- `claude-skills workflow status` / `workflow cockpit` / `workflow dashboard` / `workflow watch` — read the same ledger from different angles
- `claude-skills workflow resume --id <entry-id>` — reopens an existing entry
- `claude-skills workflow finish --id <entry-id> --proof "..."` — closes an entry with proof attached

Subcommands like `workflow setup`, `workflow first-run`, `workflow guide`, `workflow branch`, `workflow worktree`, `workflow team`, `workflow lead`, `workflow finisher`, `workflow audit`, `workflow tiers`, `workflow hooks`, `workflow await`, and `workflow shutdown` are not implemented in the Rust runtime today — invoking them returns a "not implemented" error rather than starting work. Earlier docs referenced flags such as `--workstream-key` and `--mode` on `workflow start`; the current handler accepts only `--request`, `--preset`, `--claude-home`, and `--json`.

## Five-Minute Path

### 1. Refresh the native shell and verify readiness

```bash
claude-skills install
claude-skills doctor
```

What to look for:
- managed install and binary health
- doctor follow-up guidance if the environment is not ready yet

### 2. Route the broad request to a recommended preset

```bash
claude-skills workflow route --request "Compare the current repo, fix the biggest gaps, and carry the branch to closure"
```

What to look for:
- the recommended preset name (`autopilot`, `debug`, `tdd`, `review`, `eco`, or `parallel`)
- the short rationale for that preset

### 3. Open the workflow ledger entry

```bash
claude-skills workflow start --preset autopilot --request "Compare the current repo, fix the biggest gaps, and carry the branch to closure"
```

The command writes a workflow ledger entry under `~/.claude/workflow/` and prints the new entry id (`wf-...`). Hold onto that id — `cockpit`, `resume`, and `finish` operate against it.

Preset shorthand when the job shape is already obvious:

- `autopilot`
  - `claude-skills workflow start --preset autopilot --request "Carry the current task to closure"`
  - Use when broad feature or maintenance work needs one owner driving to closure.
  - Proof to expect: brief, completion gate, cockpit, review pass, and native finish checks stay current.
  - If interrupted: reopen with `workflow status`, `workflow cockpit`, and `workflow resume --id <entry-id>`.
- `debug`
  - `claude-skills workflow start --preset debug --request "Trace the failing behavior, fix it, and prove it"`
  - Use when the root cause is still unclear or the failure crosses runtime or recovery boundaries.
  - Proof to expect: traced behavior mismatch, narrow repro or regression proof, and hosted-check repair proof when relevant.
  - If interrupted: return through `workflow cockpit` and `workflow resume --id <entry-id>`; use `gh pr checks --watch` for hosted failures.
- `tdd`
  - `claude-skills workflow start --preset tdd --request "Write the proving test first, then implement the feature"`
  - Use when failing-test-first discipline is the safest way to hold scope and prove the change.
  - Proof to expect: failing proof first, fix proof second, regression proof third, plus the normal review and finish checks.
  - If interrupted: use `workflow cockpit`, `workflow resume --id <entry-id>`, and `claude-skills memory completion-gate check --id <entry-id> --proof "..."`.
- `review`
  - `claude-skills workflow start --preset review --request "Audit the current branch and call out the real gaps"`
  - Use when verification is the primary job and implementation is secondary.
  - Proof to expect: reviewer evidence and closeout checks drive the decision.
  - If interrupted: recover from `workflow cockpit` and `workflow resume --id <entry-id>`.
- `eco`
  - `claude-skills workflow start --preset eco --request "Carry the small maintenance task to closure"`
  - Use when the task is smaller but still deserves tracked closure.
  - Proof to expect: same brief, cockpit, and finish structure with the narrowest honest proving validation for the touched scope.
- `parallel`
  - `claude-skills workflow start --preset parallel --request "Coordinate the next multi-lane task"`
  - Use when the work already implies specialist or parallel lanes coordinated by the operator.
  - Proof to expect: every required lane terminal before the entry closes.

### 4. Keep a live watch surface open

```bash
claude-skills workflow dashboard
```

Use the dashboard, `workflow status`, or `workflow watch` to see open and recently closed ledger entries.

### 5. Use the proof-board console when you need one place to look

```bash
claude-skills workflow cockpit
```

Cockpit shows open entries with stage, preset, and request, and the recent tail of closed entries.

### 6. Turn local work into proof before you call it done

```bash
cargo test --workspace
claude-skills review pre-pr --base-ref origin/main
claude-skills git-workflow preflight --repo-root . --base-ref origin/main
```

### 7. If the branch is on GitHub, wait for the real hosted result

```bash
gh pr checks --watch
```

If a hosted lane fails, fix the root cause on the same branch, push again, and rerun `gh pr checks --watch`.

### 8. Close the ledger entry only after the proof is real

```bash
claude-skills workflow finish --id <entry-id> --proof "cargo test --workspace passed; review pre-pr passed; hosted checks green"
```

The `--proof` text is recorded on the closed ledger entry so future audits can see what was claimed when the work was closed.

## Why this is the first success path

- It starts from a broad request instead of forcing workflow vocabulary first.
- It uses only commands the runtime actually implements today.
- It keeps one visible route from intake to proof to closeout against the workflow ledger.
- It ends with real proof attached to the entry, not a confident-looking summary.

## If you want the slightly shorter version

When the task is single-owner and you do not need to route first:

```bash
claude-skills workflow start --request "Carry the current task to closure"
claude-skills workflow cockpit
claude-skills workflow finish --id <entry-id> --proof "tests green"
```

That is the lower-friction default. The routed path above is the better first run when the request still feels broad.
