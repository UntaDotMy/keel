<!--
Purpose: Authoritative sources for the rules in AGENTS.md so contributors can verify a rule against its origin.
Caller: AGENTS.md and contributors reconciling drift between AGENTS.md and references.
Dependencies: AGENTS.md, the specialist SKILL.md files (roster asserted by tests/doc_parity_test.rs), keel CLI surface.
Main Functions: Map AGENTS.md sections to reference files, list related skills, list owning commands.
Side Effects: None — this file is informational.
-->
# Source Anchors

Authoritative sources for the rules in AGENTS.md. Use these when a reviewer or contributor needs to verify a rule against its origin instead of recalling it from memory.

## Doctrine Source

- `AGENTS.md` — the canonical entry point. Every reference file in this directory restates a section of AGENTS.md in the depth a reader needs to apply it.

When AGENTS.md and a reference disagree, AGENTS.md wins. Open a follow-up to reconcile the reference.

## Section-to-Reference Map

| AGENTS.md section | Reference file |
|---|---|
| Native Command Routing — Must Follow First | 10-native-command-routing.md |
| Hook Transparent Rewrite | 10-native-command-routing.md |
| Token Optimization (Native Command Compaction) | 10-native-command-routing.md |
| Skill Routing | 20-skill-routing.md |
| Specialist Skills | 20-skill-routing.md |
| Skill-Focused Execution | 20-skill-routing.md |
| Agent Profiles | 20-skill-routing.md |
| Execution Strategy | 30-execution-strategy.md |
| Iterative Development Loop (loops 0–9) | 30-execution-strategy.md |
| Flow Control, Loop Limits, General Approach | 30-execution-strategy.md |
| Code Quality Standards | 40-code-quality-and-testing.md |
| Testing Requirements | 40-code-quality-and-testing.md |
| Feature Flags | 40-code-quality-and-testing.md |
| Feature Delivery Rules | 50-delivery-and-prohibited-shortcuts.md |
| Best Practices | 50-delivery-and-prohibited-shortcuts.md |
| Prohibited Shortcuts | 50-delivery-and-prohibited-shortcuts.md |
| Windows Environment | 60-environment-and-portability.md |
| Cross-Platform Script Portability | 60-environment-and-portability.md |
| Code Review Requirements | 70-review-quality-gates-and-policies.md |
| Automated Quality Checks | 70-review-quality-gates-and-policies.md |
| Quality Gates | 70-review-quality-gates-and-policies.md |
| Final Output | 70-review-quality-gates-and-policies.md |
| Reasoning Effort Levels | 70-review-quality-gates-and-policies.md |
| Skill Model Policy | 70-review-quality-gates-and-policies.md |
| Git Identity Policy | 70-review-quality-gates-and-policies.md |

## Related Skills

These skills hand off to or from the AGENTS.md doctrine. Load the matching `<name>/SKILL.md` when the work crosses their domain.

- `software-development-life-cycle/SKILL.md` — cross-domain planning, sequencing, and architecture framing.
- `web-development-life-cycle/SKILL.md` — web app architecture, performance, SEO, browser compatibility.
- `mobile-development-life-cycle/SKILL.md` — Android/iOS lifecycle, permissions, offline sync, store release.
- `backend-and-data-architecture/SKILL.md` — API design, database schemas, microservices, messaging.
- `cloud-and-devops-expert/SKILL.md` — IaC, CI/CD, container orchestration, staged rollout, blue/green and red-team operations.
- `qa-and-automation-engineer/SKILL.md` — test design, the release ladder, validation strategy.
- `security-and-compliance-auditor/SKILL.md` — threat modeling, vulnerability hunting, compliance.
- `git-expert/SKILL.md` — branching, history surgery, push hygiene, PR/MR workflow.
- `preserve-existing-flow/SKILL.md` — brownfield ownership tracing before any existing-source edit.
- `reviewer/SKILL.md` — production-readiness verdict and final quality gate.
- `ui-design-systems-and-responsive-interfaces/SKILL.md` — design systems, responsive UI, accessibility.
- `ux-research-and-experience-strategy/SKILL.md` — research planning, journey mapping, recovery-path quality.
- `memory-status-reporter/SKILL.md` — memory health, learning recaps, mistake ledgers.
- `api-contract-design/SKILL.md` — REST/GraphQL/gRPC contract evolution and breaking-change governance.
- `react-performance-audit/SKILL.md` — React render cost, bundle size, virtualization, Core Web Vitals.
- `postgres-migration-safety/SKILL.md` — live-traffic Postgres schema changes, backfills, rollback paths.
- `stripe-integration/SKILL.md` — Checkout, Payment Intents, Subscriptions, Webhooks, Connect, refunds, disputes.
- `websocket-realtime-design/SKILL.md` — WebSocket, SSE, fan-out, reconnect, backpressure, auth lifecycle.

## Tooling Anchors

Commands referenced by AGENTS.md, with the command surface that owns them:

- `keel run -- <command>` — direct compaction wrapper for noisy shell output.
- `keel rewrite "<command>"` — inspection helper for the rewrite resolution.
- `keel code-search search` — repo-local discovery before broad scans.
- `keel code-search siblings` — completeness scan after a fix or implement.
- `keel flow start` / `flow check` / `flow finish` — brownfield evidence gates.
- `keel review pre-commit` / `pre-pr` / `gates check` — local review surfaces.
- `keel git-workflow commit-message` / `pr-body` / `lint-message` / `preflight` — commit/PR text and preflight.
- `keel hook install` / `uninstall` / `list` / `show` / `instructions` — managed lifecycle hook wiring.
- `keel memory scope resolve --create-missing --refresh-system-map` — memory refresh on session start, pre-compact, and session end (hook-driven; agent may also invoke by hand mid-session).
- `keel memory working-brief` / `system-map refresh` / `completion-gate` — durable memory writes (working brief + system map) and completion probe.
- `keel orchestration resume-status` / `task begin|progress|complete` / `runtime-preflight` / `checkpoint` — workstream lifecycle.
- `keel gain` — token-savings analytics.

These commands are owned by the `keel` Rust CLI in `rust/crates/keel/`. If the command surface changes, this anchor needs to update with it.
