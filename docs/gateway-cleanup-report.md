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

## Defects found and fixed

These were found by the new tests and benchmarks, not by reading and assuming.

| Defect | Evidence | Fix |
|---|---|---|
| Two identical cursor requests could pack a different number of tools | `catalog_cursors_reject_replay_expiry_and_foreign_identity` failed in a full run: the same cursor produced a 7-tool page and an 8-tool page | The packer takes the deadline from the incoming cursor, so one walk keeps one deadline and the emitted cursor cannot change the packing decision between calls. |
| A rejected research submission destroyed a complete bundle | A stale source submission replaced 7 sources with 1 and reset the architecture note | Validation runs before the write; a rejected submission leaves the bundle byte-identical. |
| A non-lock index failure whose text merely said "busy" was downgraded to a degraded lock state | `lock_classification_uses_the_typed_code_not_the_message_text` | The lock decision reads the typed SQLite error code. |
| Freshness used one universal 90-day window for every source type | Plan §22 forbids this; `per_source_type_windows_reject_fast_moving_evidence_sooner` | Per-source-type windows: issue 7 days, repository 30 days, official-doc 90 days. |
| The catalog ledger reported the emitted handshake cost as the full-catalog cost | The default handshake measures 671 tokens while the ledger reported 1240 | Both numbers are now labelled for what each measures. |

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
| `tools_list_traversal_is_complete_for_a_catalog_of_hundreds_of_tools` | `mcp/tools.rs` | A 300-tool adversarial catalog stays bounded, complete, and duplicate-free. |
| `tools_list_reduces_representation_before_it_fails_closed` | `mcp/tools.rs` | A giant tool is emitted reduced rather than rejected, and an impossible budget fails closed with an explicit reason. |
| `catalog_cursors_reject_replay_expiry_and_foreign_identity` | `mcp/tools.rs` | A cursor cannot be replayed into another session or a mutated catalog, expiry is named, and one walk keeps one deadline so a replay is reproducible. |
| `catalog_cursors_cannot_cross_workspaces` | `mcp/tools.rs` | §42 stop condition: a cursor cannot cross a workspace boundary. |
| `tools_list_fails_closed_when_every_tool_is_oversized` | `mcp/tools.rs` | An all-oversized catalog fails closed once with an explicit budget error. |
| `no_gate_status_can_be_read_as_a_pass` | `review/tests.rs` | No gate state can be aggregated into a false pass. |
| `lock_classification_uses_the_typed_code_not_the_message_text` | `utility/workspace_index.rs` | A non-lock failure is never downgraded to a lock by its message text. |
| `concurrent_refresh_degrades_instead_of_failing_while_a_writer_holds_the_lock` | `utility/workspace_index.rs` | A contended refresh degrades explicitly and recovers. |
| `rejected_research_does_not_clobber_the_existing_bundle` | `utility/plan.rs` | A rejected research submission preserves the on-disk bundle. |
| `per_source_type_windows_reject_fast_moving_evidence_sooner` | `utility/research_policy.rs` | Fast-moving evidence is rejected sooner than version-bound documentation. |
| `tools_list_budget_ladder_holds_at_every_mandated_value` | `mcp/tools.rs` | Every budget in §6.7's mandated ladder either emits bounded, complete pages or fails with an explicit configuration error. |
| `cursor_deadline_is_stable_within_a_ttl_window` | `mcp/tools.rs` | A fresh deadline is bucketed to the cursor TTL, stays inside `(ttl, 2*ttl]`, and survives a zero TTL. |
| `repeated_identical_requests_pack_the_same_page` | `mcp/tools.rs` | Two identical requests select the same tools at the same measured cost, with no clock allowance. |
| `concurrent_tools_list_and_tools_call_stay_independent` | `mcp/mod.rs` | Five interleaved `tools/list` requests and a `tools/call` over one stdio session each stay individually valid, bounded, and identical. |
| `session_ttl_outlives_an_inflight_call` | `mcp/http.rs` | A session TTL shorter than the tool deadline cannot reap a session that still owns a cancellable request. |
| `mcp_catalog_command_reports_the_live_packing_picture` | `mcp/mod.rs` | The catalog diagnostic reports the packer's own measurement and fails closed on a bad budget or profile. |
| `mcp_ledger_reports_the_emitted_handshake_not_a_nearby_representation` | `utility/fixed_context.rs` | The ledger's handshake number is the dispatcher's own response, not the complete-catalog cost. |
| `cache_accounting_puts_each_measurement_in_exactly_one_bucket` | `proxy/context.rs` | Every model-visible token lands in exactly one of cached/uncached, by the firewall's own rule. |
| `every_installer_platform_has_explicit_host_metadata` | `proxy/execution.rs` | Every host the installer wires has explicit capability metadata. |
| `claimed_hosts_never_resolve_through_the_unknown_default` | `proxy/execution.rs` | A wired host is distinguishable from a host keel has never seen. |
| `latency_stage_status_fails_closed_above_its_declared_ceiling` | `utility/stats.rs` | A stage over its declared latency ceiling fails the run and names the measurement. |
| `latency_benchmark_measures_every_declared_stage_and_enforces_its_ceiling` | `tests/fixed_context_budget_test.rs` | All nine §33 stages are measured through their owners and every reported status agrees with its measurement. |

