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

- `compile --goal "..." --bar "..."` — write lock + prefix + gates to the global bank
- `cast --dry-run` — isolated temp workspaces; results land in the bank, temps are deleted
- `sieve` — run lock gates
- `stamp --dry-run` — evidence rank (passing gate, then smaller clipped output)
- `loop --dry-run` — bounded refine
- `run --dry-run` — full offline pipeline
- `run` — live pipeline; the current host CLI does the LLM work (no external model client)

Bank path: `<keel-home>/memories/workspaces/<slug>/anvil/` (`--claude-home` / `KEEL_HOME`). Not `{cwd}/anvil/`. Any CLI sharing that home resumes the same lock/prefix/gates/report.

If no surface is given, print `keel anvil help`.
