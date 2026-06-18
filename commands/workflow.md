---
description: Start, watch, or finish a keel proof-first workflow (route → start → cockpit → finish) over the native JSONL ledger. Use to drive a delivery workstream with tracked proof and closeout discipline.
argument-hint: "[route|start|cockpit|finish] [request or id]"
allowed-tools: Read, Bash(keel workflow:*)
---

# /keel:workflow

Drive a keel workflow. Arguments: **$ARGUMENTS**

The first argument selects the stage; the rest is the request text or entry id.
Use the installed binary path (bare `keel` is not guaranteed on PATH):
`~/.claude/keel` (macOS/Linux), `%USERPROFILE%\.claude\keel.exe`
(Windows), or `cargo run --bin keel --` from a source checkout.

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
