<!-- keel:managed-host-file (remove this line before customizing to opt out of upgrades) -->
# Task Context (Fallback Only)

Use this ignored snapshot only when direct parent-thread handoff or agent-to-agent messaging cannot be passed directly. Keep it concise and do not
treat it as more authoritative than direct harness communication.

## TASK

_User request, scope, constraints, and confirmed requirements._

## PROJECT CONTEXT

_Relevant project structure, architecture boundaries, and existing behavior._

## PLANNER DECISION

_Planner's scope, reuse decisions, risks, workstream division, and Explorer questions._

## SKILL / CAPABILITY RECOMMENDATION

_Selected available skills/tools and fit, considered-but-rejected options and reasons, discovery results or unavailable-capability limits, and the downstream role/phase action. For named or clearly triggered skills, record that the downstream role must read the complete SKILL.md before acting._

## EXPLORATION FINDINGS

_Verified evidence, contracts, edge cases, and exact integration points._

## RELEVANT FILES

_Exact files, symbols, routes, tests, and schemas._

## EXISTING PATTERNS TO REUSE

_Established conventions, architectural precedents, and style invariants._

## IMPLEMENTATION CONTRACT

_Parent-validated goal, current/target behavior, regression boundaries, exact files, workstreams, ownership, dependencies, shared-file policy, integration requirements, edge cases, and validation/success criteria._

## WORKSTREAM HANDOFFS

_One concise completion or BLOCKED report per assigned Implementer/fix worker, including files, output contract, validation, and risks._

## INTEGRATION CHECK

_Combined diff/file ownership/dependency checks and cross-workstream validation. Record PASS before Reviewer._

## IMPLEMENTATION SUMMARY

_Changes made, files affected, and symbols modified._

## VALIDATION RESULTS

_Commands run, test outputs, exit codes, and known limitations._

## REVIEW FINDINGS

_Severity (CRITICAL/HIGH/MEDIUM/LOW), classification (CONFIRMED/REJECTED/UNCERTAIN), causal defect chain, and Reviewer verdict._

## FINAL CHANGE SUMMARY

_Actual final diff, files changed, implemented behavior, intentional behavior changes, review fixes, validation, and final impact. Consumed by the Pusher/shipper only after final Reviewer PASS and explicit user authorization._

## FINAL STATUS

_PASS, FIX REQUIRED, or shipping status with next action._
