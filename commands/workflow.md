---
description: Start, watch, or finish a claude-core proof-first workflow (route → start → cockpit → finish) over the native JSONL ledger. Use to drive a delivery workstream with tracked proof and closeout discipline.
argument-hint: "[route|start|cockpit|finish] [request or id]"
allowed-tools: Read, Bash(claude-skills workflow:*)
---

# /claude-core:workflow

Drive a claude-core workflow. Arguments: **$ARGUMENTS**

The first argument selects the stage; the rest is the request text or entry id.
Use the installed binary path (bare `claude-skills` is not guaranteed on PATH):
`~/.claude/claude-skills` (macOS/Linux), `%USERPROFILE%\.claude\claude-skills.exe`
(Windows), or `cargo run --bin claude-skills --` from a source checkout.

Map the stage in `$0` to the matching native subcommand:

- `route` → `workflow route --request "<rest of args>"` — pick the recommended preset first.
- `start` → `workflow start --preset <preset> --request "<rest of args>"` — open a workstream (presets: autopilot, debug, tdd, review, eco, parallel).
- `cockpit` (also `status`, `dashboard`, `watch`) → `workflow cockpit` — show open/closed entries, proof state, and the next command.
- `finish` → `workflow finish --id <entry-id> --proof "<proof>"` — close a workstream only with real proof.
- `resume` → `workflow resume --id <entry-id>` — reopen an interrupted workstream.

If no stage is given, run `workflow cockpit` to show current state and ask which
stage to advance. Never claim a workstream is finished without passing real
`--proof`. The subcommands `branch`, `guided-setup`, `await`, and `shutdown` are
not implemented in the current runtime — do not suggest them.
