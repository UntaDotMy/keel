---
name: running-anvil
description: Run the Anvil single-root delivery loop — compile a named bar into lock+prefix+gates with one frontier call, cast N isolated workspaces from a frozen prefix, sieve with deterministic 0-LLM gates, stamp with PPT logprob verifier (EV over A–T), and bounded-loop refine only if gates still fail. The only keel delivery loop — replaces sprint/gauntlet/work fully.
when_to_use: Any non-trivial build, fix, or multi-piece delivery — ambitious artifacts, feature drops, bug fixes, hardening passes where selection under a named bar + bounded iteration matters. All new work must use Anvil; sprint/gauntlet/work are deleted legacy surfaces.
allowed-tools: Read, Grep, Glob, Bash(keel anvil:*), Bash(keel memory:*), Bash(keel review:*)
effort: medium
---

# Running Anvil

## Purpose

`keel anvil` is the **only** delivery loop. One compile, N parallel casts against a frozen byte-identical prefix, a zero-LLM sieve of deterministic gates, a probabilistic stamp that picks the winner, and a bounded, token-efficient loop that refines only if gates still fail.

No auto-git. Isolated workspaces. No web cockpit.

## Research before Anvil

Anvil does not replace research enforcement — it sits atop it.

- **Reuse gate** → check `recall`/`memory research-cache` before any external search.
- **R1 authoritative live web** (≥1 live pass for non-trivial external facts) → **R2 community/issues** → **R3 broad**; loop-back with refined terms until specific.
- **Never trust stale memory or a generic answer**, never prompt the user for verifiable facts, websearch the exact error when stuck.
- `research-enforcement` skill is the canonical reference for the above.

Anvil's `sieve` validates gates deterministically; `stamp` selects probabilistically — but neither invents facts. If compile cannot name a fetchable bar, refuse and ask (or emit 2–3 named-bar options, then stop).

## The Loop

### 0. Global continuity (automatic)

keel is host-neutral (`~/.keel`). SessionStart/PostCompact/user-prompt inject `COMPACT_BOOTSTRAP + memory_scope_summary + workspace_digest + instincts`. The job bank is **not** `{cwd}/anvil/`. It lives under `<keel-home>/memories/workspaces/<slug>/anvil/` (same per-workspace memory lane as SYSTEM_MAP). `recall` indexes `memories/` so lock+prefix+gates+report are visible from any CLI (claude/codex/pi/opencode/cursor/cmdc) that shares that home. Isolated casts use temp dirs and are deleted after the result is copied into the bank. Do not re-`ls` what recall/map already names.

### 1. Compile — goal + bar → lock + prefix + gates (1 call)

```bash
keel anvil compile --goal "CLI that pretty-prints JSON logs" --bar "jq 1.7" --files src/parse.py,src/cli.py
```

- Splits into smallest independently testable pieces.
- Prefer `critic:none` whenever a gate can decide; only `blind_ab` for taste/visual/UX.
- Emits `anvil.lock.json` (schema: `version/bar/budget/models/criteria/pieces`), `prefix.md` + `prefix.sha256` (SHA256 of static part only, ≥2048 tokens), `gates/*` per piece.
- Prefix forbids a one-site close. Isolated-cast `echo ok` is not completeness — after the change lands, run `keel code-search siblings` in the **real** workspace and handle every hit.
- `validate_lock`: named bar (not category word), `fetch` must be `cmd:|url:|file:|git:`, `allow_training_data:false` forbids `contributor/train/free-data` model ids.
- If `--bar` missing, compile proposes ≤3 named fetchable bars and stops — do not invent one silently.
- PrefixGuard renders twice; refusing if hash drifts.

Offline demo is seeded with a checked-in lock so `anvil run --dry-run` works with no keys.

### 2. Cast — N isolated builders (cheap model, parallel)

```bash
keel anvil cast --piece parse              # one piece
keel anvil cast --dry-run                  # offline: workspace + paginated-read stub
```

- Creates `tempfile.mkdtemp` per cast (copies only listed `files+gates`), prewarms one 1-token completion on the prefix (`cache_write_tokens`), frozen tool set entire job (mutating tools breaks cache), dynamic only after breakpoint.
- Per builder: the **current host CLI** does the LLM work (Read/Write/run on the isolated workspace). Anvil does not call an external model API. Write `BUILDER.md` in each workspace with the frozen prefix and tool rules. Supervisor/filter wrap `run` — deny-list `git commit/push/rebase/branch`, clip >4000 chars (head 1500 + tail 2000), never rewrite compiler errors.
- Stop when a gate goes green / retries/tokens exhausted / supervisor kills. Anvil re-runs gates (do not trust model's "passed").
- Writes `cast_i/result.json`; no shared transcript between casts.

### 3. Sieve — 0 LLM

```bash
keel anvil sieve                      # all pieces
keel anvil sieve --gates "pytest -q" # ad-hoc gates
```

- Runs each gate with timeout, `compress_output` (dedupe, clip pass wall to count, `ANVIL_CLIPPED n→m` prefix).
- Per piece: 0 greens + `critic:none` → FAIL; ≥1 green + `critic:none` → DONE (lowest-token green); `blind_ab` + ≥2 artifacts → stamp (greens first), otherwise skip stamp.

### 4. Stamp — PPT logprob verifier (batched, cached)

```bash
keel anvil stamp           # default K=1, k=1, G=20, C=3
keel anvil stamp --strict  # K=2, k=2
```

- Payload: diffs vs start + last 80 gate lines + help/screenshot hashes — never whole repo.
- Local PPT/EV over survivors (A–T, φ(A)=1…T=20, Bradley–Terry). No external logprob API. Skip stamp when `critic:none` and ≥1 green, or when survivors < 2. Winner copied to the global bank `anvil_out/`.
- PPT: ring → rank `w_i/c_i` → top-k pivots → extra `N+k(N−k)+C(k,2)` with `N=3,k=1` ring-reuse via `(min,max)` pair cache; env router `ANVIL_COMPILE/CAST/STAMP/LOOP_MODEL`.
- Winner copied to `anvil_out/`.

### 5. Loop — bounded refinement *only if gates still fail*

```bash
keel anvil loop            # max_iterations:20 ∈[5,50], min_improvement:0.05, wall 300s
```

- Structured state (objective/constraints/decisions/unresolved/next-action) as key-value/diff, not transcript replay.
- Context shrinks: progressive retrieval (200-line window), top-3-4 evidence ranking, working-memory hygiene.
- Surgical line-range edits, never whole-file rewrites.
- Termination: all gates pass → DONE; `improvement < threshold` → STOP; `iterations ≥ max` → STOP; `wall ≥300s` → STOP; repeated identical tool calls / new errors without progress → STOP.
- Logs `loop_iterations + improvement_delta + final_gate_status` into `anvil.report.json`.

### 6. Run — thin orchestrator

```bash
keel anvil run --dry-run              # offline: lock + prefix + sieve + local PPT
keel anvil run                        # compile→cast→sieve→stamp/loop as needed; host CLI is the LLM
```

Metrics per job: `cache_hit_ratio tokens_uncached/cached critic_calls gate_pass_rate stamp_used winner_id loop_iterations improvement_delta`.

## Error path — research, not guessing

On any error: `web_search` the exact error text → refined `recall`/docs → retry with corrected prompt/flags. Do not accept a generic answer; rewrite the query and search again until specific. `context_brief`/`system_map` before code claims; `skill_get` before domain work.

## Help

```bash
keel anvil help
keel anvil compile --help
keel anvil sieve --help
```
