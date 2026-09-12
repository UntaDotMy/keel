# Adversarial plan-gap evidence

This audit records the deterministic integration coverage added for sections
53 through 56 and 58 of `keel-final-agent-operating-system-plan.md`. The test exercises
the shipped binary and the public context gateway; it does not claim that a
passing test alone proves the full release ladder in section 56.

## Coverage

| Plan requirement | Evidence in `adversarial_plan_gap_test.rs` |
| --- | --- |
| MCP budget, pagination, exact-once order, replay, invalid and cross-scope cursors | `mcp_generated_pagination_preserves_budget_order_scope_and_replay_safety` walks a full profile with a fixed hard budget and page size, measures each emitted JSON page with `TokenMeter`, checks 37 unique tools, repeats a cursor, tampers with a cursor, and reuses it from another authoritative session. |
| MCP discovery, cache hints, protocol version | The same test checks `server/discover` result completeness, `cacheScope=public`, positive `ttlMs`, private `tools/list` cache scope, and the 2026-07-28-only error for a legacy version. |
| Context empty/huge/repeated/injection-like/Unicode and hard failure semantics | `generated_context_inputs_are_bounded_deterministic_and_fail_closed` generates fixed payload variants over three budgets, checks visible tokens never exceed budget, stable IDs/reducer/metadata, duplicate suppression, injection neutralization, replacement-character Unicode, input-size rejection, and zero-budget rejection without raw fallback. |
| Planning vague and adversarial request preservation | `planner_and_research_cache_survive_adversarial_inputs_without_false_current_state` submits fixed vague, latest-version, contradictory-sounding, and Unicode requests, then checks versioned artifacts, required sections, verbatim request preservation, and clarification for the vague request. |
| Research expiry and memory scope | The same test records a cache entry, forces its stored expiry into the past, proves default lookup excludes it while `--include-stale` exposes it, and proves a separate isolated home cannot see the record. |
| Learning repeated failure, corrupt input, bounds, and lifecycle | `learning_promotes_only_repeated_scoped_signals_and_ignores_corrupt_rows` seeds nine observations across three sessions plus malformed rows, checks dry-run has no durable instinct write, runs the cycle, and bounds instinct/skill promotion and status signals. |

## Reproducible commands

```text
cargo test --locked -p keel --test adversarial_plan_gap_test
cargo fmt --all -- --check
cargo clippy --locked -p keel --test adversarial_plan_gap_test -- -D warnings
```

The tests intentionally avoid network access, non-deterministic sleeps, and
developer home state. HTTP header/body mismatch is covered by the existing
MCP protocol integration suite; packaged-binary and complete release-ladder
evidence remain separate gates and are not inferred here.
