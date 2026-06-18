# Preserve Existing Flow Knowledge Map

Use this map to load only the references needed for the brownfield change at hand.

## Capability Matrix

| Need | Primary Reference |
|---|---|
| Trace the real flow before editing (entry point through cleanup/recovery) | 10-brownfield-investigation-checklist.md |
| Build the working brief (requested vs preserved behavior, owners, evidence) | 10-brownfield-investigation-checklist.md |
| Identify producer / transport / state / queue ownership boundaries | 20-ownership-rules-and-no-overwrite.md |
| Reject direct-send, raw-queue mixing, and main-loop overwrite shortcuts | 20-ownership-rules-and-no-overwrite.md |
| Compare against a reference implementation by role, not feature parity | 20-ownership-rules-and-no-overwrite.md |
| Apply the safe extension pattern (layer beside, route through owner) | 30-safe-extension-and-implementation-gate.md |
| Answer the pre-edit implementation gate before touching code | 30-safe-extension-and-implementation-gate.md |
| Make small, owner-layer patches with re-read discipline | 30-safe-extension-and-implementation-gate.md |
| Decide when to fail a brownfield change in review | 40-review-fail-conditions.md |
| Format the final answer (verified, changed, preserved, validated, blocked) | 40-review-fail-conditions.md |
| Source anchors and related-skill handoffs | 99-source-anchors.md |

## Quick Investigation Sequence

1. Read the request twice. Mark which behavior is requested versus which behavior must stay intact.
2. Walk the flow once before touching code: entry point, producer, source of truth, storage or queue, transport or side-effect owner, consumer, cleanup and recovery.
3. Record the working brief in the global per-workspace flow-check artifact (`keel flow start`). Use `Not found` only for facts actually searched and not located.
4. Identify ownership boundaries. Producer code creates intent. Transport owner code performs side effects. State owner code decides current mode. Queue owner code defines push and pop rules.
5. Choose the safe extension shape: layer the new path beside the original, route through the existing owner, and keep producers side-effect-light.
6. Answer the implementation gate. If the gate cannot be answered, stay in read-only mode and report the gap.
7. Patch in small batches. Touch the owner layer, not only the symptom branch. Re-read the exact function before each batch and the direct callers and callees before finalizing.
8. Run the narrowest useful validation after each meaningful patch batch — startup, runtime, failure, retry, and recovery.
9. Close with the final answer format: verified, changed, preserved, validated, blocked.
