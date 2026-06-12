# Claude Code Spec Gaps — Remaining TODO

**Audit date:** 2026-06-11
**Source:** Whole-project gap audit against the official Claude Code docs (skills,
subagents, hooks, plugins) and 13 competitor harness repos.

The audit surfaced 17 findings — features the official spec documents that
claude-core's 41 skills, 24 subagents, and 30 hook events did not yet use.
**7 are done and on `main`. 10 remain.** This file tracks the 10 open items so
the next agent does not re-derive them.

---

## Done (7) — shipped on `main`

| # | Finding | Fix | Where |
|---|---------|-----|-------|
| 3 | `disallowed-tools` in skills | Added `disallowed-tools: Edit, Write, Bash(git push:*)` | `finishing-a-development-branch/SKILL.md` |
| 4 | `paths` glob activation | Confirmed present | `postgres-migration-safety`, `react-performance-audit`, `stripe-integration` |
| 5 | `argument-hint` in skills | Added autocomplete hints | `reviewer`, `git-expert` |
| 9 | `user-invocable: false` | Hid background-knowledge skills from `/` menu | `compounding-knowledge`, `compression-discipline`, `memory-status-reporter` |
| 10 | `effort` field in skills | Escalated to `effort: xhigh` | `adversarial-security-review` |
| 12 | `maxTurns` in subagents | Added `maxTurns: 30` | `.claude/agents/preserve-existing-flow.md` |
| 13 | `background` in subagents | Added explicit `background: false` | `.claude/agents/reviewer.md` |

---

## Remaining (10) — open

Each item below names the finding, why it is still open, and the concrete change
to make. Most are not "can't do" — they are deliberate work that needs a decision
or a behavioral-test pass (`writing-skills` RED-GREEN) before shipping, because
adding a frontmatter field that changes runtime behavior on a skill that fires on
every session is higher-risk than the 7 mechanical fixes above.

### Finding 1 — Skills `context: fork` + `agent:`

- **Spec:** `context: fork` runs a skill's body in a forked subagent; `agent:`
  picks the subagent type (`Explore`, `Plan`, `general-purpose`, or custom).
- **Why open:** Forking changes where the skill's tool calls land. Candidate
  skills are the heavy read-only analysis ones (`systematic-debugging`,
  `adversarial-security-review`, `ux-research-and-experience-strategy`) where the
  verbose trace should stay out of the main context. But `context: fork` with a
  guidance-only skill (no actionable task) returns nothing useful — per the docs,
  it only makes sense for skills with explicit task instructions. Each candidate
  needs a read to confirm it has an actionable task body before flipping.
- **Action:** For each candidate, verify the SKILL.md body is a task (not pure
  reference), then add `context: fork` + `agent: Explore` (read-only) or
  `agent: general-purpose` (needs edits). Pressure-test with `writing-skills`
  RED-GREEN that the forked skill still returns a usable summary.

### Finding 2 — Skills `hooks:` (scoped lifecycle hooks)

- **Spec:** A skill can define `hooks.PreToolUse` / `hooks.PostToolUse` etc. in
  its own frontmatter; they fire only while the skill is active.
- **Why open:** Needs a concrete enforcement to wire. The obvious one: a
  `security-and-compliance-auditor` skill `PreToolUse` guard that blocks `Edit`/
  `Write` while auditing. But that overlaps the subagent's `disallowedTools`
  (already enforced at the subagent layer), so the skill-layer hook only adds
  value for the main-thread skill invocation, not the subagent. Decide whether
  the main-thread auditor skill should be write-blocked before wiring.
- **Action:** Add a `PreToolUse` hook to `security-and-compliance-auditor/SKILL.md`
  that exits non-zero on `Edit`/`Write` during an active audit, OR document that
  the subagent layer is the intended enforcement point and close as won't-do.

### Finding 6 — Skills string substitutions (`$ARGUMENTS`, `${CLAUDE_SESSION_ID}`, …)

- **Spec:** `$ARGUMENTS`, `$ARGUMENTS[N]`/`$N`, `$name`, `${CLAUDE_SESSION_ID}`,
  `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}` are substituted before the skill body
  reaches Claude.
