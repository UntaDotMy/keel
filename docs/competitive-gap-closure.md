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
| RTK ("Rust Token Killer", `rtk-ai/rtk`) | Single-binary Rust command-output compaction proxy | Apache-2.0 | Near-identical to claude-core's compaction proxy; "100+ supported commands" (mostly subcommand breadth within categories we also cover — 8 git subcmds, 8 aws subcmds, etc.), `gain`/`discover`/`session`, tee recovery. Verified from the upstream README: **no auto-rewrite on native Windows** (falls back to CLAUDE.md injection) and **never intercepts Read/Grep/Glob** (Bash-tool-only hook). |
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
14. **Autonomous learning loop (Hermes/ECC headline parity — the biggest gap closed).**
    The prior `memory instincts` CLI (item 4) was the *data model* for instincts,
    but every transition was operator-driven — nothing observed behavior or
    created skills. This pass wired the full closed loop, the one capability only
    Hermes Agent and ECC had:
    - `runner/observation.rs` — append-only per-day JSONL behavioral log captured
      automatically on PostToolUse. Secret-scrubbed, truncated, navigation-noise
      filtered, daily rotation, fail-open. Command lines collapse to stable
      low-cardinality signatures (`git commit`, `cargo test`, `edit:rs`).
    - `runner/learning.rs` — the loop. Clusters observations per project+signature,
      upserts confidence-scored instincts (>=3 observations), decays and prunes
      instincts whose pattern ages out, and evolves trusted instinct clusters
      (>=2 instincts at confidence >=5 across >=2 sessions) into generated
      `SKILL.md` skills plus a paired subagent — deterministic Rust template, no
      inline LLM. Runs automatically on SessionEnd (no slash command);
      `claude-skills learn [status|dry-run|run]` is the inspection/manual-trigger
      surface.
    - **Provenance discipline** (the spine): every generated artifact is marked
      `generated`/`provenance=learned` with a content-hash sidecar. The loop never
      rewrites a built-in repo-synced skill, and respects manual edits to a
      generated skill (content-hash no-clobber guard) so the agent can freely
      refine them. Disable with `CLAUDE_SKILLS_LEARNING=off`.
    - **Always-on instinct digest** (ECC's lightweight tier): SessionStart injects
      a compact digest of the current project's trusted instincts so learned
      conventions are in context without waiting for a skill match.
    First time claude-core matches Hermes/ECC on automatic
    skill-creation-from-behavior; superpowers does it as an offline batch, and
    Claude Code/caveman/RTK/ohmyclaude do not do it at all.

Prior non-Rust work (doc/impl drift fixes, the four slash commands, the
cross-platform statusline savings badge, and hook-count doc accuracy) shipped
earlier in the same pass and remains in place; the doctrine docs were then
reconciled to describe these commands as implemented rather than planned.

## Head-to-head scorecard (post-pass)

Capability-based, after the autonomous-learning pass. Y = present, ~ = partial,
N = absent.

| Capability | claude-core | Hermes | ECC | superpowers | RTK | caveman | ohmyclaude |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Observe behavior -> auto-create skills | Y (SessionEnd, deterministic) | Y (per-turn fork) | Y (hook+observer) | ~ (offline batch) | N | N | N |
| Confidence-scored instincts w/ decay+prune | Y | ~ (usage counters) | Y | N | N | N | N |
| Provenance guard (never clobber built-in/manual) | Y | Y (`write_origin`) | ~ | n/a | n/a | n/a | n/a |
| Always-on learned-convention injection | Y (SessionStart digest) | Y (system prompt) | Y (band >=0.7) | N | N | N | N |
| Command-output compaction proxy | Y (multi-adapter, all platforms, every tool) | N | ~ | N | Y (100+ subcmds, no native-Windows auto-rewrite) | N | N |
| Output-side verbosity economy | Y (output-economy skill) | N | N | N | N | Y | N |
| Fail-closed review gate + release ladder | Y | N | ~ | ~ (TDD loop) | N | N | N |
| Brownfield preserve-existing-flow gate | Y (unique) | N | N | N | N | N | N |
| Auto-refreshed system map + recall index | Y | ~ (memory) | ~ | N | N | N | N |
| Git-backed code checkpoints | Y | N | ~ | N | N | N | N |
| Cross-harness adapters (Codex/Cursor/Gemini) | N (by design) | Y | Y | Y | Y | Y | ~ |
| Zero-manual automation (all hook-driven) | Y | ~ (CLI agent) | ~ (opt-in) | ~ | Y (hook, POSIX only) | ~ | N |

**Where we still lose (honest), updated after the breadth + prose pass:**
- **Cross-harness reach** — every comparator runs on Codex/Cursor/Gemini; we are
  Claude-Code-native by deliberate product stance (see strategic note). This is
  the one axis where every peer beats us, and it is a choice, not a defect.
  Explicitly out of scope per product direction.
- ~~RTK filter breadth~~ **(closed this pass).** Two real defects were found and
  fixed, not just breadth added:
  1. The PreToolUse auto-rewrite gate (`is_supported_noisy_command`) had drifted
     narrower than the classifier, so the `database`/`cloud`/`containers`
     adapters we already shipped were *unreachable on the automatic path* —
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
  count is mostly *subcommand* breadth within the same categories we cover, and
  RTK has no auto-rewrite at all on native Windows (falls back to CLAUDE.md
  injection) and never touches Read/Grep/Glob — our PostToolUse telemetry +
  PreToolUse rewrite fire on every tool on every platform.
- ~~Skill-prose polish~~ **(closed this pass).** `claude-skills learn synthesize`
  emits a precise, agent-actionable refinement brief for every template-state
  generated skill (carrying the observed conventions as the source of truth),
  and SessionStart now surfaces that brief autonomously (no manual slash) so the
  session model upgrades the prose in the normal course of work. The agent's
  edit is protected by the content-hash no-clobber guard, and the nudge
  self-clears once the skill is refined. The binary still never calls an LLM —
  the session model that Claude Code already runs does the authoring. `learn run
  --synthesize` also collects briefs inline for a freshly generated skill.

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
- **Stop hook "JSON validation failed"** (diagnosed, not a claude-core defect).
  Transcript evidence shows the failing Stop hook is the `/goal`
  prompt-based session hook (its `command` field is the goal text), exiting 1
  with `stderr: "JSON validation failed"`. claude-core's own Stop hook
  ("Closing native session state") succeeds in 38 ms with empty stdout. The
  `/goal` evaluator routes through the user's model proxy (`ANTHROPIC_BASE_URL=
  http://localhost:8989`, every model slot mapped to `claude-opus-4-8[1M]`),
  which is not returning the structured yes/no JSON `/goal` requires. The fix is
  environmental (point `/goal` at a model that honors the structured-output
  contract, or set its evaluator model), not a code change in claude-core — our
  hooks already use exec form (`args` array), which is the documented immunity to
  the Windows shell-profile JSON-corruption failure mode.

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
