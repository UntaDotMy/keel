# Code Search Demo And Gap Map

This page documents the honest native indexing demo surface and the remaining apples-to-apples gap versus indexing comparator.

## Native Demo Surface

Use the shared native demo command when the operator needs one proof surface that exercises both index creation and query execution:

~~~bash
keel code-search demo --workspace-root "$PWD" --query "incremental lineage proof" --format markdown
~~~

The demo is intentionally not a second indexing engine. It reuses the same native owners as the shipped search path:

- `code-search index` builds the local persisted index
- `code-search search` reuses that local index for retrieval
- `code-search demo` calls the same index and search owners, then adds notes and explicit gap framing

That keeps the demo apples-to-apples with the real runtime surface instead of relying on a synthetic benchmark-only path.

## What The Demo Proves

- the index is local and zero-hosted
- index lineage is visible through build ids, parent build ids, reuse counts, and recompute counts
- search results expose the chunk lineage that explains whether a result came from reused or recomputed data
- one command can show the exact shared index-plus-query path that the native runtime actually uses

## Current Gap Notes Versus indexing comparator

The current native gap notes stay intentionally narrow:

- search is still lexical and symbol-aware rather than embedding-backed semantic retrieval
- the local engine does not yet expose a transformation DAG, target database sync, or multi-target export graph like a full indexing comparator flow
- incremental reuse currently operates at the file and chunk cache layer and does not yet support field-level recomputation planning
- the current surface stays zero-hosted and local-first by design

## Why The Zero-Hosted Posture Stays

This repository is intentionally keeping indexing local and repo-owned:

- no hosted dependency is required to build or query the index
- agents can inspect the build lineage directly from the persisted local index
- review and closure proof can stay inside the same native CLI workflow

That posture keeps the runtime simpler and more inspectable, while the missing indexing comparator-style features are documented as real gaps instead of being implied away.
