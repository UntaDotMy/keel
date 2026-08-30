---
description: Drive the Anvil delivery loop (compile, cast, sieve, stamp, loop, run).
argument-hint: "[compile|cast|sieve|stamp|loop|run|prefix-check] [flags]"
allowed-tools: Read, Bash(keel anvil:*), Bash(keel memory:*), Bash(keel review:*)
---

# /keel:anvil

Run the Anvil delivery loop. Arguments: **$ARGUMENTS**

Use the installed binary path (bare `keel` is not guaranteed on PATH):
`~/.keel/keel` (macOS/Linux), `%USERPROFILE%\.keel\keel.exe`
(Windows), or `cargo run --bin keel --` from a source checkout.

- `compile --goal "..." --bar "..." --files "<owned files csv>"`: write lock + prefix + gates to the scoped bank
- `cast --dry-run`: validate/plan only (`writes=0 executes=0`)
- `sieve`: run lock gates
- `stamp --dry-run`: validate/plan only (`writes=0 executes=0`)
- `loop --dry-run`: validate/plan only (`writes=0 executes=0`)
- `run --dry-run`: validate/plan the pipeline only (`writes=0 executes=0`)
- `run`: live pipeline; the current host CLI does the LLM work (no external model client)

Only live `cast` and live `run` create cast/gate/winner evidence. No dry-run
stage may write an artifact or execute a builder or gate.

Bank path: `<keel-home>/memories/workspaces/<slug>/anvil/` (`--claude-home` / `KEEL_HOME`). Not `{cwd}/anvil/`. Any CLI sharing that home resumes the same lock/prefix/gates/report.
Relative `--workspace-root .` is first resolved to the absolute current scoped
lane; it does not select a global `workspaces/anvil` bank.

If no surface is given, print `keel anvil help`.
