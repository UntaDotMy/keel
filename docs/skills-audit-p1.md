# Skills audit (SUPERHARNESS P1)

**Date:** 2026-09-06  
**Scope:** All installed `SKILL.md` packs (~53 including cowork mirrors).  
**Rule:** keep | merge-into | retire with rationale. **No new megaskill in this PR.**  
**Remote skills:** Keel loads skills from the local engagement/skills tree only (`skill_get` / install mirror). There is **no remote skill URL fetch** today. If remote load is ever added, pin + content-hash enforcement is mandatory (DevSecOp).

## Decision table

| Skill | Decision | Rationale |
| --- | --- | --- |
| using-keel | keep | Bootstrap / iron-law surface; SessionStart contract. |
| cowork/skills/using-keel | keep | Cowork mirror of bootstrap; MCP-only host needs its own copy. |
| cowork/skills/bootstrap | keep | Desktop SessionStart entry; distinct from CLI using-keel. |
| running-anvil | keep | Sole delivery loop; ClarifyPacket gate docs live here. |
| brainstorming | keep | Socratic design; **not** a substitute for ClarifyPacket. |
| research-enforcement | keep | Iron-law research; P2 sieve will consume its markers. |
| dependency-and-supply-chain | keep | Freshness / supply-chain owner for P2 A8. |
| critic | keep | Mid-flight critique; MergeGate critic input later. |
| reviewer | keep | Post-implementation gate; keep separate from critic. |
| requesting-code-review | merge-into reviewer | Already documented as alias; retire standalone once routing points only at reviewer. |
| receiving-code-review | keep | Author-side review intake; distinct from reviewer. |
| subagent-driven-development | keep | Subagent orchestration; escalate ClarifyPacket to main. |
| designing-agent-teams | keep | Team design; P3 teamwork templates extend this, not a new pack. |
| dispatching-parallel-agents | keep | Parallelism rules; complements designing-agent-teams. |
| deliberation | keep | Structured disagreement; small and non-overlapping. |
| writing-plans | keep | Plan authoring before execute. |
| executing-plans | keep | Plan execution partner to writing-plans. |
| writing-skills | keep | Meta skill for authoring skills. |
| test-driven-development | keep | RED-GREEN-REFACTOR technique. |
| behavior-driven-development | keep | BDD examples; overlaps TDD lightly but different audience. |
| systematic-debugging | keep | Root-cause debugging. |
| finishing-a-development-branch | keep | Branch closeout UX. |
| using-git-worktrees | keep | Worktree isolation. |
| git-expert | keep | Git safety specialist. |
| preserve-existing-flow | keep | Brownfield ownership trace. |
| software-development-life-cycle | keep | Cross-domain SDLC orchestration. |
| web-development-life-cycle | keep | Web-specific lifecycle. |
| mobile-development-life-cycle | keep | Mobile-specific lifecycle. |
| ui-design-systems-and-responsive-interfaces | keep | UI tokens / a11y; Human 6-Step content stays here until P2 artifacts. |
| ux-research-and-experience-strategy | keep | UX research; Clarify triggers may cite it. |
| component-driven-development | keep | Atomic UI technique. |
| domain-driven-design | keep | DDD modeling. |
| api-contract-design | keep | API contracts. |
| backend-and-data-architecture | keep | Backend/data specialist. |
| authentication-and-identity | keep | Auth/OIDC specialist. |
| postgres-migration-safety | keep | Migration lock safety. |
| websocket-realtime-design | keep | Realtime systems. |
| stripe-integration | keep | Payments specialist. |
| internationalization-and-localization | keep | i18n/l10n. |
| react-performance-audit | keep | React perf audits. |
| dart-and-flutter-expert | keep | Flutter specialist. |
| data-and-ml-engineering | keep | Data/ML pipelines. |
| cloud-and-devops-expert | keep | Cloud/CI/CD. |
| cloud-cost-and-finops | keep | FinOps. |
| observability-and-incident-response | keep | OTel / incidents. |
| security-and-compliance-auditor | keep | Sec/compliance review. |
| adversarial-security-review | keep | Red-team pass; complements security auditor. |
| qa-and-automation-engineer | keep | Test strategy / automation. |
| compounding-knowledge | keep | Durable knowledge capture. |
| memory-consolidation | keep | Memory note consolidation. |
| memory-status-reporter | keep | Human-readable memory status. |
| compression-discipline | keep | Per-turn compression playbook. |
| output-economy | keep | Output-token economy. |

## SUPERHARNESS theme coverage (no new pack)

| Theme | Primary owners (keep) | Notes |
| --- | --- | --- |
| Vague-prompt / ask-user | brainstorming + running-anvil + ClarifyPacket artifact | Gate is anvil code + packet; not a megaskill. |
| Design pipeline | ui-design-systems + ux-research + writing-plans | P2 owns `design.*.json` fail-closed. |
| Research-before-code | research-enforcement | P2 sieve. |
| Package freshness | dependency-and-supply-chain | P2 sieve. |
| No-hardcode / modular | preserve-existing-flow + reviewer + security packs | P2 sieve detectors. |
| Cross-platform text | running-anvil + docs/compatibility-matrix | No literal backslash-n spam in strings; PowerShell-safe paths. |
| Critic / QA | critic, reviewer, qa-and-automation-engineer | MergeGate in P2. |
| Teamwork | designing-agent-teams, dispatching-parallel-agents, subagent-driven-development | P3 templates only. |

## Retire / merge follow-ups (not this PR)

- **requesting-code-review → reviewer:** routing already aliases; a later cleanup can remove the duplicate directory after doc_parity allows.
- No Appllama-method or Antigravity teamwork megaskill until P3 and only by extending kept packs.

## Pin + hash (remote)

N/A for current tree (local install only). Future remote skill bodies: require exact pin (URL + version/commit) and sha256 match before load; fail closed on mismatch.
