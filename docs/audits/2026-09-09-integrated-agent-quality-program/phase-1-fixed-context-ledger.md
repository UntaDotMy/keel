# Phase 1 fixed-context ledger

Phase 1 adds a runtime `o200k_base` ledger and CI budget gate for every
requested fixed-context surface. It is isolated on
`task/integrated-agent-quality-phase-1` above Phase 0 commit `a355c7b`.

## Requirement evidence

| Requirement | Evidence | Status |
|---|---|---|
| MCP serialized catalog, tool count, eager/deferred split | `tools_list_context_snapshot` returns 37 total, 37 eager, 0 deferred and 2,885 tokens | pass |
| SessionStart and both UserPromptSubmit fixtures | Runtime rows measure the fixed bootstrap, `hello`, and `fix the bug` | pass |
| Repository and generated host instructions | Three repository rows plus Claude, Codex, Oh My Pi, ZCode, and Antigravity canonical installer blocks | pass |
| Skill inline/frontmatter versus full bodies | One stable 51-skill source set yields both projections without cache writes | pass |
| Planner, research, ticket, warning, and UI pointers | Five canonical pointer rows; warning pointer is 9 tokens | pass |
| Exact +10% budgets | CI asserts every threshold equals `ceil(actual × 1.10)` | pass |
| Actionable failure | Scratch `budget + 1` case requires surface, actual, budget, and reproduction command | pass |
| Stats and gain accounting | Stats prints all 19 rows; gain marks fixed context excluded from command savings | pass |
| Future phase coupling | Runtime/doc parity makes changed sources require ledger and threshold review | pass |

The ratified counts and measurement boundaries are in
[`docs/fixed-context-ledger.md`](../../fixed-context-ledger.md).

## Test progression

The first `fixed_context_budget_test` run failed all four initial cases because
`fixedContextLedger` and `fixedContextAccounting` did not exist. The final file
contains five passing tests, including runtime budget, scratch failure, text
output, gain separation, and documentation parity.

Review found and fixed two issues before closeout:

- Budget rows now fail through the detailed assertion before aggregate status,
  preserving the required diagnostic.
- Stats reuses ledger measurements for legacy UserPrompt and MCP fields instead
  of invoking those owners twice.

## Validation

| Gate | Result | Evidence |
|---|---|---|
| Fixed-context integration | 5 passed, 0 failed | `cargo test -p keel --test fixed_context_budget_test --locked`; RawStore `20260909-033951-61f2c293` |
| Keel library | 1,264 passed, 0 failed | RawStore `20260909-030627-c1daffca` |
| MCP, host, CLI, doc integrations | 82 passed, 0 failed | RawStore `20260909-030700-45d9c032` |
| Full workspace | 1,414 passed, 0 failed, 0 ignored | RawStore `20260909-034150-63748009` |
| Rust formatting and lint | pass | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` |
| Skill lint and routing | 51/51 lint; 38/38 eval | RawStore `20260909-031000-51331182` for eval |
| Host/config audit | 0 high, 0 medium, 0 low | `keel config-audit --repo-root <repo>` |
| Review pre-commit | 0 blockers, 0 warnings | format, clippy, comment, prose, slop, flow, and completeness gates passed |

The full suite emitted only the two Phase 0 fixture-local Git line-ending
warnings for temporary `README.md` and `src/main.rs` files. No Phase 1 source
warning was added.

## Release ladder

| Rung | Verdict | Evidence |
|---|---|---|
| Smoke | pass | Local binary prints the complete text ledger and additive JSON contract |
| Functional | pass | Budget, missing-source status, warning cap, and scratch failure logic are covered |
| Integration | pass | MCP, installer-host, CLI, and documentation boundaries pass |
| UI | not applicable | No graphical surface changed |
| Load | not applicable | Read-only local reporting adds no service or concurrent workload |
| Stress | not applicable | No network, queue, retry, or shared mutable state was added |
| Security | pass | No dependency, credential, auth, or content-emission change; config and secret-hygiene checks are clean |

Hosted CI and push remain deferred under the user's program-wide no-push
instruction. The local Phase 0 commit is the recovery boundary; Phase 1 changes
are additive and can be reverted as one phase commit.
