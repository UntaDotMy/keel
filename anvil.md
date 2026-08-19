# BUILD ANVIL (RUST TOKEN-EFFICIENT LOOP EDITION)

You are implementing **Anvil** in pure **Rust** (no Python, no heavy framework crates). This document is the complete specification. Build exactly this. Do not invent a raw Gauntlet clone, multi-round critic chat, HTML workbench, or auto-git agent.

Anvil casts $N$ candidate artifacts in parallel using async tasks (`tokio`), filters losers via deterministic gates (Sieve), selects a winner using expectation-based logprob pairwise verifiers (Stamp via PPT), and conditionally runs a bounded refinement loop if gates fail.

---

## 0. Non-Negotiable Product Rules

* **Language & Stack:** Pure **Rust** (Edition 2021+). Stack: `tokio` (async/runtime), `clap` (CLI), `serde` / `serde_json` (state/serialization), `reqwest` (HTTP client), `tempfile` (isolated workspaces), `sha2` (prefix verification), `regex`. Minimal dependencies. No agent OS or LLM frameworks.
* **Bounded Loop:** Every loop has a hard max iterations cap (default: 20, range: 5–50), a minimum improvement threshold (default: 0.05 / 5%), and explicit termination conditions defined before starting.
* **No Auto-Git:** Isolated workspace temp directories (`tempfile::tempdir()`). Tests and casts **must never** commit, push, rebase, or create branches.
* **Privacy & Data Protection:** Set `allow_training_data: false` in config. Refuse model IDs matching `/(contributor|train|free-data)/i` for proprietary work.
* **Sacred Cache Prefix:** Static prefix must be byte-identical across all casts and stamps within a job (`prefix.sha256` verified).
* **Fetchable Named Quality Bar:** Requires a named, fetchable, comparable quality bar. If missing at compile time, prompt via structured JSON Q&A with 2–3 options and halt until chosen.
* **First-Class Metrics:** Print at job end and write to `anvil.report.json`: `cache_hit_ratio`, `tokens_uncached`, `tokens_cached`, `critic_calls`, `gate_pass_rate`, `stamp_used`, `winner_id`, `loop_iterations`, `improvement_delta`.
* **Delivery:** Rust library crate (`anvil`) + binary CLI (`anvil`).
* **Keel host integration:** `keel anvil` stores the job bank under
  `<keel-home>/memories/workspaces/<slug>/anvil/` (same lane as SYSTEM_MAP).
  Isolated casts use temp directories and are deleted after the result is
  copied into that bank. LLM work inherits the current host CLI; do not add
  an external model client.

---

## 1. System Architecture & CLI Subcommands

```
                 ┌──────────────┐
  goal + bar ───►│   COMPILE    │  1 frontier LLM call (or Q&A then 1 call)
                 │  (once)      │
                 └──────┬───────┘
                        ▼
              anvil.lock.json
              prefix.md          ← SHA256 frozen
              gates/*            ← executable scripts / cargo test / goldens
                        │
          ┌─────────────┼─────────────┐
          ▼             ▼             ▼
       CAST 1        CAST 2        CAST 3     cheap model, parallel (tokio)
       (ws1)         (ws2)         (ws3)     shared prefix, isolated temp dirs
          │             │             │
          └─────────────┼─────────────┘
                        ▼
                 SIEVE (0 LLM)
              tests / goldens / SSIM
                        │
              0 survivors → piece fail
              1 survivor + critic:none → DONE
              2+ or critic:blind_ab →
                        ▼
                   STAMP (PPT)
              logprob expectation
              pick winner
                        │
                        ▼
              LOOP (if gates fail)
              bounded iterations
              improvement delta check
                        │
                        ▼
              promote winner files
              write anvil.report.json

```

| Subcommand | Role | LLM Usage |
| --- | --- | --- |
| `anvil compile` | Goal $\rightarrow$ `anvil.lock.json`, `prefix.md`, `gates/`, `prefix.sha256` | 1 call (or Q&A then 1 call) |
| `anvil cast` | Runs $N$ builders in parallel async tasks | $N$ parallel cheap calls |
| `anvil sieve` | Runs deterministic test gates via `tokio::process` | 0 LLM calls |
| `anvil stamp` | Probabilistic Pivot Tournament over survivors | Pairwise stamp calls ($K=1$) |
| `anvil loop` | Bounded refinement if winner fails gates | Bounded cheap model calls |
| `anvil run` | Thin orchestrator (`compile` $\rightarrow$ `cast` $\rightarrow$ `sieve` $\rightarrow$ `stamp` $\rightarrow$ `loop`) | No extra LLM orchestrator |

