# Open-Source Memory Patterns Applied Here

This note records the live design patterns that informed the current `keel` memory layout so future edits do not drift back to a flat replay-everything model.

## Source Patterns

- External assistant memory hierarchy patterns separate user memory from project memory and let local project memory live closer to the working repo.
- Mem0 documents scoped memory identities such as `user_id`, `agent_id`, and `run_id`, which keeps shared knowledge separate from per-agent and per-run context. Source: https://docs.mem0.ai/open-source/graph_memory/overview
- `repoMemory` keeps repo-specific memory aware of branches and worktrees so code-change context does not smear across unrelated lines of work. Source: https://github.com/alioshr/repoMemory
- OpenAI's `Context_summarization_with_realtime_api` notebook keeps a live conversation-state container, summarizes older turns once token pressure crosses a threshold, preserves the newest turns verbatim, inserts the summary back into the session, and prunes summarized history. Source: https://github.com/openai/openai-cookbook/blob/main/examples/Context_summarization_with_realtime_api.ipynb
- OpenAI's `Prompt_Caching101` notebook recommends stable prompt prefixes so repeated instructions and setup are cached while volatile evidence is appended later. Source: https://github.com/openai/openai-cookbook/blob/main/examples/Prompt_Caching101.ipynb
- Public memory-bank workflows around Cline and similar agentic editors commonly externalize task state into repo-local markdown files such as project briefs, active context, and progress logs so resume does not depend on raw transcript replay. This repo adopts the same externalized-state principle, but keeps it in Rust-native scoped memory instead of third-party conventions.

## Applied Decisions In This Repo

- Keep **workspace memory** for shared repo-level guidance.
- Add **workstream memory** so a branch, feature lane, or focused task can hold its own summary and memory without polluting the whole workspace.
- Keep **role memory** for reused reviewer, worker, architect, or other role lanes inside the same workspace and workstream.
- Add **agent-instance memory** so one reused sub-agent can keep its own bounded notes instead of sharing every detail with every other same-role agent.
- Keep the **research cache shared at workspace scope** so validated research can be reused across agents without redoing the same web loop.
- Keep **archive lanes** for stale or superseded cache entries so old findings are preserved without staying in the active reuse path.
- Treat **compaction recovery** as a first-class workflow: checkpoint live state into scoped files before compaction pressure peaks, then reload those files before resuming work.
- Add a **resume-status gate** at turn start so recovery does not depend on the model realizing that compaction already happened.
- Keep the **latest high-value turns verbatim** in working memory, but summarize older progress into structured scoped artifacts instead of replaying raw transcript history.
- Resolve memory in this order: agent instance, role, workstream, workspace, shared cache, episodic rollout evidence, then global durable memory.
- Mirror the user's **L2 memory** and **L3 reference** split with explicit scoped `memory/` and `reference/` lanes so one home per fact stays enforceable instead of becoming a vague convention.

## Boundary Notes

- The repo treats the working-buffer trigger as a runtime-aware heuristic: use roughly 60 percent context usage when a runtime exposes that signal, otherwise switch on the buffer as soon as context pressure is clearly rising.
- Self-awareness, self-healing, and self-learning are implemented here as bounded maintenance loops over scoped memory, validation, reward or penalty tracking, and recalibration. They are not claims of autonomous model retraining.

## Why This Matters

- Different tasks in the same repo no longer need to read the same large memory bundle.
- Reused reviewers or workers can resume with smaller context.
- Old findings stay available, but stale findings stop crowding the active memory lane.
- The system can reward, penalize, archive, and refresh memory with narrower blast radius.
