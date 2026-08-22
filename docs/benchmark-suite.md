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