---

## 2. Algorithms & Technical Specifications

### 2.1 LLM-as-a-Verifier (Kwok et al., arXiv:2607.05391)

Instead of discrete 1–5 scoring, compute the expected score over token logprobs on a 20-letter scale ($A=1 \dots T=20$, mapped as $\phi(A)=1 \dots \phi(T)=20$):

$$R(x, \tau) = \frac{1}{C \cdot K} \sum_{c=1}^{C} \sum_{k=1}^{K} \sum_{g=1}^{G} p_\theta(v_g \mid x, c, \tau) \cdot \phi(v_g)$$

1. Min-max normalize expected scores $R(x, \tau)$ into $[0, 1]$.
2. Convert pairwise score difference to preference probability via Bradley–Terry:

$$P(\tau_i \succ \tau_j \mid x) = \frac{1}{1 + \exp(-(R_i - R_j))}$$

* **Production Defaults:** Granularity $G=20$, Samples $K=1$, Criteria $C=3$, Candidates $N=3$, Pivots $k=1$. (`--strict` sets $K=2, k=2$).
* **3 Criteria ($C=3$):** `specification` (satisfies lock requirements), `output` (matches expected fixtures/format), `errors` (free of failure signals/logs).
* **Batched Single-Call Stamp:** Evaluate all 3 criteria in one call emitting:
```xml
<spec_score_A>LETTER</spec_score_A> <spec_score_B>LETTER</spec_score_B>
<out_score_A>LETTER</out_score_A>   <out_score_B>LETTER</out_score_B>
<err_score_A>LETTER</err_score_A>   <err_score_B>LETTER</err_score_B>

```


* **Logprob Extraction:** Sum logprob mass over tokens $A$–$T$. If top logprobs are missing from API response, renormalize over observed tokens.
* **Fallback:** If logprobs are unavailable from the provider, parse discrete letters, map to 1–20, and log `stamp.mode = discrete_fallback`.

### 2.2 Probabilistic Pivot Tournament (PPT)

1. **Ring Pass:** Uniformly sample a Hamiltonian cycle $\gamma$ over candidates $\{1 \dots N\}$. Evaluate adjacent pairs $(\gamma_t, \gamma_{t+1 \pmod N})$.
2. **Pivot Selection:** Rank candidates by mean preference score $w_i / c_i$. Select top-$k$ candidates as pivot set $P$ (default $k=1$).
3. **Pivot Rounds:** Score every non-pivot against each pivot, and pivots against pivots.
4. **Soft Score Updates:**

$$w_i \leftarrow w_i + P(\tau_i \succ \tau_j), \quad w_j \leftarrow w_j + (1 - P(\tau_i \succ \tau_j)), \quad c_i \leftarrow c_i + 1, \quad c_j \leftarrow c_j + 1$$



Winner is $\text{argmax}(w_i / c_i)$.
5. **Optimization ($N=3, k=1$):** Reuse the 3 ring-pass pairwise results; skip redundant pair re-evaluations.

### 2.3 Cache Law & Prefix Management (Lumer et al., arXiv:2601.06007)

* **Structure:** Put all static content first (role, tool schemas, bar dossier, rubrics, scoring protocol). Place explicit cache breakpoint tag (`cache_control: ephemeral` on Anthropic; standard prefix breakpoint on OpenAI/Gemini). Place dynamic content (piece ID, diffs, gate logs) strictly after the breakpoint.
* **Padding Requirement:** Static prefix must be padded with stable documentation (bar dossier) to $\ge 2048$ tokens ($\ge 4096$ tokens for Gemini target models).
* **Forbidden in Prefix:** Timestamps, UUIDs, session IDs, tool trace results, mutating tool lists, file contents of dynamic pieces.
* **Execution:** JSON serialized with `serde_json` (keys sorted deterministically), UTF-8, `\n` newlines. Run a 1-token prewarm completion on the prefix before parallel casts. Validate `prefix.sha256` consistency.

### 2.4 Deterministic Supervisor & Tool Output Compression

* **LLM-Free Supervisor Rules:**
* Skip cast/stamp if gate is already green.
* Truncate tool outputs exceeding `ANVIL_TOOL_MAX_CHARS` (default 4000): retain head 1500 chars + tail 2000 chars + line count notice.
* Kill cast if `builder_retries` or `max_tokens_cast` is exceeded.
* Skip stamp if $<2$ survivors remain or if 1 survivor has `critic: none`.


