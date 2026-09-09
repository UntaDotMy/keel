# CLAUDE.md: Claude Code Host Configuration

## Host Adapter Role

This file is the thin host adapter shim for Claude Code. Canonical cross-vendor agent operating doctrine, git discipline, and core contracts live in [`AGENTS.md`](AGENTS.md) and [`AGENTS/references/`](AGENTS/references/). Technical schemas for skills, subagents, profiles, and hooks live in [`docs/host-schemas.md`](docs/host-schemas.md).

@AGENTS.md

## Claude Code Host Wiring

1. **Skill routing:** Driven by the native skill matcher against installed `~/.claude/skills/<name>/SKILL.md` frontmatter (`description`, `when_to_use`). The bootstrap skill `using-keel/SKILL.md` injects at `SessionStart`.
2. **Subagent delegation:** Delegate heavy, isolated tasks to `.claude/agents/<name>.md` subagents to conserve controller tokens. Subagents cannot spawn subagents.
3. **Templates:** Use `templates/` for commit messages, pull request summaries, reviews, and responses.
4. **Agent teams:** Experimental multi-agent teams require `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`. Teammates communicate via `SendMessage(to: <agent-id>)`. Resumed subagents retain history and auto-resume on incoming messages.
5. **Hooks:** Wired into `settings.json` via `keel hook install`. Transparently rewrites commands through `keel run --` for token compaction.

## Enforcement Gates (PostToolBatch)

Enforcement gates act as a model-independent backstop for the operating contract:
- **Brief gate** (`CLAUDE_SKILLS_BRIEF_GATE`): Requires a working brief before code edits. Cleared by `keel memory working-brief write` or `brief_create` MCP tool.
- **Review gate** (`CLAUDE_SKILLS_REVIEW_GATE`): Requires a passing review after edits. Cleared by `keel review pre-pr` or `keel review pre-commit`.
- **Memory gate** (`CLAUDE_SKILLS_MEMORY_GATE`): Nudges memory capture when code changes.
- **Learned-skill gate** (`CLAUDE_SKILLS_LEARNED_SKILL_GATE`): Alerts on promoted skills requiring synthesis.
- **Research gate** (`CLAUDE_SKILLS_RESEARCH_GATE`): Requires fresh web-search or recall evidence before edits.
- **Completeness gate** (`CLAUDE_SKILLS_COMPLETENESS_GATE`): Requires sibling scan via `keel code-search siblings`.

## Commands Quick Reference

- `keel anvil compile|cast|sieve|stamp|loop|run`: Canonical delivery loop.
- `keel review pre-commit`: Pre-commit review gate.
- `keel review pre-pr`: Pre-PR review gate.
- `keel run -- <command>`: Command execution with proxy output compaction.
- `keel memory scope resolve --create-missing --refresh-system-map`: Memory scope resolution.
- `keel eval`: Token compaction evaluation across test fixtures.
- `keel observe`: Read-only observation aggregator over recall health and briefs.
- `keel skill-eval`: Behavioral gate testing skill matcher trigger rules.
- `keel design-intelligence recommend`: Component-aware UI design recommendations.
- `keel telemetry summary`: Read-only tool timing report.
- `keel session`: Per-session token compaction savings report.
- `keel learn status|dry-run|run|synthesize`: Autonomous learning cycle.
- `keel hook install`: Install hooks into host settings.
- `keel doctor`: Diagnose MCP and tool registration health.
- `keel repair`: Repair MCP server registration.

## Host Bridge and MCP Contracts

- **MCP tool count:** Derived from tool definitions in `mcp/tools.rs`. The MCP tool count is asserted by `tests/doc_parity_test.rs` over `mcp/tools.rs` rather than hardcoding a number in documentation.
- **Bridge subcommands:** For non-Claude hosts (OpenCode, Codex, Pi, Cursor), `keel bridge <event>` dispatches: `session-start`, `user-prompt`, `observe`, `session-end`, `pre-compact`, `post-compact`, `gate-status`, `pre-tool-use`, `rewrite`.

## Canonical Reference Map

- [AGENTS.md](AGENTS.md): Canonical agent operating doctrine and core operating contract
- [AGENTS/references/10-native-command-routing.md](AGENTS/references/10-native-command-routing.md): Native command routing, hook rewrite, declarative filter registry
- [AGENTS/references/20-skill-routing.md](AGENTS/references/20-skill-routing.md): Specialist skill roster, skill-focused execution, agent profiles
- [AGENTS/references/30-execution-strategy.md](AGENTS/references/30-execution-strategy.md): Iterative loop (0-9), memory protocol, Anvil
- [AGENTS/references/40-code-quality-and-testing.md](AGENTS/references/40-code-quality-and-testing.md): Quality standards, fail-closed release ladder, build and test
- [AGENTS/references/50-delivery-and-prohibited-shortcuts.md](AGENTS/references/50-delivery-and-prohibited-shortcuts.md): Branch model, prohibited shortcuts
- [AGENTS/references/60-environment-and-portability.md](AGENTS/references/60-environment-and-portability.md): Windows environment, cross-platform portability
- [AGENTS/references/70-review-quality-gates-and-policies.md](AGENTS/references/70-review-quality-gates-and-policies.md): Review gates, automated quality checks
- [docs/host-schemas.md](docs/host-schemas.md): Full technical schemas for skills, subagents, hooks, manifests, profiles, and filters
- [docs/fixed-context-ledger.md](docs/fixed-context-ledger.md): Runtime fixed-context token ledger and surface budgets
