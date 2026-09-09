# Phase 9 Agile SDLC Lifecycle Enforcement (DoR and DoD)

## Delivered contract

Keel now enforces formal agile lifecycle stages through automated **Definition of Ready (DoR)** and **Definition of Done (DoD)** evaluation commands (`keel plan ready` and `keel plan done`), integrating into the completion gate (`keel completion-gate check --plan <id>`).

1. **Definition of Ready (DoR) Enforcement (Section 14.2)**:
   - Evaluates all 11 mandatory prerequisites before implementation begins:
     - `dor-1`: Plan initialized and exists on disk.
     - `dor-2`: Specification approved and complete (all 12 mandatory sections present).
     - `dor-3`: Research completed with source provenance and acceptable freshness.
     - `dor-4`: Architecture design complete with component boundaries and interfaces.
     - `dor-5`: Task tickets decomposed with scope matrices and required layers.
     - `dor-6`: RTM trace matrix links user requests to requirements, criteria, and tasks.
     - `dor-7`: Verification plan defined with concrete methods and expected evidence types.
     - `dor-8`: Quality gates passed without blocking failures.
     - `dor-9`: Security & privacy impact assessed with mitigation notes.
     - `dor-10`: UI/UX criteria defined when visual interfaces are impacted.
     - `dor-11`: Clarification questions answered with no blocking ambiguities.
   - CLI command: `keel plan ready --plan <id> [--json]`.
   - Records DoR result and stage transition in plan `status.json`.

2. **Definition of Done (DoD) Enforcement (Section 14.3)**:
   - Evaluates all 13 mandatory criteria before closing out delivery:
     - `dod-1`: All requirements and acceptance criteria have passing verified evidence.
     - `dod-2`: All task tickets and subtask layers are complete (`done` or justified `not_applicable`).
     - `dod-3`: Build clean with 0 blocking compiler warnings.
     - `dod-4`: Linter clean with 0 blocking linter warnings.
     - `dod-5`: Automated test suite passes with verified evidence.
     - `dod-6`: Warning ledger contains 0 unresolved blocking warnings.
     - `dod-7`: Security and privacy checks verified without unresolved risks.
     - `dod-8`: UI/UX visual verification records verified for all visual changes.
     - `dod-9`: Review passes with all trace evidence validated against files and hashes.
     - `dod-10`: Honest status semantics enforced across all tickets and gates.
     - `dod-11`: Documentation (spec, architecture, runbooks) is current and updated.
     - `dod-12`: Rollout and rollback procedures documented and reviewed.
     - `dod-13`: Completion gate criteria satisfied.
   - CLI command: `keel plan done --plan <id> [--json]`.
   - Records DoD result and stage transition in plan `status.json`.

3. **Completion Gate Integration**:
   - `keel completion-gate check` extended with `--plan <id>`.
   - Evaluates plan DoD as an explicit, load-bearing gate check.
   - Returns a non-zero exit code if DoD criteria are unsatisfied, blocking closeout until all 13 items pass.

4. **CLI Help and Documentation Parity**:
   - `rust/crates/keel/src/help_operator.txt` updated with `plan ready` and `plan done`.
   - `rust/crates/keel/src/help_advanced.txt` updated with `--plan <id>` flag for `completion-gate check`.

5. **Test Coverage**:
   - Integration tests implemented in `rust/crates/keel/tests/agile_sdlc_test.rs`:
     - `plan_ready_satisfies_all_eleven_items`
     - `plan_ready_fails_when_prerequisite_missing`
     - `plan_done_lifecycle_enforcement_and_evidence_gate`
     - `completion_gate_check_with_plan_flag`

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| 11-item DoR evaluation | `evaluate_definition_of_ready` in `rust/crates/keel/src/utility/plan.rs`; verified by `plan_ready_satisfies_all_eleven_items` and `plan_ready_fails_when_prerequisite_missing`. |
| 13-item DoD evaluation | `evaluate_plan_definition_of_done` in `rust/crates/keel/src/utility/plan.rs`; verified by `plan_done_lifecycle_enforcement_and_evidence_gate`. |
| CLI command integration | `keel plan ready` and `keel plan done` exposed in operator help and command router. |
| Completion gate integration | `--plan <id>` in `rust/crates/keel/src/utility/memory/completion_gate.rs`; verified by `completion_gate_check_with_plan_flag`. |
| Status persistence | `status.json` stores `definitionOfReady` and `definitionOfDone` records with item details and timestamps. |

## Validation Summary

| Test Surface | Result | Detail |
| --- | --- | --- |
| Agile SDLC integration tests | 4 passed, 0 failed | `cargo test -p keel --test agile_sdlc_test` |
| Doc parity test suite | 27 passed, 0 failed | `cargo test -p keel --test doc_parity_test` |
| Workspace test suite | 1,310 passed, 0 failed | `cargo test --workspace` via `review pre-pr` |
| Pre-commit review | pass (0 blocking) | `keel review pre-commit` |
| Pre-pr review | pass (0 blocking) | `keel review pre-pr --base-ref task/integrated-agent-quality-phase-8 --plan plan-1788925414247848700-3520-4648897e` |
| Rust format | pass | `cargo fmt --all -- --check` |
| Rust clippy | pass (0 warnings) | `cargo clippy --all-targets -- -D warnings` |

## Release Ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | `keel plan ready --help`, `keel plan done --help`, and `completion-gate check --help` output cleanly. |
| Functional | pass | DoR and DoD accurately evaluate missing items, passing items, and formatted JSON output. |
| Integration | pass | `agile_sdlc_test.rs` validates full plan lifecycle through DoR and DoD transitions. |
| UI | not applicable | Headless CLI / SDLC harness logic without UI elements. |
| Load | not applicable | Local JSON evaluation across bounded plan directories. |
| Stress | not applicable | Bounded artifact reads and bounded JSON deserialization. |
| Security | pass | Validated plan paths, strict schema conformance, and fail-closed gate semantics. |