* **Tool Compression Protocol (In-Process):**
* Preserve command, exit code, first error block, and failing test names exactly.
* Deduplicate repeated lines and clip passing test suites to a single summary line.
* Prefix clipped payloads with `ANVIL_CLIPPED n_bytes->m_bytes`. Optional `--tool-filter=rtk` if binary exists.



---

## 3. Configuration & Lock Schema

### `anvil.lock.json`

```json
{
  "version": 1,
  "goal": "CLI that pretty-prints JSON logs",
  "bar": {
    "name": "jq 1.7",
    "fetch": "cmd:jq --version",
    "compare": "stdout+exit",
    "notes": "Match jq on listed fixtures"
  },
  "budget": {
    "n_casts": 3,
    "k_pivots": 1,
    "critic_k": 1,
    "granularity": 20,
    "builder_retries": 2,
    "max_tokens_cast": 80000,
    "max_tokens_stamp": 40000,
    "max_tokens_loop": 100000,
    "max_tool_chars": 4000,
    "max_iterations": 20,
    "min_improvement_threshold": 0.05
  },
  "models": {
    "compile": "frontier-id",
    "cast": "cheap-id",
    "stamp": "logprob-capable-id",
    "loop": "cheap-id",
    "allow_training_data": false
  },
  "criteria": ["specification", "output", "errors"],
  "pieces": [
    {
      "id": "parse",
      "files": ["src/parse.rs"],
      "gates": ["cargo test test_parse"],
      "critic": "none"
    },
    {
      "id": "ux",
      "files": ["src/cli.rs"],
      "gates": ["cargo test test_cli_help"],
      "critic": "blind_ab",
      "ab": ["./target/debug/app --help", "jq --help"]
    }
  ]
}

```

### Validation Rules

* `bar.name` required; non-generic. `bar.fetch` must start with `cmd:`, `url:`, `file:`, or `git:`.
* `critic` must be `none` or `blind_ab`. `critic: none` pieces require $\ge 1$ gate.
* `n_casts` default 3 (min 2 if `blind_ab`, else min 1). `k_pivots` < `n_casts`.
* `allow_training_data: false` rejects model names matching `/(contributor|train|free-data)/i`.
* `max_iterations` default 20 (range 5–50). `min_improvement_threshold` default 0.05.

---

## 4. Execution Stages

### 4.1 Compile (`anvil compile`)

* Generates `anvil.lock.json`, `prefix.md`, `gates/`, and `prefix.sha256`.
* Uses 1 frontier model call. If the quality bar is ambiguous, emit structured JSON Q&A with up to 3 named-bar options, then exit and wait for input.

### 4.2 Cast (`anvil cast`)

* Creates $N$ isolated directories (`tempfile::TempDir`). Prewarms prompt cache once per job.
* Launches $N$ parallel builder tasks using `tokio::spawn`.
* Available builder tools: `read_file`, `write_file`, `run` (scoped to workspace directory; wrapped by Supervisor/filter). No git operations allowed.
* If gates fail during cast, up to `builder_retries` (max 2) are permitted, passing back only the truncated failing gate output.

### 4.3 Sieve (`anvil sieve`)

* Runs gate commands deterministically via `tokio::process::Command` with timeouts.
* If `critic: none`: 0 passes $\rightarrow$ fail piece; $\ge 1$ pass $\rightarrow$ pick passing cast with lowest token count.
* If `critic: blind_ab`: Pass green survivors to Stamp. Skip Stamp if $<2$ survivors remain.

### 4.4 Stamp (`anvil stamp`)

* Loads artifacts as minimal payloads (unified diffs, last 80 lines of gate logs, stdout/screenshots).
* Runs PPT using expected score verifier on candidate pairs.
* Promotes winner files to output path (`anvil_out/`).

### 4.5 Bounded Refinement Loop (`anvil loop` / `refine_loop` module)

Invoked only if the Stamp winner fails deterministic gates. Note: The module is named `refine_loop` or `loop_engine` internally to avoid collision with Rust's reserved `loop` keyword.

```
                    ┌─────────────────────────┐
                    │  Winner Workspace Init  │
                    └────────────┬────────────┘
                                 │
                                 ▼
                     ┌───────────────────────┐
                     │ Assemble Context State│
                     │  (Objective/Diffs)    │
                     └───────────┬───────────┘
                                 │
                                 ▼
                     ┌───────────────────────┐
                     │ Invoke Reasoning Model│
                     └───────────┬───────────┘
                                 │
                                 ▼
                     ┌───────────────────────┐
                     │ Execute Surgical Edits│
                     └───────────┬───────────┘
                                 │
                                 ▼
                    ┌──────────────────────────┐
                    │ Evaluator Gate Check     │
                    └────────────┬─────────────┘
                                 │
         ┌───────────────────────┼───────────────────────┐
         ▼                       ▼                       ▼
  [All Gates Pass]       [Delta < Threshold OR]     [Unrecoverable Error]
         │               [Max Iter / 300s Cap]           │
         ▼                       ▼                       ▼
       DONE                    STOP                    STOP

```

