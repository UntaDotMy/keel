<!--
Explicit Reason: Pi coding agent does not support @AGENTS.md imports and discovers instructions from ~/.pi/agent/AGENTS.md globally or ./AGENTS.md locally. When running in a workspace containing AGENTS.md, AGENTS.md is the canonical instruction source. This file provides the standalone bootstrap contract for Pi environments where the repository AGENTS.md is not directly loaded.
-->
# keel Iron Law for Pi Agent

You are running with keel discipline. These rules are non-negotiable. When running in a repository with `AGENTS.md`, defer to `AGENTS.md` as the canonical instruction source.

## Iron Law -- follow on every turn

0. **Read first.** Read the workspace SYSTEM_MAP and the owning file before claiming behavior; never propose changes against an imagined version.

1. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed -- the owning module, the framework, the real requirement. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode: it passes review and still gets thrown away. If the request is ambiguous in a way that changes what you build, ask before building, not after.

2. **Invoke relevant skills.** When a skill plausibly matches, check its trigger before loading it; do not auto-load on keyword proximity. Skip skills that do not plausibly match, including docs-only or formatting-only work when no content behavior changes. Use the keel MCP tools `skill_route` and `skill_get` to load the matching skill.

3. **Find the root cause.** Trace the symptom end-to-end with file:line evidence and confirm the suspect is on that path before changing anything. The real problem is usually one layer below what was asked.

## Working Workflow

- **Start work:** `keel anvil compile --goal "..." --bar "..."` then `keel anvil run --dry-run`
- **Live refine:** `keel anvil run` / `keel anvil loop` on the CLI only (not MCP)
- **Review before PR:** `keel review pre-pr --base-ref origin/feat --format compact`
- **Refresh memory:** `keel memory scope resolve --create-missing --refresh-system-map`
- **Search code:** `keel code-search search --workspace-root "$PWD" --query "<query>"`
- **Scan the class:** `keel code-search siblings` after a fix or implement

## Native Command Routing -- Must Follow First

When a native keel command owns the job, use it instead of recreating the behavior with raw shell.

- **Noisy shell commands:** prefer `keel run -- <command>` for test, build, lint, log, status, search, Docker, Kubernetes, Terraform, package-manager, and CI-style commands. Use `keel rewrite "<command>"` when unsure whether a command has native compaction.
- **Repository search:** prefer `keel code-search search --workspace-root "$PWD" --query "<query>"`. After a fix or implement, run `keel code-search siblings`. Use raw `rg`, `grep`, `find`, or `git grep` only after scoped search is insufficient.
- **Commit/PR text:** use `keel git-workflow commit-message --from-diff` and `keel git-workflow pr-body --from-diff` before submitting. Run `keel review pre-pr` before finalizing.

## Branch and Commit Discipline

- Branch model: `main` <- `dev` <- `feat` <- `task/<task>` [<- flat `task/<task>-<subtask>`] (never nested task refs, or `feat/<task>` while bare `feat` exists)
- Fix in-flight bugs on the same work branch, never a new branch
- Commits: `Add : FEATURE : short info`
- Never delete a branch after push or merge
  - Commit subjects: `Add : FEATURE : short information` (categories: Add, Config, Refactor, Wip, Fix, Docs; FEATURE uppercase; spaces around colons)
  - Example: `Wip : FEATURE : short information`

## MCP Tool Surface

The keel MCP server provides tools for awareness, search, compaction, memory, and skills. Use them via the `mcp` proxy tool or directly if registered as direct tools.

| Tool | Description |
|---|---|
| `recall` | Full-text search over durable memory: working briefs, system maps, memories |
| `system_map` | Read the workspace SYSTEM_MAP.md (call before any claim about repo structure) |
| `run_command` | Run shell commands through the compaction proxy (preferred for noisy commands) |
| `recall_status` | Check recall index health: document count, schema version, last-sync |
| `skill_route` | Route a prompt to the correct keel skill |
| `skill_get` | Load a skill's full SKILL.md body by name |
| `skill_list` | List every installed skill with name, description, and when_to_use |
| `memory_status` | Report durable-memory health: recall index snapshot, per-family record counts |
| `brief_list` | List stored working briefs (request, constraints, acceptance criteria) |
| `brief_get` | Read one stored working brief by id |
| `brief_create` | Persist a working brief so context survives compaction |
| `system_map_refresh` | Regenerate the cached workspace SYSTEM_MAP.md |
| `context_brief` | Get the keel context brief: iron law, skill catalog, memory health, newest brief |
| `cli` | Run any keel CLI subcommand (review, git-workflow, anvil, memory, etc.) |
| `anvil` | Drive the Anvil delivery loop (compile/cast/sieve/stamp/run --dry-run in-process; loop/live run background via command_output) |

## Skills and Reference Guidance

Discover and load specialist skills dynamically using `skill_route` and `skill_get`.
Detailed domain procedures, testing standards, and architecture doctrine live in the canonical references under `AGENTS/references/` in workspaces containing Keel.
