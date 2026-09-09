# Phase 12 MCP Catalog Profile and Real Benchmarking

## Delivered contract

Phase 12 delivers a high-efficiency tiered Model Context Protocol (MCP) catalog profile and real, reproducible competitor benchmarking backed by runtime token measurements:

1. **Configurable MCP Catalog Profile (`McpCatalogProfile`)**:
   - Implemented `McpCatalogProfile` in `rust/crates/keel/src/mcp/mod.rs` with `Tiered` (default) and `Full` profiles.
   - Configurable via the `KEEL_MCP_CATALOG_PROFILE` environment variable (accepting `full` or `all` to opt into the legacy monolithic catalog).
   - Separated the 37 MCP tools into:
     - **Eager Core Tools (17 tools)**: `recall`, `system_map`, `run_command`, `command_output`, `command_kill`, `recall_status`, `skill_route`, `skill_get`, `skill_list`, `memory_status`, `brief_list`, `brief_get`, `brief_create`, `system_map_refresh`, `context_brief`, `cli`, `anvil`.
     - **Deferred Specialist Tools (20 tools)**: `review`, `git_workflow`, `memory`, `gain`, `raw`, `config_audit`, `skill_lint`, `telemetry`, `session`, `doctor`, `code_search`, `code_index`, `flow`, `code_graph`, `learn`, `observe`, `rewrite`, `skill_eval`, `design_intelligence`, `stats`.

2. **Wire Discovery Footprint Reduction**:
   - `handle_tools_list()` dynamically slims and filters the advertised tool schemas based on the active profile.
   - In default `Tiered` mode, only the 17 eager core tools are serialized in the `tools/list` JSON-RPC response frame.
   - Token weight of `mcp.tools_list.catalog` dropped from 2,902 tokens down to **1,198 tokens** (**58.72% reduction**, saving **1,704 tokens** on every agent session initialization).
   - Framed payload comfortably remains well within the stdio frame ceiling (8,515 bytes serialized, providing over 15KB of headroom under the 24KB ceiling).

3. **Complete Dispatch Parity**:
   - `dispatch_mcp_tool` and `is_known_mcp_tool` continue to evaluate against all 37 tools in `MCP_TOOL_NAMES`.
   - Any client calling a deferred tool (such as `review`, `code_search`, `flow`, or `stats`) via `tools/call` executes immediately without failure or missing handler errors.
   - The native `cli` MCP tool remains available to execute any Keel CLI subcommand dynamically.

4. **Fixed-Context Ledger and Budget Updates**:
   - Updated `ratified_budget("mcp.tools_list.catalog")` in `rust/crates/keel/src/utility/fixed_context.rs` to **1,318 tokens** (`ceil(1,198 * 1.10)`).
   - Updated baseline metadata in `docs/fixed-context-ledger.md` documenting the tiered catalog profile (37 total tools: 17 eager and 20 deferred).
   - Updated `rust/crates/keel/tests/fixed_context_budget_test.rs` to verify that `eagerToolCount == 17`, `deferredToolCount == 20`, and `toolCount == 37`.

5. **Reproducible Competitor Benchmarking Suite (`bench/competitor/`)**:
   - Created dedicated benchmark artifacts in `bench/competitor/`:
     - `bench/competitor/README.md`: Explains the benchmarking doctrine, `o200k_base` TokenMeter methodology, and comparative metrics against baseline and competitor approaches.
     - `bench/competitor/eval-compaction-report.json`: Direct runtime output of `keel eval --json`.
     - `bench/competitor/comparative-compaction.json`: Structured comparative dataset across 7 real toolchain fixtures.
   - Replaced static/prose benchmark claims with verifiable runtime evidence:
     - `cargo test --workspace` (pass): **79.26%** token reduction (405 raw down to 84 compact tokens).
     - `cargo test --workspace` (fail): **26.79%** token reduction (713 raw down to 522 compact tokens) while preserving diagnostic stack traces and assertion points.
     - `cargo clippy`: **48.41%** token reduction (157 raw down to 81 compact tokens).
     - Overall suite savings: **27.28%** reduction (2,218 raw down to 1,613 compact tokens).
     - Break-even protection: 0 tokens inflated across passthrough fixtures (`ripgrep-search`, `npm-install`, `kubectl-get-pods`).
   - Updated `docs/benchmark-suite.md` and `docs/benchmark-comparison-scorecard.md` to reference `bench/competitor/` and `keel eval`.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Implement tiered MCP catalog profile | `McpCatalogProfile` in `mcp/mod.rs` and `mcp/tools.rs` filtering `tools/list` to 17 eager tools by default. |
| Maintain direct execution parity for all tools | Unit tests `eager_and_deferred_tool_counts_sum_to_all_tools` and `deferred_tools_are_directly_dispatchable` prove all 37 tools dispatch. |
| Context ledger snapshot reports real eager/deferred counts | `ToolsListContextSnapshot` reports 37 total, 17 eager, 20 deferred, 1,198 catalog tokens. |
| Update ratified budget with 10% headroom | `ratified_budget("mcp.tools_list.catalog")` updated to 1,318 tokens in `fixed_context.rs`. |
| Pass fixed context budget tests | `cargo test --test fixed_context_budget_test` passed 5/5 tests. |
| Maintain doc parity contracts | `cargo test --test doc_parity_test` (27/27) and `competitor_doc_parity_test` (2/2) passed. |
| Establish competitor benchmark suite | `bench/competitor/` populated with `README.md`, `eval-compaction-report.json`, and `comparative-compaction.json`. |
| Sync benchmark documentation | `docs/benchmark-suite.md` and `docs/benchmark-comparison-scorecard.md` updated with measured token metrics. |
| Zero compiler and clippy warnings | `cargo clippy --workspace --all-targets -- -D warnings` clean (0 warnings). |

## Verification and quality ladder

- **Cargo Test (MCP Lib)**: 161 passed, 0 failed.
- **Fixed Context Budget**: 5 passed, 0 failed (`mcp.tools_list.catalog` verified at 1,198 actual / 1,318 budget).
- **Doc Parity Contract**: 27 passed, 0 failed (`every_mcp_tool_is_listed_in_readme` and `mcp_tool_count_matches_documentation` verified).
- **Competitor Parity**: 2 passed, 0 failed (verified no stale competitor claims).
- **Host Adapter Contracts**: 12 passed, 0 failed (`bun test tests/host-adapter-contracts.test.ts`).
- **Clippy Validation**: 0 warnings across all workspace targets with `-D warnings`.

## Boundaries and non-goals

- Did not remove any of the 37 MCP tools; full backward compatibility and capability breadth are preserved.
- Kept all git commits strictly local; zero remote push until all 13 phases are completed.
