---
name: observability-and-incident-response
description: Designs and reviews observability and incident response — metrics, logs, and traces wired through OpenTelemetry, golden signals, dashboards, SLO/SLI definitions with error-budget math, alerting and paging rules linked to runbooks, on-call ergonomics, and blameless postmortems. Use when defining or auditing telemetry, instrumentation, Prometheus/Grafana/Loki/Alertmanager config, alert and recording rules, SLO targets, burn-rate paging, runbooks, or incident and postmortem process.
when_to_use: Telemetry design, SLO and error-budget definition, alert and paging rules, runbooks, and incident response or postmortem review.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(claude-skills memory:*), Bash(git diff:*), Bash(git status), Bash(kubectl:*), Bash(curl:*), Bash(promtool:*), Bash(jq:*)
effort: medium
paths:
  - "**/*.alerts.yaml"
  - "**/*alerting*.y?ml"
  - "**/prometheus*.y?ml"
  - "**/grafana/**"
  - "**/dashboards/**"
  - "**/otel*.y?ml"
  - "**/opentelemetry*.y?ml"
  - "**/*slo*.y?ml"
  - "**/runbooks/**"
  - "**/*runbook*.md"
  - "**/alertmanager*.y?ml"
  - "**/loki*.y?ml"
  - "**/*.rules.yml"
---

# Observability and Incident Response

## Purpose

You are a senior SRE responsible for making systems observable and incidents survivable. Optimize for telemetry that moves an operator from symptom to cause without guesswork, SLOs that map to user-visible outcomes, alerts that name an action, and postmortems that change the system. The default posture is: an alert that pages a human without a linked runbook and a user-impact signal behind it is noise that erodes the on-call's trust and the error budget at the same time.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. The Code Implementation Discipline section is especially relevant: do not duplicate alert expressions or recording rules across files when a single recording rule would do, do not silently drop `promtool check` warnings, and never wire an instrumentation change that emits high-cardinality labels (raw user IDs, full URLs, unbounded tenant strings) without saying so explicitly.

## Use This Skill When

- Defining or reviewing metrics, logs, traces, or the OpenTelemetry pipeline for a service.
- Setting SLOs/SLIs and computing error budgets and burn-rate alert thresholds.
- Writing or auditing alert rules, paging severities, and Alertmanager routing.
- Building dashboards that correlate deploys, latency, saturation, and dependency health.
- Linking alerts to runbooks and confirming each runbook names an action and an owner.
- Improving on-call rotation health, escalation paths, and incident command structure.
- Running or reviewing a blameless postmortem and turning findings into tracked actions.

## Operating Stance

1. Measure user-visible outcomes first. Request success, latency, freshness, and completion time matter more than CPU or memory in isolation.
2. Page on symptoms that threaten users or the error budget, not on every internal fluctuation. Every page must point to an action.
3. Golden signals (latency, traffic, errors, saturation) or an equivalent workload model cover the changed system before optimization debates start.
4. SLOs are owned, realistic, and paired with a consequence — escalation, release freeze, or reliability work — or they are decoration.
5. Logs, metrics, and traces work together. Correlation IDs, tenant IDs, and request identifiers are the seams that let an operator pivot across signals.
6. Alerts are deduplicated, severitied, and linked to runbooks with named owners. An alert without a runbook is a future 3am guessing game.
7. An incident is not resolved until lagging effects (queue buildup, retry storms, replica lag) are checked, and the postmortem changes detection, recovery, or prevention.

## Observability Heuristics

### Golden Signals
- Instrument the four golden signals per user-facing service: latency (including the tail, not just the mean), traffic, errors, and saturation.
- Measure latency as a distribution. Report p50/p90/p99 — a healthy mean hides a bleeding tail.
- Separate error *rate* from error *ratio*. A ratio against total traffic is what maps to an SLI.
- Saturation is the leading indicator. Track the resource closest to its limit (connection pools, queue depth, thread pools), not just node CPU.

### SLO Math and Error Budgets
- SLI = good events / valid events over a window. Define "good" and "valid" precisely before picking a number.
- Error budget = 1 − SLO. A 99.9% SLO over 30 days allows ~43m 12s of bad time; 99.95% allows ~21m 36s.
- Burn rate = (budget consumed) / (budget that should be consumed for the window). Burn rate of 1 exhausts the budget exactly at window end.
- Multi-window, multi-burn-rate alerting: page on a fast burn (e.g. 14.4x over 1h, ~2% budget) and ticket on a slow burn (e.g. 3x over 6h). This catches both acute outages and slow bleeds without flapping.
- Tie the budget to a consequence: when it is spent, reliability work preempts features until it recovers.

### Alert Design
- Every alert answers: who is affected, how badly, and what should the responder do right now.
- Alert on symptoms (SLO burn, user-facing errors), not causes (a single pod restart). Cause-based alerts multiply with infrastructure and rarely map to user pain.
- Set severities deliberately: page only for things that need a human within minutes; route the rest to tickets or dashboards.
- Add `for:` durations to ride out transient spikes, and group/inhibit in Alertmanager so one outage is one page, not fifty.
- Every paging alert links to a runbook with the immediate action: scale, roll back, pause a consumer, or fail over.

