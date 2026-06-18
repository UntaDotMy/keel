# Memory Recall Benchmark Bundle

This bundle publishes the current source-backed memory-recall comparison between `keel` and `memory-product comparator`.

Canonical machine-readable source: [docs/memory-recall-benchmark-scorecard.json](./memory-recall-benchmark-scorecard.json)
Published audit bundle: [docs/audits/2026-04-11-memory-recall-benchmark/audit-summary.md](./audits/2026-04-11-memory-recall-benchmark/audit-summary.md)

## Why this exists

The competitive apples-to-apples audit already scored memory depth separately from workflow and indexing. The missing artifact was a dedicated benchmark bundle that explains the memory-recall comparison on its own terms instead of burying it inside the broader scorecard.

## Current honest result

- `keel`: `7/10`
- `memory-product comparator`: `9/10`

`keel` already proves scoped durable memory, completion gates, research cache reuse, memory indexing, relation graph records, typed entity memory, explicit hook capture, and deterministic retrieval packets. `memory-product comparator` still leads the memory-specific benchmark because it ships semantic recall, a four-layer memory model, richer hook-driven save and precompact flows, tool-facing integrations, and a public benchmark runner.

## Benchmark dimensions

- Scoped artifact recall: can the system reopen workstream facts without replaying the full transcript?
- Freshness-aware research reuse: can validated findings be recalled with source and freshness notes?
- Relationship recall: can requirements, decisions, evidence, and touched files be reconnected later?
- Interrupt and resume recovery: can the operator recover the active state after compaction or interruption?
- Reproducible benchmark posture: does the project publish a repeatable benchmark or audit artifact instead of a prose-only claim?

## Evidence sources

- `docs/memoriesv2-long-term-architecture.md`
- `docs/open-source-memory-patterns.md`
- `docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md`
- anonymized memory-product peer README snapshot
- anonymized memory-product benchmark snapshot
- anonymized memory-product layer, hook, and knowledge-graph source snapshots

## Repeatable validation

~~~bash
cargo test --workspace
~~~
