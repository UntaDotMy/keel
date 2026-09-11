<!--
Purpose: Record the repository cleanup performed for the production context gateway work, per the implementation plan's cleanup-report requirement.
Caller: Operators and reviewers validating that dead and duplicate material was removed rather than documented.
Dependencies: The gateway implementation commits on task/production-context-gateway-v4.
Main Functions: None; this file is a report.
Side Effects: None.
-->
# Gateway Cleanup Report

Scope: the production context gateway hardening on `task/production-context-gateway-v4`,
compared against `origin/main`. Everything below was verified by running the named
command, not by reading and assuming.

## Removed dependencies

None. Every declared non-dev dependency is referenced by its crate's source.

- Check: `cargo metadata --locked --no-deps --format-version 1` cross-referenced
  against each crate's `src`, `tests`, `benches`, `examples`, and `build.rs`.
- Result: `keel` 11 declared / 0 unreferenced, `keel-flow` 2 / 0,
  `keel-platform` 0 / 0, `keel-professionaltext` 1 / 0, `keel-releaseassets` 0 / 0.
- No crate declares a `[features]` section, so there is no unused feature to remove.

## Removed files

None. No orphan source, fixture, asset, or template was found.

## Removed modules

None.

## Removed CLI paths

None. `cargo test --locked -p keel --test doc_parity_test` keeps
`every_help_advertised_flag_is_read_by_the_code` green, so no advertised flag is unread.

## Removed config keys

None.

## Removed code

Each removal below had zero non-test consumers, verified by `git grep` before deletion.
The build and `cargo clippy --workspace --all-targets -- -D warnings` stay clean after
each step, which proves nothing else reached them.

| Removed | File | Why it was safe |
|---|---|---|
| `resolve_skill_for_prompt_with_catalog` | `utility/skill_match.rs` | Zero references anywhere. `resolve_skill_selection` is the owner the dispatcher and evals call. |
| `paginated_read` | `utility/anvil/workspace.rs` | Zero references. It was labelled a compatibility helper with no consumer. |
| `search` | `utility/workspace_index.rs` | Zero production references; it only forwarded to `search_filtered`. The three tests that used it now call `search_filtered(.., None)` directly. |
| `reindex_after_write` | `utility/recall.rs` | Zero production references; it only forwarded to `reindex_after_write_paths` with an empty slice. Its contract test now calls the owner directly. |
| `run_hook_command_with_stdin` re-export | `runner/mod.rs` | Tests reach it through `hook_lifecycle`; the re-export had no consumer. The crate root now exposes only `run_hook_command`, which `commands.rs` calls. |

## Removed dead-code suppressions

These `#[allow(dead_code)]` attributes were stale, meaning the items were live. Removing
the attribute compiles clean, which is the proof.

| Item | File | State |
|---|---|---|
| `handle_tools_list_for_profile_params` | `mcp/tools.rs` | Live: reached through `mcp::tools_list_page` by the catalog benchmark and by tests. |
| `to_review_status` | `utility/ui_verify.rs` | Live: called at `ui_verify.rs` when writing a verdict. |
| `score_prompt_against_skills` | `utility/skill_match.rs` | Live: the selective-activation benchmark profile calls it. |
| `run_hook_command_with_stdin` export | `runner/hook_lifecycle/mod.rs` | Scoped with `#[cfg(test)]`, matching the existing convention in that file for test-only exports. |

Retained with an explicit reason, not a bare suppression:

- `record_skill_outcome` (`utility/skill_usage.rs`): its reader `skill_success_rate` feeds
  cost-aware routing, but no production signal says whether a skill helped, so the writer
  is deliberately unwired. Deleting it would make a required ranking input permanently
  unpopulatable; the doc comment now states this instead of hiding behind the attribute.
- `utility/anvil/clarify.rs` schema fields: serde-populated artifact contract fields.

## Duplicate systems

One canonical owner per responsibility holds. Verified by inspection, not by assumption:

| Responsibility | Canonical owner | Evidence |
|---|---|---|
| Token counting | `proxy/token_meter.rs` | Only this file references `tiktoken_rs` or `o200k_base_singleton`. Every other module calls `TokenMeter`. |
| Context budgets | `proxy/context.rs` | Every `DEFAULT_MAX_*_TOKENS` is defined there; `recall.rs` declares its own result bound for its own surface. |
| `tools/list` dispatch | `mcp/tools.rs::handle_tools_list_for_profile_params_with_context` | `mcp/mod.rs` routes `"tools/list"` only there. The unpaginated `handle_tools_list_for_profile` is a measurement surface for the ledger and the benchmark, never a client response. |
| Model-visible admission | `proxy/context.rs` `ContextFirewall` | `proxy/run.rs` and `mcp/tools.rs` both project through it. |
| Raw evidence | `proxy/raw_store.rs` | Recovery, `raw`/`replay`, and the review evidence gate read through it. |
| Skill routing | `utility/skill_match.rs` | MCP, the hook, and the eval all call `resolve_skill_selection`. |

