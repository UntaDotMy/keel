---
description: Report keel command-output compaction savings (exact o200k_base tokens saved, adapter breakdown, top commands) from the native event log. Use to quantify token ROI from the compaction proxy.
argument-hint: "[since: today|7d|30d|all]"
allowed-tools: Read, Bash(keel gain:*)
---

# /keel:gain

Report keel compaction savings for window: **$ARGUMENTS** (default: today)

Use the installed binary path (bare `keel` is not guaranteed on PATH):
`~/.keel/keel` (macOS/Linux), `%USERPROFILE%\.keel\keel.exe`
(Windows), or `cargo run --bin keel --` from a source checkout.

Run: `gain --since <window>` where window is `today`, a relative range like
`7d`/`30d`, or `all`. Add `--adapter <name>` to filter by reducer family and
`--top N` to change how many top commands are listed. `--json` gives structured
output.

`gain` reads only commands that were actually wrapped through
`keel run -- <command>` (or the PreToolUse rewrite hook). It reports
observed commands, compacted vs passthrough counts, exact tokens
before/after/saved, savings percentage, and per-adapter breakdown. Summarize the
numbers; do not imply savings for commands that were never wrapped.

Always-on hook, instruction, skill-catalog, pointer, and MCP catalog costs are a
separate fixed-context axis. `gain` names that exclusion in text and under
`fixedContextAccounting` in JSON; it does not subtract those costs from command
compaction or count them as savings. Inspect the full runtime ledger with
`keel stats --json --workspace-root <repo>`. See
[`docs/fixed-context-ledger.md`](../docs/fixed-context-ledger.md) for its source
boundaries and CI budget rules.

To find commands that ran *without* compaction (missed savings), run
`gain discover --since <window>` — it groups passthrough commands by name with
the estimated uncompacted tokens they sent to context, so they can be rerouted
through `keel run --`.
