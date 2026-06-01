<!--
Purpose: Thin entry point and index for the managed Claude Code routing, memory, validation, and delivery doctrine.
Caller: Claude Code agents using the synced claude_skills native guidance surface.
Dependencies: AGENTS/references/*.md, the 24 specialist SKILL.md files, claude-skills CLI surface.
Main Functions: Route to the correct reference file in AGENTS/references/ for the section a reader needs.
Side Effects: None — this file is informational.
-->
# Skill Routing and Native Skill Guidance

## Purpose

This file is the entry point for Claude Code CLI on skill routing, native command usage, memory, validation, and delivery discipline. The detailed doctrine lives under [`AGENTS/references/`](AGENTS/references/) so the entry point stays small and the rules stay searchable.

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

These rules apply to every turn. They are short by design; the reference files carry the depth.

0. **Understand before building.** Before writing any code, restate what the request actually asks, confirm the user story, and research what is genuinely needed — the owning module, the framework, the real requirement. No guessing, no assuming, no building against an imagined spec. Correct code that solved the wrong problem is the most expensive failure mode: it passes review and still gets thrown away. If the request is ambiguous in a way that changes what you build, ask before building, not after. This gates every rule below — there is no point routing a skill or refreshing memory for the wrong task. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) § 0 (ALIGN).
1. **Skills first.** Route domain work through the installed `~/.claude/skills/<name>/SKILL.md` files. Run `preserve-existing-flow` before editing any existing source file. Run `reviewer` before closing non-trivial work.
2. **Native commands before raw shell.** Prefer `claude-skills run -- <command>`, `claude-skills code-search search`, `claude-skills flow ...`, and `claude-skills review ...` when those surfaces own the job. See [10-native-command-routing.md](AGENTS/references/10-native-command-routing.md).
3. **Memory before recommendations.** Resolve scoped memory and read `SYSTEM_MAP.md` before broad analysis: `claude-skills memory scope resolve --create-missing --refresh-system-map`. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md) for the full memory protocol.
4. **Iterative loop.** ALIGN → RESEARCH → PLAN → IMPLEMENT → TEST → FIX → VERIFY → REVIEW → RECONCILE. See [30-execution-strategy.md](AGENTS/references/30-execution-strategy.md).
5. **Release ladder is fail-closed.** Smoke → Functional → Integration → UI → Load → Stress → Security. A required rung may be marked not applicable only with explicit, evidence-backed reasoning. See [40-code-quality-and-testing.md](AGENTS/references/40-code-quality-and-testing.md).
6. **One feature per branch.** Use professional commit and PR templates. Run native review gates before closing. See [50-delivery-and-prohibited-shortcuts.md](AGENTS/references/50-delivery-and-prohibited-shortcuts.md) and [70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md).
7. **Completion reconciliation.** Re-read the working brief and impacted surface before the final answer. Every explicit user requirement must map to evidence or a verified blocker. Do not present partial work as complete.

## Summary

Keep execution simple and focused. Use specialist skills when they add clear value. Prioritize code quality, security, maintainability, and native Claude Code CLI workflow surfaces. Open the matching reference file for depth.