- **Why open:** No skill currently takes arguments because all claude-core skills
  are matcher-invoked (auto-loaded by description), not slash-invoked with args.
  Adding `$ARGUMENTS` only helps the few skills a user would type with an argument
  (`/reviewer <branch>`, `/git-expert <branch>`). Finding 5 already added
  `argument-hint` to those two; finding 6 is the follow-through — actually
  consume the argument in the body.
- **Action:** Add a `## Arguments` section to `reviewer/SKILL.md` and
  `git-expert/SKILL.md` that reads `$ARGUMENTS` (e.g. "If `$ARGUMENTS` names a
  branch or base-ref, scope the review to it"). Add `${CLAUDE_SESSION_ID}` to the
  reviewer body for session-scoped finding tags.

### Finding 7 — Skills shell backtick injection (`` !`cmd` ``)

- **Spec:** `` !`<command>` `` at line start runs before the skill reaches Claude
  and the output replaces the placeholder. Multi-line via ` ```! ` fence.
- **Why open:** High value (a `git-expert` skill could prepend
  `` !`git status --short` ``), but the injected command runs on every skill load,
  so a slow or failing command degrades every invocation. Needs the command to be
  fast, read-only, and safe to fail. Also Windows-shell-dependent (see Finding 8).
- **Action:** Add `` !`git status --short` `` and `` !`git log --oneline -5` `` to
  `git-expert/SKILL.md` under a `## Current state` heading, guarded so a non-git
  directory degrades gracefully. Set `shell:` per Finding 8.

### Finding 8 — Skills `shell: powershell`

- **Spec:** `shell: powershell` runs inline `` !`cmd` `` blocks via PowerShell on
  Windows (requires `CLAUDE_CODE_USE_POWERSHELL_TOOL=1`).
- **Why open:** Coupled to Finding 7 — only matters once a skill uses backtick
  injection. claude-core is developed on Windows, so any skill that adds Finding 7
  injection should set `shell: powershell` or use commands that work in both
  shells (`git status` works in both; `ls`/`dir` do not).
- **Action:** Ship together with Finding 7. Prefer cross-shell commands; add
  `shell: powershell` only where a PowerShell-specific command is needed.

### Finding 11 — Skills `model` field

- **Spec:** `model` overrides the active model for the skill's turn (`sonnet`,
  `opus`, `haiku`, `fable`, full ID, `inherit`).
- **Why open:** A model override per skill is a cost/quality decision the project
  owner should make, not an agent. Candidates: `model: opus` for
  `adversarial-security-review` and `data-and-ml-engineering` (deep reasoning),
  `model: haiku` for `compression-discipline` and `output-economy` (lightweight).
  But forcing a model can surprise a user who set a session model deliberately.
- **Action:** Decide the policy (owner call), then add `model:` to the agreed
  skills. Default recommendation: leave `inherit` everywhere except
  `adversarial-security-review` → `opus`.

### Finding 14 — Subagent `isolation: worktree`

- **Spec:** `isolation: worktree` runs a subagent in a temp git worktree branched
  from the default branch; auto-cleaned if no changes.
- **Why open:** Only useful for a subagent that *edits* files and could collide
  with the main checkout. The 24 subagents are mostly read-only analysts
  (`disallowedTools: Edit, Write`), so worktree isolation buys nothing for them.
  The candidate is a future builder/auto-fix subagent that does not exist yet.
- **Action:** Defer until a write-capable subagent exists. If/when a
  `code-fixer`-style subagent is added, set `isolation: worktree` on it. Document
  in `using-git-worktrees/SKILL.md` that the subagent field is the preferred path
  over manual worktree creation for write-capable agents.

### Finding 15 — Subagent `memory` (persistent cross-session memory)

- **Spec:** `memory: user|project|local` gives a subagent a persistent
  `MEMORY.md` directory that survives sessions.
