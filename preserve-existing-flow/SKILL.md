---
name: preserve-existing-flow
description: Traces ownership and current behavior in brownfield code before any edit, so new behavior layers through the existing owner instead of overwriting it. Use proactively before editing any existing source file — handlers, loops, state machines, transport flows, queues, or source-of-truth modules. Returns a working brief with current flow, preserved owner, drift risks, and the safe extension shape.
when_to_use: Pre-edit ownership trace before changing existing behavior in a brownfield codebase.
allowed-tools: Read, Grep, Glob, Bash(keel flow:*), Bash(keel memory:*), Bash(git diff:*), Bash(git log:*), Bash(git show:*)
effort: high
---

# Preserve Existing Flow

## Purpose

Use this skill to avoid accidental overwrites in existing systems. It applies to brownfield codebases, firmware, drivers, desktop apps, backend services, frontend apps, CLIs, protocol handlers, transport layers, queues, loops, schedulers, callbacks, and state machines where existing behavior already works.

The job is to understand the current flow first, identify ownership boundaries, then route new behavior through the correct owner instead of bypassing or replacing existing logic.

## Core Rule

Do not modify, replace, bypass, or call around an existing flow until the current ownership path is understood.

Layer new behavior beside the current path, or route it through the same owner pattern, unless the user explicitly approves replacing or migrating the flow.

This is a universal pre-edit gate for existing source files. Run a Preserve Existing Flow check before editing any existing source file unless the task is docs-only, formatting-only, generated-only, or explicitly greenfield.

## Data and Output Preservation

The same no-overwrite rule applies to data, not just control flow. When editing any output, record, report, schema, config, or data structure:

- **ADD, never silently REPLACE.** Adding a field, column, value, or output means adding it alongside what exists. Do not delete, overwrite, or restructure existing data unless the user explicitly named the thing to remove.
- **Derived never displaces source.** A computed value (deviation, total, percentage) is additive — it never removes the measured value it was derived from.
- **"Match this format/template/example" copies STYLE only**, never the reference's omissions. Adopting a layout does not authorize dropping fields the reference happens to lack.
- **Removal needs an explicit current instruction naming the target.** "Remove the X column" authorizes it; "make it like theirs", "proceed", or silence do not.
- **Flag-After tripwire.** If you are about to write "note: I removed/changed X" after acting, that is proof you should have asked before acting. Ask first.
- **Will the new behavior change the meaning of existing data?** If yes, that is an ownership change — get explicit approval before proceeding (see the Implementation Gate below).

## Code Implementation Discipline

See `_shared/common-discipline.md` § Code Implementation Discipline for the canonical rules — four behavioral pillars (Think Before Coding, Simplicity First, Surgical Changes, Goal-Driven Execution) plus tactical rules (YAGNI, no shortforms, no silent fallbacks, no duplication, less comments + structured doc tags). They reinforce this skill's core rule: routing through the existing owner is the no-duplication / no-parallel-path principle expressed at the architecture layer. Surgical Changes maps directly to "do not refactor adjacent code while extending the owner". Reject extensions that copy the owner's logic into a new file or wrap it in a "compat shim" instead of extending the owner directly.

## Required First Step

Before proposing or editing code, produce a short working brief:

- requested behavior
- current behavior that must be preserved
- functions, loops, handlers, queues, state, storage, or transport owners that currently decide the behavior
- files, folders, reference trees, or functions that must not be changed
- evidence needed before implementation
- whether the task is read-only, review-only, or implementation-approved
- whether the global per-workspace flow-check artifact has enough owner-path and validation evidence for existing-source edits

If the user says "do not change anything yet", stay read-only and report findings only.

## Brownfield Investigation Checklist

Trace the real flow in this order before editing:

1. Entry point: where the event, packet, request, input, interrupt, click, command, or timer begins.
2. Producer: where data or intent is created.
3. Source of truth: where the final behavior decision is made.
4. Storage or queue: where the decision or data is stored for later use.
5. Transport or side-effect owner: where the system sends, writes, notifies, persists, renders, or mutates external state.
6. Consumer: where the stored value is read and acted on.
7. Cleanup and recovery: where success, failure, retry, release, reset, disconnect, reboot, or rollback is handled.

Treat the first suspicious line as an entry point, not the root cause.

Record the same evidence in the global per-workspace flow-check artifact with `keel flow start`, then run `keel flow check` before editing and `keel flow finish` before final review. The artifact must name the target file or function, current behavior to preserve, entry point, producer, source of truth, storage/state/queue owner, side-effect owner, consumers, cleanup or recovery path, edit boundary, validation needed, and validation evidence. Use `Not found` only for facts that were actually searched and not found.

## Ownership Rules

Identify and preserve ownership boundaries:

