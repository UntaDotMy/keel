# Lifecycle-gap audit, 2026-09-12

## Scope

This closeout covers only the remaining Phase 7/8 lifecycle seams. It does not
claim that the full operating-system plan or its benchmark program is complete.
The implementation is limited to the memory-family handlers, the learning loop,
the recall projection, and this regression target.

## Requirement/evidence matrix

| Requirement | Evidence in the implementation | Regression evidence |
| --- | --- | --- |
| Research findings have an explicit scope | `memory research-cache record` normalizes and bounds `--scope`; legacy records without a scope remain readable as `global`. Lookup accepts `--scope` and compares it before matching. | `scoped_replacement_and_expiry_define_current_truth` proves workspace A cannot read workspace B's answer. |
| A replacement becomes current truth without deleting evidence | `record --supersedes` requires an existing same-scope target, writes the replacement, transitions the predecessor to `state/lifecycle=superseded`, and removes the new file if that transition fails. Lookup also honors a replacement declaration from a legacy file. | The test reads the predecessor from disk, verifies `supersededBy`, and verifies only the replacement is in `matches`. |
| Expiry is durable and opt-in to inspect | `research-cache expire --id` writes `expired`, `lifecycle=expired`, and `expiredAt`; ordinary lookup excludes it while stale inspection remains available. | The scoped test expires the replacement and observes `count=0` with two stale matches. |
| Active recall reflects lifecycle state | Recall keeps source JSON files for audit, but indexes no chunks for `candidate`, `demoted`, `questioned`, `quarantined`, `superseded`, `expired`, or expired records. Lifecycle writes invalidate/reindex the changed path. | Owner recall tests and the lifecycle command path exercise the same reindex owner; no source record is removed by recall filtering. |
| Lessons require evidence before promotion | Manual instinct records accept an unverified candidate, preserve the evidence field across reinforcement, and `promote` requires non-empty evidence plus a positive evidence count. The promotion projection reports excluded high-confidence hunches. | `evidence_is_required_for_promotion_and_penalties_quarantine` verifies a three-confidence hunch is excluded and an evidence-backed lesson is promoted. |
| Lesson fields are measurable and scoped | Auto-learned instincts persist problem pattern, evidence source/count, cause hypothesis, correct response, scope, last verification, evaluation metric/status, baseline/current observations, and evaluation time. | `runner::learning::tests` verifies auto-observation promotion, failure watchouts, and persisted metadata; the integration test verifies manual scope/evidence metadata. |
| Wrong lessons are demoted/quarantined | Stale observed evidence moves through demotion metadata; repeated decay or penalties move a record to `quarantined`, set `evaluationStatus=regressed`, and preserve the record until explicit retention/pruning rules remove it. Generated skills remain protected by the existing no-clobber/manual-edit guard. | The integration test applies two penalties and verifies `quarantined` plus `regressed`; owner tests cover stale decay and manual-edit preservation. |
| Storage and projections are bounded without silent eviction | Research and instinct record admission checks reject over-limit records/stores. Instincts and family lists cap record count/serialized projection bytes. Existing records are never silently evicted; operators receive a non-zero capacity error. | `instinct_admission_and_list_projection_are_bounded_without_eviction` seeds the limit, verifies rejection, verifies the oldest record remains, and checks the bounded JSON projection. |

## State model

```text
candidate --evidence + threshold--> active/promoted
active --wrong outcome or decay--> questioned/demoted
questioned/demoted --repeated regression--> quarantined
quarantined --explicit replacement/expiry--> superseded/expired
```

The state transition is metadata-only until an explicit retention action or the
existing observed-instinct prune floor applies. This preserves raw evidence and
does not rewrite host policy or built-in skills.

## Verification boundary

Focused validation is the evidence for this change:

```text
cargo test -p keel --test lifecycle_gap_test -- --test-threads=1
cargo test -p keel runner::learning::tests -- --test-threads=1
cargo test -p keel utility::memory_families::tests -- --test-threads=1
cargo fmt --all -- --check
cargo clippy -p keel --all-targets -- -D warnings
```

The full repository release ladder, hosted CI, benchmark suite, and packaged
host conformance remain outside this scoped closeout and must not be inferred
from these focused commands.
