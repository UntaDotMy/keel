<!--
Purpose: Track the competitive-gap fixes for keel ,  what shipped in the gap-closing pass and what remains, named against the real comparators and aligned to official harness docs.
Caller: Contributors closing the gap between keel and peer the harness tooling (RTK, caveman, superpowers, ECC) and the native baseline.
Dependencies: Rust runtime (utility/memory.rs, manager/install.rs, proxy adapters), plugin manifest, command files, statusline scripts.
Main Functions: Record shipped fixes, the toolchain constraint, and the prioritized remaining work with concrete file targets.
Side Effects: None ,  documentation only.
-->
# Competitive Gap Closure

This page is the named, source-backed companion to the anonymized
[`native-gap-map.md`](./native-gap-map.md). It records the competitive audit
against real peers, what the gap-closing pass shipped, and what remains. Aligned
to the official harness docs at code.claude.com as of the audit date.

## Comparators (verified identities)

| Project | Identity | License | Overlap with keel |
| --- | --- | --- | --- |
| Official harness | The host platform (code.claude.com/docs) | Anthropic | The baseline keel extends; ~30 hook events, skills (commands merged in), built-in subagents, checkpointing/rewind, MCP Tool Search, Agent SDK. |
| RTK ("Rust Token Killer", `rtk-ai/rtk`) | Single-binary Rust command-output compaction proxy | Apache-2.0 | Near-identical to keel's compaction proxy; "100+ supported commands" (mostly subcommand breadth within categories we also cover ,  8 git subcmds, 8 aws subcmds, etc.), `gain`/`discover`/`session`, tee recovery, TOML filter DSL (8-stage declarative user-extensible pipeline ,  keel lacks this), 14 agent integrations. **Web re-verified 2026-07:** since **RTK v0.37.2** native Windows has auto-rewrite via the native binary hook (`rtk hook claude` / `rtk init -g`) on Command Prompt, PowerShell, and Windows Terminal (no bash/jq required). Older "CLAUDE.md injection only on Windows" wording is stale. **Still true:** never intercepts Read/Grep/Glob (Bash/shell-tool-only hook). *Keel differentiator vs RTK remains multi-platform rewrite plus iron-law, multi-lang review, memory, and specialists, not a false "Windows unique" claim.* |
| caveman (`JuliusBrussee/caveman`) | Skill that compresses the model's own replies (terse "caveman speak") | MIT | Token economy on the **output** side (keel only compacts command **output**); ships slash commands, statusline, MCP middleware. |
| superpowers (`obra/superpowers`) | Opinionated TDD methodology as auto-triggering skills | MIT | Skills + workflow doctrine; `writing-skills` meta-skill with a subagent eval harness; two-stage review loop (walked back to inline self-review checklists in v5.0.6 for speed); visual brainstorming. v5.1.0 (May 2026) removed its legacy slash commands and named code-reviewer agent. Cross-harness (this host/Codex/Cursor/Gemini/Copilot) ,  the one axis it still leads. After the methodology-completion pass, keel ships named first-class equivalents for **all 14** of its methodology skills (see scorecard). |
| ECC ("Everything the harness", `affaan-m/ECC`) | Multi-harness operator framework | MIT | Whole operator posture at larger scale; **Instincts** (confidence-scored learned behaviors that evolve into skills), **AgentShield** (adversarial config security audit), advisor CLI, cross-harness adapters. |
| UI/UX Pro Max (`nextlevelbuilder/ui-ux-pro-max-skill`) | Design-intelligence skill: a knowledge corpus + Python BM25 generator that turns a UI request into a design-system packet (style, palette, typography, anti-patterns, checklist) | MIT | Single-domain overlap with keel's **`design-intelligence` generator** + the `ui-design-systems-and-responsive-interfaces` skill. v2.5.0 **file-verified** corpus: 84 styles, 161 palettes, 73 font pairings, 99 UX rules, 161 reasoning rules/products, 25 charts, 1,923 Google-font table. After keel's **corpus-beat pass**, keel now leads on every comparable array: **170 archetypes, 90 styles, 230 palettes, 140 pairings, 37 charts, 112 UX guidelines** (plus 45 color moods / 30 typography moods / 15 stack profiles ,  869 total cross-referenced entries). Both persist to `design-system/MASTER.md`. They are cross-harness (18 platforms); ours ships inside the single hook-wired Rust binary (no Python runtime). Accessibility is checklist guidance, not automated WCAG validation ,  same posture both sides. No command-output compaction, no review gate, no learning loop, no brownfield gate on their side. |
| harness (`revfactory/harness`) | A single meta-skill "team-architecture factory": from a one-line domain prompt it generates a coordinated agent team plus the skills those agents use | Apache-2.0 | Niche overlap with keel's orchestration skills. Ships **1 skill + 6 reference docs, zero hooks, zero subagents, zero CLI**; depends entirely on the harness's experimental Agent Teams API (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`). Six orchestration patterns (pipeline, fan-out/fan-in, expert pool, producer-reviewer, supervisor, hierarchical) + a Phase-0 audit / duplicate-review brownfield gate. harness-only, manual invocation. Closed by keel's `designing-agent-teams` skill (the same pattern catalog + contract discipline, without the experimental-API dependency). |
| compound-engineering (`EveryInc/compound-engineering-plugin`) | "Compound engineering" plugin: front-load planning/review and codify each solved problem into a reusable knowledge base so future work is easier | MIT | Broadest methodology overlap. **38 skills + 43 subagents** (all markdown), installable to 10 harnesses via a TypeScript converter CLI; **zero hooks** (entirely manual/slash-driven). Signature is `ce-compound`: writes categorized, frontmatter-tagged solution docs to `docs/solutions/` and self-edits AGENTS.md/CLAUDE.md for discoverability. Has `ce-worktree`, multi-lens review fan-out, design + security reviewers. Closed by keel's `compounding-knowledge` skill (the same capture-and-wire-discoverability loop), which complements our automatic, hook-driven `learn` loop they lack. |

Note: published star counts for these repos (caveman/superpowers/ECC/RTK) were
flagged as implausible/unverifiable during research and are deliberately not used
as a comparison signal here. The comparison is capability-based.

## Shipped in the gap-closing pass

All changes below are markdown/JSON/shell only (no Rust recompile required) and
were validated where executable.

1. **Doc/impl drift fixed (highest-leverage).** The operating doctrine commanded
   several retired CLI surfaces that the Rust runtime no longer ships.
   The remaining `memory` family verbs are implemented and the intent is routed
   through `working-brief`, `completion-gate`, `recall`, plus L1 files. Files:
   `AGENTS/references/30-execution-strategy.md`, `_shared/common-discipline.md`,
   `docs/context-efficiency-playbook.md`.
   `docs/runtime-guardrails-and-memory-protocols.md`.
2. **Custom slash commands (discoverability gap vs. the whole field).** Added
   `/keel:anvil`, `/keel:review`, `/keel:recall`, `/keel:gain` at the plugin root
   `commands/` (Anvil is the only delivery loop; sprint/user-story commands are deleted). Each
   wraps only implemented CLI surfaces with the verified flag names.
   Frontmatter validated.
3. **Statusline savings badge (caveman/RTK-style ROI surface).**
   `statusline/statusline-keel.sh` and `.ps1` render model + context + a
   `gain`-sourced `saved N tok` badge, pinned to the real `tokensSaved` field,
   degrading gracefully (exit 0, badge omitted when unavailable). Opt-in via
   `settings.json`. Test matrix passed on both shells.
4. **Doc accuracy aligned to official docs.** Corrected the CLAUDE.md hook-count
   claim (30 in `HOOK_EVENTS`, 18 install by default, 12 opt-out: reserved no-ops plus `FileChanged`/`MessageDisplay`; all 30 dispatchable ,  historical 29/28 note corrected) and the stale `::EVENTS`
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
   sweep and uninstall reach them. The native `keel install` ships the
   slash commands, not just the plugin path. Covered by
   `install_copies_slash_commands_into_user_global_commands_directory`.
   A lifecycle audit of install/update/sync/remove-stale/remove-old/uninstall
   confirmed commands flow correctly through all stages and surfaced one gap:
   `verify` did not re-check the installed `commands/*.md` (nor the subagent
   `.claude/agents/*.md` definitions). Added `verify_installed_markdown_dir` so
   `keel verify` now byte-compares both against source ,  a drifted
   command/subagent fails verify (proven end-to-end) instead of slipping through.
2. **`MessageDisplay` hook row.** Added to `HOOK_EVENTS` in `hooks/claude.rs` with
   `installs_in_settings: false` (no matcher, fires on every assistant message,
   emits `displayContent` not `additionalContext` ,  auto-install would be a no-op
   or silently rewrite on-screen text). The opt-out invariant test was renamed to
   `only_known_events_opt_out_of_install` and pins both `FileChanged` and
   `MessageDisplay`.
3. **Planned memory commands implemented.** On a new shared `RecordStore`
   primitive (`utility/record_store.rs`) and a new `utility/memory_families.rs`
   module, the shipped memory families remain available:
   - The earlier orchestration/task-ledger and snapshot proposal was removed
     before release and is not part of the native core.
   - `memory research-cache|maintenance|agent-registry|agent-packets|loop-guard|entity|graph|retrieve|status`.
   The memory groups are isolated on disk (`<group>/<family>/`). `memory report`
   aliases the family status summary, `memory index` rebuilds the FTS5 recall index,
   and `memory hook` redirects to the real `keel hook` lifecycle surface.
4. **Learning loop (ECC Instincts-style).** `memory instincts
   record|reinforce|penalize|list|promote` in `memory_families.rs`:
   confidence-scored patterns keyed by trigger that reinforce/penalize over time
   and promote (optionally writing a markdown digest) once they meet a confidence
   threshold. This closes the biggest *conceptual* gap ,  durable memory that now
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
   `keel skill-lint` command: validates that every `<name>/SKILL.md` has
   the structural properties the matcher needs to *trigger* (non-empty
   `description`, `name` matching the directory, combined description+when_to_use
   within the 1536-char budget, scoped `allowed-tools` warning, no dangling
   `references/*.md` links). All 20 shipped skills pass; the gate fails closed when
   a skill would silently fail to trigger.
8. **`memory report|index|hook` resolved.** `report` aliases the family status
   summary, `index` rebuilds the FTS5 recall index (one index, not two), and `hook`
   redirects to the real `keel hook` lifecycle surface instead of being a
   dead stub.
9. **Database compaction adapter (RTK parity).** New `adapters/database.rs` + a
   `CommandKind::Database` variant: `psql`/`mysql`/`mariadb`/`sqlite3`/`redis-cli`/
   `mongosh` route to a result-set-aware adapter (header + sampled rows + omitted
   count for large tables, structure-only JSON for mongosh, query-error-first on
   failure, connection-string/password redaction). Bulk-export tools
   (`pg_dump`/`mysqldump`) stay on the logs adapter. Registry routing is tested.
10. **AgentShield-style config security audit.** New `utility/config_audit.rs` +
    `keel config-audit` command: audits keel's OWN config surface
    (hooks, settings/permissions, plugin manifest) for shell-metacharacter
    injection, network-fetching hooks, `bypassPermissions`, unscoped `Bash` allow
    rules, and committed secret literals in MCP env. Fails closed (exit 2) on any
    high finding. Distinct from the security-and-compliance-auditor skill, which
    audits the user's application code. Runs clean against the repo's own config.
11. **Output-economy skill (caveman axis).** New `output-economy/SKILL.md`:
    reduces the model's own reply verbosity (no preamble, no re-narration of tool
    output, length tracks the task) without dropping technical signal ,  the
    output-side counterpart to `compression-discipline`'s input-side rules. The
    one axis we previously didn't address at all. Passes `skill-lint`.
12. **Two-stage review gate (superpowers-style).** `reviewer/SKILL.md` now opens
    with an explicit ordered gate: Stage 1 spec-compliance (does it do what was
    asked) must be clean before Stage 2 code-quality runs, with mandatory
    re-review of the producing stage after any fix. Replaces the single
    undifferentiated pass so a polished implementation of the wrong spec cannot
    pass on code quality alone.
13. ~~**Git-backed code checkpoints (`/rewind` analog).**~~ Not shipped: the
    planned snapshot surface was removed before release. Current recovery uses
    the anvil bank, raw-output retention, working briefs, and the existing review
    gates rather than a separate checkpoint product.
14. **Autonomous learning loop (Hermes/ECC headline parity ,  the biggest gap closed).**
    The prior `memory instincts` CLI (item 4) was the *data model* for instincts,
    but every transition was operator-driven ,  nothing observed behavior or
    created skills. This pass wired the full closed loop, the one capability only
    Hermes Agent and ECC had:
    - `runner/observation.rs` ,  append-only per-day JSONL behavioral log captured
      automatically on PostToolUse. Secret-scrubbed, truncated, navigation-noise
      filtered, daily rotation, fail-open. Command lines collapse to stable
      low-cardinality signatures (`git commit`, `cargo test`, `edit:rs`).
    - `runner/learning.rs` ,  the loop. Clusters observations per project+signature,
      upserts confidence-scored instincts (>=3 observations), decays and prunes
      instincts whose pattern ages out, and evolves trusted instinct clusters
      (>=2 instincts at confidence >=5 across >=2 sessions) into generated
      `SKILL.md` skills plus a paired subagent ,  deterministic Rust template, no
      inline LLM. Runs automatically on SessionEnd (no slash command);
      `keel learn [status|dry-run|run]` is the inspection/manual-trigger
      surface.
    - **Provenance discipline** (the spine): every generated artifact is marked
      `generated`/`provenance=learned` with a content-hash sidecar. The loop never
      rewrites a built-in repo-synced skill, and respects manual edits to a
      generated skill (content-hash no-clobber guard) so the agent can freely
      refine them. Disable with `CLAUDE_SKILLS_LEARNING=off`.
    - **Always-on instinct digest** (ECC's lightweight tier): SessionStart injects
      a compact digest of the current project's trusted instincts so learned
      conventions are in context without waiting for a skill match.
    First time keel matches Hermes/ECC on automatic
    skill-creation-from-behavior; superpowers does it as an offline batch, and
    the harness/caveman/RTK/ohmyclaude do not do it at all.

Prior non-Rust work (doc/impl drift fixes, the four slash commands, the
cross-platform statusline savings badge, and hook-count doc accuracy) shipped
earlier in the same pass and remains in place; the doctrine docs were then
reconciled to describe these commands as implemented rather than planned.

## Phase 2: Runtime guardrails (in progress)

Five Rust-runtime features closing the remaining operational gaps against RTK
and the native baseline. Implemented one at a time, each verified with unit
tests before moving to the next.

### G1: Destructive-command guard ,  shipped

**What:** `detect_destructive_command()` in `runner/shell_rewrite.rs`
pattern-matches irreversible shell commands before they execute and emits a
harness `permissionDecision: "deny"` payload that blocks the tool call.

**Patterns blocked (severity = Block):**
- `rm -rf /`, `rm -rf ~`, `rm -rf $HOME` ,  recursive delete on root/home
- `rm -rf /*` ,  root-glob recursive delete
- `git push --force` / `git push -f` to `main`, `master`, or `dev` ,  protected-branch force push
- `dd of=/dev/sdX` ,  raw write to block device
- `mkfs.ext4` / `mkfs.*` ,  filesystem format
- `DROP DATABASE`, `DROP TABLE` ,  SQL schema destruction

**Patterns warned (severity = Warn):**
- `rm -r` on broad targets (`/`, `~`, `*`, `.`, `..`) ,  recursive delete on broad scope
- `git push --force` to non-protected branches ,  force push (not blocked, but flagged)
- `chmod -R 777` ,  world-writable recursive permission change
- `DELETE FROM` without `WHERE` clause ,  unconditional table wipe
- `TRUNCATE TABLE` ,  table wipe

**Wiring:** Called from `run_hook_pre_tool_use()` in `hook_lifecycle/mod.rs`
before the rewrite step. `analyze_command_text` → `detect_destructive_command`
→ if a Block finding is returned, emit deny payload with the finding as the
reason; if only Warn findings, emit the warning in the allow payload so the
agent sees the caution but the command proceeds.

**Tests:** 17 unit tests in `shell_rewrite.rs` covering each pattern class,
safe-command false positives (normal `rm`, `git push`, `chmod`, `DELETE` with
`WHERE`), and the Block/Warn severity split. All pass.

**Cockpit color fix (collateral):** The cockpit rendering in `workflow.rs`
had four hardcoded ANSI escape sequences (`\x1b[1;36m`, `\x1b[33m`,
`\x1b[1;33m`, `\x1b[1;32m`) that bypassed the `colorize()` function and
respected neither `--color` nor `--no-color`. Routed all four through
`colorize()` so `ColorMode::Off` strips them and `ColorMode::On` emits them.
The two cockpit color tests (`cockpit_with_no_color_flag_has_no_ansi_codes`,
`cockpit_with_color_flag_has_ansi_codes`) now pass.

### G2: AI-slop detector ,  shipped

**What:** `slop_detector.rs` module scans added diff lines for the 5 most common
AI-generated code smells. Wired into `review pre-commit` and `review pre-pr` as
a Warn-level (advisory, non-blocking) gate alongside the existing
`comment_style` gate.

**Patterns detected:**
1. **Dead defensive code** ,  `let _ = expr;` discarding a Result without
   explanation, empty `if let Ok(_) =` / `if let Some(_) =` arms that discard
   the matched value
2. **Over-commenting** ,  4+ consecutive comment lines for a single code line
   (the model over-explaining trivial code)
3. **Phantom flags** ,  function parameters prefixed with `_` (e.g.
   `fn process(data, _verbose: bool)`) indicating an unused parameter the model
   added "just in case"
4. **Hallucinated APIs** ,  `.fetch_all()` on non-ORM types, `dotenv().unwrap()`
   (panics on missing .env), `serde_json::from_str()` without `?` or `match`
5. **N+1 queries** ,  `.find()`, `.filter()`, `.position()`, `.contains()` called
   on a collection declared outside a loop, inside that loop body (O(n*m) bug)

**Wiring:** `slop_gate()` in `review.rs` calls `lint_working_slop()` (pre-commit)
or `lint_added_slop()` (pre-pr), returns a `GateResult` with `blocking: false`
and `GateStatus::Warn` when findings exist. The gate appears in the review
output as `slop_detector` alongside `comment_style`, `cargo_fmt`, `cargo_clippy`,
etc.

**Tests:** 15 unit tests in `slop_detector.rs` covering each pattern class,
exemptions (commented discards, handled serde, 2-comment blocks), context-line
exclusion, and clean-code no-findings. All pass.

### G3: shipped ,  Compaction loss visibility

**What**: Added `CompactionLossSummary` struct and `load_compaction_loss_today()`
public function to `gain.rs`. Added `render_compaction_loss()` to `workflow.rs`
that renders a COMPACTION LOSS panel in the cockpit between TEAM LANES and the
bottom border. The panel shows today's commands observed, commands compacted,
tokens before → after, tokens saved, and savings percentage. When no compaction
events exist for today, it displays "(no compaction events today)".

**Where**:
- `rust/crates/keel/src/utility/gain.rs` ,  `CompactionLossSummary` struct,
  `load_compaction_loss_today()` function (reuses `load_gain_summary` with 24h
  cutoff)
- `rust/crates/keel/src/utility/memory/workflow.rs` ,  `render_compaction_loss()`
  function, wired into cockpit rendering after `render_team_lanes`

**Tests**: 3 unit tests in `gain.rs` (savings_percent zero case, savings_percent
calculation, from real events), 2 cockpit tests in `workflow.rs` (section
presence, no-color no-ANSI). All 5 pass.

### G4: shipped — working-brief Linear/Jira export

**What:** `keel memory working-brief export --id <brief> --format linear-issue|jira-issue`
writes a title/description JSON payload from the stored brief (`title` = request,
`description` = acceptance criteria, `keelBriefId`, `workspace`). Live API push
still needs a tracker token in the host; the export is the keel-owned contract
so the ledger can sync without inventing a second brief store.

**Where:** `utility/working_brief.rs` `linear_issue_payload`, CLI
`memory working-brief export`.

### G5: shipped ,  TOML filter DSL

**What**: Extended the existing `DeclarativeFilter` in `proxy/filters.rs` with a
`stages` field that accepts an ordered pipeline of transformation stages. Each
stage is a TOML table with `type` tag selecting the transformation:

- `strip_ansi` ,  remove ANSI escape sequences
- `strip` ,  remove lines matching any pattern
- `keep` ,  keep only lines matching any pattern
- `dedup` ,  collapse consecutive identical lines with `(Nx)` count
- `head_tail` ,  keep first N + last N lines with omission marker
- `signal` ,  keep only error/warning/failure signal lines
- `json_structure` ,  compact JSON to type placeholders (`<str>`, `<num>`, etc.)
- `redact` ,  mask lines containing secret-like patterns

Stages run in declared order. When `stages` is empty, the legacy `keep`/`remove`
behavior is preserved for backwards compatibility.

**TOML example**:
```toml
[[filter]]
name = "cargo-test-staged"
command = "cargo test"
match_mode = "starts_with"

[[filter.stages]]
type = "strip_ansi"

[[filter.stages]]
type = "strip"
patterns = ["warning", "deprecated"]

[[filter.stages]]
type = "signal"
max_lines = 20

[[filter.stages]]
type = "head_tail"
head = 5
tail = 5
```

**Where**: `rust/crates/keel/src/proxy/filters.rs` ,  `FilterStage` enum,
`apply_stages()` function, `signal_lines()` helper, updated `compact()` method.

**Tests**: 6 new tests (staged TOML parsing, strip+keep pipeline, head_tail,
dedup, redact, json_structure). All 15 filter tests pass.

## Head-to-head scorecard (post-pass)

Capability-based, after the autonomous-learning pass. Y = present, ~ = partial,
N = absent.

| Capability | keel | Hermes | ECC | superpowers | RTK | caveman | ohmyclaude |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Observe behavior -> auto-create skills | Y (SessionEnd, deterministic) | Y (per-turn fork) | Y (hook+observer) | ~ (offline batch) | N | N | N |
| Confidence-scored instincts w/ decay+prune | Y | ~ (usage counters) | Y | N | N | N | N |
| Provenance guard (never clobber built-in/manual) | Y | Y (`write_origin`) | ~ | n/a | n/a | n/a | n/a |
| Always-on learned-convention injection | Y (SessionStart digest) | Y (system prompt) | Y (band >=0.7) | N | N | N | N |
| Command-output compaction proxy | Y (multi-adapter, all platforms, every tool) | N | ~ | N | Y (100+ subcmds; Windows auto-rewrite via native hook since RTK v0.37.2) | N | N |
| Output-side verbosity economy | Y (output-economy skill) | N | N | N | N | Y | N |
| TDD loop as a named skill (RED-GREEN-REFACTOR) | Y (test-driven-development) | N | N | Y | N | N | N |
| Root-cause debugging as a named skill | Y (systematic-debugging) | N | N | Y | N | N | N |
| Design-before-code brainstorming as a named skill | Y (brainstorming, brief-captured) | N | N | Y | N | N | N |
| Plan-authoring + step-verified plan-execution skills | Y (writing-plans + executing-plans) | N | N | Y | N | N | N |
| Subagent-driven development as a named loop | Y (subagent-driven-development) | ~ | ~ | Y | N | N | N |
| Parallel-agent dispatch w/ independence test | Y (dispatching-parallel-agents) | N | ~ | Y | N | N | N |
| Git-worktree isolation as a named workflow | Y (using-git-worktrees) | N | N | Y | N | N | N |
| Branch-finishing closeout skill | Y (finishing-a-development-branch) | N | N | Y | N | N | N |
| Author-side receiving-code-review skill | Y (receiving-code-review) | N | N | Y | N | N | N |
| Adversarial skill-prose eval harness | Y (writing-skills + subagent pressure-test) | N | N | Y (headline) | N | N | N |
| Fail-closed review gate + release ladder | Y | N | ~ | ~ (TDD loop) | N | N | N |
| Brownfield preserve-existing-flow gate | Y (unique) | N | N | N | N | N | N |
| Auto-refreshed system map + recall index | Y | ~ (memory) | ~ | N | N | N | N |
| Git-backed code checkpoints | Y | N | ~ | N | N | N | N |
| Cross-harness adapters (Codex/Cursor/Gemini) | ~ (adapters exist, shallow depth) | Y | Y | Y | Y | Y | ~ |
| Zero-manual automation (all hook-driven) | Y | ~ (CLI agent) | ~ (opt-in) | ~ | Y (hook; Windows native since RTK v0.37.2) | ~ | N |

**Where we still lose (honest), updated after the breadth + prose pass:**
- **Cross-harness depth** ,  every comparator runs deeply on Codex/Cursor/Gemini
  as first-class targets. keel is Claude Code-primary with bridge adapters for
  OpenCode, Codex CLI, Cursor (rules + hooks + MCP), and Pi Agent (rules + hooks
  + MCP). The adapters exist (see README § Cross-Agent Adapters) but are
  shallower than the Claude Code integration on some hosts (detection/install
  depth, host API limits). This is a depth gap, not a presence gap.
- ~~RTK filter breadth~~ **(closed this pass).** Two real defects were found and
  fixed, not just breadth added:
  1. The PreToolUse auto-rewrite gate (`is_supported_noisy_command`) had drifted
     narrower than the classifier, so the `database`/`cloud`/`containers`
     adapters we already shipped were *unreachable on the automatic path* , 
     `psql`, `az`, `gcloud`, `helm`, `podman`, `cmake`, and others were never
     auto-wrapped, so their adapters were dead code in practice. The gate now
     covers every command the classifier routes to a dedicated adapter.
  2. `adapter_name_for_rewrite` mislabeled `docker`/`kubectl`/`aws` as `logs`
     when they actually route to `containers`/`cloud`. Fixed to match real
     routing.
  Breadth was also widened (~30 new programs: mocha/ava/cypress/karma/tap/
  gotestsum/bats/ctest/tox/nox test runners; bazel/buck/buck2/meson/scons/bear/
  sbt/lein/mill/cabal/stack/mix build systems; pylint/pyright/shellcheck/hadolint/
  yamllint/stylelint/tflint/oxlint/standard/luacheck/vale linters). A new
  drift-guard test (`every_specifically_classified_program_is_auto_wrappable`)
  makes the two surfaces unable to diverge silently again. Note RTK's "100+"
  count is mostly *subcommand* breadth within the same categories we cover.
  RTK ships native-Windows auto-rewrite since v0.37.2 (`rtk hook claude`);
  older "CLAUDE.md injection only" claims are stale. RTK still does not touch
  Read/Grep/Glob (shell-tool hook only). keel's PostToolUse telemetry +
  PreToolUse rewrite fire on every tool on every platform, and keel still
  leads on iron-law, multi-lang review closeout, and memory/skills.
- ~~Skill-prose polish~~ **(closed this pass).** `keel learn synthesize`
  emits a precise, agent-actionable refinement brief for every template-state
  generated skill (carrying the observed conventions as the source of truth),
  and SessionStart now surfaces that brief autonomously (no manual slash) so the
  session model upgrades the prose in the normal course of work. The agent's
  edit is protected by the content-hash no-clobber guard, and the nudge
  self-clears once the skill is refined. The binary still never calls an LLM , 
  the session model that harness already runs does the authoring. `learn run
  --synthesize` also collects briefs inline for a freshly generated skill.
- ~~superpowers methodology-skill discoverability~~ **(closed this pass).** A
  focused re-audit against `obra/superpowers` v5.1.0 (read the actual SKILL.md
  tree, the SessionStart hook, and the marketplace manifest) confirmed parity or
  a win on everything except the by-design cross-harness axis ,  **with three
  exceptions**: superpowers ships `test-driven-development`, `systematic-debugging`,
  and `brainstorming` as *first-class, name-triggerable* skills, while keel
  carried the same doctrine only embedded inside `_shared/common-discipline.md`
  (Think-Before-Coding / Goal-Driven Execution). Embedded doctrine fires only when
  a broader skill is already loaded; a named skill activates by its own matcher.
  Closed by promoting all three to standalone skills that delegate to the shared
  discipline rather than restating it:
  - `test-driven-development` ,  the tight RED→GREEN→REFACTOR loop (watch it fail
    for the right reason, minimum code to green, refactor under green), plus the
    bug-fix-as-missing-test branch.
  - `systematic-debugging` ,  reproduce → trace-to-root-with-file:line → fix the
    source of truth → prove with a regression test; explicit "stop after two
    failed attempts and re-trace" rule.
  - `brainstorming` ,  Socratic design exploration that converges on one agreed
    design and **captures it in the working brief** (so `reviewer` Stage 1 has a
    spec to check against), the generative front half of Think-Before-Coding.
  All three pass `skill-lint`, install + byte-compare-verify cleanly, and are
  registered in the plugin manifest and the `using-keel` bootstrap catalog.
  Note superpowers authors skills by a *manual* TDD-for-prompts method;
  keel's authoring is the autonomous learning loop ,  different mechanism,
  both now cover the methodology surface.

- ~~Remaining superpowers methodology surface (the other 11 skills + the
  writing-skills eval harness)~~ **(closed this pass).** A full capability re-audit
  against `obra/superpowers` v5.1.0 (read all 14 SKILL.md frontmatters, the single
  SessionStart hook, the marketplace manifest, and the `writing-skills` eval method)
  mapped every one of its skills to keel. The prior pass had closed the
  methodology *trio*; this pass closed the rest, promoting diffuse doctrine and CLI
  surfaces into discrete name-triggerable skills and closing the one genuine
  mechanism gap. Eight new skills:
  - `writing-skills` ,  **the headline gap.** superpowers' meta-skill applies TDD to
    skill *prose*: dispatch a fresh subagent the target situation *without* the skill
    under stacked pressure (time + sunk cost + authority), capture the wrong call and
    its rationalizations, write the minimum prose that flips it, re-test under
    pressure until the subagent decides right and cites the skill. keel had
    only `skill-lint` (structural, explicitly "without invoking the live model") and
    the statistical `learn` loop ,  nothing tested whether prose changes behavior.
    Now shipped as a skill plus `references/10-testing-skills-with-subagents.md`,
    framed as the behavioral gate *above* skill-lint's structural gate.
  - `writing-plans` / `executing-plans` ,  the plan-authoring and step-by-step
    plan-execution loop (each step names files + a verification check; stop on a
    failed check), promoting what was spread across `software-development-life-cycle`
    and the workflow/orchestration ledgers into two discrete skills.
  - `subagent-driven-development` ,  delegate self-contained tasks to fresh-context
    subagents and re-verify in the main thread (the discipline behind the 24-agent
    roster, now a named loop).
  - `dispatching-parallel-agents` ,  the four-condition independence test as a
    name-triggerable skill (was per-prompt doctrine in `hook_lifecycle.rs`, no skill).
  - `using-git-worktrees` ,  isolated checkouts, prefer-native-then-worktree, with
    cleanup (was `git-expert` prose + telemetry-only WorktreeCreate/Remove hooks).
  - `finishing-a-development-branch` ,  verify → completion-gate → reviewer → present
    merge/PR/cleanup options (never unilateral force-push/merge), consolidating
    closeout that was split across `git-expert`, the workflow `finish --proof`
    ledger, and the completion gate.
  - `receiving-code-review` ,  the author-side counterpart to `reviewer`: judge each
    comment on merit, fix valid ones at root cause with evidence, push back on wrong
    ones with evidence, re-verify (superpowers separates requesting vs receiving;
    `requesting-code-review` maps to our `reviewer` + `/keel:review`).
  All eight pass `keel skill-lint` (32 skills, 0 failed, 0 warned) and are
  registered in the plugin manifest and the `using-keel` catalog. With this
  pass, keel ships a named first-class equivalent for **all 14** superpowers
  methodology skills; the only remaining superpowers lead is the by-design
  cross-harness axis.

### Skill count: manifest-driven (methodology parity + cross-comparator + domain-coverage gap closure)

The repo ships a manifest-driven skill count discovered from `.claude-plugin/plugin.json`
at install time (the binary never hardcodes a total, so no number drifts). As of the
2026-06-24 audit, the manifest lists 46 skills plus the `using-keel` bootstrap
(47 SKILL.md files on disk). Run `keel skill-lint` for the live verified count.
The methodology trio
(`test-driven-development`, `systematic-debugging`, `brainstorming`) closed the first
superpowers gap; the eight skills (`writing-skills`, `writing-plans`, `executing-plans`,
`subagent-driven-development`, `dispatching-parallel-agents`, `using-git-worktrees`,
`finishing-a-development-branch`, `receiving-code-review`) closed the rest of the
superpowers surface; the three newest close the cross-comparator gaps found in the
harness / compound-engineering / ECC audit:
- `designing-agent-teams` ,  closes the **harness** (`revfactory/harness`) gap: the
  six-pattern agent-team-architecture factory (pipeline, fan-out/fan-in, expert pool,
  producer-reviewer, supervisor, hierarchical) with per-agent role/input/output/
  verification contracts, but without harness's dependency on the experimental Agent
  Teams API. Hands execution to `dispatching-parallel-agents` + `subagent-driven-development`.
- `compounding-knowledge` ,  closes the **compound-engineering**
  (`EveryInc/compound-engineering-plugin`) gap: the `ce-compound`-style capture loop
  (categorized, deduped, evidence-bearing solution notes wired into CLAUDE.md/AGENTS.md
  discoverability pointers), as the human-readable complement to our automatic,
  hook-driven `learn` loop that they do not have.
- `adversarial-security-review` ,  closes the **ECC AgentShield** (`affaan-m/ECC`) gap:
  the red-team / blue-team / adjudicator pass (AgentShield's `--opus` three-agent loop)
  that chains static findings into concrete attacker scenarios and adjudicates each to
  confirmed/refuted/needs-proof with evidence, as the reasoning layer above our
  deterministic `keel config-audit` static scan.

The three newest specialists close the operational-domain-coverage gaps the
roster audit found (observability, supply-chain action, analytical/ML data flow):
- `observability-and-incident-response` ,  promotes what was only a
  `cloud-and-devops-expert` reference into a first-class skill: metrics/logs/traces
  via OpenTelemetry, golden signals, SLO/SLI and error-budget math, burn-rate
  paging linked to runbooks, and blameless postmortems.
- `dependency-and-supply-chain` ,  the action counterpart to
  `security-and-compliance-auditor`'s scanning: dependency upgrades, lockfile
  hygiene, semver risk tiering, major-version migration, SBOM, and provenance/signing.
- `data-and-ml-engineering` ,  the analytical/ML-flow counterpart to
  `backend-and-data-architecture`'s OLTP focus: ETL/ELT pipelines, dbt warehouse
  modeling, orchestration, data quality, and the ML lifecycle through drift.

A further three specialists close the last roster-audit domain gaps
(build-side identity, cost/FinOps, and i18n/localization):
- `authentication-and-identity` ,  the build counterpart to
  `security-and-compliance-auditor`'s read-only auditing: OAuth2/OIDC
  (authorization-code + PKCE), SSO/SAML, session and token lifecycles,
  refresh-token rotation with reuse detection, MFA/passkeys/WebAuthn, and
  argon2/bcrypt password storage.
- `cloud-cost-and-finops` ,  the spend dimension neither `cloud-and-devops-expert`
  (mechanics) nor `observability-and-incident-response` (SLOs) owned: cost
  estimation before deploy, rightsizing, commitment planning, autoscaling/spot
  strategy, cost allocation and tagging, budget guardrails, and unit economics.
- `internationalization-and-localization` ,  the message/locale layer beneath
  `ui-design-systems-and-responsive-interfaces`: message-catalog design and
  extraction, ICU MessageFormat and plurals, locale-aware formatting, RTL/bidi,
  translation workflows and fallback chains, and Unicode correctness.

`using-keel` catalog header and entries updated to match the manifest (count is manifest-driven; run `keel skill-lint` for the live verified total).

- ~~UI/UX Pro Max corpus was smaller than theirs~~ **(closed and surpassed this
  pass).** A re-audit with **file-verified** counts (parsing their actual CSVs, not
  README claims) showed UI/UX Pro Max v2.5.0 is bigger than previously recorded , 
  84 styles, 161 palettes, 73 font pairings, 99 UX rules, 161 reasoning rules ,  and
  bigger than its own README advertises. The catalog was expanded (via four parallel
  authoring waves, merged with strict cross-reference + duplicate-id + hex + enum
  validation) from 282 to **869 cross-referenced entries**, now leading on every
  comparable array: **170 product archetypes** (vs 161), **90 style families** (vs 84),
  **230 color palettes** (vs 161), **140 font pairings** (vs 73), **37 chart types**
  (vs 25), **112 UX guidelines** (vs 99), plus 45 color moods, 30 typography moods, and
  15 stack profiles with no analog on their side. All cross-references validated (every
  archetype's recommended style/color/typography moods resolve; every palette/pairing
  resolves to a real mood); all 12 design-intelligence tests pass against the expanded
  catalog; an end-to-end run confirms new entries surface correctly (e.g. a pet-care
  request routes to the new `veterinary-clinic` archetype with the Calm Sage palette at
  15.41:1 contrast and a Quicksand + Nunito Sans pairing). The 869-entry / ~726 KB
  catalog ships on install and the generator runs against the installed copy. keel
  now exceeds UI/UX Pro Max on corpus volume **and** still leads on architecture
  (single hook-wired binary, no Python runtime), brownfield + WCAG + review-gate
  discipline, and the automatic learning loop.

- ~~UI/UX Pro Max design-intelligence generator was a stub~~ **(closed this
  pass).** An audit against `nextlevelbuilder/ui-ux-pro-max-skill` (whose headline
  is a knowledge-corpus design generator) exposed a real doc/impl drift on **our**
  side: `keel design-intelligence recommend` was a three-line stub that
  ignored the request, the 47-entry `design_intelligence_catalog.json`, and every
  flag the SKILL.md and reference doc documented (`--stack`, `--component-library`,
  `--format`, `--persist`). The skill *promised* a catalog-driven generator that
  did not exist. Implemented for real in `utility/design_intelligence.rs`:
  - Loads the catalog (explicit `--catalog`, else the repo skill, else the
    installed `<claude_home>/skills/.../data/` copy), keyword-scores the request
    against product archetypes, and emits a confidence-rated packet: archetype +
    trust posture + content priorities + CTA guidance, then style family, color
    mood, typography mood, polish/recovery/verification checks, and merged
    anti-patterns.
  - `--stack` biases style/color/typography by intersecting the archetype's
    recommendations with the stack profile and appends stack-specific guidance,
    preview tools, and validation checks; an unknown stack is noted, not fatal.
  - `--component-library` adds reuse guidance; `--persist --project-name --page`
    writes a `design-system/MASTER.md` artifact (the UI/UX Pro Max persistence
    analog); `--format json|text`.
  - A request-first argument parser (flags can follow the free-text request in
    either order) replaces the shared `FlagSet`, which would have swallowed flags
    into the request string. 8 unit tests + an end-to-end install/verify run cover
    routing, JSON shape, stack bias, unknown-stack fallback, low-confidence
    fallback, persistence, and error paths.

  Honest standing vs UI/UX Pro Max, **after the corpus-expansion pass**: the
  catalog grew from 47 to 282 cross-referenced entries ,  25 product archetypes,
  23 style families, 21 color moods, 15 typography moods, 15 stack profiles, plus
  four new artifact arrays the generator now emits directly: 48 named color
  palettes (light + dark, real hex + WCAG contrast notes), 50 font pairings
  (Google Fonts + system stacks, with scale and rationale), 25 chart types, and
  60 UX guidelines. The generator was extended with `pick_palette`,
  `pick_font_pairing`, `pick_chart_types`, and `pick_ux_guidelines` so a
  recommendation now carries a concrete palette (dark-mode-biased when the request
  asks for it), a concrete type pairing, data-viz chart picks when the request
  implies a dashboard, and the archetype-scoped UX rules ranked critical-first , 
  matching the artifact types UI/UX Pro Max emits. All cross-references are
  validated (every palette/pairing resolves to a real mood; every archetype's
  recommended moods resolve). 12 unit tests (4 new for the concrete artifacts:
  palette+pairing attachment, dashboard→charts, matched UX guidelines, dark-mode
  palette bias), fmt + clippy clean under `-D warnings`, full workspace test suite
  green, and an install/verify run confirming the 221 KB catalog ships and the
  generator runs against the installed copy.

  Their corpus is still numerically larger on two axes (their 67 styles / 161
  palettes vs our 23 / 48) and they remain cross-harness, but the **artifact
  types, generator architecture, and cross-reference validation now match or
  exceed** theirs, and ours feeds a deeper UI skill with brownfield + WCAG +
  review-gate discipline they do not have inside a single hook-wired binary with
  no Python runtime. The remaining difference is incremental data volume on two
  arrays, not capability or architecture.


### Bugs found and fixed while verifying the workflows end-to-end

The user asked to confirm every workflow runs "like it should." Three latent
defects surfaced during end-to-end verification and were fixed with regression
tests:
- **Delta-patch over-reported churn.** `sync_shared_resources` returned the
  shared-*directory* count (always 1), not the file change count, so every
  re-install claimed "Synced shared resources: 1" even on a pure no-op.
  `sync_directory_delta` now returns the real changed-file count; a no-op
  re-install reports 0 across every category. Regression:
  `reinstall_is_zero_churn_when_nothing_changed`.
- **Hook uninstall left dead keys.** `remove_managed_hooks` stripped our command
  entries and empty matcher entries but left 28 empty `"Event": []` keys behind,
  so an uninstall did not restore settings.json to its pre-install shape. It now
  prunes event keys whose array became empty, while preserving any event key
  that still holds a user-authored hook. Regressions:
  `install_then_uninstall_leaves_no_managed_hook_keys`,
  `uninstall_preserves_user_authored_hook_on_shared_event`.
- **Stop hook "JSON validation failed"** (diagnosed, not a keel defect).
  Transcript evidence shows the failing Stop hook is the `/goal`
  prompt-based session hook (its `command` field is the goal text), exiting 1
  with `stderr: "JSON validation failed"`. keel's own Stop hook
  ("Closing native session state") succeeds in 38 ms with empty stdout. The
  `/goal` evaluator routes through the user's model proxy (`ANTHROPIC_BASE_URL=
  http://localhost:8989`, every model slot mapped to `claude-opus-4-8[1M]`),
  which is not returning the structured yes/no JSON `/goal` requires. The fix is
  environmental (point `/goal` at a model that honors the structured-output
  contract, or set its evaluator model), not a code change in keel ,  our
  hooks already use exec form (`args` array), which is the documented immunity to
  the Windows shell-profile JSON-corruption failure mode.

## Remaining work (deliberately out of scope)

- **Cross-harness adapter depth.** keel ships adapters for OpenCode, Codex CLI,
  Cursor (rules + hooks + MCP), and Pi Agent (rules + hooks + MCP), but Claude Code is
  the only target with full lifecycle hooks and deep integration. Every
  comparator ships deeper integration across more providers. Closing the
  depth gap on existing adapters (or adding new ones) is a distribution
  strategy question, not a defect.
- **Mobile / niche command adapters.** Mobile toolchains (xcodebuild beyond the
  generic build path, gradle device flows) and other niche CLIs still fall through
  to the generic adapter. Reasonable future additions, not audited gaps. The
  previously-listed database adapter now ships (`adapters/database.rs`).

## Native-parity note (`/rewind`)

Native harness `/rewind` auto-captures the edit tool's changes and can restore
code and conversation. Keel does not ship a separate checkpoint command. Current
recovery uses the Anvil bank, raw-output retention, working briefs, and review
gates; `/rewind` remains the harness-owned code and conversation recovery path.

## Strategic open question

Every comparator (RTK, caveman, superpowers, ECC) ships deep cross-harness
adapters (Codex/Cursor/Gemini/Copilot). keel is Claude Code-primary with
lighter adapters for OpenCode, Codex CLI, Cursor, and Pi Agent (see
README § Cross-Agent Adapters). Whether to deepen the existing adapters to
full lifecycle parity or add more provider targets is a product decision , 
recorded here so the depth-vs-breadth tradeoff is chosen, not drifted into.

## 2026-06-17 audit: high-visibility harness / workflow comparators

This section extends the capability-based comparison to **high-visibility**
harness and workflow projects, selected by ecosystem presence (community
discussion frequency, cross-references in other projects, and capability
overlap with keel's surface). Star counts are deliberately excluded as a
selection or comparison signal ,  multiple repos in this space show
anomalous star-to-commit ratios (e.g. `obra/superpowers` returned 229,888
stars via GitHub API, exceeding `anthropics/claude-code` at 132,832, which
is implausible for a skills plugin). Comparisons below are capability-based
only. Audit run after the findings #1-5 merge (PR #123, `f694519`); each
profile was built by reading the project's actual README/docs, not marketing copy.

### New comparators (selected by ecosystem visibility, 2026-06-17)

| Project | Category | Overlap with keel |
| --- | --- | --- |
| `obra/superpowers` | skills framework | Already in the table above. |
| `github/spec-kit` | spec-driven dev | Spec-as-source-of-truth pipeline (constitution→specify→plan→tasks→implement), agent-agnostic across 30+ agents. Strong gate *taxonomy* (Phase-1 Simplicity/Anti-Abstraction/Integration-First) but every gate is model-self-attested in-prompt; `[P]` parallel markers are annotations, not an executor. No worktree isolation, no recall index. |
| `ruvnet/claude-flow` | swarm orchestrator | The one comparator **ahead on memory**: HNSW vector store + knowledge graphs + neural self-learning (semantic recall we lack). Also broad: consensus topologies, GOAP A* planner, security gates (CVE/PII/injection). But benchmarks are self-reported, the lightweight plugin path is hollow (no MCP/memory tools ,  only the heavy npx+Docker+MongoDB path has them), and there is no worktree-isolated fail-closed merge. |
| `bmad-code-org/BMAD-METHOD` | agentic-agile personas | Role-persona pipeline (Analyst/PM/Architect/SM/Dev/QA) with "context-engineered" story files; mature methodology, multi-IDE. Orchestration is human-in-the-loop persona-switching ,  no parallel execution, no merge coordinator, no indexed memory, no self-eval. |
| `eyaltoledano/claude-task-master` | task manager | Dependency-aware PRD→task decomposition over an MCP surface; broad editor/provider reach. **No review/quality gates documented at all**, no worktree isolation, memory is task-state only (no recall). |
| `automazeio/ccpm` | PM workflow (GH Issues) | Closest parallel-dispatch peer: per-epic git worktrees + `conflicts_with`/`depends_on`/`parallel` task metadata. But the merge is LLM-narrated ("agents commit and coordinate via Git"), not a coded fail-closed coordinator; memory is flat markdown + grep-style bash scripts. |

### Where keel leads across *all* of them (post-#123)

The decisive axis is **enforcement mechanism**. Every project above enforces
quality by instructing the model in markdown and trusting compliance (superpowers'
"mandatory" workflows, spec-kit's Phase-1 gates, BMAD's TEA, ccpm's "No Vibe
Coding"). keel is the only one that puts gates in **compiled Rust the model
cannot talk past**. Four differentiators hold against every comparator:

1. **Fail-closed git-worktree merge coordinator** (`utility/dispatch.rs`
   `run_merge`): only a `complete` worker merges; conflict triggers `git merge
   --abort` leaving a provably clean tree (asserted by
   `merge_aborts_on_conflict_and_leaves_the_tree_clean`). ccpm and superpowers also
   use worktrees but leave the merge to model narration ,  the guarantee lives in
   their prompt, not their code. (Honest scope: `dispatch` owns the worktree
   lifecycle + ledger + merge gate; it does **not** spawn agents ,  the main thread
   still drives the subagents.)
2. **Real, reproducible compaction eval** (`utility/eval.rs`): the genuine adapter
   pipeline over fixtures with **exact o200k_base** token deltas and measured floors
   asserted in CI ,  not the self-reported speedup multipliers claude-flow/ccpm
   publish. (The legacy `bench` is a runtime-provenance/feature-parity marker, now
   clearly labeled as such; `eval` is the measurement.)
3. **Compaction break-even guard + prompt-injection neutralization** (`proxy/run.rs`):
   compacted output is emitted only when strictly fewer exact tokens than raw, and
   tool output is neutralized before the model sees it. No comparator has either , 
   they treat the harness as trusted and don't measure their own token effect.
4. **Executable delivery gate** (`keel anvil sieve` 0-LLM gates + `keel anvil stamp`
   + working-brief / completion-gate). spec-kit and BMAD have richer gate
   *prose*; keel refuses closeout when the named bar's gates are still red.

### Where keel loses (honest)

- **Semantic memory** ,  claude-flow's HNSW vector store + knowledge graphs beat our
  lexical FTS5 + trigram-fuzzy cascade for meaning-based recall. Mitigations: their
  memory tools are absent on the plugin path, and our lexical choice is the
  deliberate single-clean-binary / no-network / no-embeddings trade.
- **Eval breadth** ,  wshobson's `plugin-eval` (Static + LLM-Judge + 50-100-run Monte
  Carlo certification) is a more sophisticated eval *framework*; ours is real and
  reproducible but narrow (compaction fidelity only).
- **Cross-harness reach** ,  the standing by-design loss: every comparator runs on
  Codex/Cursor/Gemini/Copilot; we are harness-native. spec-kit (30+ agents) and
  superpowers (10+ harnesses) lead hardest here.
- **Adoption / validation** ,  8k-230k stars of battle-testing vs a private repo.

### Self-inflicted gaps from the #123 merge (found and fixed this pass)

The audit's skeptical re-read of our own post-merge code surfaced drift the merge
introduced ,  ironic, since finding #4 was itself a doc-parity test:

- **MCP tool-count drift.** CLAUDE.md claimed the server exposes **14 tools**;
  `mcp/tools.rs` defines the live tool set (Anvil, not sprint/user-story). Fixed CLAUDE.md
  and added
  `mcp_tool_count_matches_documentation` to `tests/doc_parity_test.rs` so the count
  can no longer drift silently.
- **Undocumented commands.** `dispatch` and `observe` were wired in `commands.rs`
  but absent from the CLAUDE.md Commands section. Documented both (plus the real
  `eval`) and added `audit_flagged_commands_are_documented` to pin them.
- **`bench` mislabeled as measurement.** Its byte/savings numbers come from
  hardcoded fixtures yet read like a real run. Investigation showed `bench` is not
  dead code ,  it's a deliberate runtime-provenance / feature-parity marker
  (`runtime=rust`, `goFallback=false`). Kept it (it carries a real signal) but
  relabeled its output as illustrative and added a doc-comment pointing to `eval`
  for actual measurement.

All fixes verified: `cargo fmt`/`clippy -D warnings`/`build` clean, doc-parity
suite green (7 tests, 2 new).

## What keel keeps (the moat)

Fail-closed closeout discipline (reviewer gate, completion-gate ledger, release
ladder), the preserve-existing-flow brownfield gate (no comparator has this), and
breadth-of-integration (compaction + methodology + review gates + memory in one
hook-wired binary) remain genuine differentiators even where individual pieces
are shallower than a single-purpose peer.

## Decided non-goals (chosen, not drifted)

The audit flagged several capabilities competitors have that keel does
not. After review these are **deliberate scope boundaries**, not defects ,  they
conflict with the "single Rust binary, harness-native, discipline-over-volume"
positioning. Recorded here so each is a chosen tradeoff:

- **Product-management domain skills (B1).** Product-management skills (roadmaps,
  PRDs, stakeholder alignment, prioritization frameworks) belong to a different
  tool category. keel is engineering-delivery rails, not a product-management
  platform. Building PM skills would dilute the engineering focus and expand the
  surface beyond the single-binary discipline stance. **Not pursued.**
- **Autonomous board-to-PR agent (B2).** Full-autonomy agents that take a ticket
  from a project board and drive it to a merged PR without human checkpoints
  contradict the "human-in-the-loop at every decision point" discipline. keel
  ships orchestration *patterns* (`designing-agent-teams`, `dispatching-parallel-agents`,
  `subagent-driven-development`) over the harness's native subagents/agent-teams/
  background-agents, with explicit human review gates at closeout. Autonomous
  board-to-PR would bypass those gates. **Not pursued.**
- **Multi-agent swarm runtime** (topologies/consensus, à la claude-flow). keel
  ships orchestration *patterns* (`designing-agent-teams`, `dispatching-parallel-agents`,
  `subagent-driven-development`) over the harness's native subagents/agent-teams/
  background-agents, not a separate swarm engine with consensus. A swarm runtime is
  outside the harness's native execution model and would contradict the single-binary
  stance. **Not pursued.**
- **Recency social research (B3).** Real-time social-media trend research and
  engagement signals are outside the engineering-delivery scope. keel's
  memory surfaces (`compounding-knowledge`, `instincts`, `working-briefs`) capture
  durable project knowledge, not ephemeral social signals. Niche capability with
  no clear integration into the delivery workflow. **Not pursued.**
- **Benchmark game harness (B4).** Public benchmark leaderboards and competitive
  scoring against other AI coding tools are marketing artifacts, not engineering
  value. keel's head-to-head scorecard (`docs/competitive-gap-closure.md`)
  is capability-based and honest about gaps, not a gamified leaderboard. The
  discipline-over-volume stance means we don't optimize for benchmark scores
  that diverge from real delivery value. **Not pursued.**
- **Exhaustive per-language / niche-vertical library breadth** (191-agent megalibraries
  like wshobson). keel is curated-not-exhaustive by design: manifest-driven skill count covering
  delivery domains and methodology, not one-agent-per-language. The matcher quality and
  discipline contract degrade with volume. **Not pursued; curation is the product.**
- **Passive automatic learning from corrections.** Native harness now ships Auto
  memory for this; keel's `compounding-knowledge` + `instincts` path is the
  deliberate, human-readable complement (see the native-Auto-memory note in CLAUDE.md and
  the bootstrap skill). We lean on native Auto memory rather than reinvent passive learning.
  **Complemented, not duplicated.**
- **Cross-harness portability** (Codex/Cursor/Gemini/Copilot adapters). The standing
  product decision above: deliberately harness-native. **Open product question, not a defect.**

Domain coverage, by contrast, was a *real* gap and is now closed: observability/incident
response, dependency/supply-chain, data/ML, build-side identity, cloud cost/FinOps, and
i18n/localization all ship as first-class skill triads.
