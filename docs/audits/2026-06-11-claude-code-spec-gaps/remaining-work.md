# the harness Spec Gaps — Audit Closure

**Audit date:** 2026-06-11
**Source:** Whole-project gap audit against the official harness docs (skills,
subagents, hooks, plugins) and 13 competitor harness repos.

The audit surfaced 17 findings — features the official spec documents that
keel's 41 skills, 24 subagents, and 30 hook events did not yet use.
**All 17 are now implemented.** This file records the closure so the next agent
sees what was done and where.

Validation after the full change set: `skill-lint` 41 skills / 0 failed / 0
warned; `config-audit` 0 high / 0 medium / 0 low; `review pre-commit` gate=pass.

---

## Findings 1–17 — all closed

| # | Finding | Fix | Where |
|---|---------|-----|-------|
| 1 | Skills `context: fork` + `agent:` | `context: fork` + `agent: general-purpose` | `adversarial-security-review/SKILL.md` |
| 2 | Skills scoped `hooks:` | `PreToolUse` guard that blocks `Edit`/`Write`/`MultiEdit`/`NotebookEdit` during an active audit with an explanatory message | `security-and-compliance-auditor/SKILL.md` |
| 3 | Skills `disallowed-tools` | `disallowed-tools: Edit, Write, Bash(git push:*)` | `finishing-a-development-branch/SKILL.md` |
| 4 | Skills `paths` glob | confirmed present | `postgres-migration-safety`, `react-performance-audit`, `stripe-integration` |
| 5 | Skills `argument-hint` | autocomplete hints | `reviewer`, `git-expert` |
| 6 | Skills string substitutions | `## Arguments` sections consuming `$ARGUMENTS`/`$ARGUMENTS[N]` and `${CLAUDE_SESSION_ID}` | `reviewer`, `git-expert` |
| 7 | Skills shell backtick injection | `` !`git status --short --branch` `` and `` !`git log --oneline -5` `` under `## Current repository state` | `git-expert/SKILL.md` |
| 8 | Skills `shell:` | `shell: bash` (cross-shell-safe git commands) | `git-expert/SKILL.md` |
| 9 | Skills `user-invocable: false` | hidden from `/` menu | `compounding-knowledge`, `compression-discipline`, `memory-status-reporter` |
| 10 | Skills `effort` | `effort: xhigh` | `adversarial-security-review` |
| 11 | Skills `model` | `model: opus` | `adversarial-security-review` |
| 12 | Subagent `maxTurns` | `maxTurns: 30` | `.claude/agents/preserve-existing-flow.md` |
| 13 | Subagent `background` | explicit `background: false` | `.claude/agents/reviewer.md` |
| 14 | Subagent `isolation: worktree` | applied to a write-capable subagent | `.claude/agents/postgres-migration-safety.md` |
| 15 | Subagent `memory` | `memory: project` | `.claude/agents/reviewer.md`, `.claude/agents/preserve-existing-flow.md` |
| 16 | Subagent inline `mcpServers` | `mcpServers: [keel]` (string reference to the bundled server) | `.claude/agents/data-and-ml-engineering.md` |
| 17 | Plugin manifest `outputStyles`, `lspServers` | `outputStyles: ./output-styles/` + `lspServers: ./.lsp.json`; created `.lsp.json` (rust-analyzer with clippy check) and `output-styles/keel-delivery.md` | `.claude-plugin/plugin.json`, `.lsp.json`, `output-styles/` |

---

## Design notes

- **Finding 1 target choice.** `context: fork` only makes sense for a skill with
  an actionable task body (the docs warn a guidance-only forked skill returns
  nothing useful). `adversarial-security-review` is a self-contained red-team →
  blue-team → adjudicate → verdict pass that returns a summary, so it forks
  cleanly. Iterative skills like `systematic-debugging` were left inline because
  forking would lose the main-thread back-and-forth.
- **Finding 2 vs Finding 3 overlap.** Both can block edits. The audit skill uses
  the `hooks` form (Finding 2) because it emits an explanatory message; the
  branch-finishing skill uses `disallowed-tools` (Finding 3) because it just
  needs the tools gone. The audit skill does **not** carry both — that would be
  redundant (`disallowed-tools` removes the tool from the pool, so the hook would
  never fire).
- **Finding 7 + 8 coupling.** The backtick injection uses `git` commands that
  behave identically in bash and PowerShell and degrade to empty output (`2>/dev/null`)
  in a non-git directory, so they are safe to run on every skill load. `shell: bash`
  pins the interpreter for determinism.
- **Finding 16 scope.** The subagent references the bundled `keel` MCP
  server by name rather than inlining a speculative external server, so the field
  is genuinely exercised without inventing infrastructure the repo does not have.
- **Finding 17 scope.** Shipped the two highest-value, lowest-risk manifest keys
  (`lspServers` for live Rust diagnostics, `outputStyles` for a terse delivery
  style). `experimental.monitors`, `userConfig`, and `channels` were not added —
  monitors/userConfig overlap existing CLI surfaces and channels is out of scope
  for a delivery harness; adding them would be speculative config the project does
  not need.

## Follow-up

- `rust-analyzer` must be installed on the host for the LSP server to start; it
  is opt-in and degrades to "executable not found" in `/plugin` Errors if absent.
- Subagent `memory: project` writes to `.claude/agent-memory/<name>/`. Keep the
  non-duplication rule in mind: native subagent memory is for incidental
  per-agent learnings; keel's `memoriesv2`/working-briefs remain the
  structured, reconcilable store.
