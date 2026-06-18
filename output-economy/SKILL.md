---
name: output-economy
description: Per-response output-token economy. Use when the user asks for terse/concise output, when a response would otherwise repeat tool output the user already saw, or when long-running sessions need the model's own replies to stop wasting output tokens. Cuts reply verbosity without dropping technical signal — the opposite axis from compression-discipline (which cuts input/tool-read cost).
when_to_use: Any turn where the response itself is the token cost — recaps, summaries, status updates, or repeated explanations. Distinct from compression-discipline, which governs how much you read in.
allowed-tools: Read, Grep, Glob
effort: low
---

# Output Economy

## Purpose

Reduce the tokens the model *emits*, not the tokens it reads. `compression-discipline`
governs input cost (narrower reads, search-first, summarizing logs). This skill
governs **output** cost: the reply itself. The goal is the same answer in fewer
output tokens, with zero loss of technical signal — the brevity-as-discipline
idea, applied to the model's own speech rather than the tools it calls.

## Core Rule

Say the thing once, at the right level of detail, and stop. The user already saw
the tool output on screen; the reply interprets it, it does not re-narrate it.

## The Five Cuts

### 1. Don't restate what the tool already showed
After a test run, command, or diff, the user saw the full output. Quote the 1-3
lines that carry the verdict (the failing assertion, the exit code, the count)
and give the conclusion. Do not paste the log back or walk every line.

### 2. Drop the preamble and the postamble
No "Great question!", no "Let me explain…", no "In summary, as you can see…".
Lead with the answer. End when the answer ends. A closing recap is only worth
its tokens when the task was multi-step and the user needs the final-state map.

### 3. One example beats three
When illustrating a pattern, show the single clearest example and name the rest.
Three near-identical snippets cost 3x the tokens for ~1x the understanding.

### 4. Prefer structure over prose for enumerable things
A short list or table of N items is denser than N sentences. But do not wrap a
one-line answer in a heading + table scaffold — structure earns its tokens only
when there is genuinely structured content.

### 5. Match length to the task
A yes/no question gets a sentence. A one-line fix gets the line plus where it
goes. Reserve multi-paragraph explanations for genuinely multi-part work. Length
should track the question, not a fixed template.

## What NOT to compress

Output economy never trades away correctness or safety:

- **Code blocks, commands, file paths, and error strings** stay verbatim — never abbreviate a symbol or truncate a path the user must copy.
- **Safety-critical context** (destructive-action warnings, security caveats, irreversible steps) stays explicit. Terseness here causes misreads.
- **Required proof** (what was verified, what failed, what is still open) is not preamble — it is the answer for a completion claim. Keep it.
- **Reasoning the user asked for.** If they asked "why", the explanation is the deliverable.

## Relationship to compression-discipline

| Axis | Skill | Governs |
| --- | --- | --- |
| Input / tool reads | `compression-discipline` | how much you read in (narrow ranges, search-first, summarize logs) |
| Output / replies | `output-economy` (this skill) | how much you write out (no preamble, no re-narration, length tracks task) |

Both can be active at once on a heavy turn. They do not conflict: one shrinks
what enters context, the other shrinks what leaves it.

## Anti-Patterns

- Re-pasting a build log the user just watched scroll by.
- Opening every reply with a one-sentence acknowledgment before the actual content.
- A "Summary" section that restates a three-sentence answer.
- Three code samples that differ by one line.
- Wrapping a one-line answer in headers, a table, and a closing note.

## Validation

This skill is informational; there is no `keel` subcommand to invoke. It
is matched by the description above when a turn is output-heavy. To self-check a
drafted response: if removing a sentence loses no technical signal, remove it.
