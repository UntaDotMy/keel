# Phase 11 Host Instructions Consolidation and No-Duplication Rule

## Delivered contract

Phase 11 consolidates host instruction files across all supported AI coding agents into a clean single-source architecture, eliminating duplicated facts and enforcing a strict instruction token budget:

1. **Pre-Consolidation Token Measurement**:
   - Initialized `TokenMeter` measurement across all 19 fixed-context surfaces using `keel stats --json`.
   - Baseline measurements:
     - `repo.CLAUDE.md`: 9,906 actual tokens (budget 10,897 tokens).
     - `repo.AGENTS.md`: 2,348 actual tokens (budget 2,583 tokens).
     - `repo.WORKFLOW.md`: 4,884 actual tokens (budget 5,373 tokens).

2. **Canonical Cross-Vendor Instruction Surface (`AGENTS.md`)**:
   - Pinned `AGENTS.md` as the canonical cross-vendor instruction file for all hosts (Claude Code, Pi, Codex, Cursor, OpenCode, Antigravity, ZCode).
   - Added explicit Instruction Budget and Scope Policy section capping canonical instructions at <= 2,600 tokens (<= 11 KB).
   - Established the no-duplication rule: no fact exists in two host instruction files without explicit justification.

3. **Thin Host Shim (`CLAUDE.md`)**:
   - Converted `CLAUDE.md` from a 40KB omnibus document into a thin host adapter shim.
   - Retained only Claude Code specific wiring:
     - Cross-vendor inclusion via `@AGENTS.md` import.
     - Native skill matching triggers and `SessionStart` bootstrap.
     - Subagent delegation through `.claude/agents/<name>.md`.
     - Output templates and experimental multi-agent team communication (`SendMessage`).
     - Hook installation and transparent output compaction rewrites.
     - PostToolBatch enforcement gate controls (`CLAUDE_SKILLS_*_GATE`).
     - Required CLI command references (`keel anvil`, `keel observe`, `keel eval`, `keel design-intelligence`, `keel skill-eval`, `keel telemetry`, `keel session`, `keel learn`).
     - MCP tool count contract asserted by `tests/doc_parity_test.rs` over `mcp/tools.rs`.
     - Bridge event dispatch enumeration (`session-start`, `user-prompt`, `observe`, `session-end`, `pre-compact`, `post-compact`, `gate-status`, `pre-tool-use`, `rewrite`).
   - Token weight reduced from 9,906 tokens to 1,245 tokens (87.4% reduction; 8,661 tokens saved per session).

4. **Canonical Technical Schemas Relocation (`docs/host-schemas.md`)**:
   - Relocated detailed technical schemas out of host instructions to on-demand documentation:
     - SKILL.md YAML frontmatter fields and variable substitutions.
     - Subagent markdown frontmatter fields.
     - Managed profile schema (`<name>/agents/claude.yaml`).
     - Declarative filter registry configuration (`.keel/filters.toml`).
     - Plugin manifest schema (`.claude-plugin/plugin.json`).
   - Zero information loss while removing thousands of tokens from fixed agent context.

5. **Pi Host Bridge Alignment (`pi/AGENTS.md`)**:
   - Documented explicit justification header: Pi coding agent discovers `~/.pi/agent/AGENTS.md` globally without supporting `@AGENTS.md` includes.
   - De-duplicated static skill catalog in favor of dynamic MCP routing (`skill_route`, `skill_get`, `skill_list`).
   - Updated uninstall detection in `manager/install/commands.rs` to detect managed markers reliably.

6. **Fixed-Context Ledger and Budget Updates**:
   - Updated ratified budgets in `rust/crates/keel/src/utility/fixed_context.rs`:
     - `repo.CLAUDE.md`: 1,370 tokens (ceil(1,245 * 1.10)).
     - `repo.AGENTS.md`: 2,709 tokens (ceil(2,462 * 1.10)).
   - Updated documentation table in `docs/fixed-context-ledger.md`.
   - Verified that `fixed_context_budget_test` passes cleanly.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Measure host instruction files in token ledger | `keel stats --json --workspace-root .` measured before and after consolidation. |
| Make AGENTS.md canonical cross-vendor instruction file | `AGENTS.md` defines core operating contract, git discipline, and reference map. |
| CLAUDE.md thin shim with @AGENTS.md import | `CLAUDE.md` reduced to 1,245 tokens with `@AGENTS.md` import and host-specific wiring. |
| Move detailed procedures to on-demand references | Schemas moved to `docs/host-schemas.md`; routing and gates point to `AGENTS/references/`. |
| Cross-host practical cap and budget in AGENTS.md | `AGENTS.md` documents <= 2,600 token cap (actual: 2,462 tokens). |
| Update doc parity tests | `cargo test -p keel --test doc_parity_test` (27 passed, 0 failed). |
| 11 host adapter contracts green | `bun test tests/host-adapter-contracts.test.ts` (12 passed, 0 failed). |
| No duplication without explicit reason | Justifications documented; static skill duplication removed from host files. |

## Token Ledger Comparison

| Surface | Pre-Consolidation Actual | Pre-Consolidation Budget | Post-Consolidation Actual | Post-Consolidation Budget | Delta |
| --- | ---:| ---:| ---:| ---:| ---:|
| `repo.CLAUDE.md` | 9,906 | 10,897 | 1,245 | 1,370 | -8,661 (-87.4%) |
| `repo.AGENTS.md` | 2,348 | 2,583 | 2,462 | 2,709 | +114 (+4.8%) |
| `repo.WORKFLOW.md` | 4,884 | 5,373 | 4,884 | 5,373 | 0 (0.0%) |

## Validation Summary

| Test Surface | Result | Detail |
| --- | --- | --- |
| Host adapter contracts | 12 passed, 0 failed | `bun test tests/host-adapter-contracts.test.ts` |
| Doc parity test suite | 27 passed, 0 failed | `cargo test -p keel --test doc_parity_test` |
| Fixed context budget suite | 5 passed, 0 failed | `cargo test -p keel --test fixed_context_budget_test` |
| Platform detection suite | 38 passed, 0 failed | `cargo test -p keel --test platform_detect` |
| Full workspace test suite | 1,310+ passed, 0 failed | `cargo test --workspace` |
| Rust format | pass | `cargo fmt --all -- --check` |
| Rust clippy | pass (0 warnings) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Pre-commit review | pass (0 blocking) | `keel review pre-commit` |
| Pre-PR review | pass (0 blocking) | `keel review pre-pr --base-ref task/integrated-agent-quality-phase-10` |

## Release Ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | Host instruction files load cleanly; token meter computes in <200ms. |
| Functional | pass | Claude Code, Pi, Codex, Cursor, and OpenCode adapters parse instructions correctly. |
| Integration | pass | Doc parity, platform detection, and host adapter tests pass 100%. |
| UI | not applicable | Instruction files and CLI tooling without visual UI components. |
| Load | pass | Token footprint per session reduced by 8,661 tokens for Claude Code. |
| Stress | pass | Fixed-context budget gate verifies all 19 surfaces within verified thresholds. |
| Security | pass | Host schemas and config audit continue to forbid secret literals and arbitrary command execution. |
