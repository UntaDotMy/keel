---
name: deliberation
description: Fusion-inspired structured disagreement protocol. When multiple experts or subagents provide conflicting opinions, structure the deliberation to surface consensus, contradictions, unique insights, and blind spots. Use before finalizing architecture decisions, resolving multi-model disagreement, or adjudicating conflicting review feedback.
when_to_use: Multiple experts or subagents give conflicting opinions; architecture decisions with multiple valid approaches; security vs performance tradeoffs; conflicting reviewer feedback; any decision where surfacing all perspectives before choosing is more expensive to get wrong than to delay.
allowed-tools: Read, Grep, Glob, Bash(keel memory:*), Bash(keel recall:*), Bash(git diff:*), Bash(git log:*)
effort: medium
---

# Deliberation

## Purpose

When multiple experts, subagents, or skill invocations produce conflicting opinions,
do not pick the loudest one. Structure the disagreement to surface what is agreed,
what is genuinely contested, what one expert uniquely saw, and what nobody mentioned.
The result is a scored analysis the user can act on, not a coin flip disguised as
judgment.

## When To Use It

- Two or more subagents return different recommendations for the same decision.
- A reviewer flags something a domain specialist explicitly defended.
- Architecture decisions with multiple valid approaches (monolith vs microservice,
  SQL vs NoSQL, sync vs async at a boundary).
- Security vs performance tradeoffs where both sides have legitimate weight.
- Conflicting review feedback where the author must choose which finding to address.
- Any moment the agent is about to pick one opinion and move on without surfacing
  the disagreement to the user.

Skip it for trivial decisions where the cost of being wrong is lower than the cost
of deliberation. The test: if the user would be surprised you picked without asking,
deliberate.

## The Protocol

### Step 1 — Collect all expert opinions

Gather every opinion on the decision point. Sources include:

- Subagent return values (each subagent's recommendation).
- Reviewer findings (each `file:line` finding with severity and rationale).
- Skill invocations that produced different conclusions.
- The working brief or user story that defines what matters.

Record each opinion with its source, the specific claim, and the evidence or
reasoning offered. Do not summarize away the disagreement — the disagreement
is the point.

### Step 2 — Score each point

Walk every claim from Step 1 and classify it into exactly one of four categories:

| Category | Meaning | Confidence |
|---|---|---|
| **Consensus** | Two or more independent sources agree on this point | High — proceed with confidence |
| **Contradiction** | Experts explicitly disagree on this point | Unknown — needs user input |
| **Unique insight** | Only one source raised this point | Medium — verify before acting |
| **Blind spot** | No source mentioned this concern, but it matters | Low — research before proceeding |

Scoring rules:

- A point is **consensus** only when two or more sources independently agree.
  One source restating another's opinion does not count as independent.
- A point is a **contradiction** when sources give different answers to the same
  question, not when they address different questions.
- A point is a **unique insight** when exactly one source raised it and the others
  did not address it (neither agreeing nor disagreeing).
- A point is a **blind spot** when the decision clearly involves a concern no source
  mentioned — for example, no one considered rollback safety, or no one checked
  the library's deprecation status.

### Step 3 — Synthesize

Apply the scoring to produce an action plan:

| Category | Action |
|---|---|
| Consensus | Proceed — high confidence, no user input needed |
| Contradiction | **Ask the user** — present both sides with evidence and let the user decide |
| Unique insight | **Verify** — run a targeted check (websearch, context7, code search) to confirm or refute before acting |
| Blind spot | **Research** — investigate the gap before proceeding; do not ignore it |

Do not skip the user-facing step for contradictions. The agent's job is to make
the tradeoff visible, not to make it for the user.

### Step 4 — Present the structured analysis

Show the user a clear table or list before proceeding:

```
## Deliberation: [decision point]

### Consensus (high confidence — proceeding)
- [point] — agreed by [source A], [source B]

### Contradictions (user decision needed)
- [point]
  - Source A says: [claim + evidence]
  - Source B says: [claim + evidence]
  - Recommendation: [agent's lean with rationale, but user decides]

### Unique insights (verifying)
- [point] — raised by [source only]; checking now...

### Blind spots (researching)
- [concern] — no source mentioned; researching now...
```

Only after the user resolves contradictions and unique insights are verified
should implementation proceed.

## Integration with Reviewer

The `reviewer` skill serves as the judge role in the deliberation protocol. When
reviewer returns findings that contradict a domain specialist's recommendation:

1. Record the specialist recommendation as one expert opinion.
2. Record each reviewer finding as another expert opinion.
3. Run the scoring protocol above.
4. Contradictions between reviewer and specialist go to the user — reviewer does
   not automatically override, and neither does the specialist.

This prevents two failure modes: the reviewer rubber-stamping a specialist's
choice, and the specialist dismissing valid review findings as "just style."

## Examples

### Multi-model disagreement

Two subagents recommend different caching strategies:

- Subagent A: Redis for distributed caching (cites latency benchmarks).
- Subagent B: In-process LRU for simplicity (cites deployment complexity).

Score: **Contradiction**. Present both to the user with tradeoffs. Let the user
decide based on their deployment constraints.

### Security vs performance tradeoff

Security specialist flags a missing input validation check. Performance specialist
says the validation adds 50ms per request on a hot path.

Score: **Contradiction**. Both are legitimate. Present to the user: the security
risk (unvalidated input) vs the performance cost (50ms added latency). The user
decides the acceptable tradeoff.

### Architecture decision with multiple valid approaches

Three opinions on database choice:

- Specialist A: PostgreSQL (relational, mature).
- Specialist B: SQLite (embedded, zero-ops).
- Specialist C: did not mention database at all.

Score: A and B are a **contradiction**. C is a **blind spot** — the third
specialist should have weighed in on persistence. Research C's domain to see
if the database choice affects it before presenting the contradiction to the user.

## Anti-Patterns

- Picking the first opinion without scoring the others.
- Treating a restated opinion as independent agreement (inflates consensus).
- Presenting a contradiction as "both are valid, do whatever" without a lean.
- Ignoring blind spots because no one raised them — silence is not safety.
- Deliberating on trivial decisions where the cost of being wrong is lower than
  the cost of the protocol.

## Validation

Self-check before presenting the analysis: did you record every source opinion,
score every point into exactly one category, verify unique insights, research
blind spots, and present contradictions to the user for resolution? If any step
was skipped, the deliberation is incomplete.