## New benchmarks

- `keel stats tools --benchmark --json`: five catalog configurations, exact first-page
  cost, pages to traverse, unique tools reached, discovery coverage, and latency.
- `keel stats latency --json`: the nine §33 pipeline stages measured through their
  production owners (catalog build, page pack, serialization, token count, reduction,
  dedupe, skill index, skill routing, memory retrieval, RawStore locate), each against a
  declared ceiling. A stage over its ceiling fails the run. A documented warm-up keeps
  one-time tokenizer and index construction out of the reported per-request cost.
- `keel eval --json`: adds `evidenceRetentionPercent`, `failureEvidenceRetentionPercent`,
  `reacquisitionRatePercent`, and per-case evidence, outcome, and reduce-cost fields.
- `keel skill-eval --benchmark --json`: adds the on-demand resource profile,
  `reacquisitionCalls`, `resourceCostTokensMeasured`, `localRoutingCpuMs`,
  `wrongActivationRate`, and `missedActivationRate`.
- `keel host matrix [--host] [--json]`: the §18 machine-readable host capability matrix.

## Module-size decision (plan §26)

The plan names a candidate `mcp/` split (`catalog.rs`, `profiles.rs`, `pagination.rs`,
`discovery.rs`, `activation.rs`, `dispatch.rs`, `transport.rs`) and also says not to
split files merely for aesthetics. Measured state:

| File | Production lines | Test lines | Structure |
|---|---:|---:|---|
| `mcp/tools.rs` | 4,717 | 3,311 | 37 independent `tool_*` handlers, then catalog, cursor, and discovery helpers |
| `mcp/http.rs` | ~1,500 | ~900 | Transport boundary plus its own conformance tests |
| `mcp/mod.rs` | ~1,900 | ~800 | JSON-RPC framing, serve loop, dispatch, `resources/*` |
| `proxy/context.rs` | ~1,100 | ~600 | Firewall, budget, reducers, recovery |

Decision: **not split in this pass.** The reasoning is the plan's own:

- The 4,717 production lines are 37 independent tool handlers, each a thin wrapper over
  a function that already backs a CLI command. That is breadth, not tangled complexity;
  a handler can be found and changed without reading its neighbours.
- The plan's split target would move catalog, cursor, and discovery helpers out of
  `tools.rs` while leaving the handlers in place, which relocates code without reducing
  coupling between the parts that actually call each other.
- The plan explicitly warns against splitting for aesthetics, and this pass already
  carries a verified determinism fix in the cursor path. A 1,000+ line mechanical move
  would put a green, packaged-verified build at risk for a structural gain the plan does
  not list as a completion criterion.

Recorded instead of done, with the reasoning and the measurements above, so the next
change can revisit it with the same evidence.

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
6. **The adversarial catalog and cursor matrix is covered.** Catalogs from one to 300
   tools, budgets from 5 to 4000 tokens, an exact-budget regression, a single giant tool
   at both survivable and impossible budgets, an all-oversized catalog, invalid, empty,
   truncated, and unicode cursors, cursor replay, cross-session cursors, cross-workspace
   cursors, catalog mutation between pages, cursor expiry, duplicate request ids,
   transport header and media-type abuse, and contended index writes. The transport and
   session concurrency matrix beyond those cases is not exhaustively enumerated.
7. **Per-host conformance runs are still source-level.** The host capability matrix is
   machine-readable and the adapter contracts are covered by
   `tests/host-adapter-contracts.test.ts`, but there is no live conformance run against
   a host that lacks the interception surface.
8. **Performance is measured only where a benchmark reports it.** Catalog build latency is
   recorded per profile, and per-case reduce cost in microseconds is recorded for the
   reducers. Page-pack, serialization, dedupe, skill-routing, memory-retrieval, RawStore,
   and HTTP overhead are not separately measured.