## Updated host adapters

None. No adapter contract changed. `bun test tests/host-adapter-contracts.test.ts`
remains green (13 pass).

## Updated docs

- `docs/benchmark-suite.md`: records the reducer reliability metrics and the MCP catalog
  benchmark, and describes the fifth skill profile.
- `bench/competitor/README.md`: replaces the stale catalog claim with the emitted
  handshake cost per profile and separates it from the ratified ledger surface.
- `bench/competitor/comparative-compaction.json`: adds the reliability metrics, the
  five-profile catalog benchmark, and the fifth skill profile.
- `bench/competitor/skill-selection-benchmark.json` and
  `bench/competitor/eval-compaction-report.json`: regenerated from the current binary so
  the artifacts match the code that produces them.

## New tests

| Test | File | Guards |
|---|---|---|
| `mcp_benchmark_profiles_stay_bounded_complete_and_deduplicated` | `utility/stats.rs` | Every catalog profile stays within budget with complete, duplicate-free traversal. |
| `mcp_benchmark_discovery_covers_every_representative_task` | `utility/stats.rs` | Discovery can name a capability for all twelve task families. |
| `critical_failure_evidence_survives_reduction` | `utility/eval.rs` | Failure identity survives reduction. |
| `every_case_keeps_its_outcome_visible` | `utility/eval.rs` | A reader can decide pass or fail from the visible result. |
| `reacquisition_rate_matches_per_case_measurements` | `utility/eval.rs` | The headline rate cannot drift from the per-case booleans. |
| `every_benchmark_profile_is_distinct_and_measured` | `utility/skill_eval.rs` | All five disclosure strategies exist and are measured. |
| `measured_resource_cost_matches_the_bundled_tree` | `utility/skill_eval.rs` | Resource cost comes from the real files, not an assumption. |

## New benchmarks

- `keel stats tools --benchmark --json`: five catalog configurations, exact first-page
  cost, pages to traverse, unique tools reached, discovery coverage, and latency.
- `keel eval --json`: adds `evidenceRetentionPercent`, `failureEvidenceRetentionPercent`,
  `reacquisitionRatePercent`, and per-case evidence, outcome, and reduce-cost fields.
- `keel skill-eval --benchmark --json`: adds the on-demand resource profile,
  `reacquisitionCalls`, `resourceCostTokensMeasured`, `localRoutingCpuMs`,
  `wrongActivationRate`, and `missedActivationRate`.

## Remaining known limitations

1. **Keel is a legacy-era MCP server, not a dual-era one.** The current MCP
   revision (`2026-07-28`) names three implementation eras: *modern* (per-request
   `_meta` version, `server/discover`, `resultType`, and `ttlMs`/`cacheScope` on
   list results), *legacy* (`initialize` handshake, `2025-11-25` and earlier), and
   *dual-era* (both). Keel implements `2025-11-25` plus the earlier `2024-11-05`,
   which it rejects on Streamable HTTP with an explicit migration message rather
   than mis-serving. Per that revision's own compatibility matrix, a legacy client
   interoperates with Keel and a modern `2026-07-28` client does not. Becoming
   dual-era is a scoped feature, not a defect fix; it is recorded as verified
   research `CLM-007` in the plan evidence.
2. **The fixed-context ledger measures the complete catalog, not the emitted handshake.**
   `mcp.tools_list.catalog` reports 1240 tokens against a ratified 1364 budget, while the
   default handshake a client actually receives measures 671 tokens. Both numbers are now
   labelled for what they measure. Changing the ratified budget is a product decision.
3. **`record_skill_outcome` has no production caller**, so the historical-success scoring
   input returns its neutral prior until an outcome source exists.
4. **The skill benchmark treats every positive fixture as needing its bundled resources**,
   so the on-demand profile ties the eager profile on tokens and pays extra turns on this
   corpus. A corpus with optional resources would show the reduction it can buy.
5. **Provider cached-input tokens remain unavailable** for the local fixtures, so cache
   behaviour is reported as unknown rather than estimated.
