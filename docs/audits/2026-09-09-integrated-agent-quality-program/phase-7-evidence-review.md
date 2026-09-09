# Phase 7 Evidence-Driven Review and Honest Status Semantics

## Delivered contract

`keel review` now enforces evidence-backed validation and honest status semantics across pre-commit, pre-pr, and closeout surfaces. Ambiguous boolean status passes and unverified claims are eliminated.

1. **Honest Status Semantics**: `GateStatus` expands to 8 explicit states across `rust/crates/keel/src/review/language_gates.rs`:
   - `Pass`: Verified passing evidence present.
   - `Fail`: Hard verification failure.
   - `Warn`: Non-blocking warning (advisory diagnostics).
   - `Skipped`: Intentionally skipped check with documented rationale.
   - `NotApplicable`: Gate does not apply to the changed file scope.
   - `NeedsHuman`: Verification requires human visual inspection.
   - `Unclear`: Ambiguous verification outcome requiring clarification.
   - `Blocked`: Gate blocked by prerequisite or environment constraints.
2. **Canonical Wire and Rendering Semantics**:
   - Wire serialization standardized to snake_case (`pass`, `fail`, `warn`, `skipped`, `not_applicable`, `needs_human`, `unclear`, `blocked`).
   - Status tallying cleanly separates blocking states (`Fail`, `Blocked`, `NeedsHuman`, `Unclear`, `Skipped`) from non-blocking warnings (`Warn`).
   - JSON, Markdown, and compact text formatters render status labels and indicators without ambiguous pass claims.
3. **Closeout Defect Findings**:
   - In `rust/crates/keel/src/review/closeout.rs`, gate findings use canonical `status.as_str()`.
   - Correctly justified `NotApplicable` gates do not produce defect findings.
   - Blocking gate states (`status.is_blocking()`) produce `ReviewSeverity::Major` defect findings.
4. **PR #262 Impact Gate Defect Remediation**:
   - Removed the silent `or_else` fallback in `diff_gates.rs` that masked missing code graph artifacts.
   - Wired `impact_gate` into `pre-pr` by default.
   - Brownfield edits without graph artifacts or flow evidence fail closed (`GateStatus::Fail`, `blocking: true`).
   - Brownfield edits with valid flow evidence emit an actionable warning (`GateStatus::Warn`, `blocking: false`) directing the user to run `keel code-graph build`.
   - Greenfield and non-source changes return `GateStatus::NotApplicable`.
5. **Acceptance Criteria Evidence Mapping**:
   - Implemented `evaluate_acceptance_criteria` in `rust/crates/keel/src/utility/plan.rs`.
   - Resolves plan RTM traces and task ticket evidence artifacts.
   - Evaluates each acceptance criterion to honest formatted status output:
     - `AC-xxx: pass | evidence: <IDs> | verified by: <checks>`
     - `AC-xxx: needs_human | reason: <reason> | screenshot: <ID>`
     - `AC-xxx: unclear | reason: <reason>`
     - `AC-xxx: skipped | reason: <reason>`
     - `AC-xxx: fail | missing traceability to implementation evidence`
   - Binds directly into `task_evidence_gate` in `rust/crates/keel/src/review/diff_gates.rs`.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| 8-state GateStatus | `gate_status_honest_semantics_and_serialization` validates all 8 states, string tokens, and serde round-trips in `review::tests`. |
| Honest status tally | `tally_gate_results_honest_counts` confirms blocking counts versus warning tallies. |
| Rendering formats | `gate_status_rendering_all_formats` checks JSON, Markdown, and compact text representations. |
| PR #262 impact gate fix | `impact_gate_missing_graph_without_flow_blocks` proves blocking fail on missing graph without flow; `impact_gate_missing_graph_with_valid_flow_warns` proves non-blocking warn with flow check; `impact_gate_empty_touched_returns_not_applicable` verifies greenfield clean handling. |
| Acceptance criteria mapping | `acceptance_criteria_evaluation_honest_format` verifies missing evidence failure, pass with evidence IDs, and needs_human status. |
| Review closeout findings | Closeout gate finding generation maps `is_blocking()` to major review findings. |

## Validation Summary

| Test Surface | Result | Detail |
| --- | --- | --- |
| Review unit tests | 95 passed, 0 failed | `cargo test -p keel review::tests` |
| Workspace test suite | 1,272 passed, 0 failed | `cargo test --workspace` via `review pre-pr` |
| Pre-commit review | pass (0 blocking) | `keel review pre-commit` |
| Pre-pr review | pass (0 blocking) | `keel review pre-pr --base-ref task/integrated-agent-quality-phase-6 --plan plan-1788921876709207800-14776-fe5a88ad` |
| Rust format | pass | `cargo fmt --all -- --check` |
| Rust clippy | pass (0 warnings) | `cargo clippy --all-targets -- -D warnings` |

## Release Ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | `keel review pre-commit` and `keel review policy show` run cleanly. |
| Functional | pass | 95 review unit tests verify honest status semantics, impact gate logic, and AC evaluation. |
| Integration | pass | `review pre-pr` validates plan traceability, RTM tickets, and compiler/linter rungs. |
| UI | not applicable | CLI and gate review surfaces only. |
| Load | not applicable | Gate evaluations operate synchronously over memory plan artifacts and git diffs. |
| Stress | not applicable | In-memory gate tallying and bounded diff parsing. |
| Security | pass | Fail closed on missing verification evidence or unverified brownfield modifications. |
