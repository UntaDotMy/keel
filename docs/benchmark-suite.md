<!--
Purpose: Explain the benchmark and demo suite that backs the published workflow proof posture.
Caller: Contributors, reviewers, and readers inspecting benchmark evidence surfaces.
Dependencies: Shared benchmark harness docs, scorecards, audit bundles, and demo docs.
Main Functions: Describe benchmark scope, current scorecard coverage, and proof boundaries.
Side Effects: Sets expectations for published benchmark claims in this repository.
-->
# Benchmark And Demo Suite

This suite tracks realistic operator scenarios instead of synthetic speed claims.

The shared harness contract now lives alongside this suite, so the same scenario ids and evidence fields can be reused across `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and the public swarm-automation comparator without changing the scorecard rules per repo.
The shared harness contract now also publishes a competitive surface matrix for operator shell, workflow teaching, branch closeout, and swarm automation, so the same fields stay visible across `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator`.

The current benchmark shapes are based on the workflow problems this repository is trying to solve better:

- Greenfield feature delivery that improves the first-run experience without weakening proof
- Stateful bug fixes with explicit root-cause tracing across durable workflow state
- Hosted PR rescue when a real CI lane fails after local proof was green
- Branch closeout that stays honest about proof before merge
- Multi-agent coordination where closure proof must come from the right source of truth
- Windows-heavy validation and recovery flows that must survive hosted matrix timing
- Docs-only or workflow-governance changes that still need executable proof
- Regression hardening for operator-visible defaults and contract coverage

The style targets come from the public workflow surfaces of `runtime-shell comparator`, `workflow-teaching comparator`, and `swarm-automation comparator`, but the scorecard here stays strict about evidence. The shared harness contract now exists, while the populated source-backed runs in this repository still measure whether `keel` can carry real branches through those problem shapes with explicit proof.

Published benchmark posture audit bundle: [docs/audits/2026-04-09-benchmark-posture/audit-summary.md](./audits/2026-04-09-benchmark-posture/audit-summary.md)

That bundle is the durable trust artifact for the current benchmark posture. It proves that `keel` ships source-backed runs for the tracked eight scenarios and a shared benchmark harness contract, while peer repositories still need more source-backed shared-harness entries before broader leadership claims are justified.

## Current Scorecard

Shared harness and tracked scorecard source: [docs/benchmark-scorecard.json](./benchmark-scorecard.json)

Harness explainer: [docs/shared-benchmark-harness.md](./shared-benchmark-harness.md)

Comparison framing: [docs/benchmark-comparison-scorecard.md](./benchmark-comparison-scorecard.md)

| Scenario                                      | Real source                                                 | Success metrics                                                                                     | Demo                                                                  |
| --------------------------------------------- | ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `greenfield_feature_delivery_route_ux`        | [PR #41](https://github.com/UntaDotMy/keel/pull/41) | shorter first-run Anvil command, clearer mode explanation, 6/6 hosted checks green | [Greenfield feature demo](./demo-greenfield-feature-flow.md)          |
| `stateful_bug_fix_trace_drift`                | [PR #27](https://github.com/UntaDotMy/keel/pull/27) | durable memory ownership and compatibility drift reporting hardened; 6/6 hosted checks green       | (demo doc removed; see PR #27)                                          |
| `pr_fix_windows_hosted_check`                 | [PR #32](https://github.com/UntaDotMy/keel/pull/32) | 1 hosted Windows failure repaired, 1 fix commit, 6/6 hosted checks green                            | [PR-fix demo](./demo-pr-fix-flow.md)                                  |
| `branch_closeout_release_docs`                | [PR #33](https://github.com/UntaDotMy/keel/pull/33) | clean branch closeout path, 0 fix commits after PR open, 6/6 hosted checks green                    | [Branch-closeout demo](./demo-branch-closeout-flow.md)                |
| `multi_agent_closure_proof`                   | [PR #38](https://github.com/UntaDotMy/keel/pull/38) | hosted reviewer proof accepted for closure, no fake reviewer lane required, 6/6 hosted checks green | [Multi-agent closure demo](./demo-multi-agent-closure-flow.md)        |
| `windows_validation_recovery_stable_checkout` | [PR #36](https://github.com/UntaDotMy/keel/pull/36) | stable checkout refs across delayed matrix lanes, 6/6 hosted checks green                           | [Windows validation demo](./demo-windows-validation-recovery-flow.md) |
| `docs_governance_first_success_path`          | [PR #43](https://github.com/UntaDotMy/keel/pull/43) | first-success guide published, workflow docs linked, 6/6 hosted checks green                        | [Docs governance demo](./demo-docs-governance-flow.md)                |
| `regression_hardening_autopilot_defaults`     | [PR #45](https://github.com/UntaDotMy/keel/pull/45) | Anvil compile defaults hardened with explicit regression coverage, 6/6 hosted checks green | [Regression-hardening demo](./demo-regression-hardening-flow.md)      |

## Scenario Family Coverage

The tracked suite covers these scenario families in the published scorecard and audit bundle:

- Greenfield feature delivery: `greenfield_feature_delivery_route_ux`
- Stateful bug fix with root-cause tracing: `stateful_bug_fix_trace_drift`
- Hosted PR failure rescue: `pr_fix_windows_hosted_check`
- Branch closeout with zero post-PR fixes: `branch_closeout_release_docs`
- Multi-agent coordination on independent lanes: `multi_agent_closure_proof`
- Windows-heavy validation or recovery flow: `windows_validation_recovery_stable_checkout`
- Docs-only or workflow-governance change: `docs_governance_first_success_path`
- Regression fix with explicit test hardening: `regression_hardening_autopilot_defaults`

## What this suite proves today

- The repo now has 8 tracked benchmark scenarios instead of 2 demos.
- The repo can publish proof across greenfield delivery, bug fixing, hosted recovery, closeout, governance, and regression-hardening shapes without changing the benchmark rules midstream.
- Success metrics are tracked as proof counts and closure outcomes, not as vague "felt faster" language.
- The comparison framing is now explicit about where `keel`, `runtime-shell comparator`, `workflow-teaching comparator`, and the swarm-automation comparator each currently win.

## Compaction and Catalog Footprint Benchmarking

In addition to workflow scenarios, the repository measures token efficiency at runtime via `keel eval` and the fixed context ledger:

- Compaction evaluation suite: [`bench/competitor/README.md`](../bench/competitor/README.md)
- Compaction benchmark report: [`bench/competitor/eval-compaction-report.json`](../bench/competitor/eval-compaction-report.json)
- Comparative analysis metrics: [`bench/competitor/comparative-compaction.json`](../bench/competitor/comparative-compaction.json)
- Measured compaction savings across 7 genuine toolchain fixtures: **27.28% overall** (with `cargo test` pass achieving **79.26%** savings).
- Reducer reliability over the same fixtures: **100% critical-evidence retention**, **100% failure-evidence retention**, and a **0% reacquisition rate**, so the saving does not cost the model the evidence it needs to act.
- Historical Phase 12 MCP catalog snapshot: 2,929 complete-catalog tokens down to **1,240** (**57.66% reduction**), while retaining direct dispatchability for all 37 tools. The historical values remain in the comparison artifact for auditability.
- Current target-runtime MCP verification: 2,944 complete-catalog tokens down to **1,240** (**57.88% reduction**, 1,704 tokens saved); emitted default first pages are 1,483 down to **711** (**52.06% reduction**, 772 tokens saved). The current measurements and source commit are recorded in [`bench/competitor/comparative-compaction.json`](../bench/competitor/comparative-compaction.json) under `current_runtime_verification`.

`keel eval --json` reports per-case `evidenceRetained`/`evidenceTotal`, `outcomeVisible`,
`reacquisitionRequired`, and `reduceMicros` alongside the token counts, plus the
aggregate `evidenceRetentionPercent`, `failureEvidenceRetentionPercent`, and
`reacquisitionRatePercent`. Reduction is only a win when the saving and the
evidence retention move together.

Both benchmarks declare their thresholds in the artifact before the numbers are
interpreted, and the regression tests assert against those same constants, so a
declared floor and its check cannot drift apart. Reducer floors: 100% evidence and
failure-evidence retention, 20% overall savings, 50% on the high-volume fixture,
at least three compacted fixtures, and a 0% reacquisition rate. Catalog floors:
every page within its declared budget, zero duplicate and zero omitted tools,
100% discovery coverage, and a 2000 ms ceiling on the catalog walk.

The MCP catalog profiles have their own comparison. `keel stats tools --benchmark --json`
walks `full`, `core`, `core+pagination`, `core+progressive-discovery`, and
`core+progressive-discovery+compact-schemas` through the canonical packing path, then
reports the exact first-page cost, pages to traverse, unique tools reached, and
discovery coverage across twelve representative task families. It fails closed when a
page exceeds its budget, traversal duplicates or omits a tool, or discovery misses a task.

The 2026-09-09 gateway baseline is retained in
[`docs/benchmarks/context-gateway-baseline.json`](./benchmarks/context-gateway-baseline.json),
with a clearly labelled current-runtime verification block for source commit
`30ecfe2`. Reproduce the context and catalog measurements with `keel stats context --json` and
`keel stats tools --benchmark --json`; the artifact deliberately records quality, latency,
turn-count, and provider-cache gates that must be ratified before changing the
catalog profile. A catalog footprint alone is not evidence of a successful
profile transition.

The skill gateway has a separate benchmark because skill activation and reference
budgets are independent surfaces. `keel skill-eval --benchmark --json` runs three
times per deterministic task and compares `all-skills-eager`, `metadata-only`,
`metadata+selective-activation`, `metadata+cost-aware-selection`, and
`metadata+cost-aware-selection+resource-on-demand`. It reports selection accuracy,
activation precision/recall, wrong and missed activation rates, measured tokenizer
input, reacquisition calls, turns, latency, peak context, recovery, and conflict
rate. Model-visible skill tokens are counted separately from local routing CPU
time, so cheap local work is never reported as model cost. Bundled resource cost is
measured from the real files, and the on-demand profile records the extra turn it
pays to fetch a referenced resource. The
latest committed local snapshot is [`bench/competitor/skill-selection-benchmark.json`](../bench/competitor/skill-selection-benchmark.json);
provider cached-input values remain explicitly unavailable for these local fixtures.

## What this suite does not claim

- no broad market-speed claim against every harness workflow layer
- no universal benchmark for all tasks
- no claim that every peer repo already has source-backed runs recorded for every shared-harness scenario
- no wall-clock competition claim beyond the tracked scenarios in this repository

## Repeatable validation

The benchmark/demo suite is guarded by repository contract tests and now runs inside the normal Rust validation path:

```bash
cargo test --workspace
cargo build --release --bin keel
./target/release/keel validate --repo-root . --profile smoke
```

## Scenario design rules

- Each tracked scenario must point to a real PR or comparable repository artifact.
- Each tracked scenario must show commands, proof expectations, and final outcome.
- Success metrics must stay narrow and falsifiable.
- Demo docs should explain what happened, what proof was required, and why the scenario matters to operators.
