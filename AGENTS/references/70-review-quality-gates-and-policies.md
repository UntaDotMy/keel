<!--
Purpose: Capture code review requirements, automated quality checks, quality gates, final output, reasoning effort, skill model policy, and git identity policy previously inline in AGENTS.md.
Caller: AGENTS.md before closeout, before final answer, when triggering review, or when wiring agent-profile reasoning effort.
Dependencies: reviewer skill, .claude/review.json, claude-skills review, agent-profile TOMLs.
Main Functions: Define when review is mandatory, the automated quality bar, closeout output, reasoning policy, model pinning policy, and commit identity rules.
Side Effects: None — this file is informational.
-->
# Code Review, Quality Gates, Final Output, and Policies

## Code Review Requirements

**Mandatory code review** (use reviewer skill) when:
- Changes touch more than 2 files
- Changes exceed 50 lines
- Authentication, authorization, or data handling changes
- External API integration
- Security-sensitive code
- Performance-critical code

**Security review required** for:
- User input handling
- Database queries
- File system operations
- Network requests
- Authentication/authorization logic
- Cryptography or secrets handling

## Automated Quality Checks

Before marking any task complete, verify:

### Linting & Type Checking
- All linting errors resolved (not disabled with `// eslint-disable`)
- All TypeScript errors resolved (not suppressed with `@ts-ignore`)
- Code follows project style guide

### Testing
- All tests passing (not skipped with `.skip()`)
- The mandatory ladder is satisfied in order for every applicable rung: Smoke -> Functional -> Integration -> UI -> Load -> Stress -> Security
- New features have unit tests
- Bug fixes have regression tests
- Test coverage maintained or improved

### Security
- No hardcoded secrets or credentials
- Input validation at all boundaries
- Security scan passes (no high/critical vulnerabilities)
- Dependencies up to date (no known CVEs)

### Code Quality
- No duplicate code (DRY violations)
- No commented-out code
- No debug statements (console.log, debugger)
- No TODO/FIXME without issue tracking

### Performance
- Performance impact measured (if applicable)
- No obvious performance regressions
- Images optimized
- Bundle size within budget

**Tools to use:**
- ESLint, Prettier for linting/formatting
- TypeScript for type checking
- npm audit, yarn audit for security
- Jest, Vitest, Playwright for testing
- Lighthouse, WebPageTest for performance

## Quality Gates

Before completing any task, verify ALL of these:
1. Requirements met completely
2. Code is clean and maintainable
3. All linting/type errors resolved (not disabled)
4. The mandatory release test ladder passed in order for every applicable rung: Smoke -> Functional -> Integration -> UI -> Load -> Stress -> Security
5. No security issues or vulnerabilities
6. No secrets or credentials committed
7. No duplicate code
8. Changes are minimal and focused
9. Documentation updated (if needed)
10. Code review completed (if required)

## Final Output

For non-trivial tasks, append a compact **Learning Snapshot** when memory artifacts are available:
1. What Claude Code learned today
2. Mistakes encountered and whether they were resolved
3. Tool-use mistakes that taught a reusable lesson
4. Heuristic memory-health stats such as growth or momentum

Treat this snapshot like a human progress check-in grounded in saved artifacts, not a claim of literal cognition.

Before the final answer, perform a completion reconciliation pass. Do not describe work as finished until every explicit user requirement has been checked against current evidence. A progress, recap, audit, or "what is done or not done" request does not suspend that completion loop when fixable in-scope work remains, and do not default to optional follow-up offers when the user asked for completion.

## Reasoning Effort Levels

Keep reasoning settings explicit while leaving model choice to the workspace default:

- **repo-managed specialist baseline**: Use `reasoning_effort: "high"` for synced skill profiles that perform review, planning, verification, security-sensitive work, architecture decisions, or release gates.
- **local narrow override**: A user-local override may lower reasoning for bounded status or inventory reporting, such as `memory-status-reporter`, when the task is intentionally cheaper and lower risk.

Do not pin a model to achieve these settings. Preserve reasoning effort in repo-managed profiles and let the active Claude Code workspace choose the model.

## Skill Model Policy

- Do not pin a specific model inside root Claude Code `agents/claude.yaml` files or generated agent-profile TOML. Let the workspace default model handle that choice.
- Keep root Claude Code skill `reasoning_effort` at the repo-managed specialist baseline (`high`) for deeper review and verification passes.
- Sync the 18 skill-owned agent profiles into `~/.claude/agent-profiles/*.toml` with their skill instructions attached, `model_reasoning_effort = "high"`, and no `model = ...` entry.
- A local `memory-status-reporter` override from `~/.claude/.claude-skill-manager/local-home-agent-overrides.json` may narrow only that profile to `low` reasoning unless the user explicitly changes local policy.
- When any Claude Code skill executes tools in this runtime, let Claude Code choose the best supported tool
  surface for the task.
- Use `js_repl` with `claude.tool(...)` when it is the clearest fit or when the runtime explicitly
  requires it, but do not hard-require `js_repl` for every tool call.

## Git Identity Policy

- When creating a Git commit, use the repository or global Git `user.name` and `user.email` as the commit author identity.
- Do not replace the configured Git author with an assistant or tool-branded author name.
- Treat any runtime-managed commit trailer as separate from Git author identity; the author fields should still stay on the user's configured Git identity.
