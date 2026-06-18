<!--
Purpose: Record the honest memory-product-comparator-inspired adoption order for keel.
Caller: Contributors and agents checking which memory-product-comparator-style features were adopted and why.
Dependencies: memoriesv2 retrieve, graph query, entity, and hook capture surfaces.
Main Functions: Document adoption order, rationale, and shipped status.
Side Effects: None.
-->
# Memory-Product Comparator Adoption Decision

## Scope

This note records the honest memory-product-comparator-inspired adoption order for `keel` and the current shipped status after the semantic-recall work landed on `memoriesv2 retrieve`.

Comparison date: `2026-04-13`

Live comparator surfaces reviewed:

- `external-memory-product-source`
- `external-runtime-shell-source`
- `external-workflow-teaching-source`
- `external-indexing-source`

## Decision

The next memory-product-comparator-style features were adopted in this order:

1. Relationship queries
2. Hook-driven save loops
3. Entity memory
4. No new benchmark-harness work yet

## Why This Order

### 1. Relationship queries first

`keel` already stores normalized graph edges and already uses those edges for graph-backed semantic recall. The missing product surface is queryability. Before this change, operators could only `graph add` and `graph list`, which meant the graph existed but was not yet a first-class retrieval surface.

memory-product comparator already exposes richer graph querying through its local knowledge-graph layer, including entity-focused traversal and relationship-focused access. The closest honest gap in this repository was therefore not raw storage, but the lack of a native query command over the graph it already maintains.

### 2. Hook-driven save loops second

memory-product comparator's hook posture is still stronger than `keel` on automatic save triggers and precompact interception. That remains important, but it is a riskier fit here because this repository deliberately prefers explicit, proof-first native commands over hidden prompt-only automation or fragile shell interception.

The shipped hook pass therefore became an explicit native surface instead of a rushed interception copy:

- `memoriesv2 hook capture --hook <name> --summary <text>`

That keeps save-loop parity inspectable and caller-driven while still narrowing the memory-product comparator gap honestly.

### 3. Entity memory third

memory-product comparator has a dedicated entity registry and temporal knowledge graph. Relationship-query usage proved that a separate typed entity surface was worth shipping, but it still fits best on the existing canonical graph owner rather than as a second top-level store.

The shipped entity pass is now:

- `memoriesv2 entity upsert`
- `memoriesv2 entity list`
- `memoriesv2 entity query`
- entity recall appended inside `memoriesv2 retrieve`

### 4. Benchmark harness already strong enough for now

This repository already ships a source-backed competitive audit bundle, a dedicated memory-recall benchmark bundle, and a shared harness contract for the workflow comparison repos. memory-product comparator still has the stronger memory-specific benchmark story, but the next product gap is runtime capability, not another harness document.

## Applied Change In This Branch

This branch now follows the adoption order by shipping the three runtime slices directly:

- `memoriesv2 graph query --node <text>`
- `memoriesv2 graph query --relation <type>`
- `memoriesv2 graph query --contains <text>`
- `memoriesv2 hook capture --hook <name> --summary <text>`
- `memoriesv2 entity upsert|list|query`
- typed entity recall inside `memoriesv2 retrieve`

That keeps the memory-product comparator-inspired additions native, reviewable, and directly tied to the live comparator gap without drifting into hidden interception or a duplicate memory store.
