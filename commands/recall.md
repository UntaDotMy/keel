---
description: Search durable claude-core memory (working briefs, system maps, memoriesv2) by keyword via the native FTS5 recall index. Use to recover prior decisions, traces, and context before re-researching.
argument-hint: "[search terms]"
allowed-tools: Read, Bash(claude-skills memory recall:*)
---

# /claude-core:recall

Search the claude-core durable memory index for: **$ARGUMENTS**

Run the native recall search using the installed binary (the bare name `claude-skills`
is not guaranteed on PATH — prefer the explicit installed path). The query is a
positional argument, not a flag:

- macOS / Linux: `~/.claude/claude-skills memory recall "$ARGUMENTS"`
- Windows: `%USERPROFILE%\.claude\claude-skills.exe memory recall "$ARGUMENTS"`
- Source checkout: `cargo run --bin claude-skills -- memory recall "$ARGUMENTS"`

Add `--limit N` to cap results and `--json` for structured output.

If no arguments were supplied, ask what to search for, or run
`claude-skills memory recall status` first to report index health (document
count, schema version, last-sync timestamp) so the user knows whether the index
is populated. The index refreshes automatically on every call.

Summarize the matches found — do not paste the raw index. Cite which working
brief, system map, or memoriesv2 artifact each hit came from so the user can
open it. If the recall index is empty or stale, say so plainly instead of
implying memory exists.
