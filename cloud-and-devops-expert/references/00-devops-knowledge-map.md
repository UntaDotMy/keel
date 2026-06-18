# DevOps Knowledge Map

Use this file to route cloud and DevOps work before loading deeper references.

## Start With These Questions

- Which environments, accounts, projects, or clusters are in scope?
- What is currently managed as code versus drift-prone manual state?
- What are the recovery objectives, availability targets, and compliance constraints?
- Which release steps are currently manual, risky, or opaque?
- What runtime evidence exists today: plan output, deployment logs, dashboards, alerts, or incident notes?

## Decision Map

### IaC, State, and Platform Topology

Load 10-iac-and-state-management.md when the task includes:

- Terraform or OpenTofu design, module structure, or provider configuration
- resource import, remote state, drift, workspace strategy, or environment layering
- networking, IAM, cluster baseline, or foundational platform setup

### CI/CD, Release, and Secrets

Load 20-cicd-release-and-secrets.md when the task includes:

- GitHub Actions, GitLab CI, Argo CD, or release automation
- build promotion, artifact handling, deployment strategies, or rollback gates
- secret management, workload identity, provenance, or policy enforcement

### Observability, Incidents, and SRE

Load 30-observability-incidents-and-sre.md when the task includes:

- metrics, logs, traces, dashboards, alerts, or on-call ergonomics
- SLOs, error budgets, incident command, runbooks, or postmortem quality
- capacity, saturation, or reliability planning tied to user-visible outcomes

## Standard Deliverables

- An environment and trust-boundary map
- An IaC or rollout plan with blast-radius notes
- A CI/CD gate sequence from commit to production
- An observability checklist covering dashboards, alerts, and runbooks
- A validation plan that distinguishes static review from runtime confirmation

## Real-World Scenarios

### Manual-to-IaC Platform Adoption

- Expect hidden dependencies, naming drift, and unclear ownership.
- Stabilize state management and imports before attempting broad replacement.

### Multi-Stage SaaS Release Pipeline

- Build, scan, package, promote, and roll back as separate control points rather than one opaque job.
- Treat secrets, database changes, and rollout verification as explicit release stages.

### On-Call Reliability Hardening

- Start from the noisy alerts, missing dashboards, and recurring incidents that burn operator time.
- Optimize for mean time to detect and recover, not only deployment throughput.

## Validation Starter Checklist

- Confirm the owner who can approve, roll back, and communicate for this change.
- Confirm the highest-blast-radius action: state change, credential change, network cutover, or data-plane rollout.
- Confirm which checks require live cloud, CI, or cluster access versus repository-only review.
- Confirm the operator evidence required before declaring success.

## the harness Runtime Boundaries

- the harness can synthesize IaC structure, release controls, and operational checklists from repository evidence.
- the harness cannot inspect live providers, policy engines, secret stores, or deployment controllers unless artifacts or access are supplied.
- When external access is unavailable, provide exact plan, apply, rollout, and dashboard checks for a human operator to execute.

## Related References

- 10-iac-and-state-management.md
- 20-cicd-release-and-secrets.md
- 30-observability-incidents-and-sre.md
- 99-source-anchors.md
