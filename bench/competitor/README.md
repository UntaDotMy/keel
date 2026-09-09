# Competitor Benchmarking and Compaction Evaluation Suite

This directory contains reproducible evaluation corpora and benchmark measurements comparing Keel's compaction proxy and tiered MCP catalog profile against baseline and competitor approaches.

## Measurement Doctrine

Every token count in this benchmark suite is produced at runtime using the genuine `o200k_base` tokenizer (`tiktoken_rs`). No hardcoded estimates, synthetic heuristic guesses, or character-to-token approximations are accepted.

Measurements cover:
1. End-to-end command output compaction (`keel eval --json`).
2. Break-even guarantee verification (zero negative savings / zero token inflation).
3. MCP catalog wire payload profile efficiency (Tiered vs Full profile).

## 1. Command Compaction Benchmark (`keel eval`)

The compaction pipeline runs real adapters through `classify_command -> registry.best_match -> adapter.compact -> render_compact_result`.

Reproduction command:
```powershell
cargo run --locked --bin keel -- eval --json
```

### Measured Fixture Results

| Fixture | Command | Raw Tokens | Compact Tokens | Saved Tokens | Savings | Diagnostic Integrity |
|---|---|---:|---:|---:|---:|---|
| `cargo-test-pass` | `cargo test --workspace` | 405 | 84 | 321 | 79.26% | Preserves test status and summary, collapses passed test lists |
| `cargo-test-fail` | `cargo test --workspace` | 713 | 522 | 191 | 26.79% | Preserves exact failure assertions, backtraces, and error paths |
| `git-status` | `git status` | 240 | 223 | 17 | 7.08% | Retains tracked and untracked changes, strips helper advice |
| `cargo-clippy-warnings` | `cargo clippy` | 157 | 81 | 76 | 48.41% | Retains diagnostic codes and suggestions, removes ASCII decoration |
| `ripgrep-search` | `rg TokenMeter --line-number` | 279 | 279 | 0 | 0.00% | Break-even guard passes raw text without wrapper overhead |
| `npm-install` | `npm install` | 122 | 122 | 0 | 0.00% | Passthrough preserves raw output when compaction offers no gain |
| `kubectl-get-pods` | `kubectl get pods` | 302 | 302 | 0 | 0.00% | Passthrough preserves tabular columns when below break-even |
| **Total** | **7 Fixtures** | **2,218** | **1,613** | **605** | **27.28%** | **Overall compaction savings across diverse toolchain commands** |

### Comparison Against Competitor Approaches

- **Raw Passthrough (Baseline)**: Retains all 2,218 tokens with high noise, crowding agent context windows and pushing working memory toward compaction thresholds prematurely.
- **Naive Head/Tail Truncation**: Arbitrarily chops long outputs (e.g. first 20 lines and last 20 lines). On test failures like `cargo-test-fail`, failure details located in the middle of standard output are lost, breaking agent root-cause debugging.
- **Keel Semantic Proxy**: Compresses high-noise passing lines while preserving failure stack traces, assertion details, and error codes. If compaction cannot save tokens, the break-even guard passes raw bytes directly, preventing negative savings.

## 2. MCP Catalog Wire Footprint (Tiered vs Full)

Keel provides 37 Model Context Protocol tools. To prevent large initial discovery payloads from exhausting host context windows and triggering transport timeouts, Keel implements a tiered catalog profile:

- **Tiered Profile (Default)**: Advertises 17 core eager tools in `tools/list`. Consumes **1,198 tokens** (ratified budget: 1,318 tokens).
- **Full Profile (`KEEL_MCP_CATALOG_PROFILE=full`)**: Advertises all 37 tools in `tools/list`. Consumes **2,902 tokens** (ratified budget: 3,193 tokens).
- **Token Reduction**: **1,704 tokens saved per session start (58.72% reduction)**.
- **Execution Parity**: All 37 tools remain registered, valid, and directly dispatchable through `tools/call` in both profiles.

Reproduction command:
```powershell
cargo run --locked --bin keel -- stats --json --workspace-root .
```

## Summary Artifacts

- Detailed fixture run output: [`eval-compaction-report.json`](./eval-compaction-report.json)
- Comparative analysis metrics: [`comparative-compaction.json`](./comparative-compaction.json)
