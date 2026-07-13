# Context Efficiency Playbook

This document captures the retrieval, token-efficiency, and memory-architecture techniques that best fit this harness skill pack.

## Goals

- load less context without losing correctness
- reduce token spend on repeated or stale context
- keep code changes surgical and traceable
- preserve a human-style memory model without pretending the system has literal cognition

## Recommended Architecture

### 1. Working Brief Before Retrieval

Always translate the raw request into:

- user story or job-to-be-done
- desired outcome
- constraints and non-goals
- acceptance criteria
- edge cases
- validation plan

Why it helps:

- narrows search terms before any file load
- makes query rewriting easier
- reduces wasted token spend on irrelevant code or docs

### 2. Retrieval Ladder

Use the cheapest useful retrieval layer first:

1. **Native code search for repo-local discovery**
2. **Exact path or symbol lookup**
3. **Targeted snippet reads**
4. **Full-file reads for edit scope only**
5. **Hybrid retrieval for large corpora**
6. **Compression before generation**

Use `keel code-search search` at the top of the ladder when the question is about the current repository. It provides a native, workspace-scoped search index so the agent can find likely files, snippets, and symbol-adjacent hits before spending tokens on broad file reads.

## Techniques That Save Tokens

### Stable Prompt Prefixes and Prompt Caching

Keep stable instructions and reusable setup text at the front of prompts, and append volatile evidence later. This maximizes reusable-prefix cache hits on compatible models and reduces repeated billed input.

Important boundary:

- OpenAI's Prompt Caching guidance is prefix-based and activates automatically only when the prompt is large enough for caching eligibility.
- Tool definitions, schemas, and other repeated prompt blocks must stay identical, including ordering, if you want those tokens to remain cacheable.
- Append new conversation turns and volatile evidence at the end instead of rewriting the stable front of the prompt.
- Measure actual cache reuse with runtime telemetry such as `cached_tokens` when the provider exposes it.
- Do not promise a literal 100 percent cache-hit rate. Cache eligibility, eviction, provider behavior, and prefix drift make that impossible to guarantee across all runs. The correct goal is the highest practical hit rate with stable prefixes and measured results.

### Research Cache Design

Persist reusable research findings when they answer a non-trivial question correctly enough to change future work.

Before you start a fresh live research loop, check indexed memory and any recorded research-cache entry for the same question first. If the cached finding is still within its freshness guidance and fully answers the need, reuse it and skip redundant live research. Only return to external search for the parts that are missing, stale, uncertain, or explicitly time-sensitive.

Cache these items:

- resolved API usage patterns and correct command sequences
- version-specific caveats and migration notes with freshness dates
- known library bugs or workaround patterns with source links
- external benchmark findings that are likely to be reused
- search terms or source combinations that consistently produced the right answer

Do not cache these items durably:

- raw transcripts or full tool logs
- large copied documentation blocks
- stale vendor pricing, model behavior, or release claims without freshness markers
- one-off local environment noise that is not reusable outside the current task

Freshness rules:

- official docs and standards can live longer, but refresh when version-sensitive
- community workarounds should carry a shorter freshness horizon and lower default confidence
- durable architecture principles can stay longer when they are not tied to a fast-moving version
- if a cached finding is disproven, convert it into a penalty pattern and mark it stale instead of silently reusing it

Reward and penalty loop:

- validated cache hits become rewarded patterns that future agents should prefer first
- repeated mistakes, stale assumptions, and disproven cache entries become penalty patterns that future agents should avoid or refresh

### Surgical Patching

Prefer:

- diff-like edits
- narrow replacements
- modular helper extraction
- partial file reads followed by full reads only on touched files

Avoid:

- whole-file rewrites without need
- repeating unchanged code in prompts
- expanding scope because a file is already open

### Memory Layering

Use a human-style engineering analogy:

