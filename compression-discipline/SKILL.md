---
name: compression-discipline
description: Per-turn output-compression playbook. Use when the per-prompt UserPromptSubmit hook injects "Output compression is on" — the session has accumulated enough tool calls that context is filling. Three concrete actions: read narrower line ranges, search before reading, summarize logs instead of pasting them.
when_to_use: Heavy investigation turns where the session has already burned a chunk of the context window through earlier tool calls. The auto compression hint hooks this skill on at the threshold.
allowed-tools: Read, Grep, Glob, Bash(claude-skills memory:*)
user-invocable: false
effort: low
---

# Compression Discipline

## Purpose

Reduce per-turn token cost without losing information. This skill applies on every assistant turn after the session has already paid significant context cost on tool output. The auto compression-hint heuristic fires the trigger at `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER` tool calls per session (default 40). Once it fires, every subsequent tool call should follow the rules below.

## Core Rule

Read the smallest window that still answers the question. Search to locate the answer first, then read only the located range. When command output is long, summarize what it shows instead of pasting it back.

## The Three Actions

### 1. Narrow line ranges with `offset` + `limit`

Default to `Read(file, offset, limit)` instead of `Read(file)` when the file is more than a few hundred lines. Once `Grep` or `Glob` has named the relevant line, `Read(file, offset=line-20, limit=80)` is enough to see the function plus context.

The whole-file read is only correct when:
- the file is genuinely small (under ~200 lines)
- you need the file's structure (imports, top-level layout) and a targeted search would miss it
- the user asked for the whole file

### 2. Search before reading

Use `Grep` or `Glob` to locate the symbol first. Then `Read` only the located range.

Concrete pattern:
```
Grep("fn user_prompt_submit_context", path="rust/")  # locate
Read(file, offset=<found_line> - 5, limit=60)        # read window
```

This is the same shape as the project's recall index: search-first, read-second. Never `Read` a 2000-line file looking for a single function — the search tool is faster, cheaper, and more accurate.

### 3. Summarize logs and command output instead of pasting them

When `bash`, `cargo test`, or any tool returns hundreds of lines:
- Quote the 1-3 lines that carry the actual signal (the failing assertion, the exit code, the error name)
- Describe the rest in one sentence ("the prior 80 lines list 80 passing tests; the failure is on line 81")
- Do not paste the entire log into the response

The user already saw the full log on screen. The model's response only needs to interpret it.

## When the Hint Fires

The UserPromptSubmit hook reads today's `<claude_home>/state/tool-timings/<YYYY-MM-DD>.jsonl` and counts rows tagged with the active `session_id`. When the count crosses `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER` (default 40), the per-prompt `additionalContext` payload appends:

> Output compression is on for this turn — context is heavy. Read narrower line ranges (offset+limit) instead of whole files. Search before reading: use Grep/Glob to locate the exact symbol, then Read only the relevant window. Summarize logs and command output instead of pasting them in full. Skill: compression-discipline.

That nudge is the trigger to load this skill and apply the three actions above for the rest of the session.

## Operator Knobs

- `CLAUDE_SKILLS_COMPRESSION_HINT=off` disables the auto hint regardless of threshold.
- `CLAUDE_SKILLS_COMPRESSION_HINT=force` injects the hint on every prompt for diagnostic runs.
- `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER=<N>` sets the per-session row threshold (default 40). Setting it to 0 disables the heuristic.

## Anti-Patterns

- Pasting the entire build log because "the user might want to see it" — they ran the command, they already saw it.
- Reading a 1500-line module to find one function instead of `Grep`-ing first.
- Quoting 50 consecutive lines from a file the model just edited — a 3-line excerpt with a description of the rest is enough.
- "I'll just read everything to be safe" — the safety budget is the context window, not the file.

## Validation

This skill is informational; there is no `claude-skills` subcommand to invoke. The trigger is the auto-compression hint described above. To verify the heuristic is active in a host session, run:

```
claude-skills hook diagnose
```

and inspect that `UserPromptSubmit` is wired through the managed hook entry. To force the hint on for a one-off test run, set `CLAUDE_SKILLS_COMPRESSION_HINT=force` in the session environment.
