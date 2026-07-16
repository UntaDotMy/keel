---
name: preserve-existing-flow
description: Traces ownership and current behavior in brownfield code before any edit, so new behavior layers through the existing owner instead of overwriting it. Use proactively before editing any existing source file — handlers, loops, state machines, transport flows, queues, or source-of-truth modules. Returns a working brief with current flow, preserved owner, drift risks, and the safe extension shape.
when_to_use: Pre-edit ownership trace before changing existing behavior in a brownfield codebase.
allowed-tools: Read, Grep, Glob, Bash(keel flow:*), Bash(keel memory:*), Bash(git diff:*), Bash(git log:*), Bash(git show:*)
effort: high
---

# Preserve Existing Flow

## Purpose
Before editing brownfield code: trace ownership and current behavior, then **layer** new behavior through the existing owner — never overwrite the source of truth.

## When
Any edit to existing handlers, loops, state machines, transport, queues, or SoT modules. **Proactive** before the first Write/Edit on existing source.

## Shared discipline
`_shared/common-discipline.md`. Surgical Changes applies: touch only the ownership path you traced.

## Non-negotiables
1. **Trace before change** — entry point, producer, source of truth, consumers, side effects (file:line).
2. **One owner** — extend the existing owner; do not invent a parallel path for the same state/event.
3. **SoT stays SoT** — add fields/branches alongside; do not replace the record/format silently.
4. **Working brief** — capture request, acceptance criteria, current flow, preserved owner, drift risks, recommended change shape **before** coding.
5. **Verify on real path** — exercise the owner path after the change; no "looks right" without a check.

## Procedure
1. Name the target file/function and the user-visible behavior.
2. Read the full function + callers + callees + state writes.
3. Record: entry → owner → SoT → effects.
4. Choose extension shape: wrap / branch inside owner / new helper **called by** owner (not a second writer).
5. Implement only that shape; re-read the owner after edit.
6. Prove with the smallest test or smoke that hits the same path.

## Refuse
- "Quick fix" that bypasses the owner
- Duplicate handlers for the same event
- Dropping fields to match a new format without explicit user ask
- Editing without a brief when the change is non-trivial

## Output (working brief fields)
- current_behavior_to_preserve
- entry_point / producer / source_of_truth / consumers
- drift_risks
- recommended_change_shape
- verification_check

## References
`references/` for ownership-trace patterns and anti-patterns. Load when the surface is large or multi-hop.
