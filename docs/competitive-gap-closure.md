<!--
Purpose: Track the competitive-gap fixes for claude-core — what shipped in the gap-closing pass and what remains, named against the real comparators and aligned to official Claude Code docs.
Caller: Contributors closing the gap between claude-core and peer Claude Code tooling (RTK, caveman, superpowers, ECC) and the native baseline.
Dependencies: Rust runtime (utility/memory.rs, manager/install.rs, proxy adapters), plugin manifest, command files, statusline scripts.
Main Functions: Record shipped fixes, the toolchain constraint, and the prioritized remaining work with concrete file targets.
Side Effects: None — documentation only.
-->
# Competitive Gap Closure

This page is the named, source-backed companion to the anonymized
[`native-gap-map.md`](./native-gap-map.md). It records the competitive audit
against real peers, what the gap-closing pass shipped, and what remains. Aligned
to the official Claude Code docs at code.claude.com as of the audit date.

## Comparators (verified identities)

| Project | Identity | License | Overlap with claude-core |
| --- | --- | --- | --- |
| Official Claude Code | The host platform (code.claude.com/docs) | Anthropic | The baseline claude-core extends; ~30 hook events, skills (commands merged in), built-in subagents, checkpointing/rewind, MCP Tool Search, Agent SDK. |
| RTK ("Rust Token Killer", `rtk-ai/rtk`) | Single-binary Rust command-output compaction proxy | Apache-2.0 | Near-identical to claude-core's compaction proxy; 100+ purpose-built command filters across 42 ecosystem modules, `gain`/`discover`/`session`, tee recovery. |
| caveman (`JuliusBrussee/caveman`) | Skill that compresses the model's own replies (terse "caveman speak") | MIT | Token economy on the **output** side (claude-core only compacts command **output**); ships slash commands, statusline, MCP middleware. |
| superpowers (`obra/superpowers`) | Opinionated TDD methodology as auto-triggering skills | MIT | Skills + workflow doctrine; `writing-skills` meta-skill with a subagent eval harness; two-stage review loop; visual brainstorming. |
| ECC ("Everything Claude Code", `affaan-m/ECC`) | Multi-harness operator framework | MIT | Whole operator posture at larger scale; **Instincts** (confidence-scored learned behaviors that evolve into skills), **AgentShield** (adversarial config security audit), advisor CLI, cross-harness adapters. |

Note: published star counts for these repos (caveman/superpowers/ECC/RTK) were
flagged as implausible/unverifiable during research and are deliberately not used
as a comparison signal here. The comparison is capability-based.

## Shipped in the gap-closing pass

All changes below are markdown/JSON/shell only (no Rust recompile required) and
were validated where executable.

1. **Doc/impl drift fixed (highest-leverage).** The operating doctrine commanded
   several CLI calls that the Rust runtime returns "not implemented" for
   (`orchestration task|checkpoint`, `memory research-cache|maintenance|loop-guard|agent-packets`).
   Marked them planned and routed the intent through implemented surfaces
   (`working-brief`, `completion-gate`, `recall`, plus L1 files). Files:
   `AGENTS/references/30-execution-strategy.md`, `_shared/common-discipline.md`,
   `docs/runtime-guardrails-and-memory-protocols.md`,
   `docs/context-efficiency-playbook.md`.
2. **Custom slash commands (discoverability gap vs. the whole field).** Added
   `/claude-core:workflow`, `/claude-core:review`, `/claude-core:recall`,
   `/claude-core:gain` at the plugin root `commands/`, registered via the
   manifest `commands` key. Each wraps only implemented CLI surfaces with the
   verified flag names. Frontmatter validated.
3. **Statusline savings badge (caveman/RTK-style ROI surface).**
   `statusline/statusline-claude-core.sh` and `.ps1` render model + context + a
   `gain`-sourced `saved N tok` badge, pinned to the real `tokensSaved` field,
   degrading gracefully (exit 0, badge omitted when unavailable). Opt-in via
   `settings.json`. Test matrix passed on both shells.
4. **Doc accuracy aligned to official docs.** Corrected the CLAUDE.md hook-count
   claim (29 in `HOOK_EVENTS`, 28 install, `FileChanged` opt-out; `MessageDisplay`
   documented upstream but not yet in the table) and the stale `::EVENTS`
   cross-reference (it is `HOOK_EVENTS`).

## Toolchain (resolved)