* **Structured State:** Maintains key constraints, active decisions, unresolved issues, and next actions.
* **Context Engineering:** Context contracts between turns by purging resolved tool output.
* **Surgical Operations:** Uses line-level replacement tools instead of full-file rewrites. Reads files using 200-line paginated windows.
* **Termination Triggers:**
1. All gates pass ($\rightarrow$ `DONE`).
2. Score/gate improvement delta $< 0.05$ ($\rightarrow$ `STOP`).
3. Iteration count reaches `max_iterations` ($\rightarrow$ `STOP`).
4. Wall-clock duration exceeds 300 seconds ($\rightarrow$ `STOP`).
5. Agent repeats identical tool calls with identical arguments ($\rightarrow$ `STOP`).



---

## 5. Rust Project Layout & Environment Configuration

### Workspace & Module Structure

```text
anvil/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs            # CLI entrypoint (clap parser)
│   ├── lib.rs             # Core engine exports
│   ├── cli.rs             # Command line arguments definition
│   ├── lock.rs            # Lock schema parsing & validation (serde)
│   ├── prefix.rs          # Prefix construction, padding & SHA256 hashing
│   ├── compile.rs         # Compile stage handler
│   ├── cast.rs            # Async parallel builder stage (tokio)
│   ├── sieve.rs           # Zero-LLM gate verification stage
│   ├── stamp.rs           # Pairwise verifier & PPT tournament engine
│   ├── refine_loop.rs     # Bounded refinement loop (loop_engine)
│   ├── supervisor.rs      # LLM-free enforcement rules
│   ├── filter.rs          # Tool output compression logic
│   ├── cache.rs           # Provider header building & cache prewarming
│   ├── report.rs          # anvil.report.json metrics builder
│   ├── workspace.rs       # Isolated temp directory manager
│   └── providers/
│       ├── mod.rs
│       ├── base.rs        # API Client traits
│       ├── openai.rs      # OpenAI-compatible API client (supports logprobs)
│       └── anthropic.rs   # Anthropic API client (ephemeral cache headers)
└── tests/
    ├── test_lock_schema.rs
    ├── test_prefix_guard.rs
    ├── test_ev_score.rs
    ├── test_ppt.rs
    ├── test_supervisor.rs
    ├── test_filter.rs
    ├── test_sieve.rs
    ├── test_refine_loop.rs
    └── test_no_git.rs

```

### Environment Routing Parameters

```bash
ANVIL_COMPILE_MODEL=      # Frontier model (e.g., gpt-5.5 / claude-3-7-sonnet)
ANVIL_CAST_MODEL=         # Cheap coder (e.g., gpt-4o-mini / deepseek-coder)
ANVIL_STAMP_MODEL=        # Logprob-capable mid/frontier model
ANVIL_LOOP_MODEL=         # Cheap/efficient coder model
ANVIL_API_BASE=           # OpenAI-compatible endpoint base URL
ANVIL_API_KEY=            # Endpoint authorization key
ANVIL_ALLOW_TRAINING_DATA=false

```

---

## 6. Required Rust Test Suite (`tests/*.rs`)

* `test_lock_schema.rs`: Validates `serde` deserialization, default values, and validation errors for non-conforming lock files.
* `test_prefix_guard.rs`: Asserts `prefix.sha256` immutability and rejects unpadded prefixes.
* `test_ev_score.rs`: Verifies logprob expectation math correctly resolves ranking ties present in discrete scoring.
* `test_ppt.rs`: Validates candidate selection ranking accuracy and asserts pair count bounds $\le N + k(N-k) + C(k,2)$.
* `test_supervisor.rs`: Tests command truncation, token limits, and premature termination rules.
* `test_filter.rs`: Asserts log clipping preserves test failure names, errors, and prefixes `ANVIL_CLIPPED`.
* `test_sieve.rs`: Verifies zero-LLM gate execution logic and artifact filtering.
* `test_refine_loop.rs`: Tests iteration capping, improvement delta termination, and timeout limits.
* `test_no_git.rs`: Scans workspace code base to guarantee zero git subcommands or repository modifications occur during job execution.