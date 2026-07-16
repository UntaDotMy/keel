# UX Metrics, Experiments, and Iteration

## UX Metrics Framework

Use a balanced set of UX indicators:

- Task success rate
- Time on task
- Error rate and recovery rate
- User satisfaction signals
- Retention and repeat usage for key journeys
- Accessibility quality signals for critical task completion

## HEART-Style Outcome Thinking

Map UX metrics to the HEART categories from Rodden, Hutchinson, and Fu (CHI 2010, Measuring the User Experience on a Large Scale):

- **Happiness**: attitudinal measures (satisfaction, NPS-style, perceived ease)
- **Engagement**: depth/frequency of involvement over time
- **Adoption**: new users completing a key experience in a period
- **Retention**: users still present / returning after a period
- **Task success**: efficiency, effectiveness, and error rates on critical tasks

Pair each chosen outcome with Goals, Signals, Metrics (same paper). Do not invent a sixth letter or rename the five categories.

## Metric Hygiene

- Pair leading indicators (interaction signals) with lagging indicators (retention/support impact).
- Define guardrail metrics so optimizations do not harm accessibility, trust, or performance.
- Segment metrics by user/device/context to avoid misleading aggregate conclusions.

## Experimentation

- Define hypothesis, target segment, and success threshold before launch.
- Use controlled experiments when causality is required.
- Guard against novelty effects and insufficient sample windows.
- Monitor for segment-level regressions, not only aggregate improvement.

## Iteration Loop

1. Detect issue/opportunity from evidence.
2. Prioritize by impact and confidence.
3. Implement and release with instrumentation.
4. Measure change and side effects.
5. Keep, refine, or roll back.
6. Feed validated changes back into UX/system standards to prevent regression.

## Prioritization Matrices

### Severity x Frequency
| | Common | Rare |
|---|---|---|
| **Critical** | Fix immediately | Fix soon, provide workaround |
| **Minor** | Fix when possible | Backlog |

### Impact vs Effort
| | Low Effort | High Effort |
|---|---|---|
| **High Impact** | Do first (quick wins) | Plan carefully (big bets) |
| **Low Impact** | Do when time permits | Don't do (waste) |
