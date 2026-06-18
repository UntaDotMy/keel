# Benchmark Posture Audit Summary

- Audit date: 2026-04-09
- Repository: `UntaDotMy/keel`
- Audited revision: `f1fdcf71db094e240b94eaa5dc2f9dea46345c18`
- Scope: benchmark posture, shared benchmark harness posture, scorecard honesty, and trust-artifact publication
- Evidence sources: `README.md`, `todo.md`, `docs/benchmark-suite.md`, `docs/shared-benchmark-harness.md`, `docs/benchmark-scorecard.json`, `docs/benchmark-comparison-scorecard.md`, `docs/why-keel.md`, `rust/crates/keel/src/commands.rs`, and `cargo test --workspace`

## Conclusion

The current repo snapshot closes the remaining trust-artifact gap for benchmark posture by publishing a durable audit bundle instead of relying only on scorecard prose.

This bundle proves a narrow claim:

- `keel` publishes source-backed runs for the tracked eight benchmark scenarios in this repository
- the shared benchmark harness exists with one scenario contract and one evidence format for `keel`, `runtime-shell comparator`, and `workflow-teaching comparator`
- the current benchmark posture is now anchored to a durable audit artifact rather than only to scattered comparison docs

This bundle does not claim that every peer repo already has source-backed runs recorded for every shared-harness scenario. It also does not claim universal workflow leadership, wall-clock superiority, or benchmark coverage beyond the tracked scenarios and public surfaces cited here.

## Closed Findings

1. `benchmark-audit-bundle-missing`
   Benchmark posture was documented through scorecards and harness prose, but no published audit bundle summarized the current proof boundary.
   Status: closed by publishing this benchmark posture audit bundle.
2. `benchmark-posture-links-missing`
   The trust docs and contract surfaces did not point at a durable benchmark posture audit artifact.
   Status: closed by linking the published bundle from the README, benchmark suite, shared harness doc, and contract tests.

## Remaining Boundary

This bundle is enough to support source-backed benchmark posture claims for the current repo snapshot. It is not enough to claim that `runtime-shell comparator` or `workflow-teaching comparator` have completed the same shared-harness scenarios with source-backed proof, and it is not a substitute for broader release-proof or security-audit artifacts.