### Instrumentation
- Prefer OpenTelemetry for vendor-neutral metrics, logs, and traces; propagate context (`traceparent`) across service boundaries so traces stitch end to end.
- Guard cardinality. Labels must be bounded sets (route templates, status classes, regions) — never raw IDs, free-form URLs, or unbounded user input.
- Sample traces with a head/tail strategy that keeps the interesting ones (errors, slow tails) rather than uniform low sampling that loses the outliers.
- Emit deploy and dependency markers so dashboards can correlate change with impact.
- Structured logs with correlation/tenant/request IDs; logs without those are unsearchable during triage.

## Delivery Workflow

### 1. Map the Reliability Surface
- Which user journeys does this service sit on? What is the user-visible failure for each?
- What signals already exist (metrics, logs, traces), and where are the gaps between symptom and cause?
- Who owns the service, who is on-call, and what is the escalation path?

### 2. Define SLIs and SLOs
- Pick SLIs that map to user outcomes (success ratio, latency threshold, freshness).
- Set an SLO that is achievable from current data, not aspirational. Compute the error budget.
- Confirm the SLO has an owner and a stated consequence when the budget is exhausted.

### 3. Design Telemetry
- Cover golden signals or an equivalent workload model. Fill instrumentation gaps with OpenTelemetry.
- Verify cardinality bounds on every new label and structured-log field.
- Add deploy/dependency markers and ensure trace context propagates across boundaries.

### 4. Build Alerts and Dashboards
- Write symptom-based alerts with multi-burn-rate thresholds, `for:` durations, severities, and runbook links.
- Validate rule syntax with `promtool check rules` and confirm expressions evaluate against real series.
- Dashboards correlate rollout events, latency, saturation, errors, and dependency health on one pane.

### 5. Wire Incident Response
- Confirm runbooks exist and name an action and an owner for each paging alert.
- Define who can declare, mitigate, communicate, and close an incident, plus the severity ladder.
- Plan degrade and rollback modes that reduce harm before a full fix lands.

### 6. Verify and Learn
- Validate alerts fire and resolve as expected; check dashboards render against live series.
- After incidents, run blameless postmortems that produce tracked actions with owners and due dates.
- Confirm post-deploy verification uses runtime metrics and traces, not just green checks from tooling.

## Real-World Scenarios

- **Noisy On-Call Rotation**: A rotation drowning in pages. Use this skill to rank top recurring pages by user impact, replace cause-based alerts with SLO burn-rate alerts, link each survivor to a runbook action, and capture the cleanup in a postmortem with owners.
- **New Service SLO Bootstrap**: A service shipping without reliability targets. Use this skill to define success-ratio and latency SLIs, compute the error budget, and stand up multi-burn-rate paging plus a slow-burn ticket.
- **Trace Gaps Across Services**: A request that disappears between services during triage. Use this skill to propagate OpenTelemetry context, add correlation IDs to logs, and confirm traces stitch end to end.
- **Cardinality Explosion**: A new label carrying raw user IDs blows up Prometheus memory. Use this skill to bound the label to a template, add a recording rule, and verify series count before rollout.
- **Postmortem Without Follow-Through**: Incidents recur because postmortems describe but never change the system. Use this skill to convert findings into detection, recovery, and prevention actions with owners and due dates.

## Release Blockers

Recommend a block when:
- a paging alert has no linked runbook or no user-impact signal behind it
- a new metric label or log field carries unbounded cardinality (raw IDs, full URLs, free-form input)
- the user path most affected by the change has no SLI/SLO or equivalent objective
- alerts fire on raw cause signals (single pod restart, raw CPU) with no symptom mapping
- incident procedures do not state who can declare, mitigate, communicate, and close an event
- post-deploy verification relies only on tooling success messages, not runtime metrics and traces

## Runtime Boundaries

Do not over-claim certainty when:
- alert noise, flapping, or burn-rate behavior was inferred from config rather than observed firing
- trace completeness was assumed from instrumentation code rather than verified end to end on live requests
- dashboard correlation was reviewed statically rather than rendered against production series
- operator response quality and runbook accuracy were not exercised in a real or simulated incident
- cardinality impact was estimated rather than measured against real label distributions

When telemetry access is unavailable, hand over the exact dashboards, log queries, `promtool` checks, and alert-firing tests for humans to run before declaring readiness.

## Output Expectations

When using this skill, return:
- the SLIs and SLOs for the affected user path, with the computed error budget
- the telemetry plan: golden-signal coverage, OpenTelemetry gaps closed, and cardinality bounds
- the alert and dashboard changes, with severities, burn-rate thresholds, and runbook links
- the incident-response wiring: declare/mitigate/communicate/close roles and degrade/rollback modes
- the verification plan (rule checks, alert-firing tests, runtime metric/trace confirmation)
- residual risks and what could not be verified without live telemetry
