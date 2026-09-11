# Final operating-system plan: implementation traceability

This document compares the attached **Keel: Final Agent Operating System /
Context Gateway Production Plan** with the repository at the current
`task/final-agent-operating-system` delivery commit (`HEAD`; based on
`f999658`, `Add : CONTEXT : complete production context gateway verification
and hardening`). The host conformance and CI evidence described below is
follow-up work on that branch. It is a repository evidence map, not a claim
that the plan is complete.

## Reading the status

- **Implemented** means a named owner and repository tests or durable evidence
  exist in this checkout.
- **Partial** means some of the plan surface exists, but a required contract,
  integration, or release proof is still missing.
- **Blocked** means the current implementation contradicts the plan target or
  a required gate cannot pass from the evidence available here.
- **Not verified** means this documentation pass found no sufficient
  repository evidence; it is not evidence that the capability is absent.

The plan is an authority for requested outcomes, while source code, tests, and
published evidence are the authority for current behavior. Plan research
sources are listed in the attached plan; this document does not treat those
citations as proof that Keel implements the corresponding feature.

## Phase map

| Plan phase | Current status | Evidence in this checkout | Gap or boundary |
| --- | --- | --- | --- |
| 0. Audit/freeze | **Partial** | `docs/audits/`, `docs/release-proof-bundle.md`, and the current Git history provide audit and baseline artifacts. | A single, independently verified baseline for every plan metric and every duplicate system is not established by this map. |
| 1. Modern MCP core | **Partial** | `rust/crates/keel/src/mcp/` and `rust/crates/keel/tests/mcp_protocol.rs` cover handshake-free `2026-07-28` stdio/HTTP requests, `server/discover`, routing metadata, stateless transport, bounded catalog pages, and malformed requests. | Focused modern unit/integration coverage is present, but hosted client conformance and the complete release ladder remain open. |
| 2. Canonical context gateway | **Implemented** | `rust/crates/keel/src/proxy/context.rs`, `proxy/run.rs`, and `docs/context-gateway.md`; `context_gateway_test.rs` covers bounded projections, dedupe, recovery pointers, namespace checks, and no raw fallback. | Host-specific boundaries remain capability-dependent; the documentation correctly labels unsupported host paths. |
| 3. Tool broker | **Partial** | `rust/crates/keel/src/mcp/tools.rs`; `docs/context-gateway.md`; MCP and fixed-context tests cover bounded pages, profiles, ordering, cursors, cache fields, and catalog budgets. | Full plan-level broker proof (all adversarial cases and a complete modern MCP conformance suite) is not established here. |
| 4. Skill broker | **Partial** | `skill_route`, `skill_get`, `skill_list` CLI/MCP surfaces; `skill-lint` and managed profile documentation; `docs/skills-audit-p1.md`. | Metadata/progressive loading is present, but the plan’s complete resource-graph, cost-selection benchmark, and dead-resource cleanup proof is not one verified release gate. |
| 5. Task/planning system | **Implemented** | `rust/crates/keel/src/utility/plan.rs` and `utility/task_ticket.rs`; `docs/planner.md`; `plan_workflow_test.rs` and `task_ticket_test.rs` cover classification, research sequencing, ambiguity blocking, hierarchical tickets, RTM, evidence, and tamper detection. | This is the strongest completed plan slice; it still requires the final release ladder for a release claim. |
| 6. Mandatory research | **Partial** | `utility/plan.rs` research validation and cache paths; `docs/planner.md`; tests cover official/paper sources, freshness, stale-cache refresh, local-only labeling, conflicts/priority, and relevant local evidence. | The agent/web adapter and end-to-end external research capture are host-provided. No repository evidence here proves every supported host records claims through one complete production flow. |
| 7. Memory | **Partial** | `utility/memory/`, `utility/memory_families.rs`, `utility/recall.rs`, `record_store.rs`; `docs/memory-families-usage.md`; memory and recall tests cover scoped records, provenance, research cache, bounded retrieval, and completion gates. | Full plan-level restart/recovery, retention, supersede/expire, and current-truth override proof across all hosts is not consolidated into one passing release artifact. |
| 8. Learning loop | **Partial** | `runner/learning.rs`, `learn` CLI, SessionEnd capture, and `docs/audits/2026-09-04-system-token-efficiency/audit-summary.md`; audit evidence reports noise filtering and rollback behavior. | The plan’s measured promotion/demotion benchmark and regression proof for every lesson class remain incomplete/not verified in this checkout. |
| 9. Verification/host conformance | **Partial** | `tests/host-adapter-contracts.test.ts`, adapter fixtures, `docs/compatibility-matrix.md`, UI verification surfaces, `rust/crates/keel/tests/host_conformance.rs`, and `keel host conformance --json` (eight-stage native proxy evidence) are covered by the host-conformance CI job. | Full live third-party-host, adversarial, restart, concurrency, and packaged-binary evidence required by the plan is still not one complete release gate; unsupported/partial hosts remain `not_run`. |
| 10. Cleanup/release | **Partial** | `docs/release-notes.md`, `docs/release-proof-bundle.md`, cleanup/audit reports, CI workflow, benchmark artifacts, and the fail-closed host-conformance job. | The plan requires final dead-code/file/dependency/duplicate-system cleanup plus benchmark archive and release proof; this map does not find evidence that every item has been closed. |

## Definition-of-done cross-check

| Plan area | Current evidence | Status |
| --- | --- | --- |
| Architecture and single owners | Context gateway, RawStore, planner, memory, and learning owners are named in `docs/context-gateway.md`, `docs/planner.md`, and the Rust module layout. | **Partial**. No complete duplicate-system audit is attached to this checkout. |
| Planning and evidence-bound TODOs | `docs/planner.md`; `plan_workflow_test.rs`; `task_ticket_test.rs`. | **Implemented** for the shipped planner path. |
| Research freshness and provenance | `docs/planner.md`; `utility/plan.rs`; freshness/cache tests. | **Partial**. Host adapter/end-to-end proof remains open. |
| MCP 2026-07-28 only | `mcp/mod.rs`, `mcp/http.rs`, and the modern protocol integration tests reject the retired handshake/session path and require per-request metadata. | **Partial**. The owner and focused conformance are implemented; hosted client and packaged-release proof remain open. |
| Context budgets, reduction, recovery | `proxy/context.rs`; `context_gateway_test.rs`; `fixed_context_budget_test.rs`; `docs/context-gateway.md`. | **Implemented** on governed native paths. |
| Skills, memory, and learning lifecycle | `skill-lint`, memory families/recall, `runner/learning.rs`, and associated docs/tests. | **Partial**. Lifecycle and benchmark coverage is not complete enough for the plan’s release claim. |
| Reliability and host conformance | Adapter contract tests, compatibility matrix, native eight-stage conformance fixture, and CI JSON parser. | **Partial**. The complete live-host/adversarial/restart/packaged release matrix is not verified. |
| Cleanup and release | CI, release docs, benchmark/audit artifacts. | **Partial**. No evidence that every plan checklist item is closed. |

## Safe claim boundary

Keel can accurately describe itself as a Rust-native, evidence-oriented agent
workflow with a compiled planner, bounded context gateway, memory/recall,
tool and skill discovery, learning hooks, and multiple adapter contracts.
Those are shipped surfaces with the evidence named above.

Keel may describe the MCP surface as targeting the stateless `2026-07-28`
protocol because the owner and focused conformance tests now enforce that
contract. The broader attached plan still requires the partial/release gates
shown above; green focused tests alone do not close those boundaries.