- **working memory** = active brief, current files, immediate validation target
- **workspace memory** = shared repo notes keyed by workspace slug
- **workstream memory** = focused branch, feature, or task notes under `~/.claude/memories/workspaces/<workspace-slug>/workstreams/<workstream-key>/` and mirrored second-layer notes under `~/.claude/memoriesv2/workspaces/<workspace-slug>/workstreams/<workstream-key>/`
- **role memory** = reviewer, worker, architect, or other role-local notes under scoped role or lane folders, with memoriesv2 lanes used for reusable second-layer retrieval when available
- **agent-instance memory** = one bounded lane under the scoped workstream or memoriesv2 lane path so reused sub-agents keep local context without loading every same-role note
- **episodic memory** = rollout summaries and recent task outcomes
- **durable memory** = indexed lessons and persistent user preferences
- **research cache** = freshness-aware reusable findings shared across agents in the same workspace
- **archive** = older or superseded scoped notes that should not be replayed by default

The point is not to mimic biology literally. The point is to stop replaying everything on every task.

### Scoped Retrieval Order

Before you load memory broadly:

- resolve the workspace and optional role scope first
- read agent-instance, role-local, workstream, and workspace notes before global durable memory
- check the shared workspace research cache before a new web-search loop
- mark stale or disproven findings stale or superseded instead of silently reusing them
- archive noisy older notes when fresher scoped notes supersede them, and use the archive only when current scoped lanes are too thin

### Compression and Summarization

When context is long:

- summarize first-pass findings into compact notes
- collapse old turns into reusable facts
- carry forward decisions, not raw transcript dumps
- use compact memory snapshots in final answers when the user wants learning visibility

For long-running production work, summarize before compaction instead of after it: checkpoint the working brief, working buffer, completion gate, and execution trace while the intent is still fresh, then reload those artifacts and reacquire the code surface with `keel code-search search` after compaction.

Do not depend on the model to infer that compaction happened. The safer native pattern is to run `keel orchestration resume-status` at the start of each non-trivial turn so the active workstream is reconstructed from durable artifacts even when the runtime summary is short or continuity breaks silently.

### Small-Model and Narrow-Task Routing

Use the smallest acceptable step for classification, routing, candidate filtering, query expansion, or duplicate detection. Reserve the large model context budget for actual implementation, final synthesis, and difficult reasoning across multiple evidence sources.

## How This Repo Implements the Strategy

- `AGENTS.md` requires a working brief before research or coding
- `AGENTS.md` requires a context retrieval ladder before broad context loading
- The Rust-native `keel install` command injects the shared execution-policy lines, including the cache-first research reuse gate, into `~/.claude/config.toml`
- The Rust-native `keel install` command scaffolds workspace, workstream, role, agent-instance, research-cache, archive, and report directories under `~/.claude/memories/`
- `keel code-search search` provides a native local **lexical** retrieval surface so agents can search the repo before widening to full-file reads (path filters accept `/` and `\`; there is no `index|demo|status|reset` subcommand)
- `keel memory scope resolve` resolves scoped search order and write targets for the active workspace
- `keel memoriesv2 scope resolve` proves where the mirrored second-layer workspace, workstream, lane, graph, and hook artifacts live
- `keel memory research-cache record|lookup|stale|reward|list` provides shared record, freshness-aware lookup, stale/reward marking, and listing for reusable research (scoped per command group on disk)
- `keel memory report` is implemented as an alias for `keel memory status` — a compact per-family record-count summary. The `memory-status-reporter` skill produces the richer human-readable narrative status report on top of the scoped files; use `memory report`/`status` for the structured family snapshot.
- `README.md` documents the setup and operational workflow

## Sources

- Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks — https://arxiv.org/abs/2005.11401
- LongLLMLingua: Accelerating and Enhancing LLMs in Long Context Scenarios via Prompt Compression — https://arxiv.org/abs/2310.06839
- OpenAI Prompt_Caching101 notebook — https://github.com/openai/openai-cookbook/blob/main/examples/Prompt_Caching101.ipynb
- OpenAI Context_summarization_with_realtime_api notebook — https://github.com/openai/openai-cookbook/blob/main/examples/Context_summarization_with_realtime_api.ipynb
- External assistant memory hierarchy patterns
- OpenHands microagents and memory condensation patterns — https://docs.all-hands.dev/openhands/usage/prompting/microagents
- Mem0 scoped memory patterns for user, app, agent, and run context — https://github.com/mem0ai/mem0
- GitHub code search syntax — https://docs.github.com/en/search-github/github-code-search/understanding-github-code-search-syntax
- GitHub code navigation — https://docs.github.com/en/repositories/working-with-files/using-files/navigating-code-on-github
