# Observability, Incidents, and SRE

## Use This Reference When

- Defining metrics, logs, traces, dashboards, or alert strategy
- Setting SLOs, error budgets, paging rules, or reliability priorities
- Improving incident response, rollback ergonomics, or postmortem quality
- Reviewing capacity, saturation, or dependency health visibility

## What Good Looks Like

- SLIs map to user-visible outcomes such as request success, latency, freshness, or completion time.
- SLOs are owned, realistic, and paired with escalation or release consequences.
- Logs, metrics, and traces work together; operators can move from symptom to cause without guesswork.
- Alerts are actionable, deduplicated, and linked to runbooks with named owners.
- Incident response includes command, communication, mitigation, and learning loops.

## Reliability Planning Rules

- Measure request volume, latency, errors, and saturation before debating optimizations.
- Page on symptoms that threaten users or error budgets, not on every internal fluctuation.
- Include deployment and dependency markers in dashboards so operators can correlate change with impact.
- Design rollback and degrade modes that reduce harm even when the full fix is not ready.
- Use postmortems to improve detection, recovery, and prevention instead of assigning blame.

## Real-World Scenario: Stabilize a Noisy On-Call Rotation

- Start with the top recurring pages and identify whether they reflect user impact, weak thresholds, or missing runbooks.
- Introduce a dashboard that correlates rollout events, latency, saturation, and dependency failures.
- Rewrite alerts to point to an immediate action: scale, roll back, pause a consumer, or fail over.
- Capture follow-up actions in a blameless postmortem with owners and due dates.

## Validation Gates

- Dashboards cover golden signals or equivalent workload indicators for the changed system.
- Alerts have owners, severities, thresholds, and linked runbooks.
- SLOs or equivalent service objectives exist for the user path most affected by the change.
- Incident procedures state who can declare, mitigate, communicate, and close a production event.
- Post-deploy verification includes runtime metrics and traces, not only success messages from tooling.

## Anti-Patterns

- Paging on raw CPU, memory, or restart counts with no user-impact context
- Logging every detail but omitting correlation IDs, tenant IDs, or request identifiers needed for triage
- Treating dashboards as a one-time setup instead of part of change readiness
- Keeping runbooks in people’s heads instead of accessible operator docs
- Declaring an incident resolved without checking lagging effects such as queue buildup or retry storms
- Writing postmortems that explain what happened but never change the system

## the harness Runtime Boundaries

- the harness can review instrumentation code, dashboard definitions, alert rules, and runbook content in the repository.
- the harness cannot confirm live alert noise, trace completeness, burn-rate behavior, or operator response quality without runtime evidence.
- When telemetry access is unavailable, provide exact dashboards, log queries, and alert checks for humans to run before declaring readiness.

## Related References

- 00-devops-knowledge-map.md
- 10-iac-and-state-management.md
- 20-cicd-release-and-secrets.md
- 99-source-anchors.md
