# IaC and State Management

## Use This Reference When

- Designing Terraform or OpenTofu layouts, module boundaries, or provider strategy
- Importing existing resources into managed state
- Setting up remote state, locking, drift detection, or environment separation
- Reviewing networking, IAM, cluster baselines, or foundational platform changes

## What Good Looks Like

- State ownership is clear: each stack has a bounded blast radius and well-defined operators.
- Remote state, locking, and backup or versioning protections are in place before team-wide usage.
- Existing resources are imported or modeled deliberately instead of recreated by accident.
- Environment differences are intentional and reviewable, not hidden behind copy-paste divergence.
- Destructive actions require explicit review, and identity or policy dependencies are modeled alongside infrastructure.

## Design Rules

- Keep modules cohesive around ownership and lifecycle, not around arbitrary resource categories.
- Prefer smaller states with clear interfaces over one monolithic state file that every change can break.
- Treat provider aliases, regions, and accounts as part of the design contract; document them early.
- Use policy as code or equivalent review gates for high-risk controls such as public ingress, broad IAM, and encryption settings.
- Make drift visible and corrected through code, not normalized through repeated console edits.

## Real-World Scenario: Import a Live Database Stack into IaC

- Capture the current resource graph, backup posture, maintenance settings, and network attachments before import.
- Import resources incrementally and review the first plan for unintended replacement or policy drift.
- Separate foundational network and IAM changes from data-plane changes so rollback remains understandable.
- Do not broaden permissions temporarily just to make the first apply pass.

## Validation Gates

- Remote state backend, locking, and access controls are configured before collaborative changes.
- The first plan after import or refactor is reviewed for replacement risk, deletion risk, and drift explanations.
- Sensitive values stay out of state outputs and repository history.
- Destroy protection, backup expectations, and recovery ownership are explicit for data-bearing resources.
- Environment overlays and workspace strategy are documented enough that another operator can reason about blast radius.

## Anti-Patterns

- One state file for every environment and platform component
- Recreating existing resources instead of importing or reconciling them
- Using local state or unlocked remote state for shared production workflows
- Burying critical environment differences in duplicated folders with no common contract
- Treating manual console fixes as permanent instead of codifying or intentionally removing them
- Granting wildcard IAM because module boundaries were never clarified

## the harness Runtime Boundaries

- the harness can review HCL, manifests, module structure, and plan-safe refactors in repository state.
- the harness cannot confirm actual provider state, drift details, resource replacement behavior, or access-policy enforcement without plan output or live evidence.
- When provider access is unavailable, require plan files, import logs, state summaries, and operator confirmation before declaring rollout safety.

## Related References

- 00-devops-knowledge-map.md
- 20-cicd-release-and-secrets.md
- 30-observability-incidents-and-sre.md
- 99-source-anchors.md
