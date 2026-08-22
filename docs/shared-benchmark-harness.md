# Shared Benchmark Harness

This page publishes the shared benchmark harness contract for `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator`.

Canonical machine-readable source: [docs/benchmark-scorecard.json](./benchmark-scorecard.json)

## Why this exists

The benchmark suite in this repository already proves eight realistic workflow scenarios for `keel`.

The remaining gap was cross-repo comparability: one shared scenario contract, one shared scorecard shape, and one shared evidence format that can be reused without changing the rules per repository.

## Shared repositories

- `keel`
- `runtime-shell comparator`
- `workflow-teaching comparator`
- `swarm-automation comparator` for the public automation and swarm-workflow comparator; source-backed shared-harness runs are still pending

## Shared scorecard columns

The shared scorecard uses the same columns for every repository entry:

- `scenario_id`
- `repository_id`
- `source_kind`
- `source_url`
- `evidence_status`
- `commands_or_artifacts`
- `proof_summary`
- `final_outcome`

## Shared evidence format

Every scored scenario entry must capture:

- the shared `scenario_id`
- the target `repository_id`
- a `source_kind` such as `merged_pr`, `release_artifact`, `public_docs`, `hosted_check_run`, or `issue_or_audit_bundle`
- one `source_url`
- the commands or artifacts used as proof
- a short `proof_summary`
- the `final_outcome`

Evidence status stays narrow:

- `source_backed`: the repository has a concrete scenario artifact with commands, proof, and outcome
- `public_surface_only`: the comparison is still based on public docs or public surfaces, not a source-backed shared-harness run
- `planned`: the shared scenario exists, but the repository has not been scored yet

## Competitive Surface Matrix

The machine-readable scorecard now groups the benchmark work into four competitive surfaces so the same evidence shape can be scored across the four repos without changing the columns.

- `operator_shell_and_runtime_workflow`
- `workflow_teaching_and_named_progression`
- `branch_closeout_and_hosted_repair`
- `swarm_automation_and_team_coordination`

For each surface, the scorecard keeps the repo ids, source kinds, source urls, evidence status values, proof summaries, and outcomes visible for `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator`.

`keel` stays source-backed on the repo-local benchmark suite. The peer repos stay public-surface-only until source-backed shared-harness runs are published for them.

## Current honest state

- `keel` already publishes source-backed runs for the tracked eight scenarios in this repository.
- `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator` now share the same scenario ids and evidence fields through this harness contract, while only `keel` has source-backed runs for the full tracked suite today.
- The current benchmark posture audit bundle is published in [docs/audits/2026-04-09-benchmark-posture/audit-summary.md](./audits/2026-04-09-benchmark-posture/audit-summary.md) so the benchmark posture claim is bundled, not only described in prose.
- This repository does not yet claim that every peer repo has completed every shared-harness scenario with source-backed proof.

## swarm-automation comparator Harness Study

swarm-automation comparator's public automation surface is a useful comparator for swarm-style coordination, but its harness model stays public-surface-only here until source-backed shared-harness runs exist.

| swarm-automation comparator primitive | Public swarm-automation comparator surface | Nearest `keel` surface | Where it belongs |
| --- | --- | --- | --- |
| `PhaseRunner` | `spawn`, `task`, `board`, `lifecycle` | Anvil delivery-loop stages (`keel anvil compile|cast|sieve|stamp|run`) | benchmark harness |
| `ArtifactStore` | JSON outputs, snapshots, board exports, plan and task files | benchmark scorecard JSON, audit bundles, release-proof bundles | benchmark harness |
| `ContractExecutor` | `task create`, `task update`, `board show`, `plan approve` | Rust tests under `rust/crates/` | benchmark harness |
| `ContextRecovery` | `context conflicts`, `context inject`, `team restore`, `ralph-loop` | workflow recovery, working buffer, completion gate, runtime preflight | workflow docs |
| `ExitJournal` | snapshots, gource, board history, shutdown and idle records | release-proof bundle and audit summaries | workflow docs |
| `Orchestrator` | `team spawn-team`, `inbox`, `task`, `board`, `lifecycle` | specialist agent profiles and `keel bridge` lifecycle events | workflow docs |
| `Roles` | agent types, profiles, presets | named skill progression and presets | workflow docs |
| `Spawner` | `swarm-automation comparator spawn`, `swarm-automation comparator launch` | multi-agent lanes and worker spawning | benchmark harness and workflow docs |
| `Strategies` | profiles, presets, plan flow, board workflow | benchmark scenario families and workflow presets | workflow docs |

What belongs where:

- Benchmark harness: scenario ids, evidence fields, scorecard columns, and public-surface-only comparator metadata.
- Workflow docs: routing, recovery, handoff, role assignment, and shutdown guidance.
- Out of scope: any source-backed swarm-automation comparator leadership claim until direct shared-harness runs exist.
## What changed

- The benchmark scorecard JSON now includes a `shared_harness` contract with repository ids, scorecard columns, and evidence rules.
- The benchmark suite can now point to one shared harness instead of only repo-local scenario prose.
- The comparison docs can stay honest: the shared harness exists, but peer-repo evidence still has to be collected before broad leadership claims.

## Repeatable validation

```bash
cargo test --workspace
cargo build --release --bin keel
```
