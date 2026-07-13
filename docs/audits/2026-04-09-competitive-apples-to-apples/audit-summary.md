# Competitive Apples-To-Apples Audit Summary

- Audit date: 2026-04-09
- Audited repo: `UntaDotMy/keel` at `6832b0e70bd76861c3cdf5db4883427783dc7f3e`
- Evidence sources: `README.md`, `todo.md`, `docs/benchmark-comparison-scorecard.md`, `docs/memory-families-usage.md`, `docs/open-source-memory-patterns.md`, and anonymized peer-audit snapshots for runtime-shell, workflow-teaching, memory-product, and indexing comparator classes.

## Scoring rule

This audit keeps the comparison apples-to-apples:

- workflow shell is compared against `runtime-shell comparator`
- plan and debug pedagogy is compared against `workflow-teaching comparator`
- memory product depth is compared against `memory-product comparator`
- indexing and retrieval depth is compared against `indexing comparator`

No single overall /10 is claimed because the comparison is intentionally apples-to-apples by surface, not a blended leaderboard.

## Current scores

| Surface | `keel` | Comparator | Evidence-backed conclusion |
| --- | --- | --- | --- |
| Operator workflow and runtime shell | `7/10` | `runtime-shell comparator` `9/10` | `keel` already wins on proof-first closeout through `workflow cockpit`, `workflow finish`, review gates, and completion ledgers. `runtime-shell comparator` still leads the day-to-day shell with setup, doctor, live HUD, repo-local runtime state, team-runtime, and terminal-multiplexer coordination. |
| Phase-based workflow teaching and debugging | `8/10` | `workflow-teaching comparator` `9/10` | `keel` ships native presets and stricter finish rules, but `workflow-teaching comparator` still teaches brainstorming, writing-plans, executing-plans, systematic-debugging, requesting-code-review, and branch finish more explicitly as first-class skills. |
| Durable memory system | `7/10` | `memory-product comparator` `9/10` | `keel` has scoped durable memory, completion gates, research cache, and memory indexing. `memory-product comparator` still leads as a dedicated memory product through semantic recall, a four-layer stack, hook-driven save and precompact flows, tool-facing integrations, a knowledge graph, and reproducible benchmark runners. |
| Indexing and retrieval engine | `6/10` | `indexing comparator` `9/10` | `keel` currently ships lexical and symbol-aware code search plus incremental memory indexing. `indexing comparator` already ships incremental processing, lineage, query handlers, code embeddings, vector search, and full-text retrieval across multiple targets. |
| Proof and branch closeout discipline | `9/10` | workflow peers `7/10` | The strongest current `keel` advantage is still explicit closure proof: review gates, hosted-check watching, completion ledgers, and merge-after-proof behavior live in one native CLI workflow. |

## Main findings

- The current growth path is not more governance prose. The real gaps are operator shell polish, phase-teaching ergonomics, memory recall depth, and indexing depth.
- `docs/memory-families-usage.md` already frames the indexing comparator class as the target shape for local incremental indexing and lineage, but that posture is still roadmap, not shipped parity.
- The current public workflow-teaching comparator used for this audit is anonymized by capability class so future benchmark claims stay honest without hardcoded project names.
- `keel` should protect its current strength: proof and closeout discipline already beats the workflow peers. The roadmap should extend that advantage into daily UX, memory quality, and indexing depth rather than weaken it.
