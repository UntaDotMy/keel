# keel stats: unified dashboard

## What was built
`keel stats [--days N] [--top N] [--claude-home <path>] [--workspace-root <path>] [--json]` — a single
read-only dashboard answering "what has keel done, what did it save, what did it catch, what did it
help with". It aggregates the existing readers instead of re-parsing:

- Token savings + savings %, commands observed/compacted, top space-saving commands: reuses
  `utility::gain::parse_gain_summary` over the compaction event log.
- Top tools by time: reuses `runner::telemetry::{read_rows, aggregate_rows}` over tool-timings JSONL.
- Gate/enforcement activity: reuses `runner::hook_lifecycle::gate_status_rows()` (single source of
  gate labels/dirs) and sums each gate's per-session counter files under `<claude_home>/state/`.
- Recall/memory index health: reuses `utility::recall::recall_status_snapshot` (document count,
  last-indexed ms).
- Active sprint/work progress: reuses `utility::sprint::open_stories_for_workspace`.

Default output is a compact human table that leads with the headline (`N tokens saved (X% of Y)`);
`--json` emits the machine payload. Both surfaces verified with a smoke run.

Design note: `stats` resolves one `--claude-home` and threads it through every axis, so gain and
telemetry are read from the resolved home rather than the process env home (a small correctness
improvement over calling the env-driven readers, and what makes the command hermetically testable).
Parsing itself stays single-sourced in the owning modules; only path construction is local.

## Files changed
- `rust/crates/keel/src/utility/stats.rs` (new): `run_stats_command`, `collect_snapshot`,
  `gain_summary_from_home`, `telemetry_day_files`, `gate_activity`, `StatsSnapshot` render/JSON,
  and 5 hermetic unit tests.
- `rust/crates/keel/src/utility/mod.rs`: `pub mod stats;` + `pub use stats::run_stats_command;`.
- `rust/crates/keel/src/utility/gain.rs`: widened `GainSummary`/`GainCommandSummary`/
  `parse_gain_summary` to `pub(crate)` (additive visibility only; no behavior change).
- `rust/crates/keel/src/commands.rs`: dispatch arm.
- `rust/crates/keel/src/mcp/tools.rs`: `stats` added to `MCP_TOOL_NAMES`, `mcp_tool_handler`,
  `tools_list_catalog`, and new `tool_stats` (in-process, JSON default true for agents).
- `README.md`: added `` `stats` `` to the MCP server tool row.
- `rust/crates/keel/src/help_operator.txt`: added the `stats` usage line.

## Dispatch + MCP wiring (file:line)
- CLI dispatch: `rust/crates/keel/src/commands.rs:192` (`"stats" => utility::run_stats_command(...)`).
- MCP tool name: `rust/crates/keel/src/mcp/tools.rs` `MCP_TOOL_NAMES` entry `"stats"` (~line 731).
- MCP handler table: `"stats" => tool_stats` in `mcp_tool_handler` (~line 779).
- MCP catalog entry: `"name": "stats"` block in `tools_list_catalog` (~line 597).
- MCP handler fn: `fn tool_stats` next to `tool_observe` (~line 2455).

## Test results
- `cargo build -p keel`: PASS.
- `cargo test -p keel --lib`: PASS — 989 passed, 0 failed (5 new stats tests:
  `days_cutoff_rolls_back_window`, `gate_activity_empty_when_no_state`,
  `gate_activity_sums_session_counters`, `json_payload_carries_all_axes`,
  `snapshot_renders_headline_and_axes`).
- `cargo test --workspace`: PASS — all suites green, 0 failures. Includes
  `doc_parity_test`: `every_mcp_tool_is_listed_in_readme`, `mcp_tool_count_matches_documentation`,
  and `every_help_advertised_flag_is_read_by_the_code` all pass.
- `cargo clippy -p keel --lib`: no warnings.

## Notes / not finished
- Everything requested is complete. No existing behavior of `gain`/`session`/`observe`/`telemetry`
  was changed (stats is additive/read-only).
- One issue found and fixed during development: an early draft of the stats tests mutated
  `CLAUDE_TARGET_OVERRIDE` from a module-local lock, racing the sprint tests' own env mutation
  (process-wide env, separate locks). Resolved by making `collect_snapshot` and its helpers fully
  home-driven (no env reads), so the tests pass an explicit temp home and never touch the shared
  env var. This is why `stats` threads `claude_home` into gain/telemetry rather than calling the
  env-driven reader wrappers.
