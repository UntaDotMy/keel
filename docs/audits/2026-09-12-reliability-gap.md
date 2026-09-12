# Reliability plan gap audit

This audit closes the bounded-persistence and packaged-lifecycle evidence gaps
identified in sections 49, 53, 54, 55, 56, and 58 of the attached agent
operating-system plan. It records what is executable locally and keeps release
or third-party claims fail-closed.

## Evidence matrix

| Plan requirement | Executable evidence | Status and boundary |
| --- | --- | --- |
| Bounds: entries, bytes, and TTL cleanup | `rust/crates/keel/src/proxy/raw_store.rs` bounds raw IDs and direct writes; existing unit tests cover the 64 MiB stream/screenshot ceiling, retention `0`, stale/fresh age signals, and auto-prune throttling. | **Pass for bounded individual artifacts and TTL policy.** The new contention test adds multi-writer persistence evidence but is not a multi-day soak benchmark. |
| Adversarial persistence behavior | `rust/crates/keel/tests/reliability_gap_test.rs:143-227` runs 12 writers and 4 readers against one store, validates every committed artifact, and catches a staging-directory race. | **Pass for this deterministic contention matrix.** It does not claim generated-input fuzz coverage. |
| Property/fuzz invariants | Existing focused tests cover path traversal, integrity tampering, namespace isolation, and cap behavior. | **NotRun / NeedsHuman** for generated cursors, headers, Unicode, task trees, and fuzzed projections; no property/fuzz harness was added in this slice. |
| Failure semantics | Concurrent readers tolerate an entry disappearing between directory enumeration and metadata inspection; retention keeps the active date shell while pruning stale logical days. Tests fail on unexpected filesystem errors, integrity, or content mismatches. | **Pass for the exercised RawStore race/recovery states.** No silent fallback or false success is introduced. |
| Restart and recovery | `reliability_gap_test.rs:289-376` stages a manifest-marked release bundle, installs it, removes the transient bundle, then runs the installed executable in fresh processes for status, verify, and doctor against the cached source. | **Pass for local Windows source-build binary exercising the packaged layout and cache recovery.** This is not signing or archive extraction proof. |
| Packaged binary smoke release gate | Native install/status/verify/doctor commands are exercised from the installed executable. | **Partial.** Hosted platform archives, signing, and the repository release workflow remain required for a release claim. |
| Host conformance | Existing native host fixture coverage remains available and explicitly marks unsupported hosts as `not_run`. | **NeedsHuman / NotRun** for live third-party host evidence; no external host behavior is inferred from local fixtures. |
| DoD: concurrency, recovery, bounded lifecycle | The three integration tests cover concurrent save/list/read, prune/save overlap, and a fresh-process packaged lifecycle. | **Pass for these local acceptance checks; broader soak, stress, and hosted evidence remain open.** |

## Validation record

The focused proving command is:

```text
cargo test -p keel --test reliability_gap_test -- --test-threads=1
```

It passed all three tests on the Windows workspace checkout. `cargo fmt
--all` also completed successfully. The concurrency test initially exposed a
real `RawStore::list` enumeration race and a prune/save active-day race; the
small source hardening in `raw_store.rs` now treats concurrent disappearance as
an expected retry-safe observation while preserving non-transient errors.

## Release decision

The local reliability slice is **Pass for its stated boundaries** and does not
convert unsupported external evidence into a release approval. The final plan
remains **Partial / NeedsHuman** until hosted packaged-archive smoke, signing,
security and dependency gates, full workspace validation, and live third-party
host checks are available. Unsupported live evidence is explicitly **NeedsHuman
/ NotRun** rather than a passing placeholder.
