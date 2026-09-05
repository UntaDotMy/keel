<!--
Purpose: Thin entry point and index for the managed harness routing, memory, validation, and delivery doctrine.
Caller: the harness agents using the synced keel native guidance surface.
Dependencies: AGENTS/references/*.md, the specialist SKILL.md files (roster asserted by tests/doc_parity_test.rs), keel CLI surface.
Main Functions: Route to the correct reference file in AGENTS/references/ for the section a reader needs.
Side Effects: None — this file is informational.
-->
# Skill Routing and Native Skill Guidance

## Purpose

This file is the entry point for the harness CLI on skill routing, native command usage, memory, validation, and delivery discipline. The detailed doctrine lives under [`AGENTS/references/`](AGENTS/references/) so the entry point stays small and the rules stay searchable.

## How To Use This File

- Open this file first to confirm scope.
- Open one reference file at a time, scoped to the section you actually need. Do not load every reference up front.
- When a reference and this file disagree, this file wins. Open a follow-up to reconcile the reference.
- When a reference and a specialist `SKILL.md` disagree on the specialist's own surface, the specialist `SKILL.md` wins for that surface.

## Section-to-Reference Map

| Topic | Reference file |
|---|---|
| Native command routing, hook transparent rewrite, token compaction | [AGENTS/references/10-native-command-routing.md](AGENTS/references/10-native-command-routing.md) |
| Skill routing, specialist roster, skill-focused execution, agent profiles | [AGENTS/references/20-skill-routing.md](AGENTS/references/20-skill-routing.md) |
| Execution strategy, iterative development loop, flow control, loop limits, general approach | [AGENTS/references/30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) |
| Code quality standards, testing requirements, feature flags | [AGENTS/references/40-code-quality-and-testing.md](AGENTS/references/40-code-quality-and-testing.md) |
| Feature delivery rules, best practices, prohibited shortcuts | [AGENTS/references/50-delivery-and-prohibited-shortcuts.md](AGENTS/references/50-delivery-and-prohibited-shortcuts.md) |
| Windows environment, cross-platform script portability | [AGENTS/references/60-environment-and-portability.md](AGENTS/references/60-environment-and-portability.md) |
| Code review requirements, automated quality checks, quality gates, final output, reasoning effort, model policy, git identity | [AGENTS/references/70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md) |
| Source anchors, related skills, tooling anchors | [AGENTS/references/99-source-anchors.md](AGENTS/references/99-source-anchors.md) |
| Knowledge map for AGENTS doctrine | [AGENTS/references/00-knowledge-map.md](AGENTS/references/00-knowledge-map.md) |

## Core Operating Contract

These rules **must** be followed on every turn. They are short by design; the reference files carry the depth.

0. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed — the owning module, the framework, the real requirement. Never assume, never guess, never skip or shortcut a required research, test, review, sibling-scan, or official-contract check. No building against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode: it passes review and still gets thrown away. If the request is ambiguous in a way that changes what you build, ask before building, not after. This gates every rule below — there is no point routing a skill or refreshing memory for the wrong task. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) § 0 (ALIGN).
1. **Skills first.** Route domain work through the installed `~/.claude/skills/<name>/SKILL.md` files. Run `preserve-existing-flow` before editing any existing source file. Run `reviewer` before closing non-trivial work.
2. **Native commands before raw shell.** Prefer `keel anvil`, `keel run -- <command>`, `keel code-search search`, `keel code-search siblings` after a fix or implement, `keel flow ...`, and `keel review ...` when those surfaces own the job. See [10-native-command-routing.md](AGENTS/references/10-native-command-routing.md).
3. **Memory before recommendations.** Resolve scoped memory and read `SYSTEM_MAP.md` before broad analysis: `keel memory scope resolve --create-missing --refresh-system-map`. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) for the full memory protocol.
4. **Iterative loop.** ALIGN → RESEARCH → PLAN → IMPACT → IMPLEMENT → TEST → FIX → VERIFY → REVIEW → RECONCILE. For any code change, run `keel anvil` first (`compile` then `run --dry-run`; `running-anvil` skill). Do not hand-edit before Anvil. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md).
5. **Release ladder is fail-closed.** Smoke → Functional → Integration → UI → Load → Stress → Security. A required rung **must not** be skipped — mark not applicable only with explicit, evidence-backed reasoning. See [40-code-quality-and-testing.md](AGENTS/references/40-code-quality-and-testing.md).
6. **Branch model + commit format.** `main` (stable) ← `dev` (staging) ← `feat` (integration) ← `task/<task>` work branches. Parallel subtask branches use flat sibling names such as `task/<task>-<subtask>`; Git cannot store `task/<task>` and `task/<task>/<subtask>` together. Do **not** use `feat/<task>` while bare `feat` exists for the same reason. Fixes stay on the same work branch. Never delete a branch after push or merge. Commit subjects: `Add : FEATURE : short information` (Category capitalized; FEATURE uppercase; spaces around colons). Legacy `add/` / `feature/` branches may continue with a preflight warning. See [50-delivery-and-prohibited-shortcuts.md](AGENTS/references/50-delivery-and-prohibited-shortcuts.md), [WORKFLOW.md](WORKFLOW.md), and [70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md).
7. **Completion reconciliation.** Re-read the working brief and impacted surface before the final answer. Every explicit user requirement **must** map to evidence or a verified blocker. Do not present partial work as complete.
8. **Writing Discipline.** All written output (docs, code comments, commit/PR text, review notes, chat) **must** follow: write less, be accurate not impressive, lead with the point, no filler or AI tells, stay on the asked scope. Full rule in `_shared/common-discipline.md` § Writing Discipline.
9. **Agent teams.** Use `designing-agent-teams` when a task needs coordinated multi-agent decomposition. Subagents **must not** spawn subagents — route delegation back to the main thread. Teammates **must** communicate via `SendMessage(to: <agent-id>)`. Resumed subagents retain full history and auto-resume in background on `SendMessage`.

## Git Hooks (Mandatory Enforcement)

Git hooks are **mandatory** and must be installed before making any commits or pushes. They work with **any language** (auto-detect) and **any AI agent tool** (native git hooks).

### Installation

```bash
keel hook git-hooks install
```

### What the Hooks Enforce

| Hook | What It Checks | Consequence |
|------|----------------|-------------|
| **pre-commit** | Auto-detects project language (Rust/Go/Python/JS/C++) and runs format + lint | Commit is **blocked** if checks fail |
| **pre-push** | Branch policy (blocks direct pushes to `main` or `dev`) | Push is **blocked**
**Installation Note**: `keel hook git-hooks install` configures git hooks path but requires ` .githooks/` directory to exist in your repository. Copy from keel workspace or ensure the directory is present.

### What the Hooks Enforce

### Bypassing Hooks

**Do not bypass hooks** (`git commit --no-verify` or `git push --no-verify`) unless genuine emergency. Document the bypass in the commit message and follow up with a cleanup commit.


## Review Gate Enforcement (Optional Hard Blocking)

The reviewer skill provides advisory reminders but does not block turns by default. To enable hard enforcement:

Set environment variable `CLAUDE_SKILLS_REVIEW_GATE=block` → reviewer reminders become blocking gates

Default behavior (no env var set): Advisory-only reminders that can be ignored. This allows flexibility while still prompting quality practices.

See [70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md) for full details on review surfaces and gates.

## Summary

Keep execution simple and focused. Use specialist skills when they add clear value. Prioritize code quality, security, maintainability, and native harness CLI workflow surfaces. Open the matching reference file for depth.
