---
description: Drive a Scrum-style sprint loop over confirmed user stories (backlog → implement → verify → review → LOOP until Done). Use when multi-story work must finish completely, not partially.
argument-hint: "[plan|status|advance|review|list] [story-id]"
allowed-tools: Read, Bash(claude-skills sprint:*)
---

# /claude-core:sprint

Drive a claude-core sprint. Arguments: **$ARGUMENTS**

Use the installed binary path (bare `claude-skills` is not guaranteed on PATH):
`~/.claude/claude-skills` (macOS/Linux), `%USERPROFILE%\.claude\claude-skills.exe`
(Windows), or `cargo run --bin claude-skills --` from a source checkout.

Map the action in `$0` to the matching native subcommand:

- `plan` → `sprint plan --request "<rest of args>"` — create a sprint from confirmed user stories.
- `status` → `sprint status` — show current sprint state (backlog, in-progress, done, blocked).
- `advance` → `sprint advance --story-id <id>` — move a story to the next state (implement → verify → review → done).
- `review` → `sprint review` — fail-closed gate: verify every story meets Definition of Done before closing.
- `list` → `sprint list` — list all sprints (active and archived).

If no action is given, run `sprint status` to show current state.

**This is the fail-closed loop.** The sprint **must not** close until every story is Done. Do not present partial work as complete. If a story is blocked, document the blocker honestly and continue with unblocked stories. The `sprint review` gate verifies Definition of Done for every story before allowing closure.
