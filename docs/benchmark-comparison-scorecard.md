# Benchmark Comparison Scorecard

This page keeps the benchmark comparison narrow.

The shared benchmark harness contract now exists, but this page still does not claim that `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator` all have source-backed runs published for every scenario yet. It explains which project currently appears stronger on concrete dimensions that matter to operators, based on the shipped public surfaces plus the eight source-backed scenarios in this repository and the public swarm-automation comparator automation surface.

The broader apples-to-apples audit now lives in [docs/audits/2026-04-09-competitive-apples-to-apples/audit-summary.md](./audits/2026-04-09-competitive-apples-to-apples/audit-summary.md). That bundle keeps workflow, memory, and indexing comparisons separated instead of collapsing them into one blended leaderboard. Comparator identities are anonymized by capability class so the docs do not bake third-party project names into the product surface.

## Current apples-to-apples scores

| Surface                                     | `keel` | Comparator            | Why the gap still exists                                                                                                                                                                                                                                                                                       |
| ------------------------------------------- | -------------- | --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Operator workflow and runtime shell         | `7/10`         | `runtime-shell comparator` `9/10`  | `keel` is stronger on proof and closeout, but `runtime-shell comparator` still exposes a richer runtime shell with setup, doctor, live HUD, repo-local runtime state, and team-runtime flows.                                                                                                                          |
| Phase-based workflow teaching and debugging | `8/10`         | `workflow-teaching comparator` `9/10`  | `keel` ships native presets and proof doctrine, but `workflow-teaching comparator` still teaches brainstorming, writing-plans, executing-plans, systematic-debugging, requesting-code-review, and branch finish more explicitly as first-class skills.                                                                  |
| Automation and swarm harness orchestration  | `8/10`         | `swarm-automation comparator` `9/10`     | `swarm-automation comparator`'s public harness is more automation-centric, with swarm spawning, task, inbox, board, workspace, plan, lifecycle, and template-driven team flows. `keel` still leads on proof-first closeout, review gates, and merge-after-proof discipline.                                               |
| Durable memory system                       | `7/10`         | `memory-product comparator` `9/10`    | `keel` now has scoped durable memory, completion gates, relationship queries, typed entity memory, explicit hook capture, and memory indexing, but `memory-product comparator` still adds a four-layer stack, semantic recall, tool-facing integrations, richer knowledge-graph posture, and reproducible public benchmarks. |
| Indexing and retrieval engine               | `6/10`         | `indexing comparator` `9/10`    | `keel` now ships local code search with incremental lineage, a shared demo surface, and memory indexing, while `indexing comparator` still leads on transformation graphs, code embeddings, vector or full-text retrieval, and multi-target export or sync.                                                      |
| Proof and branch closeout discipline        | `9/10`         | workflow peers `7/10` | The strongest current `keel` advantage is still explicit closure proof: review gates, hosted-check watching, completion ledgers, and merge-after-proof behavior live in one native CLI workflow.                                                                                                       |

No single blended overall score is claimed here. The comparison stays apples-to-apples by surface.

The native entity, hook-capture, lineage, and demo surfaces shipped on `2026-04-15` narrow the memory and indexing gaps, but the conservative published scores stay unchanged until the next source-backed comparator rerun updates the dedicated audit bundle.

## Current winners by dimension

| Dimension                                            | Current winner | Why                                                                                                                                                |
| ---------------------------------------------------- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Repo-local proof and closure discipline              | `keel` | The tracked scenarios show explicit local review, hosted-check watching, completion gates, and merge-after-proof behavior inside one CLI workflow. |
| Runtime-first polish and team HUD feel               | `runtime-shell comparator`  | Its public surface still presents a richer day-to-day runtime shell, role shortcuts, and HUD-style operator experience.                            |
| Phase-based software workflow teaching               | `workflow-teaching comparator`  | Its public surface remains clearer about design gates, TDD discipline, and implementation sequencing as a teaching layer.                          |
| Swarm automation and team coordination               | `swarm-automation comparator`     | Its public harness centers swarm spawning, task and inbox coordination, board and workspace state, and template-driven team automation.            |
| Release-managed native install and status flow       | `keel` | This repository already ships native install, update, status, verify, and uninstall paths for the managed pack.                                    |
| Lightweight setup for users who want a thinner layer | `workflow-teaching comparator`  | Its current install story is lighter for users who mainly want process overlays without the same manager packaging posture.                        |
| First-run conversational shell friendliness          | `runtime-shell comparator`  | Its onboarding and runtime presentation still feel friendlier than the stricter operator posture in this repository.                               |
| Hosted PR rescue and closeout proof                  | `keel` | The tracked PR-fix, branch-closeout, closure-proof, and validation-recovery scenarios are now source-backed and tied to real green outcomes.       |

## What `keel` now proves better than before

- The benchmark suite is no longer based on only two demos.
- The tracked scorecard now covers eight scenario families from real merged work in this repository.
- The shared harness now keeps one scorecard shape and one evidence format for `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and the public swarm-automation comparator.
- The comparison framing is tied to real commands, proof artifacts, and final outcomes instead of broad score claims.

## What is still intentionally unclaimed

- no overall "best the harness layer" ranking
- no claim that every peer repo already has source-backed runs under the shared harness
- no claim that benchmark breadth is complete enough yet for universal workflow conclusions

## How to read this page

Use this scorecard together with [docs/benchmark-suite.md](./benchmark-suite.md), [docs/shared-benchmark-harness.md](./shared-benchmark-harness.md), and [docs/why-keel.md](./why-keel.md).

The benchmark suite shows the repo-local proof. This scorecard explains the narrower win/loss framing. The comparison page explains the practical operator choice.
