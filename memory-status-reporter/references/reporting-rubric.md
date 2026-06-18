# Memory Reporting Rubric

## Purpose

This rubric defines the memory-health metrics used by `keel memory report` so the report stays honest and repeatable.

## Source Priority

1. Workspace-scoped, workstream-scoped, role-scoped, and agent-instance-scoped memory files for the active lane
2. The shared workspace research cache for the active workspace and workstream
3. Matching `rollout_summaries/*.md` filtered by structured metadata for the same `cwd`, matching `git_branch` or workstream key, and optional `agent_instance`
4. `MEMORY.md`
5. `memory_summary.md`
6. Archive lanes or `raw_memories.md` only when the higher-priority sources are too thin

## Rollout Evidence Rules

- Treat `rollout_summaries/*.md` as summary-layer evidence, not as a full transcript dump.
- Match rollout summaries by structured metadata fields such as `cwd:`, `git_branch:`, and `agent_instance:` instead of arbitrary body text.
- Use each summary's `rollout_path` JSONL only when the summary-layer evidence is too coarse and exact session evidence is required.
- Prefer the scoped memory lanes first; rollout summaries should enrich or confirm that scoped context rather than replace it.

## Status Labels

- **Healthy**: rollouts exist, no failed tasks, no open mistakes, and no meaningful stale-cache risk
- **Mixed**: work exists but there are partial tasks, open mistakes, unclear mistakes, or stale cache findings that need refresh
- **Needs Attention**: failed tasks exist, open mistakes outweigh resolved mistakes, or penalty pressure is clearly rising
- **Quiet**: no rollout summaries fall inside the requested window

## Metric Definitions

- **Task completion**: successful task outcomes divided by total task outcomes in the window
- **Learning capture**: matching rollout summaries in the reporting window with reusable-knowledge bullets, plus scoped-lane learnings that were active for that same window
- **Mistake resolution**: resolved mistakes divided by total mistakes in the window
- **Reward strength**: rewarded patterns captured in the window plus reusable knowledge that became durable enough to prefer next time
- **Penalty pressure**: repeated open or unclear mistakes, repeated tool mistakes, and stale findings that future work should avoid or refresh
- **Cache reuse rate**: research-cache updates marked as reused divided by total research-cache updates in the window when that sample exists
- **Cache freshness risk**: stale or refresh-needed findings captured in the reporting window
- **Brain size**: durable knowledge-bank units counted from `MEMORY.md` learnings plus reusable user-guidance bullets from `memory_summary.md`
- **Brain growth**: window growth units divided by brain size, where growth units equal reusable-knowledge bullets plus resolved mistakes
- **Learning momentum**: window growth units compared with the trailing seven-day daily average of growth units

## Resolution Labels

- **Resolved**: the issue text contains an explicit fix marker or the rollout succeeded and the issue text describes corrective action
- **Open**: the issue text still signals a blocker, dependency, or follow-up
- **Unclear**: neither condition is strong enough to classify confidently

## Confidence

- **High**: scoped lane files plus at least two matching rollout summaries in the window, or strong scoped memory plus durable memory context
- **Medium**: one matching rollout summary in the window or strong scoped memory context
- **Low**: no matching rollout summaries in the window and only fallback context remains

## Honesty Rule

Always label brain-growth and percentage metrics as heuristics derived from memory artifacts. They are reporting aids, not literal cognition measurements.
