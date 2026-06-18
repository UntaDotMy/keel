<!--
Purpose: Record the 2026-06-12 gap audit of keel against nine harness/skill competitor repos not previously covered in competitive-gap-closure.md, aligned to the official harness docs.
Caller: Contributors deciding what to build, scope-out, or leave as decided non-goals after the harness-competitor sweep.
Dependencies: Prior analysis in docs/competitive-gap-closure.md and docs/native-gap-map.md; the installed skill/subagent/hook surface; the Rust CLI.
Main Functions: Verify each comparator's identity, classify the capability delta (real gap / partial / decided non-goal / already covered), and prioritize.
Side Effects: None — documentation only.
-->
# Harness-Competitor Gap Audit — 2026-06-12

**Scope.** Audit keel against nine repos the user named that the existing
[`competitive-gap-closure.md`](../../competitive-gap-closure.md) did **not** yet
cover. That doc already handles RTK, caveman, superpowers, ECC, UI/UX Pro Max,
`revfactory/harness`, and compound-engineering. This pass adds the rest and
aligns each delta to the official harness docs (skills, subagents, hooks,
plugins) as the baseline.

**Method.** Read the keel surface first (41 SKILL.md, 24 subagents, 30
hook events, ~41k LOC Rust CLI, the bootstrap skill, routing doctrine, execution
strategy). Then fetched each comparator's README/skill listing directly. Claims
below are evidence-based from those sources; star counts were ignored as a signal
(several looked templated/inflated, consistent with the prior audit's note).

**Headline.** Most named repos are either already covered at parity or are
deliberate scope boundaries. **Two surface genuine, buildable capability gaps**
that touch keel's own iron law ("understand before building"):
codebase-understanding depth and harness self-evaluation.

---

## Comparator identities (verified this pass)

| Project | Identity | License / lang | Class |
|---|---|---|---|
| `china-qijizhifeng/agentic-harness-engineering` (AHE) | Frozen base model, **evolve the harness**; evaluate→analyze→improve loop with falsifiable predictions + auto-rollback | MIT / Python | Methodology + harness optimizer |
| `arcprize/ARC-AGI-3-Agents` | Starter framework to run agents against ARC-AGI-3 **games** (perception→action loop, `FrameData`/`GameAction`) | MIT / Python | Benchmark game harness |
| `openai/symphony` | **Board-to-merged-PR** autonomous pipeline: watch Linear → spawn Codex agents per task → proof-of-work bundle → land PR | Apache-2.0 / Elixir | Autonomous orchestrator (preview) |
| `multica-ai/andrej-karpathy-skills` | One CLAUDE.md of four coding-discipline principles (Think Before Coding, Simplicity First, Surgical Changes, Goal-Driven Execution) | MIT / markdown | Behavior guidance |
| `phuryn/pm-skills` | 68 **product-management** skills across 9 plugins (discovery, strategy, GTM, pricing, OKRs) | MIT / markdown + 1 validator | Domain skill pack (non-engineering) |
| `mvanhorn/last30days-skill` | Time-bounded **recency research**: parallel social/web search → AI-judged cited brief | MIT / Python engine | Research skill (runtime-backed) |
| `addyosmani/agent-skills` | 24 lifecycle engineering skills + 4 personas + 7 slash commands + hooks; explicitly **Kiro-compatible** | MIT / shell+JS | Engineering skill pack + light harness |
| `Egonex-AI/Understand-Anything` | Multi-agent pipeline building a committable **knowledge-graph** (Tree-sitter structural + LLM semantic) with a visual dashboard | MIT / TypeScript | Codebase-comprehension harness |

---

## Gap classification

### A. Real, buildable gaps (recommend acting)

**A1 — Deterministic codebase-understanding graph (`Understand-Anything`).**
This is the strongest finding because it strikes keel's own foundation.
The iron law is "Read first / understand before building," and the artifact that
serves it today is `SYSTEM_MAP.md` — a flat, textual, top-level folder/entrypoint
map auto-refreshed by a hook — plus `code-search` (lexical) and the
`preserve-existing-flow` trace (manual, per-edit). Understand-Anything instead
produces:
- a **Tree-sitter-extracted structural graph** (imports, definitions, call sites)
  so "the same code always yields the same edges" — deterministic AST facts, not
  a model's prose summary of layout;
- a **committable JSON artifact** (`knowledge-graph.json`) teammates/agents reuse
  without re-running the pipeline;
- **diff-impact analysis** over that graph (`/understand-diff`);
- a semantic layer (LLM) on top of the deterministic skeleton.

keel's SYSTEM_MAP is shallower on every axis: no AST edges, no call graph,
no committable graph artifact, no graph-based diff impact. The `preserve-existing-flow`
gate asks the model to *manually* trace owner/producer/consumer — exactly the
edges a Tree-sitter graph would supply deterministically. **Gap is real and
on-mission.** Recommendation below.

**A2 — Falsifiable harness self-evaluation (`AHE`).**
keel has a learning loop (`runner/observation.rs` + `runner/learning.rs`:
observe behavior → confidence-scored instincts → promote to generated skills).
But it is *observation-clustering*, not *evaluation-driven*. AHE's loop is
fundamentally different and stronger on one axis: it **evaluates the harness
against tasks, analyzes failure traces to root cause, makes a predicted-impact
edit, then empirically falsifies that prediction on the next iteration and
auto-rolls-back if wrong.** keel never benchmarks its own skills/prompts
or rolls back a skill edit that made behavior worse. The `writing-skills` skill
(pressure-test prose with a subagent) is the closest analog but is manual,
per-skill, and not tied to a task-pass-rate signal with rollback. **Partial gap
with a clear, bounded improvement** (see rec A2).

### B. Genuine gaps that are likely (but not yet formally) decided non-goals

**B1 — Product-management domain (`pm-skills`, 68 skills).** keel has zero
PM skills (discovery, strategy canvas, pricing, TAM/SAM/SOM, OKRs, GTM,
positioning). This is adjacent to but outside "software delivery." It fits the
existing "curated-not-exhaustive, delivery-focused" stance, but the *boundary*
between delivery and product is not recorded as a decided non-goal. Recommend
recording the decision explicitly (it currently reads as an unexamined gap).

**B2 — Autonomous board-to-PR pipeline (`symphony`).** Watch an external tracker,
spawn agents per task unattended, bundle proof-of-work, land the PR. keel
has the *pieces* (subagents, completion gate, review gate, proof bundles, finish
flow) but deliberately keeps a human in the loop and is single-binary /
harness-native. This conflicts with the same positioning that already made
"multi-agent swarm runtime" a decided non-goal. Recommend folding it into that
existing non-goal entry rather than building it.

**B3 — Recency/social research (`last30days`).** Dedicated last-30-days
multi-platform social search with engagement-weighted ranking. keel has a
3-round research-escalation doctrine but no recency-specialized retrieval and no
social-engagement signal. Niche; runtime-backed (external APIs). Likely non-goal
for a delivery toolkit, but the research doctrine could *cite* recency as a
first-class freshness concern (it already has freshness metadata in
`research-cache`).

**B4 — Benchmark game harness (`ARC-AGI-3-Agents`).** Perception→action game loop
against a reasoning benchmark. Not a software-delivery concern at all; clearly out
of scope. Record and move on.

### C. Already covered at parity (no action)

**C1 — `andrej-karpathy-skills`.** Its four principles (Think Before Coding,
Simplicity First, Surgical Changes, Goal-Driven Execution) are **already the
verbatim four pillars** of keel's `_shared/common-discipline.md` §
Code Implementation Discipline, surfaced in the `using-keel` bootstrap and
restated in the routing doctrine. Full parity; nothing to do. (Worth noting both
trace to the same Karpathy source.)

**C2 — `addyosmani/agent-skills` (mostly).** Its lifecycle skills map onto
keel's existing surface: `test-driven-development`, `git-workflow-and-versioning`,
`security-and-hardening`, `performance-optimization`, `code-review-and-quality`,
`observability-and-instrumentation`, `planning-and-task-breakdown`,
`debugging-and-error-recovery` all have keel equivalents
(`test-driven-development`, `git-expert`, `security-and-compliance-auditor` +
`adversarial-security-review`, `react-performance-audit` + perf doctrine,
`reviewer`, `observability-and-incident-response`, `writing-plans`/`executing-plans`,
`systematic-debugging`). **Three of its skills are only partially covered** — see
A-tier-adjacent recs C2a–C2c.

---

## Partial-coverage items from `addyosmani/agent-skills`

| Their skill | keel today | Delta |
|---|---|---|
| `spec-driven-development` | `brainstorming` (capture design in brief) + `writing-plans` | No single "spec is the source of truth, generate from it" skill; the intent is split across two skills. Minor. |
| `browser-testing-with-devtools` (Chrome DevTools MCP) | `qa-and-automation-engineer` mentions Playwright/Cypress | No DevTools-MCP-based browser-inspection skill. The execution-strategy doc *does* call for Playwright for web bug repro. Minor gap; MCP-specific. |
| `context-engineering` (named skill) | `compression-discipline` + `output-economy` | Covered in substance under two skills; not named "context-engineering". Cosmetic. |
| `doubt-driven-development` (adversarial fresh-context review) | `adversarial-security-review` + `reviewer` two-stage gate | Covered for security; the general "fresh-context doubt review of any change" is close but security-scoped. Minor. |

Note: `addyosmani/agent-skills` lists **Kiro** as a supported host (`.kiro/skills/`).
Relevant to this environment but not a keel capability gap.

---

## Recommendations (prioritized)

1. **A1 — Deepen codebase understanding toward a deterministic graph.** Highest
   leverage because it serves the iron law directly. Options, smallest first:
   - **Min:** extend SYSTEM_MAP generation to emit a committable structural
     artifact (imports/definitions/call edges) per app, so
     `preserve-existing-flow` can read owner/consumer edges instead of re-deriving
     them by hand each edit.
   - **Full:** a Tree-sitter-backed `code-search`/`system-map` graph mode with
     diff-impact. This is real Rust work and should be scoped as its own brief, not
     folded into a doc pass. Decide min-vs-full before building.
2. **A2 — Add a falsifiable signal to the learn loop.** Smallest viable step:
   when `learn` promotes/edits a generated skill, record a predicted-impact note
   and re-check it against subsequent observation outcomes, rolling back a
   promotion whose predicted improvement did not materialize. This imports AHE's
   evidence→root-cause→prediction→falsify discipline into the existing loop
   without a benchmark runner. Scope as a brief.
3. **B1/B2/B3/B4 — Record decided non-goals.** Add explicit entries to
   `competitive-gap-closure.md` § Decided non-goals for: product-management domain
   (delivery-vs-product boundary), autonomous board-to-PR pipeline (fold into the
   existing swarm-runtime non-goal), recency social research, and benchmark game
   harnesses. These are currently unexamined rather than chosen.
4. **C2a–c — Optional minor skill polish.** Only if cheap: name
   `context-engineering` explicitly (alias/cross-ref), broaden
   `doubt-driven-development` beyond security, and note Chrome-DevTools-MCP browser
   testing in `qa-and-automation-engineer`. Low priority; cosmetic-to-minor.

## What this audit did NOT change

Documentation only — no code, no skills, no manifest edits. Recommendations A1
and A2 are real engineering and must each go through their own working brief +
`reviewer` pass before implementation. The decided-non-goal entries (rec 3) are a
follow-up doc edit to `competitive-gap-closure.md`, deliberately left separate so
the scope decision is reviewed, not silently merged.

---

## Closure — A1 and A2 implemented (2026-06-12)

Both real gaps are now shipped in the Rust CLI under working brief
`wb-19eba8aedb1`. Validation: `cargo test --workspace` 530+ pass / 0 fail; fmt
clean; `clippy -D warnings` clean.

**A1 — `code-graph` (new `utility/code_graph.rs`, ~700 LOC + 7 tests).**
`keel code-graph build` writes a deterministic, committable
`.understand/code-graph.json` (nodes = files + top-level symbol defs + import
specifiers; edges = resolved in-repo `imports`). `code-graph impact --changed`
returns the transitive reverse-dependency closure for review scoping. Line-based
extraction, no tree-sitter/LLM dependency, byte-identical across runs; edges only
for imports that resolve to a real file (relative JS/TS with index, relative
Python with `__init__`, Rust `mod`), so the closure stays honest. Rust/JS/TS/
Python/Go. Wired into the dispatcher and `utility/mod.rs`; documented in CLAUDE.md
§ Code Graph. Smoke-tested on the repo's own `rust/` tree (75 files, 75 edges;
impact on `code_graph.rs` → `main.rs`, `utility/mod.rs`).

**A2 — falsifiable prediction + rollback in the learn loop (`runner/learning.rs`,
+4 tests).** The generated-skill marker now records `predictedSignatures` at
promotion (the signatures whose trusted recurrence justified the skill). A new
step 4 in `run_learning_cycle` re-checks each generated skill's prediction every
cycle and **rolls back** (removes skill dir + paired generated agent) any whose
behavior no longer holds at the trust bar — the empirical-falsification half of
AHE's evidence→prediction→falsify discipline. Guards preserved: pre-A2 markers
(no prediction) are never auto-removed; a manually-refined skill (content hash ≠
marker) is never rolled back; dry-run mutates nothing. Surfaced as
`skillsRolledBack` in `learn run --json` and the text summary. No benchmark runner
added (the brief's bound) — the existing observation/decay signal is the evaluator.
