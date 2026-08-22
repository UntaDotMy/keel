# Source priority and extended workflow

## Workflow

1. Determine the reporting window. Default to today in the local timezone unless the user asks for a different period.
2. Resolve the workspace scope first so the report can prefer agent-instance, workstream, and workspace files over broad global memory. When the scoped folders do not exist yet, create them on the same call:
   ```bash
   keel memory scope resolve --workspace-root "$PWD" --create-missing --format json
   ```
3. Refresh the scoped system map when the workspace layout has changed since the last report so the source-priority lookup walks an accurate tree:
   ```bash
   keel memory system-map refresh --workspace-root "$PWD"
   ```
4. Read the scoped artifacts directly with your host's read and search tools, walking the Source Priority list below. The human-readable narrative report is composed from those files. A `keel memory report` command does exist — it is an alias for `memory status`, a compact health summary across the implemented memory families — so use it for the structured family snapshot, but compose the narrative status report from the scoped files rather than expecting `memory report` to write the prose for you.
5. Inspect the latest scoped working brief and any open completion-gate entries to anchor "what is in progress" and "what is unresolved":
   ```bash
   keel memory working-brief list --json
   keel memory working-brief show --id <brief-id> --json
   ```
6. For non-trivial tasks that have a tracked completion gate, surface its current state in the report:
   ```bash
   keel memory completion-gate check --id <entry-id> --json
   ```
7. Read every command output before responding. Do not paraphrase away uncertainty.
8. If tool-use mistakes were part of the work, ensure the rollout summary captures the tool name, failure symptom, cause, verified fix, and prevention note so future reports can surface it.
9. If research produced a reusable finding, record it inside the scoped working brief so the next task rereads it with source and freshness context, and archive stale or superseded entries instead of replaying them forever.
10. If the user wants a saved artifact, write the composed report to a file with the `Write` tool under `~/.claude/memories/reports/<date>-memory-status.md`.
11. If the user wants a broader window, widen the file walk to a trailing seven-day slice ending on the anchor date by filtering scoped files by their dated names or modification times.
12. When the user supplies a durable correction or decision, the main lane should persist it through the right layer before this skill summarizes the updated memory state. For corrections that must survive a single task (a name spelling, a permanent preference, a confirmed constraint), append to `SESSION-STATE.md` with the `Write` tool. For task-scoped decisions tied to active work, capture them in the scoped working brief instead:
   ```bash
   keel memory working-brief write \
     --request "Option B is the confirmed direction." \
     --acceptance-criteria "Subsequent reports reflect Option B."
   ```
13. For high-context work only, append the newest breadcrumb to the scoped `working-buffer.md` with the `Write` tool before the thread gets noisy. Keep entries terse and dated.
14. For non-trivial or compaction-prone work, persist the scoped working brief before the thread gets noisy:
   ```bash
   keel memory working-brief write \
     --id req-1 \
     --request "Persist the working brief" \
     --acceptance-criteria "working-brief show returns req-1." \
     --constraints "No drift between brief and live work."
   ```
15. For non-trivial tasks tracked in a scoped working brief, surface the completion gate before delivering the final answer:
   ```bash
   keel memory completion-gate check --brief-id <brief-id> --proof "Evidence summary."
   ```
16. When a memory write is requested, report what changed and which scoped files were touched before final delivery.
17. Archive overflow from L1 memory files instead of letting always-read files grow without bound. Move stale entries into the scoped `archive/` folder with the `Write` tool.
18. When long sessions or repeated mistakes suggest drift, re-read the scoped L1 files and compare observed behavior notes against the current canonical rules, then capture the reconciliation in the working brief.
19. When the same tool shape or plan keeps failing, record the failure signature in the scoped working brief, check whether the retry budget is exhausted, and change approach before repeating the same failure a third time.
20. Keep local runtime state and memory storage separate from model-visible context unless they are intentionally exposed. Prefer concise scope notes over replaying full histories, choose one conversation continuation strategy per thread unless there is an explicit reconciliation plan, and preserve workflow names plus validation evidence for non-trivial reports.
21. Before the final answer, reconcile every explicit user requirement against current evidence, rerun the scoped completion gate for non-trivial tasks, and do not present unresolved work as complete.

## Source Priority

1. `~/.claude/memories/agents/<role>/<workspace-slug>/workstreams/<workstream-key>/instances/<agent-instance>/MEMORY.md` for the current scoped role instance memory
2. `~/.claude/memories/agents/<role>/<workspace-slug>/workstreams/<workstream-key>/MEMORY.md` for role-local notes within the active workstream
3. `~/.claude/memories/workspaces/<workspace-slug>/workstreams/<workstream-key>/memory/SESSION-STATE.md` and `working-buffer.md` for WAL-backed corrections and high-context breadcrumbs
4. `~/.claude/memories/workspaces/<workspace-slug>/workstreams/<workstream-key>/SUMMARY.md` and `MEMORY.md` for focused branch or task notes
5. `~/.claude/memories/workspaces/<workspace-slug>/SUMMARY.md` and `MEMORY.md` for workspace-shared notes
6. `~/.claude/memories/research_cache/<workspace-slug>/cache.jsonl` for shared reusable findings, freshness notes, and reward or penalty status
7. Matching `~/.claude/memories/rollout_summaries/*.md` summary entries for dated task outcomes, reusable knowledge, rewarded patterns, penalty patterns, research-cache updates, and captured tool-use failure patterns; follow each summary's `rollout_path` into the deeper session `.jsonl` only when exact evidence is needed
8. `~/.claude/memories/workspaces/<workspace-slug>/reference/` and `~/.claude/memories/workspaces/<workspace-slug>/workstreams/<workstream-key>/reference/` for deeper L3 references opened on demand
9. `~/.claude/memories/archive/<workspace-slug>/workstreams/<workstream-key>/` for stale or superseded notes that should not be replayed by default
10. `~/.claude/memories/MEMORY.md` for durable cross-session learnings
11. `~/.claude/memories/memory_summary.md` for user-needs context
12. `~/.claude/memories/raw_memories.md` only when higher-priority files are too thin

## Guardrails

- Never present brain growth as literal cognition. Say it is a heuristic derived from memory artifacts.
- Treat self-awareness, self-healing, self-training, and self-learning language as bounded maintenance behavior over memory artifacts, validation loops, and research-cache updates, not as hidden model retraining or free-form autonomy.
- Prefer no percentage over a fake percentage. If the sample is too small, say so.
- Distinguish clearly between "no learning captured" and "no work happened".
- Quote only short snippets when necessary; otherwise summarize.
- If the report window has no artifacts, say that directly and recommend the next useful window.
- Do not invent tool mistakes; report only tool-use failures that are actually captured in memory artifacts.
- Do not claim a rewarded pattern unless the artifacts show a validated win, a clear reuse success, or durable guidance that future work should prefer.
- Do not claim research-cache reuse or staleness unless the artifacts actually record that update.
- Do not present unresolved work as complete when the user asked for a finished status report or closure decision.

## References

- `references/reporting-rubric.md` for metric definitions and status thresholds