- **Why open:** Highest-value remaining item. `reviewer` (recurring-pattern
  memory), `preserve-existing-flow` (per-repo ownership conventions), and
  `git-expert` (per-repo branch/commit conventions) would each benefit. But it
  overlaps claude-core's own memory surfaces (`memoriesv2`, working briefs,
  SYSTEM_MAP) — wiring native subagent memory risks duplicating the same fact in
  two stores. Needs a decision on which store owns what.
- **Action:** Decide the boundary (native subagent `memory` for incidental
  per-agent learnings; claude-core memory families for structured reconcilable
  artifacts), then add `memory: project` to `reviewer` and `preserve-existing-flow`
  subagents. Update `_shared/subagent-iron-law.md` § Memory write surfaces to name
  the new native directory and the non-duplication rule.

### Finding 16 — Subagent inline `mcpServers`

- **Spec:** A subagent can declare an inline MCP server scoped only to itself,
  keeping its tools out of the main conversation.
- **Why open:** The candidate (`postgres-migration-safety` or
  `data-and-ml-engineering` scoping a Postgres MCP server) requires an actual MCP
  server to scope — claude-core ships one MCP server (`claude_core`) and none of
  the specialist subagents need a private external server today. This is
  speculative until a real external-tool need exists.
- **Action:** Defer until a specialist genuinely needs a private external MCP
  server. Document the pattern in `CLAUDE.md` § Subagent frontmatter (already
  done) so it is reachable when needed.

### Finding 17 — Plugin manifest `outputStyles`, `lspServers`, `experimental.monitors`, `userConfig`, `channels`

- **Spec:** All five are supported plugin-manifest keys.
- **Why open:** Each is a distinct feature, not one change:
  - **`lspServers`** — highest value: declare `rust-analyzer` so Claude gets live
    diagnostics on the Rust workspace during editing, not just on `cargo build`.
    Requires the user to install `rust-analyzer`; ship as opt-in.
  - **`experimental.monitors`** — a `compression-discipline` monitor watching
    context fill rate; overlaps the existing CLI gain/session surface. Decide
    whether a Claude-visible monitor adds value over the CLI.
  - **`outputStyles`** — custom terse/prose styles; overlaps the four built-in
    styles. Low value unless a specific style is needed.
  - **`userConfig`** — prompt for branch-model / commit-category preferences at
    enable time instead of hand-editing settings. Useful but needs the runtime to
    read the config back.
  - **`channels`** — Telegram/Slack/Discord injection; out of scope for a delivery
    harness.
- **Action:** Ship `lspServers` with `rust-analyzer` as opt-in first (highest
  value, lowest risk). Evaluate `userConfig` for branch-model/commit-category
  prompts second. Defer `monitors`, `outputStyles`, `channels` as low-value or
  out-of-scope.

---

## Why these 10 are not "just do it"

The 7 done items were mechanical frontmatter additions with no behavioral risk and
were verified by `skill-lint` (41 skills, 0 failed). The 10 remaining each carry
one of:

- **A behavioral-test requirement** (1, 6, 7) — the change alters what the skill
  does at runtime, so it needs a `writing-skills` RED-GREEN pass to prove the new
  prose/injection actually improves behavior, not just passes lint.
- **An owner decision** (11, 15, 17) — model overrides, memory-store boundaries,
  and LSP/monitor opt-ins are project-policy calls, not agent calls.
- **A missing prerequisite** (2, 14, 16) — the feature only adds value once a
  write-capable subagent or a private external MCP server exists, neither of which
  is in the repo today.

Shipping them blind would either regress behavior (the failure mode the audit's
own iron law warns against — "correct code that solved the wrong problem still
gets thrown away") or add speculative config the project does not need yet.

## Next steps

1. Owner decides policy for Findings 11, 15, 17 (model overrides, subagent memory
   boundary, LSP opt-in).
2. Agent ships Findings 1, 6, 7, 8 together behind `writing-skills` RED-GREEN
   proof (they are coupled: fork + arguments + injection + shell).
3. Defer 2, 14, 16 until their prerequisites exist; keep the documented patterns
   in `CLAUDE.md` so they are reachable.