The audit machine initially could not link a Rust binary (default rustup toolchain
was `windows-gnu` with no `gcc`, and a 32-bit MinGW once installed could not build
the 64-bit SQLite dependency). This was resolved by installing 64-bit MinGW-w64
(`x86_64-w64-mingw32`, gcc 15.2.0) at `C:\ProgramData\mingw64\mingw64\bin`. Builds
and tests run with that directory on `PATH` and
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` pointed at its `gcc.exe`. All Rust work
below was implemented and verified with `cargo build` + `cargo test --workspace`
(clean under `-D warnings`).

## Shipped Rust work (verified)

1. **Installer `sync_commands` arm.** `manager/install.rs` now mirrors
   `<repo>/commands/*.md` to `<claude_home>/commands/` (a new `commands_directory`
   helper in `runtime.rs`), tracked via `managed-files.txt` so the per-file orphan
   sweep and uninstall reach them. The native `claude-skills install` ships the
   slash commands, not just the plugin path. Covered by
   `install_copies_slash_commands_into_user_global_commands_directory`.
   A lifecycle audit of install/update/sync/remove-stale/remove-old/uninstall
   confirmed commands flow correctly through all stages and surfaced one gap:
   `verify` did not re-check the installed `commands/*.md` (nor the subagent
   `.claude/agents/*.md` definitions). Added `verify_installed_markdown_dir` so
   `claude-skills verify` now byte-compares both against source — a drifted
   command/subagent fails verify (proven end-to-end) instead of slipping through.
2. **`MessageDisplay` hook row.** Added to `HOOK_EVENTS` in `hooks/claude.rs` with
   `installs_in_settings: false` (no matcher, fires on every assistant message,
   emits `displayContent` not `additionalContext` — auto-install would be a no-op
   or silently rewrite on-screen text). The opt-out invariant test was renamed to
   `only_known_events_opt_out_of_install` and pins both `FileChanged` and
   `MessageDisplay`.
3. **Planned memory/orchestration commands implemented.** On a new shared
   `RecordStore` primitive (`utility/record_store.rs`) and a new
   `utility/memory_families.rs` module:
   - `orchestration task begin|progress|complete|list` and `orchestration checkpoint`
     (JSONL task ledger + snapshot), in `utility/memory.rs`.
   - `memory|memoriesv2 research-cache|maintenance|agent-registry|agent-packets|loop-guard|entity|graph|retrieve|status`.
   The two command groups are isolated on disk (`<group>/<family>/`). `memory
   report` now aliases the family status summary, `memory index` rebuilds the
   FTS5 recall index, and `memory hook` redirects to the real `claude-skills
   hook` lifecycle surface (it was never a memory concept).
4. **Learning loop (ECC Instincts-style).** `memory|memoriesv2 instincts
   record|reinforce|penalize|list|promote` in `memory_families.rs`:
   confidence-scored patterns keyed by trigger that reinforce/penalize over time
   and promote (optionally writing a markdown digest) once they meet a confidence
   threshold. This closes the biggest *conceptual* gap — durable memory that now
   learns and promotes validated patterns into reusable guidance.
5. **`gain discover` (RTK missed-savings finder).** `utility/gain.rs` reads the
   same event log and reports passthrough (non-compacted) commands grouped by name
   with estimated uncompacted tokens. Parsing split into a pure
   `parse_missed_opportunities` for direct unit tests.
6. **Dedicated cloud adapter (RTK parity).** New `adapters/cloud.rs` + a
   `CommandKind::Cloud` variant: `aws`/`az`/`gcloud` now route to a purpose-built
   adapter that reduces large JSON to structure-only, redacts cloud secrets
   (access keys, session tokens, passwords, private keys) by key name, leads with
   error signals on failure, and passes small lookups through verbatim. Container/k8s
   keep their existing `containers` adapter; `terraform` deliberately stays on the
   logs adapter. Registry routing is regression-tested.
7. **Skill eval harness (superpowers-style).** New `utility/skill_lint.rs` +
   `claude-skills skill-lint` command: validates that every `<name>/SKILL.md` has
   the structural properties the matcher needs to *trigger* (non-empty
   `description`, `name` matching the directory, combined description+when_to_use
   within the 1536-char budget, scoped `allowed-tools` warning, no dangling
   `references/*.md` links). All 20 shipped skills pass; the gate fails closed when
   a skill would silently fail to trigger.
8. **`memory report|index|hook` resolved.** `report` aliases the family status
   summary, `index` rebuilds the FTS5 recall index (one index, not two), and `hook`
   redirects to the real `claude-skills hook` lifecycle surface instead of being a
   dead stub.
9. **Database compaction adapter (RTK parity).** New `adapters/database.rs` + a
   `CommandKind::Database` variant: `psql`/`mysql`/`mariadb`/`sqlite3`/`redis-cli`/
   `mongosh` route to a result-set-aware adapter (header + sampled rows + omitted
   count for large tables, structure-only JSON for mongosh, query-error-first on
   failure, connection-string/password redaction). Bulk-export tools
   (`pg_dump`/`mysqldump`) stay on the logs adapter. Registry routing is tested.
10. **AgentShield-style config security audit.** New `utility/config_audit.rs` +
    `claude-skills config-audit` command: audits claude-core's OWN config surface
    (hooks, settings/permissions, plugin manifest) for shell-metacharacter
    injection, network-fetching hooks, `bypassPermissions`, unscoped `Bash` allow
    rules, and committed secret literals in MCP env. Fails closed (exit 2) on any
    high finding. Distinct from the security-and-compliance-auditor skill, which
    audits the user's application code. Runs clean against the repo's own config.
11. **Output-economy skill (caveman axis).** New `output-economy/SKILL.md`:
    reduces the model's own reply verbosity (no preamble, no re-narration of tool
    output, length tracks the task) without dropping technical signal — the
    output-side counterpart to `compression-discipline`'s input-side rules. The
    one axis we previously didn't address at all. Passes `skill-lint`.
12. **Two-stage review gate (superpowers-style).** `reviewer/SKILL.md` now opens
    with an explicit ordered gate: Stage 1 spec-compliance (does it do what was
    asked) must be clean before Stage 2 code-quality runs, with mandatory
    re-review of the producing stage after any fix. Replaces the single
    undifferentiated pass so a polished implementation of the wrong spec cannot
    pass on code quality alone.
13. **Git-backed code checkpoints (`/rewind` analog).** New `utility/checkpoint.rs`
    + `claude-skills checkpoint create|list|show|restore`: snapshots tracked
    working-tree changes via `git stash create` pinned under
    `refs/claude-checkpoints/<id>`, lists/shows them, and restores one. Restore is
    the only destructive verb — gated behind `--confirm` and an automatic
    pre-restore safety snapshot so the restore itself is reversible. An external
    binary cannot hook Claude's edit tool the way native `/rewind` does, but git
    is the real code-undo; this exposes it as a first-class checkpoint surface.

Prior non-Rust work (doc/impl drift fixes, the four slash commands, the
cross-platform statusline savings badge, and hook-count doc accuracy) shipped
earlier in the same pass and remains in place; the doctrine docs were then
reconciled to describe these commands as implemented rather than planned.

## Remaining work (deliberately out of scope)

- **Cross-harness adapters.** Every comparator ships Codex/Cursor/Gemini/Copilot
  adapters; claude-core stays Claude-Code-native by design (see the strategic note
  below). Not a defect — a product stance.
- **Mobile / niche command adapters.** Mobile toolchains (xcodebuild beyond the
  generic build path, gradle device flows) and other niche CLIs still fall through
  to the generic adapter. Reasonable future additions, not audited gaps. The
  previously-listed database adapter now ships (`adapters/database.rs`).

## Native-parity note (`/rewind`)

Native Claude Code `/rewind` auto-captures the edit tool's changes and can restore
code *and* conversation. `claude-skills checkpoint` is the code half: a git-backed
working-tree snapshot/restore that an external binary can actually own. It does
not capture conversation state (only Claude Code itself can), so the two are
complementary rather than identical — use `/rewind` for conversation+code inside a
session, `checkpoint` for durable, named, git-pinned code snapshots that survive
across sessions and tools.

## Strategic open question

Every comparator (RTK, caveman, superpowers, ECC) ships cross-harness adapters
(Codex/Cursor/Gemini/Copilot). claude-core is Claude-Code-only. Whether to go
multi-harness or keep a deliberately Claude-native stance is a product decision,
not a defect — recorded here so it is chosen, not drifted into.

## What claude-core keeps (the moat)

Fail-closed closeout discipline (reviewer gate, completion-gate ledger, release
ladder), the preserve-existing-flow brownfield gate (no comparator has this), and
breadth-of-integration (compaction + methodology + review gates + memory in one
hook-wired binary) remain genuine differentiators even where individual pieces
are shallower than a single-purpose peer.
