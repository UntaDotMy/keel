# Final Agent Operating System Research-Gap Closure

- Repository: `UntaDotMy/keel`
- Retrieval date: 2026-09-12 (Asia/Singapore)
- Scope: the final-plan research contract (sections 11-13, 45, 53-58)
- Grounding status: live official documentation and papers, with current local
  implementation evidence

## Decision

The final research gaps were a provenance boundary, an explicit artifact
envelope, compact projection limits, and executable regression coverage. The
planner now rejects a local-index result when the request contains an exact
external/current research signal; accepts only the declared `host`, `cache`,
or `local-index` origins; requires `query`, `researchSource`, and boolean
`truncated`; rejects a truncated complete artifact; requires cache identity for
cache-origin evidence; and validates bounded source, claim, conflict, and
traceability fields without silently clipping the submitted JSON.

This closes the validator-side gap. Keel still does not interpret websites or
perform web research itself: the connected host/agent remains responsible for
research reasoning and for submitting source-backed claims.

## Current-source records

### SRC-001: MCP 2026-07-28 release and SDK migration

- Cache record: `rc-1a093257329-10124-1`
- URLs: [MCP 2026-07-28 release](https://blog.modelcontextprotocol.io/posts/2026-07-28/);
  [TypeScript SDK migration support](https://ts.sdk.modelcontextprotocol.io/v2/migration/support-2026-07-28)
- Source type: official protocol/vendor documentation
- Retrieved: 2026-09-12
- Relevant finding: the release describes a stateless request/response core,
  retired `initialize`/`initialized` and `Mcp-Session-Id`, self-describing
  requests, optional `server/discover`, routing headers, and cache hints. The
  SDK migration guide says the 2025-era protocol remains the default for
  hand-constructed v2 clients/servers and that the 2026-07-28 protocol is
  explicit opt-in.
- Constraint: Keel may enforce a modern-only boundary, but the guide does not
  establish modern behavior as the default for all SDK clients.

### SRC-002: Agent Skills progressive disclosure

- Cache record: `rc-1a09325afaf-13272-1`
- URL: [Agent Skills specification](https://agentskills.io/specification)
- Source type: official specification
- Retrieved: 2026-09-12
- Relevant finding: a skill exposes bounded metadata first, `SKILL.md` on
  activation, and deeper scripts/references/assets on demand; the
  specification recommends keeping `SKILL.md` under 500 lines.
- Constraint: this informs the shape of compact evidence, but it does not
  establish a Keel token or task-success measurement.

### SRC-003--SRC-005: Planning and reflection research

- Cache records: `rc-1a093260300-6608-1`, `rc-1a0932602b4-16756-1`,
  `rc-1a0932602cd-7656-1`
- URLs: [LLM agent planning survey](https://arxiv.org/abs/2402.02716);
  [Devil's Advocate reflection](https://arxiv.org/abs/2405.16334);
  [ReAcTree hierarchical planning](https://arxiv.org/abs/2511.02424)
- Source type: historical research papers
- Retrieved: 2026-09-12
- Relevant finding: the papers cover decomposition, plan selection, external
  modules, reflection/backtracking, completion review, and hierarchical
  subgoals with goal-specific memory.
- Constraint: these are design inputs, not Keel quality, accuracy, or
  performance results. The earlier combined cache record was marked stale and
  is not used as evidence.

## Implementation mapping

| Contract gap | Repository owner | Evidence added |
| --- | --- | --- |
| Current external facts must not silently use local truth | `rust/crates/keel/src/utility/research_policy.rs` (`classify_research_requirement`, `validate_research_origin`) | Exact-token classifier and fail-closed `local-index`/host/cache origin checks |
| Research identity and completeness must be explicit | `research_policy.rs` (`validate_research_artifact`) | Required `query`, `researchSource`, and `truncated`; complete + truncated is rejected |
| Cache reuse must retain identity | `research_policy.rs` (`validate_research_origin`) | Every cache-origin source requires `cacheId`; current external cache evidence requires an external source |
| Compact research must remain bounded | `research_policy.rs` bounds | 32 KiB artifact, bounded query/IDs/URLs/evidence, source/claim/conflict/reference/consumer count limits |
| Conflicts cannot disappear in projection | `research_policy.rs` (`validate_claim_relations`) | Conflict arrays and claim references are bounded; unresolved or unverified resolutions continue to fail |
| Gaps need executable regressions | `rust/crates/keel/tests/research_gap_test.rs` and policy unit tests | CLI tests cover local fallback rejection and malformed envelope; unit tests cover classifier, cache identity, and oversized evidence |

The validator reports an invalid research result through `status.json` while
preserving the submitted `research.json` for diagnosis. This preserves raw
evidence and avoids replacing a rejected artifact with an invented result.

## Verification boundary

Focused unit and CLI integration tests pass. The tests prove the planner-side
admission policy and the local fallback behavior in an isolated temporary
workspace. They do not prove that every connected host invokes a web adapter,
that a host's source extractor returns truthful citations, or that a remote
site remains available. They also do not replace the final-plan requirements
for generated-input property/fuzz coverage, full release gates, clippy with
`-D warnings`, security audit, benchmarks, or packaged-binary smoke evidence.
Those remain separately evidenced release work.
