# Phase 3 Research and Freshness Gate Evidence

## Delivered contract

Phase 3 makes research a checked implementation and review dependency. The
compiled planner accepts source provenance from a web-capable host, reuses only
complete fresh cache records, falls back visibly to `local-only` evidence, and
rejects stale external product claims with `re-search required`. Brownfield
pre-PR and closeout review require a named plan whose research remains current
and linked to its REQ/AC consumers.

The default external product-evidence window is 90 days. A project can set a
positive `[research].max_age_days` value in `keel.filters.toml`; zero and
negative values fail instead of disabling the gate. Fresh official documentation
has cache priority over repository, issue, historical standard, and paper
records. Unsupported secondary-source types cannot pass validation.

## Requirement evidence

| Requirement | Evidence |
| --- | --- |
| Versioned research schema | `research.json` keeps `schemaVersion: 1` and records claim/source IDs, URL, type, optional publication date, retrieval timestamp, support, freshness class, and REQ/AC `usedBy` links. |
| Empty research blocks progression | `plan tasks` validates research before compiling tasks; the integration fixture proves a newly specified plan fails with `research.json status is not complete`. |
| Current primary evidence | Host-supplied `official-doc`, `repository`, and `issue` records must be `fresh`; a 90-day default retrieval window applies, and cache selection prioritizes the current official source. |
| Historical evidence | `paper` and `standard` records can be older only when labeled `historical` and carrying `publicationDate`. |
| Search-less host boundary | Local workspace-index evidence is emitted with artifact and command payload grounding set to `local-only`; absence of a local match remains insufficient. |
| Cache reuse and TTL | Research-cache records add citation metadata alongside the existing `freshness` TTL. Complete fresh matches are reused, while complete stale matches block silent local fallback. |
| Project configuration | `[research].max_age_days` is optional and defaults to 90; non-positive values fail closed. |
| Review enforcement | The blocking `research_traceability` pre-PR gate detects committed and uncommitted established-source edits, requires `--plan`, and lists missing, stale, malformed, or unlinked records. The MCP schema and closeout path forward the same plan and home arguments. |
| Token accounting | The review schema fields move the MCP catalog from 2,885 to 2,902 tokens. The ratified exact +10% budget is 3,193 and the fixed-context gate recomputes it at runtime. |

The real branch plan is
`plan-1788904950191646600-7528-f5415070`. It passed the full
`specify -> research -> tasks -> check --rtm` sequence with fresh host evidence
from the locked [Chrono 0.4.45 DateTime API](https://docs.rs/chrono/0.4.45/chrono/struct.DateTime.html#method.parse_from_rfc3339).
The historical-standard fixture uses the primary
[RFC 3339 publication](https://www.rfc-editor.org/rfc/rfc3339).

## TDD progression

The first planner run passed 7 of 11 tests and failed the four new research
cases because external provenance, freshness, cache reuse, and visible
local-only status were not implemented. RawStore:
`20260909-052336-b89608de`. The first review test compilation also failed before
the gate accepted a plan reference. RawStore: `20260909-052543-7858f50e`.

Final focused proof:

| Gate | Result | Evidence |
| --- | --- | --- |
| Planner workflow | 14 passed, 0 failed | `cargo test -p keel --test plan_workflow_test --locked`; RawStore `20260909-062312-31195007` |
| Review research gate | 2 passed, 0 failed | `cargo test -p keel --lib review::tests::research_gate --locked`; RawStore `20260909-060002-1ac17634` |
| Research cache | 4 passed, 0 failed | `cargo test -p keel --lib utility::memory_families::tests::research_cache --locked`; RawStore `20260909-060011-2b8054d8` |
| Research policy | 2 passed, 0 failed | Default/configuration/cannot-disable unit filter |
| MCP review contract | 9 passed, 0 failed | `cargo test -p keel --lib mcp::tools::tests::review_ --locked`; RawStore `20260909-060052-63d9071d` |
| Documentation parity | 27 passed, 0 failed | RawStore `20260909-060207-62b77c51` |
| CLI integration | 7 passed, 0 failed | RawStore `20260909-060358-2edd2c60` |
| Fixed-context budget | 5 passed, 0 failed | RawStore `20260909-060739-0ded38d1` |

## Full validation

| Gate | Result | Evidence |
| --- | --- | --- |
| Anvil compile and dry-run | pass | One piece, three casts, one gate; frozen-prefix check passed. |
| Rust formatting | pass | `cargo fmt --all -- --check` |
| Rust lint | pass | `cargo clippy -p keel --all-targets --locked -- -D warnings` |
| Keel library | 1,273 passed, 0 failed | Included in final workspace RawStore `20260909-062506-d6a34e9d` |
| Full workspace | 1,437 passed, 0 failed | RawStore `20260909-062506-d6a34e9d` |
| Scoped pre-commit | pass, 0 warnings | Format, clippy, comment, prose, slop, flow, completeness, and impact gates passed. |
| Researched pre-PR | pass, 0 warnings | Full workspace and `research_traceability` passed against Phase 2 with the real Phase 3 plan. |
| Completion gate | pass | All 7 working-brief acceptance criteria carry focused, plan, review, and final-workspace proof. |

The first full-workspace attempt correctly failed two fixed-context assertions
after the MCP schema grew. Updating only the runtime-measured catalog row and
its exact +10% budget restored the five-test budget gate before the successful
workspace rerun.

## Release ladder

| Rung | Verdict | Evidence |
| --- | --- | --- |
| Smoke | pass | Real local binary completed the branch plan with fresh official evidence. |
| Functional | pass | All seven acceptance cases and project-policy/primary-source precedence cases pass. |
| Integration | pass | Planner, review, MCP, CLI, docs, fixed-context, library, and workspace suites pass together. |
| UI | not applicable | No graphical surface changed. |
| Load | not applicable | The feature adds bounded local artifact validation and no service workload. |
| Stress | not applicable | No network client, queue, retry, or concurrent mutable service was added. |
| Security | pass | Plan IDs remain safe path segments; source schemes and timestamps are validated; no credentials or executable source content are introduced. |

Hosted CI and push remain deferred under the program-wide no-push instruction.
The Phase 2 commit is the recovery boundary; Phase 3 is isolated on its own
stacked branch and will be committed as one local phase after final review.
