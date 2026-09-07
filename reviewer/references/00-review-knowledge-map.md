# Reviewer Knowledge Map

Use this map to load only the references needed for the requested review target.

## Capability Matrix

| Need | Primary Reference |
|---|---|
| Requirement/PRD/user story alignment | 10-requirements-traceability-and-prd-review.md |
| Code quality, security, performance checks | 20-code-quality-security-performance-review.md |
| Function reuse and overengineering checks | 21-function-reuse-and-simplicity-review.md |
| Code-integrity anti-pattern execution checks (types, casts, objects, imports, duplication, completion, hardcoded, fallback) | 22-code-integrity-anti-pattern-review.md |
| Hook safety and interactive UI regression checks (hook-order safety, repeated state-transition runtime validation, bundle patch static guards) | 23-hook-safety-and-interactive-ui-regression-review.md + 40-testing-release-production-readiness-review.md |
| API layer quality and contract checks | 25-api-layer-and-contract-review.md |
| Architecture modularity and maintainability checks | 27-architecture-modularity-and-maintainability-review.md |
| OOP/modular boundary quality and single-source-of-truth reuse checks | 21-function-reuse-and-simplicity-review.md + 27-architecture-modularity-and-maintainability-review.md |
| Type-safety, casting, partial-object, and inline-type anti-pattern checks | 21-function-reuse-and-simplicity-review.md + 29-style-formatting-and-readability-review.md |
| Import boundary integrity and overlap/duplication anti-pattern checks | 25-api-layer-and-contract-review.md + 27-architecture-modularity-and-maintainability-review.md |
| Existing structure/style continuity and naming clarity checks | 27-architecture-modularity-and-maintainability-review.md + 29-style-formatting-and-readability-review.md |
| Commit payload cleanliness and secret/artifact hygiene checks | 31-gitignore-and-secret-hygiene-review.md + 40-testing-release-production-readiness-review.md |
| File read-refresh discipline checks (read-before-write and re-read-after-change) | 40-testing-release-production-readiness-review.md + 50-feedback-style-and-remediation.md |
| Context re-read discipline checks (re-read user story plus relevant files/folders before actions) | 10-requirements-traceability-and-prd-review.md + 40-testing-release-production-readiness-review.md |
| Understanding/reasoning discipline checks (problem re-read, approach rationale, pseudocode/outline, test-debug evidence) | 10-requirements-traceability-and-prd-review.md + 40-testing-release-production-readiness-review.md |
| Post-modification reviewer-trigger checks (review run + re-run after fixes on latest modified state) | 40-testing-release-production-readiness-review.md + 50-feedback-style-and-remediation.md |
| Post-implementation code quality/readability checks (no shortform shortcuts, no duplicate functions, robust maintainable minimal code, no overengineering, @param coverage) | 21-function-reuse-and-simplicity-review.md + 27-architecture-modularity-and-maintainability-review.md + 29-style-formatting-and-readability-review.md |
| Database query/index/scaling checks | 28-database-query-performance-and-scaling-review.md |
| Stack formatting/readability checks | 29-style-formatting-and-readability-review.md |
| Dependency freshness and supply-chain checks | 30-dependency-freshness-supply-chain-review.md |
| .gitignore hygiene and secret-leak prevention checks | 31-gitignore-and-secret-hygiene-review.md |
| Testing, release, and production readiness | 40-testing-release-production-readiness-review.md |
| Windows shell execution compatibility checks | 40-testing-release-production-readiness-review.md |
| Human-style feedback and remediation planning | 50-feedback-style-and-remediation.md |
| UI/UX consistency and system-wide impact checks | 60-ui-ux-consistency-and-system-impact-review.md |
| Dark/light theme parity and CTA/button visibility checks | 60-ui-ux-consistency-and-system-impact-review.md |
| Shared component/token propagation checks for global UI updates | 60-ui-ux-consistency-and-system-impact-review.md |
| Regression-prevention evidence gate checks | 40-testing-release-production-readiness-review.md + 60-ui-ux-consistency-and-system-impact-review.md |
| Flow-quality dimension checks (challenge-skill fit, goals, feedback, concentration/noise control) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md |
| Hardcoded-value elimination and fallback-necessity checks | 20-code-quality-security-performance-review.md + 21-function-reuse-and-simplicity-review.md + 25-api-layer-and-contract-review.md |
| Execution strategy checks (sequential vs optional parallel, session stability, single-writer discipline) | 50-feedback-style-and-remediation.md + 40-testing-release-production-readiness-review.md + 27-architecture-modularity-and-maintainability-review.md |
| Pre-execution brainstorm/voting quality checks (parallel proposal quality, scoring rubric, no-surprise scope control) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md + 40-testing-release-production-readiness-review.md |
| Execution-strategy communication checks (clear ownership/steps, no confusing parallel claims) | 50-feedback-style-and-remediation.md + 40-testing-release-production-readiness-review.md |
| Runtime feature-state gate checks (`claude features list`, `config.toml`, capability-dependent fallback decisions) | 40-testing-release-production-readiness-review.md + 99-source-anchors.md |
| Host-native command-runner timeout/reset stability checks (runtime compatibility, timeout settings, chunking, recovery evidence) | 40-testing-release-production-readiness-review.md + 99-source-anchors.md |
| RALPH loop compliance checks (Read, Assess, Lookup, Propose, Harden) | 10-requirements-traceability-and-prd-review.md + 50-feedback-style-and-remediation.md + 99-source-anchors.md |
| Research reuse/freshness/refinement and memory-capture checks | 30-dependency-freshness-supply-chain-review.md + 50-feedback-style-and-remediation.md + 99-source-anchors.md |
| Deliberate action/tooling checks (think-before-tool-use, failure fingerprinting, retry rationale) | 50-feedback-style-and-remediation.md + 40-testing-release-production-readiness-review.md |
| Resolved mistake/error memory-capture checks (root cause, fix, evidence, prevention) | 50-feedback-style-and-remediation.md + 99-source-anchors.md |
| Low-reasoning exploration quorum checks (minimum two low-effort exploration streams plus synthesis consensus) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md |
| Medium-reasoning dual-stream synthesis checks (exactly two medium-effort streams, min 2 max 2, with synthesis output) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md |
| Final-reviewer role binding checks (final review must run in reviewer role, not default) | 50-feedback-style-and-remediation.md + 40-testing-release-production-readiness-review.md |
| Host-native-tools-only compliance checks for agents (tool calls through the active host's native tool flow) | 99-source-anchors.md + 40-testing-release-production-readiness-review.md |
| Strict code-quality zero-tolerance checks (no small mistakes in readability/reuse/duplication/naming/maintainability) | 21-function-reuse-and-simplicity-review.md + 27-architecture-modularity-and-maintainability-review.md + 29-style-formatting-and-readability-review.md |
| Anti-hallucination recovery harness checks (detect, re-read, reproduce, re-research, retry, re-validate) | 50-feedback-style-and-remediation.md + 99-source-anchors.md |
| Summary-first handoff quality checks (compact packet, no raw full-prompt/history replay) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md |
| Role-output discipline checks (non-reviewer concise packet output, reviewer full narrative output) | 50-feedback-style-and-remediation.md + 40-testing-release-production-readiness-review.md |
| Skill quality/completeness checks (for SKILL.md artifacts) | 10-requirements-traceability-and-prd-review.md + 50-feedback-style-and-remediation.md |
| Domain-first routing quality checks (clear specialist ownership, reviewer only for audits or final validation) | 50-feedback-style-and-remediation.md + 10-requirements-traceability-and-prd-review.md |
| Standards/source verification | 99-source-anchors.md |

## Quick Review Sequence

1. Confirm scope and requirements baseline.
2. Run risk-focused checks by severity.
3. Validate understanding/reasoning discipline (problem re-read, approach rationale, pseudocode/outline, and test-debug evidence).
4. Validate code-integrity anti-patterns with the dedicated anti-pattern reference.
5. Validate hook-safety gate (top-level hook order, interactive state-transition checks, and bundle-patch static guards when applicable).
6. Validate API/database/formatting guidance with current authoritative sources.
7. Validate dependencies and advisories with current sources.
8. Validate source evidence quality (links + checked-on date) for time-sensitive claims.
9. Validate research reuse decision and freshness-window fit (reuse vs fresh lookup with rationale).
10. If initial research was generic, validate refinement rounds and final specificity.
11. Validate verified-fix memory capture status (or explicit manual capture note when unavailable).
12. Validate .gitignore/secret-hygiene controls and tracked-file exposure risks.
13. Validate testing and release-readiness evidence.
14. Validate regression-prevention evidence coverage and gate status before verdict.
15. Validate commit payload cleanliness (no secrets or local artifacts; includes necessary tests) with evidence (git status --porcelain, git diff --cached --name-only).
16. Validate context re-read discipline for user story and relevant files/folders before actions.
17. Validate file read-refresh discipline for all modified files (read-before-write and re-read-after-change).
18. Validate post-modification reviewer trigger evidence (review run and re-run are aligned to latest modified state).
19. Validate post-implementation code quality/readability gate (clean readable code, no shortform shortcuts, no duplication, robust minimal maintainable design, and @param coverage).
20. Validate deliberate action/tooling evidence (think-first note, expected outcome, and fallback per non-trivial action).
21. Validate resolved-error learning capture (error fingerprint, root cause, resolution evidence, and memory capture or manual note).
23. Validate low-reasoning exploration quorum evidence (minimum two low-effort streams plus synthesis consensus) when low exploration is used.
24. Validate medium-reasoning dual-stream synthesis evidence (exactly two medium-effort streams, min 2 max 2, plus synthesized output) when medium analysis/context/review streams are used.
25. Validate final-reviewer role binding evidence (reviewer role used for final review, not default).
  26. Validate host-native-tools-only compliance for agents (tool calls through the active host's native tool flow when applicable).
27. Validate strict code-quality zero-tolerance handling (no unresolved small code-quality mistakes in strict categories).
28. Validate anti-hallucination recovery harness evidence when uncertainty occurred (detect -> re-read -> reproduce -> re-research -> retry -> re-validate).
29. Validate summary-first handoff quality (compact packet used and no raw full-prompt/history replay).
30. Validate role-output discipline (non-reviewer concise packets; reviewer full narrative output).
31. Validate structure/style continuity and variable naming clarity against existing project conventions.
32. Validate dark/light theme parity and CTA/button visibility when UI is in scope.
33. Validate shared component/token propagation safety for global UI changes.
34. Validate hardcoded-value elimination and fallback necessity (clarification-first when fallback behavior is ambiguous).
35. Validate execution strategy and session stability (sequential vs optional parallel, single-writer discipline, and checkpointed steps).
36. Validate pre-execution brainstorm/voting loop quality when applicable (proposals, rubric scoring, tie handling, no-surprise scope check).
37. Validate runtime feature-state gate handling (`claude features list`, `config.toml`, and capability-dependent fallback decisions).
  38. Validate host-native command-runner timeout/reset stability (runtime compatibility, timeout settings, chunking, and recovery evidence).
39. Validate domain-first routing quality (clear specialist ownership + reviewer only when needed for audits or final validation).
40. Validate flow-quality dimension alignment when process quality is in scope.
41. Validate skill expertise depth and vague-prompt handling quality (for skill reviews).
42. Return actionable feedback and readiness verdict.
