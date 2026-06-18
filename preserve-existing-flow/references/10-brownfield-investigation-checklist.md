# Brownfield Investigation Checklist

Use this reference before editing any existing source file. The job is to understand the real flow first, identify ownership boundaries, and produce evidence the change can be made without overwriting working behavior.

## Working Brief (Required First Step)

Before proposing or editing code, write a short working brief:

- **Requested behavior** — exactly what the user asked for, in one or two sentences.
- **Current behavior to preserve** — what already works and must not regress.
- **Decision owners** — functions, loops, handlers, queues, state, storage, or transport owners that currently decide the behavior.
- **Read-only zones** — files, folders, reference trees, or functions that must not be changed.
- **Evidence needed before implementation** — what must be proven (callers, callees, recovery paths, validation outputs) before code is touched.
- **Mode** — whether the task is read-only, review-only, or implementation-approved.
- **Flow-check artifact status** — whether the global per-workspace flow-check artifact has enough owner-path and validation evidence for existing-source edits.

If the user said "do not change anything yet", stay read-only and report findings only.

## The Seven-Step Flow Trace

Trace the real flow in this order before editing. Treat the first suspicious line as an entry point, not the root cause.

1. **Entry point** — where the event, packet, request, input, interrupt, click, command, or timer begins.
2. **Producer** — where data or intent is created.
3. **Source of truth** — where the final behavior decision is made.
4. **Storage or queue** — where the decision or data is stored for later use.
5. **Transport or side-effect owner** — where the system sends, writes, notifies, persists, renders, or mutates external state.
6. **Consumer** — where the stored value is read and acted on.
7. **Cleanup and recovery** — where success, failure, retry, release, reset, disconnect, reboot, or rollback is handled.

Every brownfield investigation must name an anchor (file path, function, or symbol) for each of the seven roles, or explicitly mark a role `Not found` after a real search.

## Flow-Check Artifact Discipline

Record the same evidence in the global per-workspace flow-check artifact:

```
keel flow start    # opens the artifact for this change
keel flow check    # gate before edits — refuses if evidence is thin
keel flow finish   # gate before final review
```

The artifact must name:

- target file or function
- current behavior to preserve
- entry point
- producer
- source of truth
- storage / state / queue owner
- side-effect owner
- consumers
- cleanup or recovery path
- edit boundary
- validation needed
- validation evidence

Use `Not found` only for facts that were actually searched and not found. Empty rows or vague phrases ("probably here", "looks like") fail the gate.

## Reading Order

When walking an unfamiliar flow, read in this order:

1. **Public entry point** (route handler, command dispatcher, packet handler, interrupt vector, UI event handler).
2. **First level of dispatch** that decides what kind of request this is.
3. **Producer / mapper** that turns the input into the system's internal shape.
4. **Source-of-truth resolver** — config lookup, mode register, state machine, feature flag, persisted decision.
5. **Owner that performs the side effect** — transport drain, writer, queue popper, notifier, renderer.
6. **Cleanup path** — acknowledgment, retry, release, reset, error recovery.
7. **Tests, fixtures, or simulators** that exercise the same flow — they often name the contract better than the production code does.

If two reads cover the same role, the second one is the override; record which one is authoritative.

## When to Stop Reading and Ask

Stop reading and ask the user when:

- the source of truth is split across modules with no clear owner
- two functions appear to perform the same side effect from different places
- a "main loop" is mutating state another scheduler also touches
- documentation, tests, and code disagree on the contract
- the requested change implies migrating ownership (move side effects to a different layer)

Asking is cheap; a wrong overwrite is expensive.