- Producer code may create intent or data, but should not directly perform transport or side effects unless it is already the established owner.
- Transport owner code sends data, notifies hosts, writes to external systems, pops queues, acknowledges packets, or performs I/O.
- State owner code decides current state, mode, mapping, persistence, or recovery behavior.
- Queue owner code defines packet shape, push rules, pop rules, overflow behavior, and success or failure handling.

If new behavior needs another report type, command type, event type, or message type, prefer a typed packet or layered path through the existing queue or handler pattern. Do not push incompatible data into an old raw queue unless every consumer is updated together.

## No-Overwrite Rules

Reject these shortcuts:

- Do not directly send from mapping, business, or producer code when an existing transport drain owns sending.
- Do not put a new payload shape into an existing queue unless every consumer of that queue is updated together.
- Do not modify a main loop just because a new feature needs polling; first find the existing scheduler, tick owner, callback, or event source.
- Do not duplicate an existing function with a similar new function unless the old owner is proven wrong for the new behavior.
- Do not replace original behavior when a layered extension can preserve it.
- Do not patch only one consumer branch if the source of truth is somewhere else.
- Do not add speculative fallback behavior to hide an untraced root cause.
- Do not refactor unrelated code while adding the requested behavior.
- Do not remove or replace an existing field, column, output, or record to fit a new format — add alongside and ask before dropping anything the user did not name.

## Reference Comparison Rule

When a reference implementation exists, compare by role instead of copying blindly:

- entry point to entry point
- producer to producer
- queue or storage to queue or storage
- transport owner to transport owner
- recovery path to recovery path

Copy the architecture pattern, not necessarily the exact feature set. If the reference only supports one report type but the current product needs multiple report types, preserve the reference flow idea while adding typed or layered handling for the extra report types.

## Safe Extension Pattern

Prefer this implementation shape:

1. Keep the original path intact.
2. Add a new typed intent, packet, command, event, or adapter beside the original path.
3. Route new behavior through the same owner layer that already performs side effects.
4. Keep producers side-effect-light.
5. Pop, acknowledge, clear, or finalize only after the owner confirms success.
6. Validate startup, runtime, failure, retry, and recovery paths.

## Implementation Gate

Before editing, answer these questions:

- What currently works?
- Which function or module owns the current behavior?
- Which function only produces data?
- Which function performs the side effect?
- Which queue, state, storage format, or transport contract is being reused?
- Will the new behavior change the meaning of existing data?
- What other consumers read this data?
- What breaks if this structure changes?
- Is this a layered extension or an overwrite?
- Is user approval needed because ownership is changing?

If these cannot be answered, do not edit. Continue reading and report the gap.

## Reporting Format Before Code

When asked to understand first, report in this structure:

- Current flow: concise step-by-step flow with file and function anchors.
- Preserved owner: the original function or module that should remain authoritative.
- Drift or risk: where current or planned code bypasses, overwrites, duplicates, or mixes ownership.
- Recommended shape: how to layer the change without replacing the original flow.
- Implementation boundary: files and functions that would need changes later, if approved.
- Blockers or unknowns: facts not proven yet.

## Code Change Rules

When implementation is approved:

- Make small patches.
- Touch the owner layer, not only the symptom branch.
- Re-read the exact function before patching.
- Re-read direct callers and callees before finalizing.
- Keep old working behavior unless the user explicitly asked to replace it.
- Delete dead duplicate logic only when the replacement owner is proven and validated.
- Add comments only for non-obvious ownership or protocol rules.
- Run the narrowest useful validation after each meaningful patch batch.

## Review Fail Conditions

Fail the change if it:

- bypasses an existing owner without explaining why
- sends, writes, notifies, persists, or mutates external state from a producer path that should only create intent
- mixes different packet or data shapes in one raw queue without updating every consumer
- changes a loop, scheduler, interrupt handler, callback, or recovery path without tracing dependent behavior
- copies reference code blindly instead of preserving the reference architecture pattern
- presents a partial slice as complete while another required owner path is still inconsistent
- changes original or reference files that were marked read-only
- ignores a user instruction to report first or avoid edits

## Final Answer Requirements

When finished, state:

- what was verified
- what changed, if anything
- what existing behavior was preserved
- what validation ran
- what remains unverified or blocked

Never claim production-ready completion if ownership, recovery, or validation is incomplete.

## Reference Files

Deep guidance in `references/`. Load on demand:

- `00-preserve-flow-knowledge-map.md` — Capability matrix and quick investigation sequence
- `10-brownfield-investigation-checklist.md` — Working brief, seven-step flow trace, flow-check artifact discipline
- `20-ownership-rules-and-no-overwrite.md` — Producer / transport / state / queue boundaries, rejected shortcuts, reference comparison rule
- `30-safe-extension-and-implementation-gate.md` — Safe extension pattern, pre-edit gate questions, code change rules, batch validation discipline
- `40-review-fail-conditions.md` — Fail conditions, severity ladder, final answer format, reviewer handoff
- `99-source-anchors.md` — SKILL.md section map, related skills, tooling anchors
