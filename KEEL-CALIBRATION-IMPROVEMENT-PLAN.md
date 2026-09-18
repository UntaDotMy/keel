<!--
Purpose: Post-Jev calibration upgrades using only local math and collected outcomes. No LLM calls, no Jev API, no new dependencies.
Caller: Maintainers and agents implementing bounded, evidence-backed work packages.
Dependencies: KEEL-JEV-INSPIRED-IMPROVEMENT-PLAN.md (J01-J11 shipped on task/jev-decisions), current HEAD.
Side Effects: None. This document authorizes planning, not automatic source edits or releases.
-->

# Keel Calibration Improvement Plan

Revision: 2026-09-19. Follow-up to the Jev-inspired decision layer.
Principle: every confidence number traces to measurements or is labeled prior.

## Packages

### K01: Shared calibration core (no behavior change)
- **Evidence/owners:** `utility/decision.rs` (`SkillCalibrationRecord`, buckets, blend), `utility/skill_match.rs` (gate), review decile need.
- **Change:** New `utility::calibration` module with pure helpers over `(total, correct)` counts: Laplace rate, N-weighted blend, Brier score, ECE over (confidence, outcome) pairs. Existing file formats untouched. Routing behavior identical.
- **Acceptance:** Full suite green with zero threshold changes; new unit tests pin rate/blend/Brier/ECE math.
- **Checks:** `cargo test -p keel --lib calibration_` exercises the core.

### K02: Hierarchical blend, global prior, sample reporting
- **Evidence/owners:** `utility/decision.rs` (`calibrated_confidence`, `record_and_save_skill_calibration`, `get_calibrated_confidence`), `utility::skill_match.rs` (gate, match path).
- **Change:** Three-tier blend bin to skill to global to computed: new skills inherit the global aggregate (`state/skill-calibration/global.json`, updated on every record) instead of flat prior; empty data returns input exactly. `decision calibrate` output gains `samples` and `prior_dominated` (computed still dominates).
- **Acceptance:** Convergence test still within ±0.10; gate test still silences; new tests prove global-prior tempering and prior-dominated flagging.
- **Checks:** Existing `calibration_*` and `low_calibrated_confidence_stays_silent` stay green.

### K03: Score fusion against ReviewOutcome deciles
- **Evidence/owners:** `utility/decision.rs` (`evaluate_review_scores`, `evaluate_plan_scores`, `handle_decision_tool` score arm), `save_review_outcome` / `aggregate_review_outcomes`.
- **Change:** Pure-policy caps inside the evaluators: confidence capped by criterion agreement (`1 - spread`) and forced below pass threshold inside a 0.05 boundary band. Learned part at the tool boundary: per-confidence-decile empirical precision table (`state/review-calibration.json`, updated incrementally on every saved outcome), blended with the same N=100 weighting. Empty tables pass input through unchanged.
- **Acceptance:** Existing pass/escalate/boundary tests unaffected on clean homes; new tests prove spread cap, boundary flip, decile blend movement, malformed still escalates.
- **Checks:** `review_score_*`, `plan_score_*`, `score_malformed_feedback_escalates` green.

### K04: Noul override labels per pattern family
- **Evidence/owners:** `utility/decision.rs` (`ShellNoulDecision`, tier helpers, `handle_decision_tool`), session-end reconcile pattern from J08.
- **Change:** Every verdict carries a stable `family` tag (pipe, fork, canonical, novel, echo, substitution, obfuscated, default). New explicit `noul-feedback` tool + CLI action records human allow/deny per family (`state/shell-calibration.json`). No automatic probability changes: families surface agreement rates for human threshold tuning, mirroring the J09 precedent.
- **Acceptance:** Family present on every verdict; feedback round-trips into the file; report shows rates. Existing shell tests updated only for the new field.
- **Checks:** `shell_noul_*`, `noul_feedback_*` green.

### K05: Composition outcome tracking
- **Evidence/owners:** `utility/skill_match.rs` (`match_skill_composition_for_prompt`, pending ledger), `run_session_end_learning`, observation scan.
- **Change:** Stage composition decisions in `state/composition-pending.json`; reconcile at session end against cited skills (both cited means compose-correct; single-cited means single-correct; none-cited means generic-correct). Aggregates in `state/composition-calibration.json` (no auto-tuning of the 0.65/0.60 gates, mirroring J09 precedent).
- **Acceptance:** Cited/uncited/generic unit tests; reconcile consumes the ledger; no change to injection behavior.
- **Checks:** `skill_composition_*`, `reconcile_*`, `composition_outcome_*` green.

### K06: Time-decay and catalog-tied skill decay
- **Evidence/owners:** `load_skill_calibration`, `match_skill_for_prompt_with_details` (has skill mtimes via discovery scan).
- **Change:** Lazy 30-day half-life decay on record load (usize-safe rounding, format-stable, fail-open, writes back only when decay applied). Catalog-tied: matched skill file newer than its record timestamp halves that skill once. No background jobs.
- **Acceptance:** Old records halve deterministically in tests; fresh records untouched; full suite green.
- **Checks:** `calibration_decay_*` green.

### K07: Calibration report action
- **Evidence/owners:** `handle_decision_tool`, `run_decision_command` (follows the `cache-stats` precedent).
- **Change:** New `calibration-report` tool + CLI action: per-skill samples/empirical/blend-weight/prior-dominated flags, review decile precision + sessions + Brier, composition precision, shell family agreement rates, overall ECE. Read-only, additive.
- **Acceptance:** Shape asserted on synthetic data in tests; works against a fresh home (empty sections, no errors).
- **Checks:** `calibration_report_*` green; live CLI smoke.

### K08: Verification and delivery
- Full `cargo test -p keel`, workspace clippy `-D warnings`, `cargo fmt --check`, doc-parity + budget suites, sibling scan, flow start/finish, release rebuild + reinstall + smoke, commit on `task/jev-calibration`, push.

## Non-goals
- Jev API / `JEV_API_KEY` integration (plan §7 Option B stays long-term).
- Classical ML dependencies (no GBDT/CatBoost; online logistic-level math only where learning happens).
- Automatic threshold tuning anywhere (human-tuned from reports, J09 precedent).
- Changing verdict semantics for existing passing tests (additive behavior only, except the specified boundary-band flips which get their own tests).
