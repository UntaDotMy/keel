## Summary

Closes the final HIGH-priority gap (H2) from the comprehensive repo gap analysis by adding dedicated MCP tools for sprint management and user-story validation.

## Changes

### New MCP Tools (H2 - the last remaining gap)
- **sprint** — Manage Scrum-style sprint loops (plan, status, advance, review, list)
- **user_story_lint** — Validate user stories against strict Agile/Jira format (Connextra + Gherkin + INVEST)

### Documentation and Wiring Fixes
- `commands/sprint.md` — Slash command for sprint operations
- `commands/user-story.md` — Slash command for user-story linting
- `requesting-code-review/SKILL.md` — Alias skill pointing to reviewer (M5)
- `using-claude-core/SKILL.md` — Updated slash commands list and skill count (43)
- `CLAUDE.md` — Fixed routing rules numbering, added new commands
- `WORKFLOW.md` — Added closeout gate reference (M6)
- `AGENTS.md` — Fixed skill counts (18 technique, 43 total, 42 matcher-invokable)
- `00-skill-routing-and-escalation.md` — Removed duplicate rule 9, strengthened mandatory language
- `.claude-plugin/plugin.json` — Registered requesting-code-review alias
- `.gitignore` — Added karpathy-skills-cmp/ (L2)
- `.claude/settings.local.json` — Removed duplicate outputStyle (L1)
- `docs/competitive-gap-closure.md` — Added B1-B4 decided non-goals (L3)

### Mandatory Language Strengthening
- Converted all permissive language ("should", "prefer") to mandatory ("must", "is required") across AGENTS.md, 00-skill-routing, CLAUDE.md, and command files

### Test Updates
- Updated MCP tool count from 14 to 16 in both test assertions (tools.rs and mod.rs)
- All 552 tests passing

## Gap Analysis Coverage

All 12 identified gaps are now closed:
- H1 (slash commands) - closed
- H2 (MCP tools) - closed by this PR
- M1-M6 (docs/wiring) - closed
- L1-L4 (config/audit) - closed
